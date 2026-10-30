#!/usr/bin/env python3
"""Bound idle CPU and pointer latency on a freshly built 1080p desktop (Linux host)."""
import json
import os
import pathlib
import re
import socket
import subprocess
import tempfile
import time
import sys
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True
sys.path.insert(0,str(ROOT/'tools'))
from qemu_platform import arguments as platform_arguments
from qemu_test import disk_image, copy_disk


def cpu_seconds(pid):
    fields = pathlib.Path(f'/proc/{pid}/stat').read_text().split(') ', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')


def main():
    with tempfile.TemporaryDirectory(prefix='rustos-perf-') as tmp:
        tmp = pathlib.Path(tmp)
        disk = copy_disk(disk_image(ROOT), tmp)
        log = tmp / 'serial.log'
        sock = tmp / 'qmp'
        qemu = subprocess.Popen([
            'qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-machine', 'pc,i8042=on,vmport=on', '-vga', 'std',
            '-serial', f'file:{log}', '-display', 'none', '-no-reboot',
            '-qmp', f'unix:{sock},server,nowait'] + platform_arguments())
        try:
            end = time.monotonic() + 60
            while not sock.exists() or '[desktop] ready:' not in log.read_text(errors='replace'):
                assert qemu.poll() is None, 'QEMU exited'
                assert time.monotonic() < end, 'desktop boot timeout'
                time.sleep(.1)
            with socket.socket(socket.AF_UNIX) as qmp:
                qmp.settimeout(3)
                qmp.connect(str(sock))
                stream = qmp.makefile('rwb')
                json.loads(stream.readline())

                def command(name, arguments=None):
                    request = {'execute': name}
                    if arguments is not None:
                        request['arguments'] = arguments
                    stream.write(json.dumps(request).encode() + b'\n')
                    stream.flush()
                    while True:
                        response = json.loads(stream.readline())
                        assert 'error' not in response, response
                        if 'return' in response:
                            return response['return']

                command('qmp_capabilities')
                time.sleep(2)
                start = time.monotonic()
                cpu = cpu_seconds(qemu.pid)
                time.sleep(3)
                idle = (cpu_seconds(qemu.pid) - cpu) / (time.monotonic() - start)
                latencies = []
                last_point = None
                for x, y in [(1919, 1079), (960, 540), (0, 0), (400, 300)] * 2:
                    start = time.monotonic()
                    command('input-send-event', {'events': [
                        {'type': 'abs', 'data': {'axis': axis, 'value': (v * 32767 + size - 2) // (size - 1)}}
                        for axis, v, size in [('x', x, 1920), ('y', y, 1080)]]})
                    while True:
                        capture = tmp / 'screen.ppm'
                        command('screendump', {'filename': str(capture)})
                        with Image.open(capture) as image:
                            assert image.size == (1920, 1080)
                            found = image.getpixel((x, y)) == (8, 19, 33)
                            if found and last_point is not None:
                                assert image.getpixel(last_point) != (8, 19, 33), 'old cursor was not restored'
                        if found:
                            break
                        assert time.monotonic() - start < 1, f'pointer timeout at {(x, y)}'
                        time.sleep(.005)
                    latencies.append(time.monotonic() - start)
                    last_point = (x, y)
                print(f'idle CPU: {idle:.1%} of one host core; pointer max: {max(latencies)*1000:.0f} ms', flush=True)
                assert idle < .15, f'idle desktop burns {idle:.1%} CPU (budget 15%)'
                assert max(latencies) < .2, f'pointer response exceeded 200 ms: {latencies}'
                def click(x, y):
                    command('input-send-event', {'events': [
                        {'type': 'abs', 'data': {'axis': axis, 'value': (v * 32767 + size - 2) // (size - 1)}}
                        for axis, v, size in [('x', x, 1920), ('y', y, 1080)]]})
                    time.sleep(.1)
                    for down in [True, False]:
                        command('input-send-event', {'events': [
                            {'type': 'btn', 'data': {'button': 'left', 'down': down}}]})
                        time.sleep(.1)

                def idle_sample(name, seconds, budget):
                    start = time.monotonic()
                    cpu = cpu_seconds(qemu.pid)
                    time.sleep(seconds)
                    usage = (cpu_seconds(qemu.pid) - cpu) / (time.monotonic() - start)
                    print(f'{name} CPU: {usage:.1%} of one host core', flush=True)
                    assert usage < budget, f'{name} burns {usage:.1%} CPU (budget {budget:.0%})'

                click(350, 65)
                end = time.monotonic() + 10
                while 'self_rust_os forge v1.0' not in log.read_text(errors='replace'):
                    assert time.monotonic() < end, 'terminal launch timeout'
                    time.sleep(.05)
                end = time.monotonic() + 1
                while True:
                    capture = tmp / 'terminal.ppm'
                    command('screendump', {'filename': str(capture)})
                    with Image.open(capture) as image:
                        has_text = image.crop((90, 154, 560, 230)).getcolors(50000) or []
                        has_text = any(color == (190, 213, 232) for count, color in has_text)
                    if has_text:
                        break
                    assert time.monotonic() < end, 'terminal surface snapshot missing its text pixels'
                    time.sleep(.02)
                idle_sample('idle terminal', 2, .15)
                # A short burst should not accumulate a frame of delay for
                # every make/break scancode in the keyboard queue.
                for key in ['e', 'c', 'h', 'o', 'spc', 's', 'p', 'e', 'e', 'd', 'ret']:
                    command('human-monitor-command', {'command-line': f'sendkey {key} 1'})
                    time.sleep(.025)
                start = time.monotonic()
                end = start + .15
                def output():
                    return ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',
                        log.read_text(errors='replace'), re.S))
                while '[portal] output: speed\n' not in output():
                    assert time.monotonic() < end, 'keyboard burst took over 150 ms to drain'
                    time.sleep(.01)
                print(f'keyboard burst drain: {(time.monotonic()-start)*1000:.0f} ms', flush=True)
                click(450, 65)
                end = time.monotonic() + 10
                while 'Snake game starting' not in log.read_text(errors='replace'):
                    assert time.monotonic() < end, 'Snake launch timeout'
                    time.sleep(.02)
                idle_sample('Snake launch/wait with terminal', 1, .25)
                # Spin intentionally saturates a core: other processes must
                # still render and accept focused input while it makes progress.
                click(92 + 470, 136 + 12)
                click(550, 65)
                end = time.monotonic() + 10
                while 'spin: counter=' not in log.read_text(errors='replace'):
                    assert time.monotonic() < end, 'CPU-bound process made no progress'
                    time.sleep(.02)
                start = time.monotonic()
                command('input-send-event', {'events': [
                    {'type': 'abs', 'data': {'axis': 'x', 'value': 32767}},
                    {'type': 'abs', 'data': {'axis': 'y', 'value': 32767}}]})
                while True:
                    capture = tmp / 'loaded.ppm'
                    command('screendump', {'filename': str(capture)})
                    with Image.open(capture) as image:
                        if image.getpixel((1919, 1079)) == (8, 19, 33):
                            break
                    assert time.monotonic() - start < .2, 'pointer stalled under CPU load'
                    time.sleep(.005)
                print(f'pointer under CPU load: {(time.monotonic()-start)*1000:.0f} ms', flush=True)
                click(85, 180)
                for key in ['e', 'c', 'h', 'o', 'spc', 'l', 'o', 'a', 'd', 'ret']:
                    command('human-monitor-command', {'command-line': f'sendkey {key} 1'})
                    time.sleep(.025)
                end = time.monotonic() + 1
                while '[portal] output: load\n' not in output():
                    assert time.monotonic() < end, 'focused input stalled under CPU load'
                    time.sleep(.01)
                print('PASS: empty/terminal/Snake CPU budgets, keyboard bursts, and input under CPU load')
        except Exception:
            if log.exists():
                print(log.read_text(errors='replace')[-6000:])
            raise
        finally:
            qemu.terminate()
            qemu.wait(timeout=5)


if __name__ == '__main__':
    main()
