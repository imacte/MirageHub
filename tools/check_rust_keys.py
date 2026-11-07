"""Compare native key encoding and macro validation without touching the desktop.

Build studio's key_oracle example first. The Python injector only records its
encoded inputs; neither implementation calls SendInput or opens a keyboard.
"""
from __future__ import annotations

import json
from pathlib import Path
import random
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.devices import get_database
from openrazer_win.keys import codes, inject, macros, service
from check_rust_devices import difference


class Encoder(inject.Injector):
    def __init__(self):
        self.events = []

    def _emit(self, events):
        self.events.extend(events)


def reference(request):
    try:
        op = request['op']
        if op == 'source':
            result = codes.source_name(request['scan'], request['ext'], request['vk'])
        elif op == 'output':
            result = codes.output_for(request['name'])
        elif op == 'named':
            result = codes.key_named(request['name'])
        elif op == 'sequence':
            events = codes.parse_sequence(request['text'])
            result = {'events': events, 'description': [codes.describe_event(event) for event in events]}
        elif op in ('key', 'raw', 'text'):
            encoder = Encoder()
            if op == 'key':
                target = request['target']
                encoder.key(codes.output_for(target) if isinstance(target, str) else target, request['down'])
            elif op == 'raw':
                event = request['event']
                encoder.raw(event['vk'], event['scan'], event['ext'], event['down'])
            else:
                encoder.text(request['text'])
            result = encoder.events
        elif op == 'validate':
            result = macros.validate(request['action'])
        elif op == 'report':
            held = service.parse_report(bytes(request['report']))
            result = None if held is None else sorted(held)
        elif op == 'metadata':
            data = service.load_key_data()
            result = [{'pid': info.pid, 'wants': info.driver == 'kbd' and info.driver_mode and f'{info.pid:04x}' not in data['no_translation_pids'],
                       'table': dict(data['tables'].get(data['tables_by_pid'].get(f'{info.pid:04x}', data['default_table']), []))}
                      for info in get_database()]
        else:
            raise AssertionError(op)
        return json.loads(json.dumps({'ok': True, 'result': result}))
    except (ValueError, TypeError, KeyError, AttributeError):
        return {'ok': False}


def cases():
    rng = random.Random(8186)
    yield {'op': 'metadata'}
    for scan in range(256):
        for ext in (False, True):
            for vk in (0, 0x13):
                yield {'op': 'source', 'scan': scan, 'ext': ext, 'vk': vk}
    for name in [*codes.KEYS, *codes.VIRTUAL_KEYS, *codes.USB_HID_NAMES,
                 'USB_HID_KEY_F1', 'USB_HID_KEY_MEDIA_STOPCD', 'NOT_A_KEY']:
        yield {'op': 'output', 'name': name}
    for name in [*(name[4:] for name in codes.KEYS), *(name[4:] for name in codes.VIRTUAL_KEYS), *codes._ALIASES, '', 'bogus']:
        for spelling in (name, f'  {name.lower()}  '):
            yield {'op': 'named', 'name': spelling}
    for target in [*codes.KEYS, *codes.VIRTUAL_KEYS, *range(256)]:
        for down in (False, True):
            yield {'op': 'key', 'target': target, 'down': down}
    for _ in range(1000):
        yield {'op': 'raw', 'event': {'vk': rng.randrange(256), 'scan': rng.choice([0, rng.randrange(65536)]),
                                      'ext': rng.choice([False, True]), 'down': rng.choice([False, True]), 'delay': 0}}
    for text in ['', 'ctrl+bogus', '+++', 'CTRL+SHIFT+ESC', 'ctrl+c, ctrl+v', 'win+shift+s', 'altgr+q', 'mute play f24', '你好']:
        yield {'op': 'sequence', 'text': text}
    names = list(codes._ALIASES) + [name[4:] for name in codes.KEYS]
    for _ in range(800):
        yield {'op': 'sequence', 'text': ', '.join('+'.join(rng.choices(names, k=rng.randrange(1, 5))) for _ in range(rng.randrange(1, 5)))}
    for text in ['', '中文😀\r\n', 'Hello\tWorld', '\0', ''.join(chr(n) for n in range(0x10000) if not 0xd800 <= n <= 0xdfff), '🌈' * 4000]:
        yield {'op': 'text', 'text': text}
    for action in [None, [], {}, {'type': 'teleport'}, {'type': 'keys', 'events': []}, {'type': 'text', 'text': ''},
                   {'type': 'text', 'text': '🌈' * 4000}, {'type': 'text', 'text': '🌈' * 4001},
                   {'type': 'launch', 'target': '  notepad.exe  '}, {'type': 'launch', 'target': '\t'},
                   {'type': 'disabled', 'extra': 5}, {'type': 'keys', 'events': [{'vk': 65}] * 1001}]:
        yield {'op': 'validate', 'action': action}
    integers = [None, 'bad', [], {}, 0, 1, -1, ' 65 ', True, False, 1.9, -1.9, 255, 256, 65535, 65536, 30000, 30001]
    truths = [None, False, True, 0, 1, [], [0], {}, {'x': 0}, '', 'false']
    for _ in range(2000):
        event = {key: rng.choice(integers) for key in ('vk', 'scan', 'delay') if rng.random() < .9}
        event.update({key: rng.choice(truths) for key in ('ext', 'down') if rng.random() < .9})
        yield {'op': 'validate', 'action': {'type': 'keys', 'events': [event]}}
    for _ in range(300):
        yield {'op': 'report', 'report': [rng.choice([0, 3, 4])] + [rng.randrange(256) for _ in range(rng.randrange(32))]}
    yield {'op': 'report', 'report': []}


def main():
    runner = ROOT / 'studio/target/debug/examples/key_oracle.exe'
    process = subprocess.Popen([str(runner)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                               encoding='utf-8', bufsize=1, creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == 'win32' else 0)
    count = 0
    try:
        for index, request in enumerate(cases()):
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
    print(f'{count} cases: key translation, input encoding, macro validation and device tables match Python')


if __name__ == '__main__':
    main()
