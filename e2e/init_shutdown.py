#!/usr/bin/env python3
"""Drive granted Spark shutdown and assert completions plus actual terminal CPUs."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
ROOT=Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True
sys.path.insert(0,str(ROOT/'tools'))
from init_latency import drive_options
from qemu_platform import arguments
from qemu_test import console_text,connect,copy_disk,disk_image

def unit(name,extra):
    return f'Version=1\nKind=service\nName={name}\nExec=/BIN/FLICKER.SVC\nReady=notify\nStartTimeoutTicks=90\nRestart=always\n{extra}'

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case',choices=['cooperative','global-deadline','pending-launch','backoff','restarted-prerequisite'],default='cooperative')
    case=parser.parse_args().case;cpus=int(os.environ.get('RUSTOS_CPUS','1'))
    evidence=ROOT/'target/init-shutdown';evidence.mkdir(exist_ok=True)
    log=evidence/f'{case}-{cpus}.log'
    with tempfile.TemporaryDirectory(prefix='rustos-shutdown-') as temporary:
        private=Path(temporary);disk=copy_disk(disk_image(ROOT),private);image=f'{disk}@@1048576'
        subprocess.run(['mdel','-i',image,'::/ETC/INIT/SYSTEM/*.UNIT'],capture_output=True,check=True)
        for program in ('initator','flicker','spark'):
            subprocess.run(['mcopy','-o','-i',image,str(ROOT/f'user_programs/{program}/{program}.bin'),f'::/BIN/{program.upper()}.SVC'],check=True)
        extra='Arg=--watchdog\nStopTimeoutTicks=1000\n' if case=='global-deadline' else 'Arg=--cooperative\nStopTimeoutTicks=6\n'
        files={'A.UNIT':unit('beacon',extra),'B.UNIT':unit('ember',extra+'Requires=beacon\n')}
        command={'cooperative':'shutdown','global-deadline':'shutdown','pending-launch':'--probe-shutdown-pending','backoff':'--probe-shutdown-backoff','restarted-prerequisite':'--probe-shutdown-restarted'}[case]
        after='ember';required='ember'
        if case=='backoff':
            files['B.UNIT']='Version=1\nKind=service\nName=ember\nExec=/BIN/FLICKER.SVC\nArg=--fail\nRequires=beacon\nRestart=always\nRestartDelayTicks=120\nRestartMaxDelayTicks=120\n'
        if case=='pending-launch':
            files.pop('B.UNIT');after=required='beacon'
            subprocess.run(['mcopy','-o','-i',image,str(ROOT/'user_programs/zload/zload.bin'),'::/BIN/BULK.SVC'],check=True)
            files['BULK.UNIT']='Version=1\nKind=service\nName=bulk\nExec=/BIN/BULK.SVC\nStartTimeoutTicks=10000\n'
        files['SPARK.UNIT']=f'Version=1\nKind=service\nName=spark\nExec=/BIN/SPARK.SVC\nArg={command}\nAfter={after}\nControlAccess=manage\nReady=notify\n'
        files['BOOT.UNIT']=f'Version=1\nKind=target\nName=boot\nRequires={required}\nRequires=spark\n'
        for name,text in files.items():
            path=private/name;path.write_text(text)
            subprocess.run(['mcopy','-o','-i',image,str(path),f'::/ETC/INIT/SYSTEM/{name}'],check=True)
        log.write_text('')
        process=subprocess.Popen(['qemu-system-x86_64','-drive',drive_options(disk,8388608 if case=='pending-launch' else 0),'-vga','std','-serial',f'file:{log}','-display','none','-no-reboot','-monitor','none','-qmp',f'unix:{private / "qmp"},server=on,wait=off']+arguments(cpus=cpus))
        try:
            deadline=time.monotonic()+60
            connection=connect(private/'qmp',deadline,process);stream=connection.makefile('rwb',buffering=0)
            json.loads(stream.readline())
            def qmp(execute,args=None):
                stream.write((json.dumps({'execute':execute,'arguments':args or {}})+'\n').encode())
                while True:
                    response=json.loads(stream.readline())
                    if 'return' in response:return response['return']
                    if 'error' in response:raise AssertionError(response)
            qmp('qmp_capabilities')
            while True:
                raw=log.read_text(errors='replace');text=console_text(raw)
                assert process.poll() is None,raw[-8000:]
                assert 'panicked' not in raw and 'Kernel panic' not in raw and '[initator-fatal]' not in raw,raw[-8000:]
                assert '[spark] unknown command' not in text,'Spark must implement shutdown requests'
                if '[system-halt]' in raw:
                    assert '[system-halt] action=Halt parked=true dma_disabled=true' in raw,raw[-8000:]
                    assert '[spark] shutdown accepted job=' in text
                    begun=re.search(r'\[initator\] shutdown job=(\d+) tick=(\d+) deadline=Some\((\d+)\)',text)
                    drained=re.search(r'\[initator\] shutdown drained job=(\d+) tick=(\d+)',text)
                    assert begun and drained and begun[1]==drained[1],text[-8000:]
                    starts=re.findall(r'\[initator\] started name=(\w+) unit=\d+ run=\d+ pid=(\d+)',text)
                    assert len(starts)==(2 if case=='pending-launch' else 4 if case=='restarted-prerequisite' else 3),starts
                    for name,pid in starts:
                        completion=re.search(r'\[initator\] completion pid='+pid+r' cause=(\w+) status=(\d+) tick=(\d+)',text)
                        assert completion and completion.start()<drained.start(),(name,pid,text[-8000:])
                    # Validate exact outcomes independently of the PID-1 halt banner.
                    completions={pid:(cause,int(status),int(tick)) for pid,cause,status,tick in re.findall(r'\[initator\] completion pid=(\d+) cause=(\w+) status=(\d+) tick=(\d+)',text)}
                    for name,pid in starts:
                        cause,status,tick=completions[pid]
                        if case in ('cooperative','restarted-prerequisite') or name=='spark':assert (cause,status)==('Exited',0),(name,cause,status)
                        if case=='global-deadline' and name!='spark':assert cause=='Killed' and tick>=int(begun[3])
                    if case in ('cooperative','restarted-prerequisite'):
                        names=dict(starts)
                        end=text.index(f'[initator] completion pid={names["ember"]} ')
                        stop=text.index(f'[initator] stopping name=beacon ',begun.start())
                        assert end<stop,'prerequisite stopped before dependent completion'
                        assert text.count('[service-probe] cooperative-cleaned ')==(3 if case=='restarted-prerequisite' else 2)
                    if case=='global-deadline':assert int(drained[2])-int(begun[2])<1000,'global stop failed to override per-service grace'
                    if case=='pending-launch':
                        assert '[initator] launching name=bulk ' in text
                        assert '[initator] started name=bulk ' not in text
                        assert '[initator] spawn failed name=bulk ' in text
                    if case=='backoff':
                        assert re.search(r'state name=ember unit=\d+ state=Backoff',text)
                        assert sum(name=='ember' for name,_ in starts)==1
                        assert all(match.start()<begun.start() for match in re.finditer(r'\[initator\] launching name=',text))
                    registers=qmp('human-monitor-command',{'command-line':'info registers -a'})
                    states=re.findall(r'HLT=(\d)',registers)
                    if len(states)==cpus and all(state=='1' for state in states):
                        (evidence/f'{case}-{cpus}-registers.txt').write_text(registers)
                        print(f'PASS: {cpus} CPUs {case}, all owned children completed and system halted: {log}');return
                assert time.monotonic()<deadline,raw[-8000:]
                time.sleep(.02)
        finally:
            if process.poll() is None:process.terminate()
            process.wait(timeout=5)
if __name__=='__main__':main()
