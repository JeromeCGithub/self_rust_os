#!/usr/bin/env python3
"""Measure PID 1's watchdog service while loading a large private ELF."""
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
from qemu_platform import arguments
from qemu_test import console_text,copy_disk,disk_image
from init_latency import metrics,drive_options

def unit(name,path,extra=''):
    return f'Version=1\nKind=service\nName={name}\nExec={path}\nStartTimeoutTicks=10000\n{extra}'

def main():
    cpus=int(os.environ.get('RUSTOS_CPUS','1'))
    evidence=ROOT/'target/init-latency';evidence.mkdir(exist_ok=True)
    read_bps=int(os.environ.get('RUSTOS_INIT_READ_BPS','0'))
    cancel=os.environ.get('RUSTOS_INIT_CANCEL_DEPENDENT','0')=='1'
    label=f'pressure-{cpus}-bps{read_bps}'+('-cancel' if cancel else '')
    log=evidence/f'{label}.log'
    report=evidence/f'{label}.json';report.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='rustos-init-latency-') as temporary:
        private=Path(temporary);disk=copy_disk(disk_image(ROOT),private);image=f'{disk}@@1048576'
        subprocess.run(['mdel','-i',image,'::/ETC/INIT/SYSTEM/*.UNIT'],capture_output=True,check=True)
        subprocess.run(['mmd','-i',image,'::/PROBES'],check=True)
        subprocess.run(['mcopy','-o','-i',image,str(ROOT/'user_programs/initator/initator.bin'),'::/BIN/INITATOR.SVC'],check=True)
        for n in range(4):
            subprocess.run(['mcopy','-o','-i',image,str(ROOT/'user_programs/zload/zload.bin'),f'::/PROBES/LOAD{n}.SVC'],check=True)
        files={f'W{n}.UNIT':unit(f'warm{n}',f'/PROBES/LOAD{n}.SVC') for n in range(3)}
        files['WICK.UNIT']=unit('wick','/BIN/FLICKER.SVC','Ready=notify\nWatchdogTicks=6\nArg=--watchdog\n'+''.join(f'Requires=warm{n}\n' for n in range(3)))
        # Independent pressure must finish loading even when the optional
        # watchdog fails. The separate cancellation case makes it a prerequisite.
        dependencies='Requires=wick\n' if cancel else 'After=wick\n'+''.join(f'Requires=warm{n}\n' for n in range(3))
        files['BULK.UNIT']=unit('bulk','/PROBES/LOAD3.SVC',dependencies)
        files['BOOT.UNIT']='Version=1\nKind=target\nName=boot\nRequires=bulk\n'+('' if cancel else 'Wants=wick\n')
        for name,text in files.items():
            source=private/name;source.write_text(text)
            subprocess.run(['mcopy','-o','-i',image,str(source),f'::/ETC/INIT/SYSTEM/{name}'],check=True)
        log.write_text('');ready_wall=None;stop_wall=None
        process=subprocess.Popen(['qemu-system-x86_64','-drive',drive_options(disk,read_bps),
            '-vga','std','-serial',f'file:{log}','-display','none','-no-reboot','-monitor','none']+arguments(cpus=cpus))
        try:
            until=time.monotonic()+60
            while True:
                raw=log.read_text(errors='replace');text=console_text(raw)
                assert process.poll() is None,raw[-8000:]
                assert 'panicked' not in raw and 'Kernel panic' not in raw and '[initator-fatal]' not in raw,raw[-8000:]
                if ready_wall is None and '[initator] ready name=wick ' in text:
                    assert '[initator] stopping name=wick ' not in text,'observer missed the readiness-to-stop interval'
                    ready_wall=time.monotonic()
                if stop_wall is None and '[initator] stopping name=wick ' in text:stop_wall=time.monotonic()
                acknowledged='[initator] spawn failed name=bulk ' if cancel else '[initator] started name=bulk '
                outcome='Failed' if cancel else 'Degraded'
                if ready_wall is not None and stop_wall is not None and acknowledged in text and f'[initator] activation outcome={outcome}' in text:
                    assert text.count('cause=Killed status=9')==(4 if cancel else 1),text[-8000:]
                    if cancel: assert '[initator] started name=bulk ' not in text,text[-8000:]
                    assert text.count('[load-probe] verified')>=3,text[-8000:]
                    result=metrics(text,stop_wall-ready_wall,allow_cancelled=cancel)
                    report.write_text(__import__('json').dumps(result,indent=2)+'\n')
                    print(f'PASS: {cpus} CPUs, large-loader watchdog: {result}, serial={log}');return
                assert time.monotonic()<until,raw[-8000:]
                time.sleep(.01)
        finally:
            process.terminate();process.wait(timeout=5)
if __name__=='__main__':main()
