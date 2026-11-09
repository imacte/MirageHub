# Sony INZONE support

Mirage Hub ports OpenInzone's device-control protocol to the existing Rust daemon. It does not launch a second C# daemon or require .NET. Source: [OpenInzone](https://github.com/penguinwokrs/openinzone), revision `4dccfdef99755c51b9f7cce109565da622625169` (GPL-3.0-only). See [third-party notices](../THIRD-PARTY-NOTICES.md).

## Models and transport

| Model | Receiver | Validation |
|---|---|---|
| INZONE H5 | `054C:0EBF`, HID usage page `FF04`, 64-byte reports | Mirage Hub read/write checks on physical hardware, 2026-11-09 |
| INZONE Buds | `054C:0EC2`, same HID channel | Upstream hardware verification; Rust protocol fixtures |
| INZONE H9 | `054C:0E53`, USB serial | Upstream hardware verification; Rust capability fixtures |
| INZONE H7 | Same serial dongle as H9 | Upstream untested |
| INZONE H9 II | HID; identified by model reply `5` | Upstream hardware verification |
| Other receiver IDs from upstream | Capability-matched HID only | Experimental; identified from the model reply |

The serial channel uses 460800 baud, 8N1, no flow control, with RTS/DTR enabled. It is exclusive: close INZONE Hub or another controller before connecting H9/H7. HID receivers use shared handles. An unrelated Sony device or consumer-control collection is never sent Razer commands.

## Controls

The GUI displays headphone volume as 0–100% (hardware level 15 = 50%). Adjustments snap to the nearest supported hardware level. The CLI and RPC use the protocol's 0–30 volume levels. Game/chat balance is 0–100: **0 = game, 50 = balanced, 100 = chat**. Sidetone is 0–10. Writes first read the current packet and change only the selected field; sentinel and sibling bytes are preserved. A matching acknowledgement is required before reporting success.

Microphone mute is writable only on Buds. Other headsets report their physical mute state. Microphone level is a separate Windows capture-endpoint control, matched by USB topology, and is disabled if no unique matching endpoint exists. Emulated devices cannot access the real microphone.

Noise control, ambient level, voice focus, power-off choices and voice language are offered only for recognized values. H5 has no noise-cancelling controls. Buds expose left/right/case battery readings; the case is a relayed snapshot. `FF` and invalid percentages remain unknown. Battery status bytes are not guessed to be charging flags.

Firmware updates, OpenInzone's hotkey daemon and Stream Deck plugin are outside this integration. Proprietary Sony binaries and sound resources are not bundled.

## Sony APO audio effects (H5)

The H5 sound page controls the Sony APO already attached to the **Game** endpoint: ten EQ bands (31.5 Hz–16 kHz, −12 to +12 dB), EQ bypass, standard spatial sound and restoration of the configuration saved before Mirage Hub's first edit. These are **Windows settings**, not headset firmware settings. They continue working after both Mirage Hub and its daemon exit, as long as the Sony APO remains installed, Windows audio enhancements are enabled and the configured resources remain present. The Chat endpoint is never edited. Other INZONE models and personalized spatial profiles are not yet supported by these controls.

Exit INZONE Hub from its tray menu before editing; Mirage Hub refuses writes while that process is running. Endpoint discovery uses the receiver's USB topology and the eight-channel, 48 kHz Game format, requires exactly one match, and checks the Sony SFX/endpoint-effect bindings. A renamed Windows output still works; multiple H5 receivers, a different format or missing bindings fail closed.

**组件准备与使用：** 在 H5 的声音页面点击“准备 Sony 音效组件”。这一步只下载与校验，不安装 INZONE Hub，也不更改驱动。已有 Sony APO 的电脑可直接调整均衡器；切换“标准空间音效”时会优先使用 Mirage Hub 自己保存的资源。需要独立驱动时，再点击“安装 Sony 驱动…”并在 Windows UAC 中授权。安装可能使音频短暂重连，某些电脑需要重启。

**Optional components:** preparation downloads Sony's official INZONE Hub 1.0.20.0 package (144,917,976 bytes / about 138 MiB), checks its exact SHA-256 and Sony Authenticode signature, reads its MSI cabinet without executing the installer, then extracts only the 23 allowlisted driver/resource files (2,858,082 bytes). Each file has a pinned hash; the SYS, DLL and catalogue also require valid Microsoft hardware publisher signatures. Only the verified files are retained after success. Downloads and failed preparations can be retried. Component operations run in the daemon, independently of the window and Mirage Hub's application updater.

The separate installation button launches an embedded helper with UAC, revalidates bytes before placing the six driver files in an administrator-only temporary directory, and invokes Windows PnPUtil for the three signed INFs. It does not execute the complete Sony installer, uninstall INZONE Hub, downgrade a higher-ranked driver, reboot Windows or modify unrelated audio settings. A partial installation failure is reported and can be retried. Independent installation on a clean Windows system and cold-boot persistence have **not** yet been verified; this path is explicitly experimental.

Paths:

