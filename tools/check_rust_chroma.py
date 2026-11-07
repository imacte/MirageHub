"""Compare native Chroma grids, all device projections and session transitions.

No server, game, device or user configuration is opened. Build the native
chroma_oracle example before running this check.
"""
from __future__ import annotations

import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.chroma import grid
from openrazer_win.chroma.bridge import ChromaBridge, KIND_OF
from openrazer_win.devices import get_database
from check_rust_devices import difference, reference as device_reference


def sessions(request):
    now = [0]
    bridge = ChromaBridge(lambda: [], clock=lambda: now[0])
    effects, answers = {}, []
    for action in request['actions']:
        now[0] = action['time']
        number = action.get('number', 0)
        try:
            op = action['op']
            result = None
            if op == 'open':
                result = bridge.open(action['info'])
            elif op == 'close':
                bridge.close(number)
            elif op == 'heartbeat':
                result = bridge.heartbeat(number)
            elif op == 'show':
                bridge.show(number, action['kind'], action['body'])
            elif op == 'create':
                effects[action['label']] = bridge.create(number, action['kind'], action['body'])
                result = action['label']
            elif op == 'apply':
                bridge.apply(number, [effects.get(v, v) for v in action['ids']])
            elif op == 'delete':
                bridge.delete(number, [effects.get(v, v) for v in action['ids']])
            elif op == 'expire':
                bridge._expire()
            else:
                raise AssertionError(op)
            answer = {'ok': True, 'result': result}
        except (ValueError, KeyError, TypeError):
            answer = {'ok': False}
        answers.append({'result': answer, 'status': bridge.status()})
    return answers


def reference(request):
    try:
        op = request['op']
        if op == 'parse':
            result = grid.parse(request['kind'], request['body'])
        elif op == 'project':
            caps = request['capabilities']
            device = type('Device', (), {'capabilities': lambda _: caps})()
            if caps.get('zone_colours'):
                device.set_zone_colours = None
            result = ChromaBridge.output(device, request['kind'], grid.parse(request['kind'], request['body']))
        elif op == 'resample':
            result = grid.resample(request['colours'], request['count'])
        elif op == 'sessions':
            result = sessions(request)
        else:
            raise AssertionError(op)
        return json.loads(json.dumps({'ok': True, 'result': result}))
    except (ValueError, TypeError, KeyError, IndexError):
        return {'ok': False}


def cases(scratch):
    rng = random.Random(9238)
    for kind in grid.GRIDS:
        for body in [{'effect': 'CHROMA_NONE'}, {'effect': 'CHROMA_STATIC', 'param': 255},
                     {'effect': 'CHROMA_STATIC', 'param': {'color': -1}}, {'effect': 'CHROMA_STATIC', 'param': {'color': ' 16711680 '}},
                     {'effect': 'CHROMA_CUSTOM', 'param': [[0], [], [255, 0xFF0000]]},
                     {'effect': 'CHROMA_CUSTOM_KEY', 'param': {'color': [[0]], 'key': [[0x010000FF]]}},
                     {'effect': 'CHROMA_CUSTOM2', 'param': {'color': [[0]], 'key': None}},
                     {'effect': 'CHROMA_CUSTOM', 'param': [[None]]}, {'effect': 'CHROMA_CUSTOM', 'param': []},
                     {'effect': 'CHROMA_CUSTOM', 'param': [[0] * 33]}, {'effect': 'CHROMA_CUSTOM', 'param': [[0]] * 33},
                     {'effect': 'CHROMA_WAVE'}, None]:
            yield {'op': 'parse', 'kind': kind, 'body': body}
    for _ in range(1000):
        kind = rng.choice(list(grid.GRIDS))
        values = [[rng.choice([rng.randrange(-2**32, 2**32), -123.7, True, '255']) for _ in range(rng.randrange(33))] for _ in range(rng.randrange(1, 33))]
        effect = rng.choice(['CHROMA_CUSTOM', 'CHROMA_CUSTOM_KEY', 'CHROMA_CUSTOM2'])
        param = {'color': values, 'key': [[rng.randrange(2**25) for _ in range(32)] for _ in range(8)]} if kind == 'keyboard' and effect != 'CHROMA_CUSTOM' else values
        yield {'op': 'parse', 'kind': kind, 'body': {'effect': effect, 'param': param}}
    for meta in get_database():
        caps = device_reference(meta, False, [], scratch)['capabilities']
        kind = KIND_OF.get(meta.type, 'chromalink')
        rows, columns = grid.GRIDS[kind]
        for body in [{'effect': 'CHROMA_NONE'}, {'effect': 'CHROMA_STATIC', 'param': {'color': 0x287CF2}},
                     {'effect': 'CHROMA_CUSTOM', 'param': [[rng.randrange(2**24) for _ in range(columns)] for _ in range(rows)]}]:
            yield {'op': 'project', 'kind': kind, 'body': body, 'capabilities': caps}
    for count in range(90):
        for length in (0, 1, 5, 14, 15, 22, 132):
            yield {'op': 'resample', 'count': count, 'colours': [[rng.randrange(256) for _ in range(3)] for _ in range(length)]}
    actions = []
    for index, info in enumerate([None, {}, {'title': ''}, {'title': '🌈' * 100}, {'title': True}, {'title': 123}, {'title': False}, {'title': 'Game'}], 1):
        actions.append({'op': 'open', 'info': info, 'time': 0})
        actions.append({'op': 'create', 'number': index, 'kind': 'keyboard', 'body': {'effect': 'CHROMA_NONE'}, 'label': f'effect{index}', 'time': 0})
    for index in range(1, 9):
        actions.append({'op': 'apply', 'number': index, 'ids': [f'effect{index}'], 'time': 0})
    for step in range(400):
        number = rng.randrange(1, 12)
        op = rng.choice(['heartbeat', 'show', 'apply', 'delete', 'close', 'expire'])
        action = {'op': op, 'number': number, 'time': step / 10}
        if op == 'show':
            action.update(kind=rng.choice(list(grid.GRIDS)), body={'effect': 'CHROMA_STATIC', 'param': {'color': rng.randrange(2**24)}})
        elif op in ('apply', 'delete'):
            action['ids'] = [f'effect{number}', 'missing'] if step % 4 == 0 else [f'effect{number}']
        actions.append(action)
    yield {'op': 'sessions', 'actions': actions}


def main():
    runner = ROOT / 'studio/target/debug/examples/chroma_oracle.exe'
    process = subprocess.Popen([str(runner)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                               encoding='utf-8', bufsize=1, creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == 'win32' else 0)
    count = 0
    try:
        with tempfile.TemporaryDirectory() as directory:
            for index, request in enumerate(cases(Path(directory))):
                expected = reference(request)
                process.stdin.write(json.dumps(request, ensure_ascii=False) + '\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError(f'oracle stopped: {process.poll()}')
                actual = json.loads(line)
                if actual != expected:
                    raise AssertionError(f'case {index} {request["op"]}: {difference(actual, expected)}; request={str(request)[:500]}')
                count += 1
        process.stdin.close()
        if process.wait(timeout=10):
            raise RuntimeError('native runner failed')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    print(f'{count} cases: Chroma grids, projections for all 279 models and session transitions match Python')


if __name__ == '__main__':
    main()
