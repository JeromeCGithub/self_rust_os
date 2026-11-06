#!/usr/bin/env python3
"""Native restricted worker probe in a normal Ring 3 child, on a private disk."""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_platform import arguments
from qemu_test import copy_disk, disk_image


def reserve_log(directory):
    """Reserve a fresh path so an earlier VM's output cannot satisfy assertions."""
    descriptor, name = tempfile.mkstemp(prefix='serial-', suffix='.log', dir=directory)
    os.close(descriptor)
    return Path(name)


def main():
    out = ROOT / 'target/browser-p1' / ('cpus-' + os.environ.get('RUSTOS_CPUS', '1'))
    out.mkdir(parents=True, exist_ok=True)
    log = reserve_log(out)
    with tempfile.TemporaryDirectory(prefix='rustos-browser-p1-') as temporary:
        disk = copy_disk(disk_image(ROOT), Path(temporary))
        for name in ['WM.BIN', 'ZSANDBOX.BIN']:
            subprocess.run([
                'mcopy', '-o', '-i', f'{disk}@@1048576',
                str(ROOT / 'user_programs/zsandbox/zsandbox.bin'), f'::/BIN/{name}',
            ], check=True)
        process = subprocess.Popen([
            'qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-display', 'none', '-serial', f'file:{log}', '-monitor', 'none',
            '-no-reboot', '-nic', 'none',
        ] + arguments())
        try:
            deadline = time.monotonic() + 60
            while True:
                raw = log.read_text(errors='replace')
                output = raw + ''.join(re.findall(
                    r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))
                assert not any(s in output for s in [
                    'panicked', 'panic:', 'Kernel panic',
                ]), output[-6000:]
                if '[sandbox] PASS restrictions' in output:
                    assert re.search(r"sys_spawn: '/BIN/ZSANDBOX.BIN' -> pid [1-9][0-9]*", raw)
                    print('PASS: restricted Ring 3 worker and repeated frame recovery; evidence:', log)
                    return
                assert process.poll() is None, output[-6000:]
                assert time.monotonic() < deadline, output[-6000:]
                time.sleep(.1)
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == '__main__':
    main()
