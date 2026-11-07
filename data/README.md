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

After changing devices or the upstream version, regenerate the documentation table:

```powershell
cd studio
cargo run --locked -p rsrazer-tools -- docs-data
```
