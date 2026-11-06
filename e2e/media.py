#!/usr/bin/env python3
"""Real Ring3 WAV player controls and captured PCM on a disposable desktop."""
import sys
from pathlib import Path
sys.path=[p for p in sys.path if Path(p or '.').resolve()!=Path(__file__).resolve().parent]
import array
import json
import re
import subprocess
import tempfile
import time
import wave
from PIL import Image
ROOT=Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True;sys.path.insert(0,str(ROOT/'tools'))
from qemu_test import copy_disk,disk_image,launchers,connect,console_text
from qemu_platform import arguments
from audio_capture import finalize

def main():
    out=ROOT/'target/media';out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='rustos-media-') as directory:
        tmp=Path(directory);disk=copy_disk(disk_image(ROOT),tmp);clip=tmp/'tone.wav';log=tmp/'serial.log';sound=tmp/'sound.wav'
        samples=array.array('h')
        for frame in range(96000):samples.extend([6000 if frame%48<24 else -6000]*2)
        if sys.byteorder!='little':samples.byteswap()
        with wave.open(str(clip),'wb') as wav:wav.setparams((2,2,48000,0,'NONE','none'));wav.writeframes(samples.tobytes())
        subprocess.run(['mcopy','-o','-i',f'{disk}@@1048576',str(clip),'::/HOME/TONE.WAV'],check=True)
        qemu=subprocess.Popen(['qemu-system-x86_64','-drive',f'format=raw,file={disk}','-machine','pc,i8042=on,vmport=on','-vga','std','-display','none','-serial',f'file:{log}','-no-reboot','-qmp',f'unix:{tmp}/qmp,server,nowait','-audiodev',f'wav,id=test,path={sound},out.frequency=48000,out.channels=2,out.format=s16','-device','AC97,audiodev=test']+arguments())
        try:
            qmp=connect(tmp/'qmp',time.monotonic()+60,qemu);qmp.settimeout(5)
            with qmp,qmp.makefile('rwb') as stream:
                stream.readline()
                def command(name,args=None):
                    req={'execute':name}
                    if args is not None:req['arguments']=args
                    stream.write(json.dumps(req).encode()+b'\n');stream.flush()
                    while True:
                        response=json.loads(stream.readline());assert 'error' not in response,response
                        if 'return' in response:return response['return']
                command('qmp_capabilities')
                def output():return console_text(log.read_text(errors='replace'))
                def wait(text,mark=0):
                    end=time.monotonic()+30
                    while text not in output()[mark:]:
                        assert qemu.poll() is None and time.monotonic()<end,'missing '+text+'\n'+output()[-2000:]
                        time.sleep(.02)
                def click(x,y):
                    command('input-send-event',{'events':[{'type':'abs','data':{'axis':axis,'value':(value*32767+size-2)//(size-1)}} for axis,value,size in [('x',x,1920),('y',y,1080)]]});time.sleep(.06)
                    for down in [True,False]:command('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.06)
                def shot(name):
                    command('screendump',{'filename':str(tmp/'frame.ppm')});image=Image.open(tmp/'frame.ppm').convert('RGB');image.save(out/(name+'.png'));return image
                def color(point,rgb,name):
                    end=time.monotonic()+10
                    while True:
                        image=shot(name)
                        if image.getpixel(point)==rgb:return image
                        assert time.monotonic()<end,(point,image.getpixel(point),rgb)
                        time.sleep(.02)
                wait('[desktop] ready:');click(launchers(disk)['JPLAY'],65);wait('[jplay] ready')
                # Native placement follows wm/window.rs; this is the first client.
                ox,oy=80,154
                progress=(ox+16,oy+112,ox+544,oy+124)
                color((ox+516,oy+97),(49,95,135),'ready')
                click(ox+46,oy+67);wait('[jplay] playing');color((ox+516,oy+97),(47,121,95),'playing')
                click(ox+166,oy+67);wait('[jplay] paused');color((ox+516,oy+97),(139,111,62),'paused')
                first=shot('paused');time.sleep(.2);second=shot('paused-stable')
                assert first.crop(progress).tobytes()==second.crop(progress).tobytes(),'paused progress moved'
                mark=len(output());click(ox+406,oy+67);wait('[jplay] seek',mark)
                end=time.monotonic()+10
                while first.crop(progress).tobytes()==shot('seek').crop(progress).tobytes():
                    assert time.monotonic()<end,'seek did not change progress';time.sleep(.02)
                mark=len(output());click(ox+46,oy+67);wait('[jplay] playing',mark);wait('[jplay] finished',mark)
                color((ox+516,oy+97),(49,95,135),'finished')
                pid=re.search(r"sys_spawn: '/BIN/JPLAY.BIN' -> pid (\d+)",log.read_text()).group(1)
                mark=len(output());click(ox+542,oy-14)
                end=time.monotonic()+10
                while f'[sched] process {pid} exited with code 0' not in log.read_text():
                    assert time.monotonic()<end,'player did not exit after close';time.sleep(.02)
                assert 'panic:' not in output()[mark:]
        finally:
            qemu.terminate()
            try:qemu.wait(timeout=5)
            except subprocess.TimeoutExpired:qemu.kill();qemu.wait()
            (out/'serial.log').write_text(log.read_text(errors='replace'))
        raw=sound.read_bytes();(out/'capture-original.wav').write_bytes(raw);(out/'capture.wav').write_bytes(finalize(raw))
        with wave.open(str(out/'capture.wav'),'rb') as wav:
            data=array.array('h',wav.readframes(wav.getnframes()))
            if sys.byteorder!='little':data.byteswap()
            assert data[::2]==data[1::2]
            assert sum(value!=0 for value in data[::2])>20000,'player output was silent/truncated'
        print('PASS: native WAV load, play, partial pause/resume, seek, progress pixels and captured stereo sound',flush=True)

if __name__=='__main__':main()
