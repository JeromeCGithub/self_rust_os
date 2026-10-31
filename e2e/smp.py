#!/usr/bin/env python3
"""Native Ring 3 threading acceptance on explicit disposable SMP images."""
import argparse
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_test import copy_disk, disk_image

def run(count, stress=False):
    out = ROOT / 'target/smp'
    out.mkdir(parents=True, exist_ok=True)
    log = out / f'{count}.log'
    log.write_text('')
    with tempfile.TemporaryDirectory(prefix='rustos-smp-') as directory:
        disk = copy_disk(disk_image(ROOT), Path(directory))
        # The test fixture selects the Ring 3 probe as its initial application.
        # Production boot policy and the real desktop image remain unchanged.
        subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576',
                        str(ROOT / 'user_programs/zthreads/zthreads.bin'), '::/BIN/WM.BIN'], check=True)
        if stress:
            config=Path(directory)/'stress.cfg'
            config.write_bytes(b'\1')
            subprocess.run(['mcopy','-i',f'{disk}@@1048576',str(config),'::/TMP/SMPTEST.CFG'],check=True)
        vm = subprocess.Popen(['qemu-system-x86_64', '-machine', 'pc,i8042=on,vmport=on',
            '-m', '1024', '-cpu', 'qemu64,rdrand=on', '-smp', str(count),
            '-rtc', 'base=utc,clock=host', '-drive', f'format=raw,file={disk}',
            '-vga', 'std', '-display', 'none', '-serial', f'file:{log}', '-no-reboot',
            '-monitor', 'stdio'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 60
            while True:
                raw = log.read_text(errors='replace') if log.exists() else ''
                output = ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))
                if 'panic:' in raw or 'Kernel panic' in raw:
                    raise AssertionError(raw[-6000:])
                if f'[smp] online={count}' in raw and 'THREAD_TEST_PASS' in output:
                    diagnostic, _ = vm.communicate('info registers -a\nquit\n', timeout=5)
                    (out/f'{count}-cpus.log').write_text(diagnostic)
                    control = [int(value,16) for value in re.findall(r'CR0=([0-9a-fA-F]+)', diagnostic)]
                    assert len(control)==count, 'missing per-CPU control-register evidence'
                    assert all(value & 0x60000000 == 0 for value in control), 'CPU caches remain disabled after AP startup'
                    print(f'PASS: {count} CPUs; native threads and lifecycle assertions; {log}')
                    return
                if vm.poll() is not None or time.monotonic() >= deadline:
                    if vm.poll() is None:
                        diagnostic, _ = vm.communicate('info registers -a\nquit\n', timeout=5)
                        (out/f'{count}-cpus.log').write_text(diagnostic)
                    raise AssertionError(f'missing CPU/thread acceptance at {count} CPUs\n{raw[-6000:]}')
                time.sleep(.1)
        finally:
            vm.terminate()
            try:
                vm.wait(timeout=5)
            except subprocess.TimeoutExpired:
                vm.kill()
                vm.wait()

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cpus', nargs='+', type=int, default=[1,2,8,32])
    parser.add_argument('--stress', action='store_true', help='also exhaust physical memory and verify failed-spawn rollback')
    args = parser.parse_args()
    if any(not 1 <= n <= 32 for n in args.cpus):
        parser.error('CPU counts must be between 1 and 32')
    for n in args.cpus:
        run(n,args.stress)
