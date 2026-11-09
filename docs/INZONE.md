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

Volume is 0–30. Game/chat balance is 0–100: **0 = game, 50 = balanced, 100 = chat**. Sidetone is 0–10. Writes first read the current packet and change only the selected field; sentinel and sibling bytes are preserved. A matching acknowledgement is required before reporting success.

Microphone mute is writable only on Buds. Other headsets report their physical mute state. Microphone level is a separate Windows capture-endpoint control, matched by USB topology, and is disabled if no unique matching endpoint exists. Emulated devices cannot access the real microphone.

Noise control, ambient level, voice focus, power-off choices and voice language are offered only for recognized values. H5 has no noise-cancelling controls. Buds expose left/right/case battery readings; the case is a relayed snapshot. `FF` and invalid percentages remain unknown. Battery status bytes are not guessed to be charging flags.

Sony APO equalization/spatial processing, firmware updates, vendor assets, OpenInzone's hotkey daemon and Stream Deck plugin are outside this integration.

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
