"""Freeze the Python backend's wire output for native-backend regression tests.

This is a development-only oracle. Rust builds and executables never invoke
Python; cargo test reads the checked-in fixture directly.
"""
from __future__ import annotations

import argparse
import hashlib
import inspect
import json
from pathlib import Path
import random
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.devices.database import get_database
from openrazer_win.protocol.chroma import BUILDERS
from openrazer_win.protocol import blackshark, kraken, razer_ble


def headset_cases():
    ble = []
    for level in [-9, 0, 1, 127, 128, 255, 999]:
        ble.append({'operation': 'brightness', 'level': level,
                    'packet': list(razer_ble.brightness_command(level)),
                    'percent': razer_ble.level_to_percentage(level)})
    for percent in [-1, 0, 0.5, 10, 25, 50, 75, 90, 99.5, 100, 101]:
        ble.append({'operation': 'percentage', 'percent': percent,
                    'level': razer_ble.percentage_to_level(percent)})
    for colours in [[(255, 0, 0)], [(11, 22, 33), (44, 55, 66)]]:
        ble.append({'operation': 'colour', 'colours': colours,
                    'packet': list(razer_ble.colour_command(colours))})
    for opcode in [razer_ble.OP_STATUS, razer_ble.OP_BATTERY, razer_ble.OP_IDLE_TIMER,
                   razer_ble.OP_CHARGING, razer_ble.OP_BRIGHTNESS, razer_ble.OP_SET_IDLE_TIMER]:
        ble.append({'operation': 'read', 'opcode': opcode, 'packet': list(razer_ble.read_request(opcode))})
    ble.append({'operation': 'takeover', 'packet': list(razer_ble.takeover_command())})
    ble.append({'operation': 'off', 'packet': list(razer_ble.off_command())})

    shark = []
    for sequence in [0, 1, 31, 32, 63, 255]:
        for domain in [0, 128]:
            for command, payload in [(0, []), (0x21, []), (0x99, [0]), (0x95, list(range(10))), (0x15, list(range(48)))]:
                shark.append({'sequence': sequence, 'domain': domain, 'command': command,
                              'payload': payload, 'packet': list(blackshark.request(sequence, domain, command, payload))})
    bands = [list(range(-9, 1)), [0] * 10, [6] * 10]

    kraken_cases = []
    colours = [(11, 22, 33), (44, 55, 66), (77, 88, 99)]
    for pid, layout in sorted(kraken.LAYOUTS.items()):
        operations = [('none', kraken.effect_none(layout)), ('spectrum', kraken.effect_spectrum(layout)),
                      ('static', kraken.effect_static(layout, *colours[0], intensity=127, with_colour=pid not in kraken.COLOURLESS_PIDS)),
                      ('custom', kraken.effect_custom(layout, *colours[0], intensity=43))]
        operations += [(f'breathing_{count}', kraken.effect_breathing(layout, colours[:count]))
                       for count in range(1, layout.breathing_modes + 1)]
        for operation, reports in operations:
            kraken_cases.append({'pid': pid, 'operation': operation, 'packets': [list(r) for r in reports]})
    return {'ble': ble, 'blackshark': shark, 'kraken': kraken_cases,
            'eq': [{'gains': g, 'encoded': list(blackshark.encode_bands(g)),
                    'stored': list(bytes(v & 255 for v in g)),
                    'decoded': blackshark.decode_bands(bytes(v & 255 for v in g))} for g in bands]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='fail if the checked-in oracle is stale')
    options = parser.parse_args()
    randomizer = random.Random(1532)
    cases = []
    boundaries = [-1, 0, 1, 2, 3, 4, 255, 4096, 65535]
    for name, builder in sorted(BUILDERS.items()):
        parameters = inspect.signature(builder).parameters
        for index, boundary in enumerate(boundaries):
            args = []
            count = index % 6
            start = index % 3
            stop = start + (40 if index == 8 else index % 7)
            for parameter in parameters:
                if parameter == 'rgb_data':
                    value = [randomizer.randrange(256) for _ in range(126)]
                elif parameter.startswith('rgb'):
                    value = [randomizer.randrange(256) for _ in range(3)]
                elif parameter == 'dpi':
                    value = [randomizer.randrange(100, 45001) for _ in range(count * 2)]
                elif parameter == 'count':
                    value = count
                elif parameter == 'start_col':
                    value = start
                elif parameter == 'stop_col':
                    value = stop
                elif parameter == 'packet_length':
                    value = 0 if index % 2 else 0x47
                else:
                    value = boundary
                args.append(value)
            cases.append({'builder': name, 'args': args, 'packet': list(builder(*args).pack())})
            # Exercise every optional argument's default independently as well.
            required = sum(p.default is inspect.Parameter.empty for p in parameters.values())
            if required < len(args):
                short = args[:required]
                cases.append({'builder': name, 'args': short, 'packet': list(builder(*short).pack())})

    sources = ['openrazer_win/protocol/chroma.py', 'openrazer_win/protocol/report.py',
               'openrazer_win/devices/database.py', 'openrazer_win/devices/data/devices.json',
               'openrazer_win/protocol/razer_ble.py', 'openrazer_win/protocol/kraken.py',
               'openrazer_win/protocol/blackshark.py']
    payload = {
        'sources': {p: hashlib.sha256((ROOT / p).read_text(encoding='utf-8').encode('utf-8')).hexdigest() for p in sources},
        'builders': cases,
        'devices': [entry.to_dict() for entry in get_database()],
        'headsets': headset_cases(),
    }
    destination = ROOT / 'studio/daemon/tests/fixtures/protocol.json'
    content = json.dumps(payload, ensure_ascii=False, separators=(',', ':')) + '\n'
    if options.check:
        if not destination.exists() or destination.read_text(encoding='utf-8') != content:
            raise SystemExit('Protocol fixture is stale: run tools/export_rust_protocol_fixtures.py')
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding='utf-8')
    print(f'{len(BUILDERS)} builders, {len(cases)} packet cases, {len(payload["devices"])} devices')


if __name__ == '__main__':
    main()
