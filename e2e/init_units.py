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
from init_rollback import verify_early_exit
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
    if case in ('notify','notify-timeout','watchdog','heartbeat','wrong-ready','log-ready','foreign-ready'):
        mode={'notify':'--notify','notify-timeout':'--never-ready','watchdog':'--watchdog','heartbeat':'--notify','wrong-ready':'--wrong-ready','log-ready':'--log-ready','foreign-ready':'--foreign-ready'}[case]
        extra=f'Ready=notify\nStartTimeoutTicks=18\nArg={mode}\n'
        if case in ('watchdog','heartbeat'):extra+='WatchdogTicks=6\nRestart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=1000\nHealthyResetTicks=90\n'
        files['A.UNIT']=unit('beacon','/BIN/FLICKER.SVC',extra)
        files['B.UNIT']=unit('ember','/BIN/GLOW.SVC','Requires=beacon\n')
        return files
    if case in ('log-flood', 'log-exit'):
        policy = ('Ready=notify\nStartTimeoutTicks=18\nWatchdogTicks=6\n'
                  'Restart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\n'
                  'RestartBurst=3\nRestartWindowTicks=1000\nHealthyResetTicks=90\n')
        files['A.UNIT'] = unit('beacon', '/BIN/FLICKER.SVC', policy + 'Arg=--log-flood\n')
        files['B.UNIT'] = unit('ember', '/BIN/GLOW.SVC', 'Requires=beacon\n')
        mode = '--log-exit' if case == 'log-exit' else '--watchdog'
        files['C.UNIT'] = unit('wick', '/BIN/FLICKER.SVC', policy + f'Arg={mode}\n')
        files['BOOT.UNIT'] += 'Wants=wick\n'
        return files
    if case == 'control-pressure':
        return {
            'A.UNIT': unit('beacon','/BIN/GLOW.SVC'),
            'C.UNIT': unit('spark','/BIN/SPARK.SVC','ControlAccess=status\nArg=--probe-pressure\n'),
            'D.UNIT': unit('wick','/BIN/FLICKER.SVC','Ready=notify\nStartTimeoutTicks=18\nWatchdogTicks=6\nArg=--watchdog\nRestart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=1000\nHealthyResetTicks=90\n'),
            'BOOT.UNIT': 'Version=1\nKind=target\nName=boot\nRequires=beacon\nRequires=spark\nWants=wick\n',
        }
    if case in ('control-status', 'control-manage', 'control-restart', 'control-cooperative', 'control-stop-closed'):
        access = 'status' if case == 'control-status' else 'manage'
        probe = 'restart' if case == 'control-restart' else access
        files = {
            'A.UNIT': unit('beacon', '/BIN/GLOW.SVC'),
            'C.UNIT': unit('spark', '/BIN/SPARK.SVC', f'ControlAccess={access}\nArg=--probe-{probe}\n'),
            'BOOT.UNIT': 'Version=1\nKind=target\nName=boot\nRequires=beacon\nRequires=spark\n',
        }
        if case in ('control-manage','control-cooperative','control-stop-closed'):
            extra='RestartBurst=32\nStopTimeoutTicks=4\n'+('Ready=notify\nStartTimeoutTicks=18\nArg=--cooperative\n' if case=='control-cooperative' else '')
            if case=='control-stop-closed': extra='RestartBurst=32\nStopTimeoutTicks=90\nReady=notify\nStartTimeoutTicks=18\nArg=--stop-close\n'
            files['D.UNIT'] = unit('fern', '/BIN/FLICKER.SVC', extra)
        return files
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
        files['A.UNIT'] += 'Ready=bus-name\n'
    elif case in ('required-failure', 'optional-failure'):
        files['B.UNIT'] = unit('ember', '/BIN/ABSENT.SVC', 'Requires=beacon\n')
        if case == 'optional-failure':
            files['BOOT.UNIT'] = 'Version=1\nKind=target\nName=boot\nWants=ember\n'
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=['configured','packaged','cycle','unknown','malformed',
        'required-failure','optional-failure','vectors','too-many','total-budget','oversize','encoding','early-exit','service-budget','order-only','notify','notify-timeout','watchdog','heartbeat','wrong-ready','log-ready','foreign-ready','log-flood','log-exit','control-status','control-manage','control-pressure','control-restart','control-cooperative','control-stop-closed'], default='configured')
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
        if args.case in ('notify','notify-timeout','watchdog','heartbeat','wrong-ready','log-ready','foreign-ready','log-flood','log-exit'):
            subprocess.run(['mcopy','-o','-i',image,
                str(ROOT/'user_programs/flicker/flicker.bin'),'::/BIN/FLICKER.SVC'],check=True)
        if args.case.startswith('control-'):
            for program in ('initator', 'spark', 'flicker'):
                subprocess.run(['mcopy','-o','-i',image,str(ROOT/f'user_programs/{program}/{program}.bin'),f'::/BIN/{program.upper()}.SVC'],check=True)
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
                if args.case in ('notify','notify-timeout','watchdog','heartbeat','wrong-ready','log-ready','foreign-ready','log-flood','log-exit'):
                    assert 'panicked' not in raw,raw[-6000:]
                if args.case == 'control-pressure':
                    assert 'panicked' not in raw and '[spark] unknown command' not in output,raw[-6000:]
                    if '[spark] reply pressure PASS messages=16 closed=true' in output and '[glow] beat=3' in output:
                        assert output.count('reason=Some(Watchdog)')==3
                        assert output.count('cause=Killed status=9')==3
                        assert output.count('[initator] started name=wick ')==3
                        assert '[initator] stopping name=beacon ' not in output
                        assert output.index('cause=Killed status=9')<output.index('[spark] reply pressure PASS')
                        print(f'PASS: full control replies do not block watchdogs; malformed client is disconnected: {log}');return
                if args.case == 'control-restart':
                    assert 'panicked' not in raw and '[spark] unknown command' not in output,raw[-6000:]
                    if '[spark] referenced restart PASS' in output and '[glow] beat=3' in output:
                        starts=re.findall(r'\[initator\] started name=beacon unit=0 run=(\d+) pid=(\d+)',output)
                        assert len(starts)==2 and len({pid for _,pid in starts})==2
                        assert output.count('[initator] stopping name=beacon ')==1
                        assert output.count('cause=Killed status=9')==1
                        assert output.index('cause=Killed status=9')<output.index('[spark] referenced restart PASS')
                        assert output.count('[initator] started name=spark ')==1
                        print(f'PASS: one restart job replaces a referenced service after completion: {log}');return
                if args.case in ('control-manage','control-cooperative','control-stop-closed'):
                    assert 'panicked' not in raw and '[spark] unknown command' not in output,raw[-6000:]
                    if '[spark] management lifecycle PASS cycles=16' in output and '[glow] beat=3' in output:
                        starts=re.findall(r'\[initator\] started name=fern unit=2 run=(\d+) pid=(\d+)',output)
                        assert len(starts)==17 and len({pid for _,pid in starts})==17
                        assert output.count('[initator] stopping name=fern ')==17
                        if args.case == 'control-cooperative':
                            assert output.count('[service-probe] cooperative-cleaned ')==17,'cooperative cleanup must run before service completion'
                            assert 'cause=Killed status=9' not in output,'cooperative services must not be killed'
                            for _,pid in starts:
                                assert f'[initator] completion pid={pid} cause=Exited status=0' in output
                        else:
                            assert output.count('cause=Killed status=9')==17
                            if args.case=='control-manage':
                                assert output.count('reason=Some(StopTimeout)')==17
                                stops=dict(re.findall(r'\[initator\] stopping name=fern unit=2 pid=(\d+) tick=(\d+)',output))
                                forced=re.findall(r'\[initator\] escalating name=fern unit=2 pid=(\d+) tick=(\d+)',output)
                                assert len(forced)==17
                                assert all(int(tick)-int(stops[pid])>=4 for pid,tick in forced),'forced stop preceded grace deadline'

                        if args.case=='control-stop-closed':
                            assert output.count('reason=Some(StopDelivery)')==17,'closed peer must force exact pending stops without grace delay'
                            assert 'reason=Some(StopTimeout)' not in output
                        assert '[initator] stopping name=beacon ' not in output
                        print(f'PASS: Spark start/stop jobs, identities and repeated resource recovery: {log}');return
                if args.case == 'control-status':
                    assert 'panicked' not in raw,raw[-6000:]
                    if '[spark] status authorization PASS' in output and '[glow] beat=3' in output:
                        assert '[initator] started name=beacon ' in output
                        assert '[initator] stopping name=beacon ' not in output
                        assert 'cause=Exited status=0' in output
                        print(f'PASS: explicit readonly control pages and denied lifecycle effects: {log}');return
                if args.case in ('log-flood', 'log-exit'):
                    cause = 'cause=Exited status=23' if args.case == 'log-exit' else 'cause=Killed status=9'
                    if (output.count(cause) == 3 and '[glow] beat=3' in output
                            and '[service-probe] flood-work=12 ' in output
                            and '[initator] state name=wick unit=3 state=Failed' in output):
                        assert output.count('[initator] started name=beacon ') == 1
                        assert output.count('[initator] started name=wick ') == 3
                        assert '[initator] state name=wick unit=3 state=Failed' in output
                        assert '[initator] stopping name=beacon ' not in output
                        assert '[service-probe] log-pressure accepted=' in output
                        assert re.search(r'log-pressure accepted=[1-9][0-9]* full=[1-9][0-9]*', output)
                        if args.case == 'log-flood':
                            assert output.count('reason=Some(Watchdog)') == 3
                        else:
                            assert output.count('[service-probe] log-exit status=23') == 3
                        print(f'PASS: bounded log flood preserves healthy work, child collection and watchdog deadlines: {log}');return
                elif args.case=='notify':
                    if '[service-probe] work=3 ' in output and '[glow] beat=3' in output:
                        assert '[initator] activation outcome=Succeeded' in output
                        assert output.index('[initator] ready name=beacon')<output.index('[initator] started name=ember ')
                        assert output.count('[initator] started name=beacon ')==1
                        print(f'PASS: dependent waits for authenticated Ready after setup: {log}');return
                elif args.case=='heartbeat':
                    if '[service-probe] work=6 ' in output and '[glow] beat=3' in output:
                        assert '[initator] activation outcome=Succeeded' in output
                        assert output.count('[initator] started name=beacon ')==1
                        assert '[initator] completion' not in output and '[initator] stopping' not in output
                        print(f'PASS: heartbeats from completed work keep the run healthy: {log}');return
                elif args.case in ('wrong-ready','log-ready','foreign-ready'):
                    if '[initator] activation outcome=Failed' in output:
                        assert f'[service-probe] forged mode=--{args.case}' in output
                        assert '[initator] ready name=beacon' not in output
                        assert '[initator] started name=ember ' not in output
                        assert 'reason=Some(StartTimeout)' in output and 'cause=Killed status=9' in output
                        print(f'PASS: {args.case} cannot authorize readiness or activate dependent: {log}');return
                elif args.case=='notify-timeout':
                    if '[initator] activation outcome=Failed' in output:
                        assert '[service-probe] initialized run=' in output
                        assert '[initator] started name=ember ' not in output
                        assert 'reason=Some(StartTimeout)' in output and 'cause=Killed status=9' in output
                        print(f'PASS: missing Ready times out, terminates and acknowledges failure: {log}');return
                elif args.case=='watchdog':
                    if 'state=Failed' in output and output.count('cause=Killed status=9')==3:
                        assert output.count('[initator] started name=beacon ')==3
                        assert output.count('[initator] started name=ember ')==1
                        assert output.count('reason=Some(Watchdog)')==3
                        after=output.rsplit('cause=Killed status=9',1)[1]
                        if '[glow] beat=' in after:
                            print(f'PASS: work-path watchdog expiry, three capped runs and surviving dependent: {log}');return
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
                        verify_early_exit(output)
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
                elif args.case in ('required-failure','optional-failure'):
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
