#!/usr/bin/env python3
"""Two real terminal/shell pairs: launch, pointer and input latency on SMP.

Build with tools/build-grub-disk.sh, then RUSTOS_CPUS=16 python3 -B
e2e/desktop_parallel.py. Every run owns a disposable disk and records evidence.
"""
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import time

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_platform import arguments
from qemu_network import arguments as network_arguments
from qemu_test import launchers, connect, copy_disk, disk_image


def main():
    platform = arguments()
    cpus = int(platform[platform.index('-smp') + 1]) if '-smp' in platform else 1
    evidence = ROOT / 'target/desktop-parallel' / str(cpus)
    evidence.mkdir(parents=True, exist_ok=True)
    metrics = {'cpus': cpus, 'launch_ms': [], 'pointer_ms': [], 'keyboard_ms': []}
    with tempfile.TemporaryDirectory(prefix='rustos-parallel-') as directory:
        tmp = pathlib.Path(directory)
        disk = copy_disk(disk_image(ROOT), tmp)
        log = tmp / 'serial.log'
        sock = tmp / 'qmp'
        vm = subprocess.Popen([
            'qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-machine', 'pc,i8042=on,vmport=on', '-vga', 'std',
            '-display', 'none', '-serial', f'file:{log}', '-no-reboot',
            '-qmp', f'unix:{sock},server,nowait'] + platform + network_arguments())
        try:
            with connect(sock, time.monotonic() + 60, vm) as qmp:
                stream = qmp.makefile('rwb')
                json.loads(stream.readline())

                def command(name, args=None):
                    request = {'execute': name}
                    if args is not None:
                        request['arguments'] = args
                    stream.write(json.dumps(request).encode() + b'\n')
                    stream.flush()
                    while True:
                        response = json.loads(stream.readline())
                        assert 'error' not in response, response
                        if 'return' in response:
                            return response['return']

                def output():
                    return ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',
                        log.read_text(errors='replace'), re.S))

                def wait(predicate, seconds, description):
                    deadline = time.monotonic() + seconds
                    while not predicate():
                        assert vm.poll() is None, 'QEMU exited'
                        if time.monotonic() >= deadline:
                            diagnostic = command('human-monitor-command', {'command-line': 'info registers -a'})
                            (evidence / 'cpus.log').write_text(diagnostic)
                            raise AssertionError(description)
                        time.sleep(.005)

                def move(x, y):
                    command('input-send-event', {'events': [
                        {'type': 'abs', 'data': {'axis': axis,
                         'value': (value * 32767 + size - 2) // (size - 1)}}
                        for axis, value, size in [('x', x, 1920), ('y', y, 1080)]]})

                def click(x, y):
                    move(x, y)
                    time.sleep(.1)
                    for down in [True, False]:
                        command('input-send-event', {'events': [
                            {'type': 'btn', 'data': {'button': 'left', 'down': down}}]})
                        time.sleep(.1)

                def pointer(x, y):
                    start = time.monotonic()
                    move(x, y)
                    def visible():
                        capture = tmp / 'screen.ppm'
                        command('screendump', {'filename': str(capture)})
                        with Image.open(capture) as image:
                            return image.getpixel((x, y)) == (8, 19, 33)
                    wait(visible, 1, 'pointer stalled during terminal launch')
                    metrics['pointer_ms'].append((time.monotonic() - start) * 1000)

                command('qmp_capabilities')
                wait(lambda: '[desktop] ready: ' in output(), 60, 'desktop boot timeout')
                time.sleep(1)
                for point in [(0,0),(1919,1079),(960,540),(1500,800)] * 4:
                    pointer(*point)
                for terminal in range(2):
                    start = time.monotonic()
                    click(launchers(disk)['PORTAL'], 65)
                    # Exercise the compositor while disk loading and process
                    # construction proceed; do not wait for the shell first.
                    for x, y in [(1500, 700), (1800, 850), (1400, 650)]:
                        pointer(x, y)
                    wait(lambda: output().count('self_rust_os forge v1.0') == terminal + 1,
                         10, 'terminal shell did not start')
                    metrics['launch_ms'].append((time.monotonic() - start) * 1000)
                    # Each newly registered window gets focus. A completed
                    # shell response proves the entire input/stdio/render path.
                    marker = ['first', 'second'][terminal]
                    for key in ['e', 'c', 'h', 'o', 'spc', *marker, 'ret']:
                        if key == 'ret':
                            start = time.monotonic()
                        command('human-monitor-command', {'command-line': f'sendkey {key} 1'})
                        if key != 'ret':
                            time.sleep(.025)
                    wait(lambda: f'[portal] output: {marker}\n' in output(),
                         1, 'focused shell input stalled')
                    metrics['keyboard_ms'].append((time.monotonic() - start) * 1000)
                    if terminal == 0:
                        # Separate the real windows so the final artifact shows
                        # both independent shell responses, not a hidden client.
                        move(120, 138)
                        time.sleep(.1)
                        command('input-send-event', {'events': [
                            {'type': 'btn', 'data': {'button': 'left', 'down': True}}]})
                        time.sleep(.1)
                        move(920, 330)
                        time.sleep(.1)
                        command('input-send-event', {'events': [
                            {'type': 'btn', 'data': {'button': 'left', 'down': False}}]})
                        time.sleep(.1)
                assert output().count('[portal] output: first\n') == 1
                assert output().count('[portal] output: second\n') == 1, 'input reached multiple shells'
                def responses_painted():
                    capture = evidence / 'responses.ppm'
                    command('screendump', {'filename': str(capture)})
                    with Image.open(capture) as image:
                        # Check the final glyph on each output row, so a frame
                        # containing only a prefix of the response cannot pass.
                        return all(sum(count for count, color in image.crop(rect).getcolors(1000)
                                       if color == (190, 213, 232)) > 3
                                   for rect in [(924, 406, 932, 414), (144, 216, 152, 224)])
                wait(responses_painted, .3, 'both shell responses must reach their visible surfaces')
                command('screendump', {'filename': str(evidence / 'two-terminals.ppm')})
                with Image.open(evidence / 'two-terminals.ppm') as image:
                    image.save(evidence / 'two-terminals.png')
                print(json.dumps(metrics), flush=True)
                assert max(metrics['launch_ms']) < 1000, 'terminal launch exceeded 1 second'
                assert max(metrics['pointer_ms']) < 50, 'pointer exceeded 50 ms while idle or launching'
                assert max(metrics['keyboard_ms']) < 75, 'input drain exceeded 75 ms'
                print('PASS: two independent shells and responsive compositor on SMP')
        finally:
            if log.exists():
                (evidence / 'serial.log').write_text(log.read_text(errors='replace'))
            (evidence / 'metrics.json').write_text(json.dumps(metrics, indent=2) + '\n')
            vm.terminate()
            vm.wait(timeout=5)


if __name__ == '__main__':
    main()
