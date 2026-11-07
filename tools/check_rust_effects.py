"""Compare native layer and effect frames against the original Python renderer.

No device, hook, audio capture, user configuration or sleep is involved.
Build the effect_oracle example in studio before running this check.
"""
from __future__ import annotations

import colorsys
import json
from pathlib import Path
import random
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.effects import engine
from openrazer_win.effects.frame import Frame
from openrazer_win.effects.layers import KINDS, LayerStack, clean_layers
from openrazer_win.effects.keyboard_hook import VK_TO_KEY, EXTENDED_OVERRIDES
from check_rust_devices import difference


def reference(request):
    try:
        if request['op'] == 'keynames':
            return {'ok': True, 'result': [[VK_TO_KEY.get(vk), EXTENDED_OVERRIDES.get(vk, VK_TO_KEY.get(vk))] for vk in range(256)]}
        if request['op'] == 'layers':
            rows, columns = request['rows'], request['columns']
            clean = clean_layers(request['layers'], rows, columns)
            stack = LayerStack(clean, rows, columns)
            frames = []
            for step in request['steps']:
                frames.append(list(stack.render(Frame(rows, columns), step['elapsed'],
                                                [(age, tuple(cell)) for age, cell in step['presses']]).to_payload()))
            result = {'clean': clean, 'animated': stack.animated, 'key_driven': stack.key_driven,
                          'longest_press': stack.longest_press, 'frames': frames}
        else:
            result = render_effect(request)
        return {'ok': True, 'result': result}
    except (ValueError, TypeError, KeyError, IndexError):
        return {'ok': False}


def render_effect(request):
    cap = request['capabilities']
    device = type('Device', (), {'matrix_dimensions': cap['matrix_dimensions'],
                                 'type': cap['type'], 'name': cap['name'], 'serial': 'ORACLE'})()
    options = dict(request['options'])
    if request['effect'] == 'zones':
        options['zones'] = {name: options['zones'][name] for name in cap['zone_order']}
    thread = engine.EffectThread(device, request['effect'], options, None)
    thread.started = 0.0
    needs_audio = thread.effect == 'music' or any(effect == 'music' for effect, _ in thread.zone_effects)
    peak = [0.0]
    if needs_audio:
        thread.meter = type('Meter', (), {'peak': lambda _: peak[0]})()
    frames, phase, now, shown = [], 0.0, [0.0], None
    clock, random_colour = engine.time.monotonic, engine._random_colour
    try:
        engine.time.monotonic = lambda: now[0]
        for step in request['steps']:
            peak[0] = step['peak']
            for key in step['keys']:
                now[0] = key['at']
                engine._random_colour = lambda hue=key['hue']: tuple(int(c * 255) for c in colorsys.hsv_to_rgb(hue, 1, 1))
                thread.on_key(key['name'])
            now[0] = step['elapsed']
            if step['accent'] is not None:
                thread._accent = tuple(step['accent'])
            thread._accent_colour = lambda: thread._accent
            if thread.stack:
                thread._presses = [p for p in thread._presses if now[0] - p[0] < thread.stack.longest_press]
                presses = [(now[0] - at, pos) for at, pos, _ in thread._presses]
                thread.stack.render(thread.frame, now[0], presses)
                changed = thread.frame != shown
                shown = thread.frame.copy()
            else:
                effect = thread.effect
                if effect in ('ripple', 'ripple_random'):
                    changed = thread._render_ripple()
                elif effect == 'spectrum_soft':
                    thread._render_spectrum(phase)
                    changed = True
                elif effect == 'wave_soft':
                    thread._render_wave(phase)
                    changed = True
                elif effect == 'music':
                    changed = thread._render_music()
                elif effect == 'zones':
                    changed = thread._render_zones(phase)
                else:
                    changed = thread._render_cells(effect, thread.colour)
                if changed or effect in ('zones',) + engine.CELL_EFFECTS:
                    phase = (phase + thread.refresh) % 10.0
            frames.append({'changed': changed, 'payload': list(thread.frame.to_payload()), 'phase': phase,
                               'peak': thread.recent_peak, 'level': thread.level})
    finally:
        engine.time.monotonic, engine._random_colour = clock, random_colour
    return {'frames': frames, 'still': thread.still, 'needs_keys': thread.needs_keys, 'needs_audio': needs_audio}


