"""Compare the native device API with Python using deterministic fake replies.

The runner and this oracle never access devices or the user's configuration.
Run `cargo build -p openrazer-daemon --example device_oracle` in studio first.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from openrazer_win.core.device import RazerDevice, EFFECTS
from openrazer_win.core.kraken import KrakenDevice
from openrazer_win.core.blackshark import BlackSharkDevice
from openrazer_win.core.ble_device import BleDevice
from openrazer_win.core.persistence import Persistence, ZONES
from openrazer_win.core.profiles import snapshot, apply_profile
from openrazer_win.devices import get_database, get_recipes
from openrazer_win.hid.base import HidDeviceInfo
from openrazer_win.protocol import blackshark
from openrazer_win.protocol.report import RazerReport


class Transport:
    def __init__(self, meta, argb):
        params = get_recipes().transport_params(meta.driver, meta.pid)
        self.info = HidDeviceInfo(path=f'oracle:{meta.pid:04x}', vendor_id=meta.vid,
                                  product_id=meta.pid, interface=3 if meta.pid in get_recipes().kraken_pids else params['index'], feature_length=91)
        self.wait = max(params['wait_us'], 1000) / 1_000_000
        self.argb_info = self.info if argb else None
        self.kraken_info = self.info if meta.pid in get_recipes().kraken_pids else None
        self.sent = []

    def send_payload(self, request, want_response=True):
        self.sent.append({'packet': list(request.pack()), 'response': want_response})
        if not want_response:
            return None
        reply = RazerReport.unpack(request.pack())
        reply.status = 2
        reply.arguments[:] = bytes((i * 17 + 23) % 256 for i in range(80))
        if request.command_class == 0 and request.command_id == 0x82:
            reply.arguments[:] = bytes(80)
            reply.set_arguments(0, f'ORACLE{self.info.product_id:04X}'.encode('ascii'))
        elif request.command_class == 0 and request.command_id == 0x81:
            reply.set_arguments(0, [2, 7])
        return reply

    def send_argb(self, channel, data):
        self.sent.append({'channel': channel, 'argb': list(data)})

    def close(self):
        pass

    def send_kraken(self, reports, settle=0):
        self.sent.extend({'output': list(report)} for report in reports)


class Channel:
    def __init__(self, meta):
        self.info = HidDeviceInfo(path=f'oracle:{meta.pid:04x}', vendor_id=meta.vid,
                                  product_id=meta.pid, interface=3, output_length=64, input_length=64)
        self.sent = []

    def exchange(self, request, accept):
        self.sent.append({'output': list(request)})
        answers = {blackshark.Command.SERIAL: f'ORACLE{self.info.product_id:04X}'.encode('ascii'),
                   blackshark.Command.FIRMWARE: [1, 7, 0, 0], blackshark.Command.EQ_BANDS: [n & 255 for n in range(-9, 1)],
                   blackshark.Command.BATTERY: [87], blackshark.Command.MIC_MUTED: [1], blackshark.Command.SIDETONE: [1],
                   blackshark.Command.EQ_ENABLED: [1], blackshark.Command.SIDETONE_LEVEL: [9], blackshark.Command.EQ_PRESET: [7],
                   blackshark.Command.IDLE_MINUTES: [15], blackshark.Command.DONGLE_LED: [2]}
        payload = [0] if request[10] & 128 else answers.get(request[10], [0])
        reply = bytearray(blackshark.request(request[2], request[9], request[10], payload))
        reply[1], reply[8], reply[11] = 2, 128, 1
        if self.info.product_id == blackshark.PID_WIRED:
            reply[9] = 128
        reply[62] = 0
        for byte in reply[1:62]:
            reply[62] ^= byte
        assert accept(reply)
        return bytes(reply)

    def close(self):
        pass


class Ble:
    address = 0xaabbccddeeff

    def __init__(self):
        self.sent = []

    def write(self, payload, hold=False):
        self.sent.append({'ble': list(payload), 'hold': hold})

    def read(self, opcode):
        self.sent.append({'read': opcode, 'payload': []})
        return bytes([{0x41: 127, 0x21: 87}.get(opcode, 1)])

    def release(self):
        self.sent.append({'release': True})

    def close(self):
        pass


def commands():
    result = []

    def add(method, *args, **kwargs):
        result.append({'method': method, 'args': args, 'kwargs': kwargs})

    add('enter_driver_mode', False)
    add('enter_driver_mode', True)
    add('leave_driver_mode')
    add('get_device_mode')
    add('set_device_mode', 3, 0)
    colours = [11, 22, 33, 44, 55, 66, 77, 88, 99]
    for zone in ZONES:
        add('available_effects', zone)
        add('get_brightness', zone)
        for percent in [42, -5, 120]:
            add('set_brightness', percent, zone=zone)
        for effect in EFFECTS:
            count = {'static': 3, 'reactive': 3, 'blinking': 3, 'breath_single': 3,
                     'breath_dual': 6, 'breath_triple': 9, 'starlight_single': 3,
                     'starlight_dual': 6}.get(effect, 0)
            args = colours[:count]
            if effect in ('wave', 'wheel'):
                args = [2]
            elif effect == 'reactive' or effect.startswith('starlight_'):
                args = [*args, 3]
            add(f'set_{effect}', *args, zone=zone)
    add('trigger_reactive')
    add('set_key_row', [0, 0, 1, 1, 2, 3, 4, 5, 6])
    add('set_custom_frame', [[0, 0, [9, 8, 7, 6, 5, 4]], [1, 0, [3, 2, 1]]])
    add('get_dpi')
    add('set_dpi', 3200)
    add('set_dpi', 100000, 10)
    add('get_dpi_stages')
    add('set_dpi_stages', 2, [[400, 500], [800, 900], [1600, 1700]])
    add('get_poll_rate')
    add('set_poll_rate', 125)
    add('supported_poll_rates')
    add('get_battery_level')
    add('is_charging')
    add('get_idle_time')
    add('set_idle_time', 300)
    add('get_low_battery_threshold')
    add('set_low_battery_threshold', 20)
    add('get_game_mode')
    add('set_game_mode', True)
    add('get_macro_mode')
    add('set_macro_mode', False)
    add('set_macro_effect', 1)
    add('set_fn_toggle', 1)
    add('get_keyboard_layout')
    add('get_scroll_mode')
    add('set_scroll_mode', 1)
    add('get_scroll_acceleration')
    add('set_scroll_acceleration', True)
    add('get_scroll_smart_reel')
    add('set_scroll_smart_reel', True)
    add('get_audio')
    add('get_microphone_muted')
    add('set_sidetone', True)
    add('set_sidetone_level', 12)
    add('set_sidetone_level', 16)
    add('set_equalizer', True)
    add('set_equalizer_preset', 'music')
    add('set_equalizer_preset', 'missing')
    add('set_equalizer_bands', list(range(-9, 1)))
    add('set_equalizer_bands', [7] * 10)
    add('set_do_not_disturb', False)
    add('set_dongle_led', 'battery')
    add('restore')
    add('profile_snapshot')
    add('profile_apply', {'device': {'zone_colours': [1, 2, 3, 4, 5, 6]},
                          'zones': {'backlight': {'effect': 'static', 'colors': [1, 2, 3, 4, 5, 6, 0, 0, 0], 'brightness': 73}}})
    add('profile_snapshot')
    return result


def reference(meta, argb, plan, scratch):
    store = Persistence(str(scratch / 'not-written.json'))
    if meta.transport == 'ble':
        transport = Ble()
        device = BleDevice(meta, transport, store)
        device.classic_address = 0xaabbccddeefe
    elif meta.pid in blackshark.PRODUCT_IDS:
        transport = Channel(meta)
        device = BlackSharkDevice(meta, transport, store)
    else:
        transport = Transport(meta, argb)
        device_type = KrakenDevice if meta.pid in get_recipes().kraken_pids else RazerDevice
        device = device_type(meta, transport, store)
    serial = device.serial
    try:
        firmware = device.firmware_version
    except Exception:
        firmware = None
    steps = []
    for command in plan:
        transport.sent.clear()
        ok = True
        try:
            if command['method'] == 'profile_snapshot':
                result = snapshot(device, store)
            elif command['method'] == 'profile_apply':
                result = apply_profile(device, store, command['args'][0])
            elif command['method'] == 'leave_driver_mode' and not hasattr(device, 'leave_driver_mode'):
                result = None
            else:
                result = getattr(device, command['method'])(*command['args'], **command['kwargs'])
        except Exception:
            ok, result = False, None
        steps.append({'ok': ok, 'result': result, 'sent': list(transport.sent), 'state': store.as_dict()})
    # JSON normalizes Python tuples exactly as the production RPC serializer.
    return json.loads(json.dumps({'serial': serial, 'firmware': firmware,
                                'capabilities': device.capabilities(), 'steps': steps}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runner', type=Path, default=ROOT / 'studio/target/debug/examples/device_oracle.exe')
    options = parser.parse_args()
    process = subprocess.Popen([str(options.runner)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               text=True, encoding='utf-8', bufsize=1,
                               creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == 'win32' else 0)
    plan = commands()
    devices = 0
    operations = 0
    try:
        with tempfile.TemporaryDirectory(prefix='openrazer-rust-oracle-') as directory:
            for meta in get_database():
                device_plan = plan + ble_commands() if meta.transport == 'ble' else plan
                argb = meta.pid == 0x0f1f
                expected = reference(meta, argb, device_plan, Path(directory))
                process.stdin.write(json.dumps({'pid': meta.pid, 'argb': argb, 'commands': device_plan}) + '\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError(f'Rust oracle ended unexpectedly: {process.poll()}')
                actual = json.loads(line)
                for key in ['serial', 'firmware', 'capabilities']:
                    if actual[key] != expected[key]:
                        raise AssertionError(f'{meta.pid:04x} {meta.name} {key}: {difference(actual[key], expected[key])}')
                for index, (a, e) in enumerate(zip(actual['steps'], expected['steps'])):
                    if a != e:
                        raise AssertionError(f'{meta.pid:04x} {meta.name} step {index} {device_plan[index]}: {difference(a, e)}')
                if len(actual['steps']) != len(expected['steps']):
                    raise AssertionError('incomplete native reply')
                devices += 1
                operations += len(device_plan)
        process.stdin.close()
        if process.wait(timeout=10):
            raise RuntimeError('Rust oracle failed')
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    print(f'{devices} HID/BLE devices, {operations} API operations: Python/Rust results, packets and persistence match')


def ble_commands():
    commands = []

    def add(method, *args, **kwargs):
        commands.append({'method': method, 'args': args, 'kwargs': kwargs})

    add('zone_colours_now')
    add('set_zone_colours', [[1, 2, 3]])
    add('zone_colours_now')
    add('set_static', 20, 30, 40, zone='left')
    add('set_none', zone='right')
    add('restore')
    add('set_zone_colours', [[-2, 300, 90], [7, 8, 9]], hold=False)
    add('zone_colours_now')
    add('set_zone_colours', [])
    add('set_zone_colours', [['invalid', 2, 3]])
    add('set_zone_colours', [[1, 2, 3]] * 3)
    for payload in [[0], [0, 1, 1, 9, 8, 7], [99, 0, 1, 4, 5, 6], [0, 3, 4, 1, 2, 3], [0, 2, 1]]:
        add('set_key_row', payload)
    add('set_none')
    add('restore')
    add('zone_colours_now')
    return commands


def difference(actual, expected, path=''):
    if isinstance(actual, dict) and isinstance(expected, dict):
        if actual.keys() != expected.keys():
            return f'{path} keys Rust={list(actual)} Python={list(expected)}'
        for key in actual:
            if actual[key] != expected[key]:
                return difference(actual[key], expected[key], f'{path}.{key}')
    if isinstance(actual, list) and isinstance(expected, list):
        if len(actual) != len(expected):
            return f'{path} lengths Rust={len(actual)} Python={len(expected)}'
        for index, (a, e) in enumerate(zip(actual, expected)):
            if a != e:
                return difference(a, e, f'{path}[{index}]')
    return f'{path} Rust={actual!r} Python={expected!r}'


if __name__ == '__main__':
    main()
