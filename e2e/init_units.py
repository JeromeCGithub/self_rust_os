#!/usr/bin/env python3
"""Configured Initator acceptance tests on a private writable disk copy."""
import argparse
import os
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
from qemu_test import copy_disk, disk_image, console_text


def unit(name, executable, extra=''):
    return f'Version=1\nKind=service\nName={name}\nExec={executable}\n{extra}'


def fixture(case):
    files = {
        'A.UNIT': unit('beacon', '/BIN/GLOW.SVC'),
        'B.UNIT': unit('ember', '/BIN/FLICKER.SVC', 'Requires=beacon\n'),
        'BOOT.UNIT': 'Version=1\nKind=target\nName=boot\nRequires=ember\n',
    }
    if case == 'service-budget':
        files={f'U{n:02}.UNIT':unit(f'u{n:02}','/BIN/GLOW.SVC') for n in range(57)}
        files['BOOT.UNIT']='Version=1\nKind=target\nName=boot\n'+''.join(f'Requires=u{n:02}\n' for n in range(57))
        return files
    if case == 'order-only':
        files['A.UNIT']=unit('beacon','/BIN/ABSENT.SVC')
        files['B.UNIT']=unit('ember','/BIN/GLOW.SVC','After=beacon\n')
        files['BOOT.UNIT']='Version=1\nKind=target\nName=boot\nRequires=ember\nWants=beacon\n'
        return files
    if case == 'early-exit':
        files['A.UNIT']=unit('beacon','/BIN/FLICKER.SVC','Arg=--fault\n')
        files['B.UNIT']=unit('ember','/BIN/GLOW.SVC','Requires=beacon\n')
    elif case == 'too-many':
        return {f'U{n:02}.UNIT':unit(f'u{n:02}','/BIN/GLOW.SVC') for n in range(65)}
    if case == 'total-budget':
        files={}
        for n in range(17):
            text=unit(f'u{n:02}','/BIN/GLOW.SVC')
            files[f'U{n:02}.UNIT']=text+'#'+'x'*(8192-len(text)-1)
        return files
    if case == 'oversize':
        files['A.UNIT']='x'*8193
    elif case == 'encoding':
        files['A.UNIT']=b'\xff'
    elif case == 'vectors':
        return {
            'VECTORS.UNIT': unit('vectors','/BIN/ZINIT.SVC',
                'Arg=--unit-probe\nArg=two words\nArg=$(literal)*\nArg=\n'
                'Arg=café\nEnv=UNIT_EXAMPLE=value=literal\nEnv=UNIT_EMPTY=\n'),
            'BOOT.UNIT':'Version=1\nKind=target\nName=boot\nRequires=vectors\n',
        }
    if case == 'cycle':
        files['A.UNIT'] += 'After=ember\n'
    elif case == 'unknown':
        files['A.UNIT'] += 'Requires=missing\n'
    elif case == 'malformed':
        files['A.UNIT'] += 'Ready=notify\n'
    elif case in ('required-failure', 'optional-failure'):
        files['B.UNIT'] = unit('ember', '/BIN/ABSENT.SVC', 'Requires=beacon\n')
        if case == 'optional-failure':
            files['BOOT.UNIT'] = 'Version=1\nKind=target\nName=boot\nWants=ember\n'
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=['configured','packaged','cycle','unknown','malformed',
        'required-failure','optional-failure','vectors','too-many','total-budget','oversize','encoding','early-exit','service-budget','order-only'], default='configured')
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    cpus = int(os.environ.get('RUSTOS_CPUS','1'))
    evidence = ROOT / 'target/init-units'
    evidence.mkdir(parents=True,exist_ok=True)
    log = evidence / f'{args.case}-{cpus}.log'
    with tempfile.TemporaryDirectory(prefix='rustos-init-units-') as temporary:
        private = Path(temporary)
        disk = copy_disk(disk_image(ROOT), private)
        image = f'{disk}@@1048576'
        # A private test owns these directories. Remove installed defaults, then
        # inject the complete fixture; the source image is never mutated.
        if args.case != 'packaged':
            subprocess.run(['mmd','-i',image,'::/ETC/INIT','::/ETC/INIT/SYSTEM'],capture_output=True)
            subprocess.run(['mdel','-i',image,'::/ETC/INIT/SYSTEM/*.UNIT'],capture_output=True)
        for name, text in ({} if args.case == 'packaged' else fixture(args.case)).items():
            source=private/name; source.write_bytes(text.encode() if isinstance(text,str) else text)
            subprocess.run(['mcopy','-o','-i',image,str(source),f'::/ETC/INIT/SYSTEM/{name}'],check=True)
        if args.case == 'vectors':
            subprocess.run(['mcopy','-o','-i',image,
                str(ROOT/'user_programs/zinit/zinit.bin'),'::/BIN/ZINIT.SVC'],check=True)
        log.write_text('')
        process = subprocess.Popen(['qemu-system-x86_64','-drive',f'format=raw,file={disk}',
            '-vga','std','-serial',f'file:{log}','-display','none','-no-reboot',
            '-monitor','none'] + arguments(cpus=cpus))
        try:
            deadline=time.monotonic()+args.timeout
            while True:
                raw=log.read_text(errors='replace'); output=console_text(raw)
                assert process.poll() is None,raw[-6000:]
                assert 'Kernel panic' not in raw and '[initator-fatal]' not in raw,raw[-6000:]
                if args.case in ('cycle','unknown','malformed','too-many','total-budget','oversize','encoding','service-budget'):
                    if '[initator] configuration rejected' in output:
                        assert '[initator] started' not in output
                        assert re.findall(r'\[sched\] spawned process (\d+) ',raw)==['1']
                        expected={'cycle':'Cycle','unknown':'Missing','malformed':'Unsupported','too-many':'unit-count',
                            'total-budget':'catalog-budget','oversize':'file-or-budget','encoding':'Encoding','service-budget':'reason=Limit'}[args.case]
                        assert expected in output,output[-4000:]
                        print(f'PASS: {args.case} rejected before any service spawn: {log}');return
                elif args.case == 'order-only':
                    if '[glow] beat=3' in output and '[initator] activation outcome=Degraded' in output:
                        assert '[initator] started name=beacon ' not in output
                        assert output.count('[initator] started name=ember ')==1
                        assert output.index('[initator] spawn failed name=beacon')<output.index('[initator] started name=ember ')
                        assert '[initator] stopping' not in output
                        print(f'PASS: After waits for settlement without requiring success: {log}');return
                elif args.case == 'early-exit':
                    if 'cause=Fault status=6' in output and '[initator] activation outcome=Failed' in output:
                        # Spawned readiness cannot make a userspace event check
                        # atomic with the next syscall. If the exit races that
                        # launch, exclusive cleanup must finish before failure.
                        started=output.find('[initator] started name=ember ')
                        completion=output.index('cause=Fault status=6')
                        if started>=0:
                            assert started<completion,'dependent started after observed exit'
                            assert output.count('[initator] started name=ember ')==1
                            assert '[initator] stopping name=ember ' in output
                            assert 'cause=Killed status=9' in output
                            assert output.index('cause=Killed status=9')<output.index('activation outcome=Failed')
                        print(f'PASS: prerequisite exit blocks activation or completes raced-child rollback: {log}');return
                elif args.case == 'vectors':
                    if '[unit-probe] vectors PASS' in output and '[initator] activation outcome=Succeeded' in output:
                        assert output.count('[initator] started name=vectors ')==1
                        assert '[initator] completion' not in output
                        print(f'PASS: literal args, Unicode, empty values and explicit environment: {log}');return
                elif args.case in ('configured','packaged'):
                    if '[glow] beat=3' in output and '[initator] activation outcome=Succeeded' in output:
                        starts=re.findall(r'\[initator\] started name=(\w+) unit=\d+ run=\d+ pid=(\d+)',output)
                        assert [name for name,_ in starts]==(['beacon','ember'] if args.case=='configured' else ['flicker','glow']),starts
                        assert len({pid for _,pid in starts})==2
                        assert '[initator] catalog validated units=3' in output
                        assert '[flicker] started' in output
                        print(f'PASS: configured ordering and two live services: {log}');return
                else:
                    expected='Failed' if args.case=='required-failure' else 'Degraded'
                    if f'[initator] activation outcome={expected}' in output:
                        assert output.count('[initator] started name=beacon ')==1
                        assert '[initator] started name=ember ' not in output
                        assert '[initator] spawn failed name=ember' in output
                        assert '[initator] stopping name=beacon' in output
                        assert 'cause=Killed status=9' in output
                        assert output.index('cause=Killed status=9') < output.index(f'activation outcome={expected}')
                        print(f'PASS: {args.case} exclusive dependency cleanup acknowledged: {log}');return
                assert time.monotonic()<deadline,raw[-6000:]
                time.sleep(.05)
        finally:
            if process.poll() is None: process.terminate()
            process.wait(timeout=5)


if __name__=='__main__': main()
