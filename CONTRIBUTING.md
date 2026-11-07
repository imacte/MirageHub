# Contributing

## Build and check

Clone `git@github.com:imacte/RsRazer.git`. On Windows, install Microsoft C++ Build Tools
with the Windows SDK and the Rust version pinned in `studio/rust-toolchain.toml`.

```powershell
./studio/tools/build-native.ps1 -IncludeClassic
cd studio
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p rsrazer-tools -- check-translations
cargo run --locked -p rsrazer-tools -- docs-data --check
cd ../gui
cargo clippy --locked -- -D warnings
cargo test --locked
```

Tests use simulated devices and recorded protocol fixtures. Executable tests create isolated
configuration directories. `studio/tools/installer-cycle.ps1` is for clean CI accounts only;
it installs, repairs and removes the application.

The portable `rsrazer-tools` crate also builds on Linux without the Windows application.
No repository tool requires Python.

After configuring GitHub Pages to use Actions, set the repository variable `PAGES_ENABLED=true`
to enable the documentation deployment workflow. CI checks the documentation data independently.
`studio/tools/capture-docs.ps1` refreshes screenshots from simulated devices after a native build.

## Repository tools

Run these from `studio/`:

```powershell
cargo run --locked -p rsrazer-tools -- check-translations
cargo run --locked -p rsrazer-tools -- docs-data
cargo run --locked -p rsrazer-tools -- release-notes v1.6.1
cargo run --locked -p rsrazer-tools -- icons
```

Translations live in `studio/core/src/ru.rs` and `zh_cn.rs`. The checker checks missing and unused
strings, duplicate keys and formatting placeholders. Release notes come from the matching version
in `CHANGELOG.md`, with optional `CHANGELOG.ru.md` and `CHANGELOG.zh-CN.md` translations.
English notes are required. Language markers let the Updates page select the appropriate text.
Changes people notice belong in the Unreleased section of the changelog.

## Testing updates

`OPENRAZER_STUDIO_TEST_FEED` can be set at build time to use a test release feed.
Released builds use the RsRazer GitHub repository; runtime settings cannot redirect that feed.
The legacy environment-variable prefix is retained for existing development setups.

## Device support

Include `rsrazer-daemon doctor` output in a device report. See [data/README.md](data/README.md)
for provenance and the historical generators. Current tools do not automatically import upstream
releases. Protocol or database changes should cite their upstream source and include regression
cases against the Rust emulator or report fixtures.

## Source layout

The workspace is under `studio/`: `daemon` contains protocols, transports, effects, keys, RPC and
the CLI; `app`, `core`, `setup`, `chroma` and `tools` contain the UI, shared components, installer,
SDK DLL and development tools. The classic panel has its own workspace in `gui/`.
Shared branding geometry lives under `assets/`; device data lives under `data/`.

Keep changes focused, retain protocol attribution, and explain hardware quirks in comments.
