#!/usr/bin/env python3
"""Run the existing eight-worker Ring 3 file probe under native Initator PID 1."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from init_file_io import completed_probe, fixture
from qemu_platform import arguments
from qemu_test import console_text, copy_disk, disk_image


def main():
    cpus = int(os.environ.get('RUSTOS_CPUS', '1'))
    evidence = ROOT / 'target/init-file-io'
    evidence.mkdir(parents=True, exist_ok=True)
    log = evidence / f'file-io-{cpus}.log'
    with tempfile.TemporaryDirectory(prefix='rustos-init-file-io-') as temporary:
        private = Path(temporary)
        disk = copy_disk(disk_image(ROOT), private)
        image = f'{disk}@@1048576'
        subprocess.run(['mdel', '-i', image, '::/ETC/INIT/SYSTEM/*.UNIT'], check=True, capture_output=True)
        for name, definition in fixture().items():
            source = private / name
            source.write_text(definition)
            subprocess.run(['mcopy', '-o', '-i', image, str(source), f'::/ETC/INIT/SYSTEM/{name}'], check=True)
        subprocess.run(['mcopy', '-o', '-i', image, str(ROOT / 'user_programs/zfileio/zfileio.bin'), '::/BIN/ZFILEIO.BIN'], check=True)
        log.write_text('')
        process = subprocess.Popen(['qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-vga', 'std', '-serial', f'file:{log}', '-display', 'none', '-no-reboot',
            '-monitor', 'none'] + arguments(cpus=cpus))
        try:
            deadline = time.monotonic() + 60
            while True:
                raw = log.read_text(errors='replace')
                assert process.poll() is None, raw[-8000:]
                assert not any(marker in raw for marker in ('panicked', 'Kernel panic', '[initator-fatal]')), raw[-8000:]
                assert '[sched] spawned process 0 ' not in raw, 'native-init image required'
                if completed_probe(console_text(raw)):
                    print(f'PASS: {cpus} CPUs, native supervised file probe, eight shared-cursor workers and surviving sibling: {log}')
                    return
                assert time.monotonic() < deadline, raw[-8000:]
                time.sleep(.01)
        finally:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)


if __name__ == '__main__':
    main()
