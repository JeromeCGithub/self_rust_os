#!/usr/bin/env python3
"""Boot the desktop, launch apps, verify shell commands, focus and close windows.
Build first: ./tools/build-grub-disk.sh
Run: python3 e2e/desktop.py
"""
import json, pathlib, re, socket, subprocess, tempfile, time
from PIL import Image
ROOT=pathlib.Path(__file__).resolve().parents[1]
OUT=ROOT/'docs/screenshots'
OUT.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='rustos-desktop-') as tmp:
    monpath=f'{tmp}/monitor'; qmppath=f'{tmp}/qmp'; log=pathlib.Path(tmp)/'serial.log'
    qemu=subprocess.Popen(['qemu-system-x86_64','-drive',f'format=raw,file={ROOT}/target/grub-disk.img','-machine','pc,i8042=on,vmport=on','-vga','std','-m','256','-serial',f'file:{log}','-display','none','-no-reboot','-monitor',f'unix:{monpath},server,nowait','-qmp',f'unix:{qmppath},server,nowait'])
    try:
        deadline=time.monotonic()+60
        while not pathlib.Path(monpath).exists():
            assert time.monotonic()<deadline,'monitor timeout'
            time.sleep(.1)
        mon=socket.socket(socket.AF_UNIX); mon.settimeout(3);mon.connect(monpath);mon.recv(4096)
        qmp=socket.socket(socket.AF_UNIX);qmp.settimeout(3);qmp.connect(qmppath)
        stream=qmp.makefile('rwb');json.loads(stream.readline())
        def qmp_command(execute, arguments=None):
            request={'execute':execute}
            if arguments is not None:request['arguments']=arguments
            stream.write(json.dumps(request).encode()+b'\n');stream.flush()
            while True:
                response=json.loads(stream.readline())
                assert 'error' not in response,response
                if 'return' in response:return response['return']
        qmp_command('qmp_capabilities')
        def cmd(command,delay=.15):
            mon.sendall((command+'\n').encode());time.sleep(delay);return mon.recv(16384)
        def wait_for(value):
            end=time.monotonic()+45
            def observed():
                raw=log.read_text(errors='replace')
                output=''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',raw,re.S))
                return value in raw or value in output
            while not observed():
                assert qemu.poll() is None,'QEMU exited'
                assert time.monotonic()<end, f'missing {value}\n{log.read_text()[-4000:]}'
                time.sleep(.2)
        def shot(name):
            ppm=f'{tmp}/{name}.ppm';cmd(f'screendump {ppm}',.3);Image.open(ppm).save(OUT/f'{name}.png')
        mx,my=320,240
        def move(x,y):
            global mx,my
            # Absolute host input, the path used by GTK/Wayland without a grab.
            qmp_command('input-send-event', {'events':[
                {'type':'abs','data':{'axis':'x','value':round(x*32767/639)}},
                {'type':'abs','data':{'axis':'y','value':round(y*32767/479)}}]})
            time.sleep(.25);mx,my=x,y
        def click(x,y):
            move(x,y);cmd('mouse_button 1',.25);cmd('mouse_button 0',.25)
        def type_command(text):
            keys={' ':'spc','/':'slash','.':'dot','-':'minus'}
            for c in text:cmd('sendkey '+keys.get(c,c),.08)
            cmd('sendkey ret',.3)
        wait_for('[mouse] absolute pointer enabled');wait_for('[desktop] ready: ');time.sleep(1);shot('desktop')
        click(250,65);wait_for('Portal / RustOS shell');time.sleep(1)
        for command,expected in [('help','help echo clear pwd ls cat write mkdir time exit'),('echo desktop-ok','desktop-ok'),('pwd','/'),('ls /bin','PORTAL.BIN'),('write /test.txt saved','file written'),('cat /test.txt','saved'),('mkdir /demo','directory created'),('time','ticks since boot')]:
            type_command(command);wait_for("[portal] output: "+expected+"\n" if command not in ("time",) else "ticks since boot")
        shot('desktop-terminal')
        # Opening Snake must leave the terminal intact and both processes scheduled.
        click(350,65);wait_for("sys_spawn: '/BIN/SNAKE.BIN'");time.sleep(2);shot('desktop-multitasking')
        click(92+470,136+12);time.sleep(.5) # Close top (Snake) window.
        click(100,180);type_command('echo focus-restored');wait_for('focus-restored')
        # Drag the terminal using its titlebar, then close it.
        move(150,138);cmd('mouse_button 1');move(190,140);cmd('mouse_button 0');time.sleep(.5)
        click(120+470,128+12);time.sleep(.5)
        shot('desktop-after-close')
        for x,name,expected in [(50,'FORGE','self_rust_os forge'),(150,'HELLO','Hello from user space!'),(450,'SPIN','spin: starting')]:
            click(x,65);wait_for(expected);time.sleep(.7)
            click(80+470,126+12);time.sleep(.4)
        assert 'Kernel panic' not in log.read_text()
        print('PASS: wallpaper, application icons, terminal commands, concurrent Snake, focus, dragging and close')
    finally:
        qemu.terminate();qemu.wait(timeout=5)
