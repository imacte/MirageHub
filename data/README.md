# Device data

These files are embedded by the Rust backend; they are required at build time and do not require Python.

- `devices.json`: device identities, capabilities and metadata.
- `recipes.json`: per-device protocol operations derived from OpenRazer's driver sources.
- `keys.json` and `keymap.json`: keyboard report and key mappings.
- `upstream-version.txt`: the upstream revision represented by the bundled data.
- `upstream_patches/`: provenance for support taken from pending upstream changes.

The base data comes from [OpenRazer](https://github.com/openrazer/openrazer) at `3.12.4+477f4d6`,
with the documented pending patch and Windows-specific additions. Derived data remains covered
by the project's GPL-2.0-or-later licence.

The Python extractors and transpilers were removed with the legacy backend. They remain available
at commit `6198c1d` in Git history (`tools/`); automatic upstream imports are not part of the new
Rust tool. Keep changes tied to an upstream source or verified protocol capture, and update native
protocol/recipe regression tests when behaviour changes.

The native Naga V2 Pro onboard implementation follows
[OpenRazer #2962](https://github.com/openrazer/openrazer/pull/2962), commit
`73071e54b3eb6f657518ea95fd223d22e97b2432`, by Daniel Hobley. Only `00A7` and
`00A8` gain its metadata methods. Wire commands live in
`studio/daemon/src/protocol/onboard.rs`, with HID emulator and regression fixtures;
the legacy recipe generator is not involved. Writes are never retried, BUSY reads
are bounded, and macro deletion checks all known buttons, stores and layers.

After changing devices or the upstream version, regenerate the documentation table:

```powershell
cd studio
cargo run --locked -p rsrazer-tools -- docs-data
```
