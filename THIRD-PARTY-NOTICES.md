# Third-party notices

Mirage Hub combines device support from the projects below. The combined work is distributed under **GPL-3.0-only**; see [LICENSE](LICENSE).

## OpenRazer and openrazer-win

Razer device data, protocol recipes and portions of the Windows implementation derive from [OpenRazer](https://github.com/openrazer/openrazer) and [openrazer-win](https://github.com/Ar4ikov/openrazer-win). Their original copyright notices and GPL-2.0-or-later terms remain applicable to those portions. GPL version 3 is selected when distributing this combined work. The original GPL version 2 text is retained in [LICENSES/GPL-2.0.txt](LICENSES/GPL-2.0.txt).

## OpenInzone

Sony INZONE protocol framing, settings semantics, capability maps and serial transport behavior were ported to Rust from [penguinwokrs/openinzone](https://github.com/penguinwokrs/openinzone), revision `4dccfdef99755c51b9f7cce109565da622625169`.

Copyright (C) 2026 penguinwokrs. SPDX-License-Identifier: GPL-3.0-only. The port is in `studio/daemon/src/protocol/inzone.rs`, `studio/daemon/src/inzone/` and `studio/daemon/src/device/inzone.rs`. The license text is also retained in [LICENSES/GPL-3.0.txt](LICENSES/GPL-3.0.txt).

Mirage Hub is independent of Razer and Sony. Their names identify compatible hardware; neither company endorses this project. No proprietary INZONE Hub code or assets are included. Optional Sony audio drivers/resources are downloaded directly from Sony only when requested by the user and remain subject to Sony's terms. The source tree contains extraction instructions and hash metadata, not those binaries. See [Sony audio setup](docs/INZONE.md#sony-apo-audio-effects-h5).