- Retained components: `%LOCALAPPDATA%\mirage-hub\sony-audio\1.0.20.0\files`.
- Runtime sound resources: `%PROGRAMDATA%\MirageHub\SonyAudio\1.0.20.0`. The Windows audio process needs read access; the user's private LocalAppData directory is unsuitable. Only non-executable sound/configuration resources are copied here.
- First-edit backups: `%LOCALAPPDATA%\mirage-hub\sony-audio\backups\{endpoint-guid}.yaml`.
- Active configuration: `%APPDATA%\Sony\INZONE Hub\APO\{endpoint-guid}.yaml`. The Sony APO expects this location; using a Mirage Hub-only configuration path would require changing the proprietary APO itself.

Preparing components preserves a separate copy of the files. **Uninstalling INZONE Hub may still remove Sony drivers and its active configurations**; after uninstalling it, the optional driver installation and a new Game-endpoint configuration may be necessary. Merely copying resource files does not guarantee that Windows will keep loading the APO. Mirage Hub does not run Sony's uninstaller for you.

EQ edits preserve unrelated DSP sections and comments, recalculate both parameters and normalized biquad coefficients, write atomically and verify readback. The first original file is backed up once. Standard spatial sound uses `standard_hrtf.hki` and `WH-G500_standard.ba`; disabling it keeps Sony's `downmix.hki` active so the Game output continues downmixing. Existing custom spatial settings remain untouched by EQ edits.

The implementation uses Rust throughout: `ureq` for HTTPS, `msi`/`cab` for portable archive parsing, and `ring` for SHA-256. Only signature validation, elevation, driver-store operations and audio endpoint access use a Windows module. No PowerShell script or external archive utility is needed.

Sources: [Sony official download and license information](https://www.sony.com/electronics/support/software/00279252), [RBJ peaking-EQ equations published by W3C](https://www.w3.org/TR/audio-eq-cookbook/#peakingeq), [Rust MSI reader](https://docs.rs/msi/0.10.0/msi/), [Rust CAB reader](https://docs.rs/cab/0.6.0/cab/). The pinned file metadata lives in `assets/sony-audio-manifest.json`; it contains no vendor binaries.

Hardware feasibility measurements with INZONE Hub stopped confirmed a 9.49 dB change at 8 kHz when bypassing the original EQ, plus rear-left-channel spatial rendering into both stereo output channels using separately extracted resources. Both changes took effect in an already-running audio stream. The original configuration was restored byte for byte after each test. These measurements establish APO runtime independence from the Hub process, not a completed clean-install certification.

The integrated Rust implementation was then tested on H5 with continuous WASAPI playback: a −6 dB setting at 1 kHz measured **−5.999997 dB**; standard spatial sound changed the right-channel tone from zero to **0.00056728** and back to effectively zero. Atomic file replacement hot-reloaded successfully, the INZONE Hub conflict guard rejected writes, and the restore action returned the exact original bytes. The generic Rust HTTPS/MSI/CAB path also downloaded and verified all 23 files. A direct driver-helper invocation without elevation was rejected before modifying drivers. Full elevated installation was not run on this already-configured machine.

Development checks (the first command is read-only; `prepare` downloads without installing):

```powershell
cargo run -p mirage-hub-daemon --example sony_audio -- read
cargo run -p mirage-hub-daemon --example sony_audio -- prepare
cargo test -p mirage-hub-daemon --lib inzone::apo
```

## H5 measurements

The following anonymized replies were read from a connected H5; model serial fields are omitted:

```text
01 link       01
02 model      03 00 00 00 FF 00  (first six bytes only)
04 battery    00 46
06 part 1     03 00 46 00 0F 32 32 0A 64
07 part 2     00 FF FF FF FF FF FF FF FF FF
08 part 3     FF FF FF FF FF 05 FF 02
21 volume     00 0F FF
22 balance    32
23 sidetone   0A 64
24 microphone 00 FF FF
81 power off  05
83 language   02
```

H5 part 3 has exactly eight bytes, ending after voice language. Its absent trailing slots are treated as unsupported only for this observed model/layout. Unknown short layouts still fall back to individual reads. The map's volume percentage can differ from the individual read's `FF` sentinel; setters preserve the individual read.

Hardware writes and readbacks succeeded for volume 14, balance 40, sidetone 9, and power-off values 0/5/15/30/60/180 minutes. Original values 15/50/10/5 were restored and read back. Other models were not physically tested in this workspace.

The complete `get_audio` path also read the H5's battery (70%), physical microphone state, voice language, and its matching Windows microphone endpoint (100%). The microphone level and voice language were read without changing them.

## Diagnostics and tests

```powershell
cd studio
cargo test -p mirage-hub-daemon --test inzone
cargo test -p mirage-hub-daemon --test manager
cargo run -p mirage-hub-daemon --example inzone_probe
cargo run -p mirage-hub-daemon --example inzone_probe -- --audio
```

The probe sends GET requests only by default and prints no model serials. Its explicit `--verify-h5-writes` option performs temporary H5 adjustments with restoration/readback; it should be used only when the headset owner expects it. Normal application operations are exposed through `device.call` and `mirage-hub-daemon headset`.
