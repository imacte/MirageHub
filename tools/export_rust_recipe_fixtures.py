"""Record recipe lookup and execution against the original Python backend.

No hardware is accessed. Every reply is deterministic and synthetic; the
corpus checks command selection and interpretation, not physical USB I/O.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.devices.database import get_database
from openrazer_win.devices.recipes import Environment, NotSupported, RecipeError, execute, get_recipes
from openrazer_win.protocol.report import RazerReport


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='fail if the checked-in oracle is stale')
    options = parser.parse_args()
    table = get_recipes()
    pool_indices = {id(steps): i for i, steps in enumerate(table._pool)}
    selected = {}
    lookups = []
    for device in get_database():
        supported = {}
        for attribute in table.attributes(device.driver):
            try:
                steps, bindings = table.lookup(device.driver, attribute, device.pid)
            except NotSupported:
                continue
            ref = pool_indices[id(steps)]
            supported[attribute] = [ref, bindings]
            selected.setdefault((ref, json.dumps(bindings, sort_keys=True)),
                                (device.driver, attribute, device.pid, steps, bindings))
        lookups.append({'driver': device.driver, 'pid': device.pid, 'attributes': supported,
                        'transport': table.transport_params(device.driver, device.pid),
                        'blade': device.pid in table.blade_pids, 'kraken': device.pid in table.kraken_pids})

    variables = {
        'acceleration': 1, 'active_stage': 1, 'active_varstore': 1, 'brightness': 173,
        'channel': 2, 'direction': 2, 'dpi': [800, 900, 1600, 1700], 'dpi_x': 3200,
        'dpi_x_byte': 76, 'dpi_y': 1800, 'dpi_y_byte': 45, 'enabled': 1, 'idle_time': 300,
        'led': 5, 'led_id': 5, 'mode': 3, 'offset': 3, 'orochi2011_dpi': 76,
        'orochi2011_led': 3, 'orochi2011_poll': 2, 'param': 0, 'pid': 0x0078,
        'polling_rate': 1000, 'rgb': [12, 34, 56], 'rgb1': [78, 90, 12],
        'row_id': 1, 'saved_brightness': 142, 'scroll_mode': 1, 'smart_reel': 1,
        'speed': 3, 'stages_count': 2, 'start_col': 0, 'state': 1, 'stop_col': 2,
        'sz': 2, 'threshold': 35, 'varstore': 1,
    }
    buffers = [[], [0], [2], [7, 8], [17, 29, 41], [3, 13, 23, 33],
               [11, 22, 33, 44, 55, 66], [2, 11, 22, 33, 44, 55, 66],
               [3, 11, 22, 33, 44, 55, 66, 77, 88, 99],
               [0, 0, 1, 11, 22, 33, 44, 55, 66, 1, 0, 0, 77, 88, 99]]
    cases = []
    errors = Counter()
    for driver, attribute, pid, steps, bindings in selected.values():
        for buf in buffers:
            initial = {**variables, **bindings}
            env = Environment(bytes(buf), **initial)
            sent = []

            def send(request, want_response, record=sent):
                record.append({'packet': list(request.pack()), 'response': want_response})
                if not want_response:
                    return None
                reply = RazerReport.unpack(request.pack())
                reply.status = 2
                reply.arguments[:] = bytes((i * 17 + 23) % 256 for i in range(80))
                return reply

            def send_argb(channel, data, record=sent):
                record.append({'channel': channel, 'argb': list(data)})

            send.send_argb = send_argb
            failure = None
            try:
                execute(steps, env, send)
            except (ValueError, TypeError, KeyError, IndexError, RecipeError) as error:
                failure = type(error).__name__
                errors[str(error)] += 1
            cases.append({'driver': driver, 'attribute': attribute, 'pid': pid,
                          'buf': buf, 'bindings': bindings, 'sent': sent, 'error': failure,
                          'out': list(env.out),
                          'changed_vars': {k: v for k, v in env.vars.items() if k not in initial or v != initial[k]},
                          'removed_vars': [k for k in initial if k not in env.vars],
                          'reply': list(env.response.pack()) if env.response else None})
    sources = ['openrazer_win/devices/recipes.py', 'openrazer_win/devices/data/recipes.json']
    destination = ROOT / 'studio/daemon/tests/fixtures/recipes.json'
    payload = {'sources': {p: hashlib.sha256((ROOT / p).read_text(encoding='utf-8').encode('utf-8')).hexdigest() for p in sources},
               'variables': variables, 'pool': table._pool, 'lookups': lookups, 'cases': cases}
    content = json.dumps(payload, ensure_ascii=False, separators=(',', ':'), default=list) + '\n'
    if options.check:
        if not destination.exists() or destination.read_text(encoding='utf-8') != content:
            raise SystemExit('Recipe fixture is stale: run tools/export_rust_recipe_fixtures.py')
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding='utf-8')
    print(f'{len(lookups)} devices, {len(selected)} distinct recipes/bindings, {len(cases)} executions')
    print(f'{sum(c["error"] is None for c in cases)} successful, {sum(errors.values())} rejected by the Python oracle')
    print('Oracle rejection types:', errors.most_common(8))


if __name__ == '__main__':
    main()
