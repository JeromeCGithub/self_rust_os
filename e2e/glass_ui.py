#!/usr/bin/env python3
"""Verify the real Ring 3 window SDK, glass pixels and widget interactions.

Build first with tools/build-grub-disk.sh. Only the disposable development image
is attached. Screenshots supplement the input and framebuffer assertions.
"""
import json
import pathlib
import re
import socket
import subprocess
import tempfile
import time
from PIL import Image, ImageChops

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix='rustos-glass-') as directory:
        tmp = pathlib.Path(directory)
        log = tmp / 'serial.log'
        sock = tmp / 'qmp'
        qemu = subprocess.Popen([
            'qemu-system-x86_64', '-drive', f'format=raw,file={ROOT}/target/grub-disk.img',
            '-machine', 'pc,i8042=on,vmport=on', '-vga', 'std', '-m', '256',
            '-serial', f'file:{log}', '-display', 'none', '-no-reboot',
            '-qmp', f'unix:{sock},server,nowait'])
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
                    stream.write(json.dumps({'execute': name, **({'arguments': arguments} if arguments else {})}).encode() + b'\n')
                    stream.flush()
                    while True:
                        result = json.loads(stream.readline())
                        assert 'error' not in result, result
                        if 'return' in result:
                            return result['return']

                command('qmp_capabilities')

                def move(x, y):
                    command('input-send-event', {'events': [
                        {'type': 'abs', 'data': {'axis': axis, 'value': (value * 32767 + size - 2) // (size - 1)}}
                        for axis, value, size in [('x', x, 1920), ('y', y, 1080)]]})
                    time.sleep(.15)

                def button(down):
                    command('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': down}}]})
                    time.sleep(.2)

                def click(x, y):
                    move(x, y)
                    button(True)
                    button(False)

                def wait(value, after=0):
                    deadline = time.monotonic() + 10
                    def observed():
                        raw = log.read_text(errors='replace')[after:]
                        return raw + ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))
                    while value not in observed():
                        assert qemu.poll() is None, 'QEMU exited'
                        assert time.monotonic() < deadline, f'missing {value}'
                        time.sleep(.05)

                def shot(name=None):
                    path = tmp / 'screen.ppm'
                    command('screendump', {'filename': str(path)})
                    with Image.open(path) as image:
                        result = image.convert('RGB')
                    if name:
                        result.save(ROOT / 'docs/screenshots' / f'{name}.png')
                    return result

                time.sleep(.5)
                backdrop = shot()
                click(150, 65)
                wait('[hello-ui] ready: 640x400 glass')
                wait('[hello-ui] invalid requests rejected')
                move(900, 700)
                time.sleep(.5)
                glass = shot('desktop-glass')
                assert glass.getpixel((80, 126)) == backdrop.getpixel((80, 126)), 'rounded corner must reveal backdrop'
                assert glass.getpixel((400, 400)) != backdrop.getpixel((400, 400)), 'glass material missing'
                colors = glass.crop((104, 178, 500, 200)).getcolors(10000) or []
                assert any(color == (246, 249, 255) for _, color in colors), 'foreground text must remain sharp'
                assert glass.getpixel((700, 400)) != backdrop.getpixel((700, 400)), 'requested 640px content size ignored'

                # Button activates once on release, and not when released outside.
                offset = log.stat().st_size
                move(140, 298)
                button(True)
                assert '[hello-ui] accent changed' not in log.read_text()[offset:]
                button(False)
                wait('[hello-ui] accent changed: 1', offset)
                time.sleep(.3)
                assert log.read_text()[offset:].count('[hello-ui] accent changed:') == 1
                offset = log.stat().st_size
                move(140, 298)
                button(True)
                move(850, 700)
                button(False)
                time.sleep(.3)
                assert '[hello-ui] accent changed' not in log.read_text()[offset:], 'release outside activated widget'

                # Dragging changes the sampled backdrop and restores the exposed area.
                before = shot()
                move(300, 140)
                button(True)
                move(1150, 640)
                button(False)
                move(1800, 950)
                time.sleep(.4)
                moved = shot('desktop-glass-drag')
                assert moved.getpixel((400, 400)) == backdrop.getpixel((400, 400)), 'old window pixels survived drag'
                assert moved.getpixel((1550, 900)) != backdrop.getpixel((1550, 900)), 'dragged window missing'
                old_patch = before.crop((400, 400, 450, 450))
                new_patch = moved.crop((1250, 900, 1300, 950))
                assert ImageChops.difference(old_patch, new_patch).getbbox(), 'glass did not resample backdrop'

                # Closing and reopening must restore drawing/input ownership.
                click(1550, 640)
                time.sleep(.4)
                assert shot().getpixel((1250, 900)) == backdrop.getpixel((1250, 900)), 'closed window left stale pixels'
                offset = log.stat().st_size
                click(150, 65)
                wait('[hello-ui] ready: 640x400 glass', offset)
                click(140, 298)
                wait('[hello-ui] accent changed: 1', offset)
                # A second native app owns focused input while covering Hello.
                click(250, 65)
                wait('Portal / RustOS shell')
                offset = log.stat().st_size
                click(140, 298)
                assert '[hello-ui] accent changed' not in log.read_text()[offset:], 'covered window received pointer input'
                # Raise Hello using its exposed right side. The terminal cursor
                # keeps blinking underneath, invalidating the stationary glass.
                click(700, 200)
                move(900, 700)
                time.sleep(.3)
                first = shot().crop((118, 196, 146, 210))
                deadline = time.monotonic() + 2
                while not ImageChops.difference(first, shot().crop((118, 196, 146, 210))).getbbox():
                    assert time.monotonic() < deadline, 'stationary glass ignored changing lower-layer content'
                    time.sleep(.08)
                shot('desktop-glass-overlap')
                offset = log.stat().st_size
                click(140, 298)
                wait('[hello-ui] accent changed: 2', offset)
                assert 'Kernel panic' not in log.read_text()
                print('PASS: Ring 3 toolkit, negotiated size, glass/rounded pixels, sharp text, button capture, backdrop resampling, close/reopen, focus and live lower-layer damage')
        except Exception:
            if log.exists():
                print(log.read_text(errors='replace')[-7000:])
            raise
        finally:
            qemu.terminate()
            qemu.wait(timeout=5)


if __name__ == '__main__':
    main()
