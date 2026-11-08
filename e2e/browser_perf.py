#!/usr/bin/env python3
"""Native browser timing: real socket fixtures, fresh processes and painted pixels."""
import sys
from pathlib import Path
sys.path=[p for p in sys.path if Path(p or '.').resolve()!=Path(__file__).resolve().parent]
import http.server
import json
import os
import socket
import subprocess
import tempfile
import threading
import time
from PIL import Image
ROOT=Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True
sys.path.insert(0,str(ROOT/'tools'))
from browser_metrics import assess
from qemu_test import connect,copy_disk,disk_image,launchers,console_text
from qemu_platform import arguments as platform
from qemu_network import arguments as networking

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_GET(self):
        if self.path=='/theme.css':body=b'body{background:#eaf4ff}h1{color:#124578}'
        elif self.path=='/next':body=b'<title>Timing next</title><style>body{background:#d2eddd}</style><h1>Next document</h1>'
        else:body=b'<title>Timing page</title><link rel=stylesheet href=/theme.css><h1>Measured browser</h1><a href=/next>Next document</a>'
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)

def main():
    out=ROOT/'target/browser-perf';out.mkdir(parents=True,exist_ok=True)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    samples=[]
    try:
      with tempfile.TemporaryDirectory(prefix='jbrowse-perf-') as directory:
        tmp=Path(directory);disk=copy_disk(disk_image(ROOT),tmp);log=tmp/'serial.log'
        qemu=subprocess.Popen(['qemu-system-x86_64','-drive',f'format=raw,file={disk}','-machine','pc,i8042=on,vmport=on','-vga','std','-display','none','-serial',f'file:{log}','-no-reboot','-qmp',f'unix:{tmp}/qmp,server,nowait']+platform()+networking())
        try:
          qmp=connect(tmp/'qmp',time.monotonic()+60,qemu);qmp.settimeout(5)
          with qmp,qmp.makefile('rwb') as stream:
            stream.readline()
            def command(name,args=None):
                request={'execute':name}
                if args is not None:request['arguments']=args
                stream.write(json.dumps(request).encode()+b'\n');stream.flush()
                while True:
                    response=json.loads(stream.readline());assert 'error' not in response,response
                    if 'return' in response:return response['return']
            command('qmp_capabilities')
            def output():return console_text(log.read_text(errors='replace'))
            def wait(text,mark=0):
                end=time.monotonic()+45
                while text not in output()[mark:]:
                    assert qemu.poll() is None,'QEMU exited'
                    assert time.monotonic()<end,'missing '+text
                    time.sleep(.01)
            def click(x,y):
                command('input-send-event',{'events':[{'type':'abs','data':{'axis':axis,'value':(value*32767+size-2)//(size-1)}} for axis,value,size in [('x',x,1920),('y',y,1080)]]})
                for down in [True,False]:
                    command('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.04)
            def key(name):command('human-monitor-command',{'command-line':'sendkey '+name+' 1'})
            def type_text(value):
                keys={'/':'slash',':':'shift-semicolon','.':'dot','-':'minus',' ':'spc'}
                for char in value:key(keys.get(char,char));time.sleep(.025)
            def frame(point,color,name):
                end=time.monotonic()+30
                while True:
                    command('screendump',{'filename':str(tmp/'screen.ppm')})
                    with Image.open(tmp/'screen.ppm') as image:
                        if image.getpixel(point)==color:
                            image.save(out/(name+'.png'));return time.monotonic()
                    assert qemu.poll() is None and time.monotonic()<end,'paint watchdog: '+name
                    time.sleep(.01)
            wait('[desktop] ready:');click(launchers(disk)['PORTAL'],65);wait('[portal] shell pid=')
            for trial in range(3):
                type_text(f'./jbrowse http://10.0.2.2:{server.server_port}/')
                mark=len(output());started=time.monotonic();key('ret')
                wait('[jbrowse] ready',mark)
                window=frame((110,166),(38,61,89),f'window-{trial}')-started
                wait('[jbrowse] loaded: Timing page',mark)
                first=frame((592,356),(234,244,255),f'first-{trial}')-started
                # Native link row; excludes URL typing time from navigation.
                mark=len(output());started=time.monotonic();click(140,292)
                wait('[jbrowse] loaded: Timing next',mark)
                navigation=frame((592,356),(210,237,221),f'next-{trial}')-started
                sample=dict(window=window,first_page=first,navigation=navigation)
                samples.append(sample);print(json.dumps({'trial':trial,**sample}),flush=True)
                mark=len(output());click(714,142);type_text('echo perf-closed');key('ret');wait('[portal] output: perf-closed',mark)
            result=assess(samples)
            result.update(samples=samples,cpus=int(os.environ.get('RUSTOS_CPUS','1')),clock='host monotonic',profile='1GiB, TCG, 1920x1080, independent local HTTP/CSS')
            (out/'metrics.json').write_text(json.dumps(result,indent=2)+'\n')
            (out/'serial.log').write_text(log.read_text(errors='replace'))
            print(json.dumps(result,indent=2),flush=True)
            assert result['passed'],'browser exceeded fixed startup/page/navigation budgets'
        finally:
            qemu.terminate()
            try:qemu.wait(timeout=5)
            except subprocess.TimeoutExpired:qemu.kill();qemu.wait()
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
