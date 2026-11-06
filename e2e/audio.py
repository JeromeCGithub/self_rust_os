#!/usr/bin/env python3
"""Native PCM syscall/ownership assertions plus independent captured-sound checks."""
import sys
from pathlib import Path
sys.dont_write_bytecode=True
import argparse
import array
import json
import os
import subprocess
import tempfile
import time
import wave
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tools'))
from qemu_test import copy_disk,disk_image,console_text
from qemu_platform import arguments
from audio_capture import finalize

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--missing',action='store_true');args=parser.parse_args()
    out=ROOT/'target/audio'/('cpus-'+os.environ.get('RUSTOS_CPUS','1'));out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='rustos-audio-') as directory:
        tmp=Path(directory);disk=copy_disk(disk_image(ROOT),tmp);log=tmp/'serial.log';capture=tmp/'audio.wav'
        for name in ['WM.BIN','ZAUDIO.BIN']:
            subprocess.run(['mcopy','-o','-i',f'{disk}@@1048576',str(ROOT/'user_programs/zaudio/zaudio.bin'),f'::/BIN/{name}'],check=True)
        command=['qemu-system-x86_64','-drive',f'format=raw,file={disk}','-display','none','-serial',f'file:{log}','-monitor','none','-no-reboot','-nic','none']+arguments()
        if not args.missing:command+=['-audiodev',f'wav,id=test,path={capture},out.frequency=48000,out.channels=2,out.format=s16','-device','AC97,audiodev=test']
        # Native argv ABI is exercised by launching the same probe as WM; the
        # missing-hardware variant is selected by an explicit fixture file.
        if args.missing:
            mode=tmp/'audio.mode';mode.write_bytes(b'M')
            subprocess.run(['mcopy','-o','-i',f'{disk}@@1048576',str(mode),'::/TMP/AUDIO.MOD'],check=True)
        qemu=subprocess.Popen(command)
        try:
            end=time.monotonic()+60
            expected='[audiotest] PASS missing hardware' if args.missing else '[audiotest] PASS PCM'
            while True:
                raw=log.read_text(errors='replace') if log.exists() else ''
                output=raw+console_text(raw)
                assert not any(value in output for value in ['panicked','panic:','Kernel panic']),output[-6000:]
                if expected in output:break
                assert qemu.poll() is None and time.monotonic()<end,output[-6000:]
                time.sleep(.05)
        finally:
            qemu.terminate()
            try:qemu.wait(timeout=5)
            except subprocess.TimeoutExpired:qemu.kill();qemu.wait()
            (out/('missing.log' if args.missing else 'serial.log')).write_text(log.read_text(errors='replace'))
        if not args.missing:
            raw=capture.read_bytes()
            (out/'capture-original.wav').write_bytes(raw)
            # This backend leaves zero RIFF/data lengths on host termination.
            # Preserve it and finalize only those lengths, retaining every sample.
            finalized=out/'capture.wav';finalized.write_bytes(finalize(raw))
            with wave.open(str(finalized),'rb') as wav:
                assert (wav.getnchannels(),wav.getsampwidth(),wav.getframerate())==(2,2,48000)
                data=array.array('h',wav.readframes(wav.getnframes()))
                if sys.byteorder!='little':data.byteswap()
                left=data[::2];right=data[1::2]
                assert left==right,'stereo channels differ'
                active=[sample for sample in left if sample]
                assert len(active)==36864,f'captured playback lost/replayed PCM frames: {len(active)}'
                assert max(active)>=5000 and min(active)<=-5000,'tone amplitude absent'
                crossings=sum((a<0)!=(b<0) for a,b in zip(active,active[1:]))
                frequency=48000*crossings/(2*len(active))
                assert 950<=frequency<=1050,f'wrong captured frequency: {frequency}'
                report=dict(sample_rate=48000,channels=2,active_frames=len(active),frequency_hz=frequency)
            (out/'capture.json').write_text(json.dumps(report,indent=2)+'\n')
            print('PASS: independently captured non-silent stereo PCM',report,flush=True)
        else:print('PASS: absent audio hardware returns typed NoDevice',flush=True)

if __name__=='__main__':main()
