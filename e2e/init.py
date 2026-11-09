#!/usr/bin/env python3
"""Assert native Initator behavior on a private writable GRUB disk copy."""
import argparse
import os
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_platform import arguments
from qemu_test import copy_disk, disk_image, console_text, connect


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=['two-services', 'fatal-exit', 'fatal-fault', 'fatal-thread', 'abi', 'lifecycle', 'orphans', 'exhaustion', 'missing-init', 'bad-init', 'fault-service', 'steady', 'ipc', 'ipc-lifecycle'], default='two-services')
    case = parser.parse_args().case
    evidence = ROOT / 'target/init'
    evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='rustos-init-') as temporary:
        private = Path(temporary)
        disk = copy_disk(disk_image(ROOT), private)
        marker = private / 'init.run'
        marker.write_bytes(case.encode())
        subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576', str(marker),
                        '::/ETC/INIT.RUN'], check=True)
        if case in ('two-services', 'orphans', 'fault-service'):
            # Test behavior belongs in the service's literal argument vector.
            # PID 1 no longer interprets application-specific mode files.
            definition = (ROOT / 'config/init/system/FLICKER.UNIT').read_text()
            argument = {'two-services': '--fail', 'orphans': '--orphans',
                        'fault-service': '--fault'}[case]
            source = private / 'FLICKER.UNIT'
            source.write_text(definition + f'Arg={argument}\n')
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576', str(source),
                            '::/ETC/INIT/SYSTEM/FLICKER.UNIT'], check=True)
        if case == 'missing-init':
            subprocess.run(['mdel', '-i', f'{disk}@@1048576', '::/BIN/INITATOR.SVC'], check=True)
        elif case == 'bad-init':
            malformed = private / 'bad-init.svc'
            malformed.write_bytes(b'\x7fELFbad')
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576', str(malformed), '::/BIN/INITATOR.SVC'], check=True)
        elif case not in ('two-services', 'orphans', 'fault-service', 'steady'):
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576',
                str(ROOT / 'user_programs/zinit/zinit.bin'), '::/BIN/INITATOR.SVC'], check=True)
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576',
                str(ROOT / 'user_programs/zinit/zinit.bin'), '::/BIN/ZINIT.SVC'], check=True)
            malformed = private / 'bad.svc'
            malformed.write_bytes(b'\x7fELFbad')
            subprocess.run(['mcopy', '-o', '-i', f'{disk}@@1048576', str(malformed), '::/BIN/BAD.SVC'], check=True)
        log = evidence / (case + '-' + str(os.environ.get('RUSTOS_CPUS', '1')) + '.log')
        log.write_text('')
        process = subprocess.Popen(['qemu-system-x86_64', '-drive', f'format=raw,file={disk}',
            '-vga', 'std', '-serial', f'file:{log}', '-display', 'none', '-no-reboot',
            '-monitor', 'none', '-qmp', f'unix:{private / "qmp"},server=on,wait=off'] + arguments(cpus=int(os.environ.get('RUSTOS_CPUS', '1'))))
        try:
            deadline = time.monotonic() + 60
            connection = connect(private / 'qmp', deadline, process)
            stream = connection.makefile('rwb', buffering=0)
            json.loads(stream.readline())
            def command(execute, args=None):
                stream.write((json.dumps({'execute': execute, 'arguments': args or {}}) + '\n').encode())
                while True:
                    response = json.loads(stream.readline())
                    if 'return' in response:
                        return response['return']
                    if 'error' in response:
                        raise AssertionError(response)
            command('qmp_capabilities')
            while True:
                raw = log.read_text(errors='replace')
                output = console_text(raw)
                if case in ('missing-init', 'bad-init'):
                    assert '[sched] spawned process' not in raw, 'invalid init image published a process'
                    reason = 'Initial executable /BIN/INITATOR.SVC unavailable' if case == 'missing-init' else 'Failed to spawn initial process'
                    if 'Kernel panic' in raw and reason in raw:
                        registers = command('human-monitor-command', {'command-line': 'info registers -a'})
                        halted = re.findall(r'HLT=(\d)', registers)
                        if len(halted) == int(os.environ.get('RUSTOS_CPUS', '1')) and all(x == '1' for x in halted):
                            print('PASS: bad initial image fails before Ring 3 publication:', log)
                            return
                    assert time.monotonic() < deadline, raw[-6000:]
                    time.sleep(.05)
                    continue
                assert 'Kernel panic' not in raw and 'panicked' not in raw, raw[-6000:]
                assert '[sched] spawned process 0 ' not in raw, 'Expected native PID 1; legacy PID 0 booted'
                assert process.poll() is None, raw[-6000:]
                if not case.startswith('fatal-'):
                    assert '[initator-fatal]' not in raw, raw[-6000:]
                if case == 'steady' and '[glow] beat=3' in output:
                    starts = re.findall(r'\[initator\] started name=(\w+) unit=\d+ run=(\d+) pid=(\d+)', output)
                    assert len(starts) == 2 and {x[0] for x in starts} == {'glow', 'flicker'}, starts
                    assert '[initator] completion' not in output, output[-4000:]
                    assert '[initator] PID 1 entered Ring 3' in output
                    assert '[flicker] started' in output
                    print('PASS: ordinary native boot keeps both foreground services live:', log)
                    return
                if case == 'ipc-lifecycle' and '[init-probe] IPC lifecycle PASS cycles=64 frames=' in output:
                    print('PASS: Ring 3 bound-channel spawn/exit/reap cycles and exact frame recovery:', log)
                    return
                if case == 'ipc' and '[init-probe] IPC PASS fifo=8 moved=true closed=true' in output:
                    assert '[init-probe] IPC PASS fifo=8 moved=true closed=true writable=true' in output
                    assert '[init-probe] IPC validation PASS pointers=true rollback=true quota=true' in output
                    print('PASS: Ring 3 channels, binding move, FIFO/backpressure, peer/child lifecycle:', log)
                    return
                if case == 'exhaustion' and '[init-probe] exhaustion PASS children=' in output:
                    print('PASS: spawn allocation failure rollback and complete recovery:', log)
                    return
                if case == 'lifecycle' and '[init-probe] lifecycle PASS cycles=64 frames=' in output:
                    print('PASS: repeated Ring 3 spawn/kill/reap and frame recovery:', log)
                    return
                if case == 'abi' and '[init-probe] ABI PASS' in output:
                    print('PASS: Ring 3 identity, child events, authorization and usercopy boundaries:', log)
                    return
                if case.startswith('fatal-'):
                    if re.search(r'\[initator-fatal\] cause=\w+ status=\d+ parked=(?:true|false)\n', raw):
                        expected = {'fatal-exit': ('Exited', 42), 'fatal-fault': ('Fault', 6),
                                    'fatal-thread': ('Exited', 0)}[case]
                        assert f'cause={expected[0]} status={expected[1]}' in raw, raw[-4000:]
                        registers = command('human-monitor-command', {'command-line': 'info registers -a'})
                        halted = re.findall(r'HLT=(\d)', registers)
                        count = int(os.environ.get('RUSTOS_CPUS', '1'))
                        if len(halted) == count and all(value == '1' for value in halted):
                            (evidence / f'{case}-{count}-registers.txt').write_text(registers)
                            print('PASS: fatal init cause and all CPUs halted:', log)
                            return
                    elif '[init-probe] departing' in output:
                        after = output.split('[init-probe] departing', 1)[1]
                        assert '[glow] beat=' not in after, 'PID 1 died but another service kept running'
                # M2 can exhaust an optional service's retries before boot settles;
                # that terminal job is Degraded. After a completed boot, the unit's
                # terminal state is Failed. Both must satisfy the same three-run,
                # timing, typed-cause, identity and surviving-sibling assertions.
                terminal = re.search(r'\[initator\] (?:state name=flicker unit=\d+ state=Failed|activation outcome=Degraded)', output)
                if case in ('two-services', 'orphans', 'fault-service') and terminal:
                    starts = re.findall(r'\[initator\] started name=(\w+) unit=\d+ run=(\d+) pid=(\d+)', output)
                    a = [x for x in starts if x[0] == 'glow']
                    b = [x for x in starts if x[0] == 'flicker']
                    assert len(a) == 1 and len(b) == 3, starts
                    assert len({x[2] for x in starts}) == 4, starts
                    timed_starts = re.findall(r'\[initator\] started name=flicker unit=\d+ run=\d+ pid=(\d+) tick=(\d+)', output)
                    timed_exits = dict(re.findall(r'\[initator\] completion pid=(\d+) cause=(?:Exited|Fault) status=(?:23|17|6) tick=(\d+)', output))
                    assert len(timed_starts) == 3, 'native restart timing must be observable'
                    for index in (1, 2):
                        previous_pid = timed_starts[index - 1][0]
                        elapsed = int(timed_starts[index][1]) - int(timed_exits[previous_pid])
                        assert elapsed >= (2 if index == 1 else 4), 'restart occurred before backoff deadline'
                    assert '[initator] PID 1 entered Ring 3' in output
                    if case == 'fault-service':
                        assert output.count('cause=Fault status=6') == 3, output[-6000:]
                    else:
                        assert output.count('cause=Exited status=23') == 3, output[-6000:]
                    if case == 'orphans':
                        assert output.count('cause=Exited status=17') == 3, output[-6000:]
                        assert output.count('cause=Killed status=9') == 3, output[-6000:]
                    after = output[terminal.end():]
                    if '[glow] beat=' in after:
                        print('PASS: native PID 1, two services, three failed runs, restart cap, surviving sibling:', log)
                        return
                assert time.monotonic() < deadline, raw[-6000:]
                time.sleep(.05)
        finally:
            if 'stream' in locals(): stream.close()
            if 'connection' in locals(): connection.close()
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)


if __name__ == '__main__':
    main()
