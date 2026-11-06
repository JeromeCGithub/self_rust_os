#!/usr/bin/env python3
"""Real Ring 3 filesystem probe on a private copy of a freshly built GRUB disk.

Replace only the disposable copy's initial WM executable, so the probe executes
without GUI timing dependencies. Logs are retained under target/file-io/.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_platform import arguments as platform_arguments
from qemu_test import copy_disk, disk_image


def main(mode=None):
    output = ROOT / ('target/'+mode if mode else 'target/file-io')
    output.mkdir(parents=True, exist_ok=True)
    log = output / 'serial.log'
    with tempfile.TemporaryDirectory(prefix='rustos-file-io-') as temporary:
        disk = copy_disk(disk_image(ROOT), Path(temporary))
        for destination in ('WM.BIN', 'ZFILEIO.BIN'):
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576',
                            str(ROOT / ('user_programs/zlarge/zlarge.bin' if mode == 'cap-image' else 'user_programs/ztls/ztls.bin' if mode == 'cap-tls' else 'user_programs/zfileio/zfileio.bin')),
                            f'::/BIN/{destination}'], check=True)
        if mode == 'arguments':
            marker = Path(temporary) / 'argv.run'
            marker.write_bytes(b'1')
            subprocess.run(['mcopy', '-i', f'{disk}@@1048576', str(marker), '::/TMP/ARGV.RUN'], check=True)
        if mode and mode.startswith('cap-'):
            marker=Path(temporary)/'caps.run'
            marker.write_text(mode[4:])
            subprocess.run(['mcopy','-i',f'{disk}@@1048576',str(marker),'::/TMP/CAPS.RUN'],check=True)
        if mode == 'cap-names':
            fixture=Path(temporary)/'host.txt';fixture.write_bytes(b'host LFN fixture')
            subprocess.run(['mcopy','-i',f'{disk}@@1048576',str(fixture),'::/TMP/Host created Unicode café name.txt'],check=True)
        log.write_text('')
        process = subprocess.Popen([
            'qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-vga', 'std', '-serial', f'file:{log}', '-display', 'none',
            '-no-reboot', '-monitor', 'none',
        ] + platform_arguments())
        try:
            deadline = time.monotonic() + 60
            while True:
                text = log.read_text(errors='replace') if log.exists() else ''
                assert 'panicked' not in text and 'Kernel panic' not in text, text[-6000:]
                if ('[captest] PASS' if mode and mode.startswith('cap-') else '[argtest] PASS' if mode == 'arguments' else '[filetest] PASS:') in text:
                    if mode == 'cap-names':
                        process.terminate();process.wait(timeout=5)
                        contents=subprocess.check_output(['mtype','-i',f'{disk}@@1048576','::/TMP/Directory with a long name/café 文書 and notes.txt'])
                        assert contents==b'long file data',contents
                    print('PASS: Ring 3 '+(mode[4:] if mode and mode.startswith('cap-') else 'process arguments' if mode == 'arguments' else 'file I/O')+'; evidence:', log)
                    return
                assert process.poll() is None, text[-6000:]
                assert time.monotonic() < deadline, text[-6000:]
                time.sleep(.1)
        finally:
            if process.poll() is None:process.terminate()
            process.wait(timeout=5)


if __name__ == '__main__':
    main()
