# RsRazer

[简体中文](README.zh-CN.md) · English

Control Razer devices on Windows 10 / 11 with a native Rust application and background service.

No Synapse, custom kernel driver or Python runtime is required.

[![CI](https://github.com/imacte/RsRazer/actions/workflows/ci.yml/badge.svg)](https://github.com/imacte/RsRazer/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-GPL--2.0--or--later-green)](LICENSE)

![RsRazer lighting editor](docs/images/rsrazer-lighting.png)

## Download

Download **[rsrazer-setup.exe](https://github.com/imacte/RsRazer/releases/latest/download/rsrazer-setup.exe)** from [Releases](https://github.com/imacte/RsRazer/releases).
It contains the application, native daemon and 32/64-bit Chroma SDK DLLs.
The installer works per user and supports repair and removal.
The installer includes English, Simplified Chinese and Russian.

The tray and UI run in separate processes. Closing the window saves the current page and editor drafts,
then exits the UI process to release its memory. Device communication, automatic profiles and update
downloads continue in the tray; click its icon to reopen the window.

| File | Purpose |
|---|---|
| `rsrazer-setup.exe` | Complete installer; the recommended download |
| `rsrazer.exe` | Main application; needs `rsrazer-daemon.exe` beside it when used without the installer |
| `rsrazer-daemon.exe` | Native background service and command line |
| `rsrazer-classic.exe` | Classic control panel with the daemon embedded |

Native release builds include the C runtime. Close Synapse before controlling the same devices.
Settings, profiles, macros and logs are stored in `%LOCALAPPDATA%\rsrazer`.
RsRazer does not automatically read or migrate data from `%LOCALAPPDATA%\openrazer-win`.

## Features

- A database of 279 product IDs, with USB HID and supported Bluetooth LE devices.
- Hardware lighting and software effects, per-key layers, audio-reactive lighting and profiles.
- DPI, polling rate, brightness and supported headset controls.
- Macro keys, key bindings and application-specific profiles.
- Windows accent colour and Chroma integrations.
- A local authenticated JSON-RPC interface and scriptable native CLI.

Available controls depend on the device. Database coverage does not mean every model has been tested on physical hardware.
The [device list](docs/devices.json) and `rsrazer-daemon supported` show bundled models.

## Command line

```powershell
rsrazer-daemon daemon start
rsrazer-daemon list
rsrazer-daemon info
rsrazer-daemon effect static red
rsrazer-daemon brightness 60
rsrazer-daemon dpi 1800
rsrazer-daemon keyswitch gaming         # Huntsman V2 / V2 Tenkeyless
rsrazer-daemon snaptap                  # Status, off by default
rsrazer-daemon snaptap pair add W S
rsrazer-daemon profile save night
rsrazer-daemon profile load night
rsrazer-daemon profile create work
rsrazer-daemon profile create gaming --source work
rsrazer-daemon integration accent on
rsrazer-daemon rpc daemon.version
rsrazer-daemon doctor
```

Use `--help` for commands and options. `--device` selects a serial, product ID or name fragment;
`--json` prints machine-readable results. `daemon start --emulate` uses simulated devices and separate demo data.

Saving, creating or loading a profile makes it active for that device. Later
changes are saved into it automatically. Profiles restore lighting, switch mode,
DPI stages, DPI and polling rate; keyboard profiles also carry Snap Tap's enable
state and key pairs. Snap Tap's consent and application exclusions stay global.
`profile create NAME` starts from defaults; `--source NAME` duplicates a profile.
The CLI marks the active profile with `*`. These profiles are stored on the PC.

Snap Tap runs in the daemon and applies to every keyboard. Enable it from the
keyboard Performance page after reading the warning, or use `snaptap on --accept`.
Some games prohibit software input changes; `cs2.exe` is excluded by default.
Use `snaptap deny add game.exe` or `snaptap allow add game.exe` to edit the lists,
and `snaptap off` to disable it. Exiting the daemon stops Snap Tap. Keyswitch
optimization is a keyboard firmware setting and remains in effect without it.

Naga V2 Pro (USB IDs `1532:00A7` and `1532:00A8`) also has an **Onboard memory**
page for mouse buttons, keyboard keys and macros stored in the mouse. Slot 1 is
live and survives power cycles; slots 2–5 hold other mappings. These settings are
independent of the PC profiles above. Equivalent commands:

```powershell
rsrazer-daemon onboard --device 00a7
rsrazer-daemon onboard buttons --device 00a7
rsrazer-daemon onboard action 0x50 --device 00a7
rsrazer-daemon onboard key 0x50 A --device 00a7
rsrazer-daemon onboard macro-add 7 "down A; wait 50; up A" --device 00a7
rsrazer-daemon onboard macro-bind 0x50 7 --device 00a7
```

Use the product ID or serial reported by `list` for the connection in use.
`--store 1..5` and `--layer 0..1` select a mapping; layer 1 is Hypershift.
`--modifiers` accepts the HID modifier mask for a keyboard binding, and
`--while-held` repeats a bound macro. `onboard macro-delete ID` refuses to delete
a macro while any button still uses it. After deletion the firmware can omit live
IDs from its list; `onboard macro-length ID` checks a known ID. Upload never
overwrites an existing ID. `--direct` provides the same hardware commands while
the daemon is stopped. This port is tested with the emulator and protocol fixtures;
physical Naga testing is documented in [upstream #2962](https://github.com/openrazer/openrazer/pull/2962).

## Build from source

Install the pinned Rust toolchain and Microsoft C++ Build Tools with the Windows SDK, then run:

```powershell
git clone git@github.com:imacte/RsRazer.git
cd RsRazer
./studio/tools/build-native.ps1 -IncludeClassic
```

The installer and executables are written to `dist/`. No Python or PyInstaller step is used.

```powershell
cd studio
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p rsrazer-tools -- check-translations
cargo run --locked -p rsrazer-tools -- docs-data --check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for development tools and [data/README.md](data/README.md) for database provenance.

| Directory | Contents |
|---|---|
| `studio/daemon` | Rust HID/BLE transports, protocols, effects, macros, RPC service and CLI |
| `studio/app` | Main graphical application |
| `studio/core` | Shared UI, translations, configuration and installer support |
| `studio/setup` | Installer with embedded application and daemon |
| `studio/chroma` | Native Chroma SDK DLL |
| `studio/tools` | Rust tools and PowerShell build/verification scripts |
| `gui` | Classic Rust control panel |
| `data` | Device, recipe and key data |

## Credits and feedback

RsRazer builds on [Ar4ikov/openrazer-win](https://github.com/Ar4ikov/openrazer-win)
and the device protocol work of [OpenRazer](https://github.com/openrazer/openrazer).
Upstream attribution and licence notices are retained. The legacy Python implementation is in Git history.

Report bugs or request devices in [Issues](https://github.com/imacte/RsRazer/issues).
For security reports, see [SECURITY.md](SECURITY.md). Licensed under [GPL-2.0-or-later](LICENSE).