def cases():
    rng = random.Random(84102)
    yield {'op': 'keynames', 'steps': []}
    times = [0, .001, .04, .2, .599, .6, .8, 1, 2, 2.5, 4, 7.333, 10, 42]
    for index in range(120):
        rows, columns = [(6, 22), (1, 2), (4, 5), (1, 80)][index % 4]
        cells = [[r, c] for r in range(rows) for c in range(columns)]
        layers = []
        for _ in range(1 + index % 16):
            selected = rng.sample(cells, rng.randrange(1, len(cells) + 1))
            selected += [selected[0], [rows + 1, columns], [-1, 0]]
            layers.append({'kind': rng.choice(KINDS), 'cells': selected,
                               'colours': [[rng.randrange(-10, 270) for _ in range(3)] for _ in range(rng.randrange(1, 6))],
                               'speed': rng.choice([.05, .5, 1, 2, 7]), 'direction': rng.choice([1, 2]),
                               'name': '中文灯效' * 15, 'enabled': index % 7 != 0})
        steps = [{'elapsed': t, 'presses': [[age, rng.choice(cells)] for age in [0, .1, .4, 1, 9]]} for t in times]
        yield {'op': 'layers', 'rows': rows, 'columns': columns, 'layers': layers, 'steps': steps}
    for invalid in [None, 'bad', [3], [{'kind': 'unknown'}], [{'kind': 'static', 'cells': [['x', 0]]}],
                    [{'kind': 'static', 'colours': [[1]]}], [{'kind': 'static'}] * 17]:
        yield {'op': 'layers', 'rows': 6, 'columns': 22, 'layers': invalid, 'steps': []}
    for effect in engine.SOFTWARE_EFFECTS:
        for name, dimensions in [('Razer keyboard', [6, 22]), ('Tartarus', [4, 5]), ('Orbweaver', [4, 5]),
                                 ('Naga Hex V2', [1, 7]), ('Headset', [1, 2]), ('ARGB', [1, 80])]:
            cap = {'name': name, 'type': 'keyboard', 'matrix_dimensions': dimensions, 'zone_order': ['left', 'right']}
            options = {'colour': [240, 170, 60], 'refresh_rate': .04}
            if effect == 'zones':
                options['zones'] = {'left': {'effect': 'music', 'colour': [255, 100, 50]},
                                    'right': {'effect': 'breath_soft', 'colour': [10, 20, 255]}}
            if effect == 'layers':
                options['layers'] = [{'kind': kind, 'cells': [[r, c] for r in range(dimensions[0]) for c in range(dimensions[1])],
                                     'colours': [[240, 170, 60]]} for kind in ['static', 'reactive', 'ripple']]
            steps = []
            for index in range(150):
                elapsed = index * .04
                steps.append({'elapsed': elapsed, 'peak': rng.choice([0, .001, .02, .3, 1]),
                                  'accent': [20, 80, 250] if index == 40 else None,
                                  'keys': [{'name': rng.choice(['G', 'ESC', 'RETURN', 'ENTER', 'SPACE', 'NOT_A_KEY']),
                                             'at': elapsed, 'hue': rng.random()}] if index % 5 == 0 else []})
            yield {'op': 'effect', 'effect': effect, 'options': options, 'capabilities': cap, 'steps': steps}
    for left in engine.ZONE_EFFECTS:
        for right in engine.ZONE_EFFECTS:
            cap = {'name': 'Headset', 'type': 'headset', 'matrix_dimensions': [1, 2], 'zone_order': ['left', 'right']}
            options = {'zones': {'left': {'effect': left, 'colour': [255, 60, 20]},
                                  'right': {'effect': right, 'colour': [30, 80, 255]}}}
            steps = [{'elapsed': i * .04, 'peak': rng.random() if i % 3 else 0, 'keys': [],
                          'accent': [10, 30, 70] if i > 40 else None} for i in range(150)]
            yield {'op': 'effect', 'effect': 'zones', 'options': options, 'capabilities': cap, 'steps': steps}


def equal(actual, expected):
    if isinstance(actual, float) and isinstance(expected, (float, int)):
        return abs(actual - expected) <= 1e-12
    if isinstance(actual, dict) and isinstance(expected, dict):
        return actual.keys() == expected.keys() and all(equal(actual[k], expected[k]) for k in actual)
    if isinstance(actual, list) and isinstance(expected, list):
        return len(actual) == len(expected) and all(equal(a, e) for a, e in zip(actual, expected))
    return actual == expected


def main():
    runner = ROOT / 'studio/target/debug/examples/effect_oracle.exe'
    process = subprocess.Popen([str(runner)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                               encoding='utf-8', bufsize=1, creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == 'win32' else 0)
    count = frames = 0
    try:
        for index, request in enumerate(cases()):
            expected = reference(request)
            process.stdin.write(json.dumps(request, ensure_ascii=False) + '\n')
            process.stdin.flush()
            line = process.stdout.readline()
            if not line:
                raise RuntimeError(f'oracle stopped: {process.poll()}')
            actual = json.loads(line)
            if not equal(actual, expected):
                raise AssertionError(f'case {index} {request.get("effect", "layers")}: {difference(actual, expected)}')
            count += 1
            frames += len(request['steps'])
        process.stdin.close()
        if process.wait(timeout=10):
            raise RuntimeError('native runner failed')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    print(f'{count} scenarios, {frames} frames: layer validation, RGB payloads and renderer state match Python')


if __name__ == '__main__':
    main()
