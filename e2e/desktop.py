#!/usr/bin/env python3
"""Boot the desktop, launch apps, verify shell commands, focus and close windows.
Build first: ./tools/build-grub-disk.sh
Run: python3 e2e/desktop.py
"""
import json, pathlib, re, socket, subprocess, tempfile, time
from PIL import Image
ROOT=pathlib.Path(__file__).resolve().parents[1]
WIDTH,HEIGHT=1920,1080
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
        def wait_for(value,after=0):
            end=time.monotonic()+45
            def observed():
                raw=log.read_text(errors='replace')[after:]
                output=''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',raw,re.S))
                return value in raw or value in output
            while not observed():
                assert qemu.poll() is None,'QEMU exited'
                assert time.monotonic()<end, f'missing {value}\n{log.read_text()[-4000:]}'
                time.sleep(.2)
        def shot(name):
            ppm=f'{tmp}/{name or "capture"}.ppm';cmd(f'screendump {ppm}',.3)
            image=Image.open(ppm).convert('RGB')
            if name:image.save(OUT/f'{name}.png')
            return image
        def wait_pixel(name,position,color,timeout=45):
            end=time.monotonic()+timeout
            while True:
                image=shot(name)
                if image.getpixel(position)==color:return image
                assert qemu.poll() is None,'QEMU exited'
                assert time.monotonic()<end,f'pixel {position}: expected {color}, got {image.getpixel(position)}'
                time.sleep(.2)
        mx,my=320,240
        def move(x,y):
            global mx,my
            # Absolute host input, the path used by GTK/Wayland without a grab.
            # Round up so the guest's integer scaling lands on the requested pixel.
            qmp_command('input-send-event', {'events':[
                {'type':'abs','data':{'axis':'x','value':(x*32767+WIDTH-2)//(WIDTH-1)}},
                {'type':'abs','data':{'axis':'y','value':(y*32767+HEIGHT-2)//(HEIGHT-1)}}]})
            time.sleep(.25);mx,my=x,y
        def click(x,y):
            move(x,y);cmd('mouse_button 1',.25);cmd('mouse_button 0',.25)
        def type_command(text):
            keys={' ':'spc','/':'slash','.':'dot','-':'minus'}
            for c in text:cmd('sendkey '+keys.get(c,c),.08)
            cmd('sendkey ret',.3)
        wait_for('[mouse] absolute pointer enabled');wait_for('[desktop] ready: ');time.sleep(1)
        wait_for("sys_spawn: '/BIN/DE.BIN'")
        wait_for('[compositor] desktop surface committed')
        assert log.read_text().count('[compositor] desktop surface committed')>=2,'DE redraw was not copied/released'
        image=shot('desktop')
        assert image.size==(WIDTH,HEIGHT),f'expected 1920x1080 framebuffer, got {image.size}'
        # Panels and wallpaper must fill the new geometry, including the final row/column.
        assert image.getpixel((WIDTH-1,0))==(0x11,0x1d,0x30),'top panel does not span display'
        assert image.getpixel((WIDTH-1,HEIGHT-1))==(0x10,0x1c,0x2e),'bottom panel misplaced'
        assert image.getpixel((WIDTH-1,HEIGHT//2))!=(0,0,0),'wallpaper missing at right edge'
        with Image.open(ROOT/'user_programs/de/wallpaper.ppm') as wallpaper:
            for point in [(1000,300),(1500,700),(1800,900)]:
                assert image.getpixel(point)==wallpaper.getpixel(point),'default space wallpaper was not presented by DE'
        for x,y in [(WIDTH-1,HEIGHT-1),(WIDTH-8,HEIGHT//2),(WIDTH//2,HEIGHT-14),(0,0)]:
            move(x,y);time.sleep(.5)
            assert shot(None).getpixel((x,y))==(8,19,33),f'pointer did not reach {(x,y)}'
        click(250,65);wait_for('Portal / RustOS shell');time.sleep(1)
        for command,expected in [('help','help echo clear pwd ls cat write mkdir time exit'),('echo desktop-ok','desktop-ok'),('pwd','/'),('ls /bin','PORTAL.BIN'),('write /test.txt saved','file written'),('cat /test.txt','saved'),('mkdir /demo','directory created'),('time','ticks since boot')]:
            type_command(command);wait_for("[portal] output: "+expected+"\n" if command not in ("time",) else "ticks since boot")
        shot('desktop-terminal')
        # Opening Snake must leave the terminal intact and both processes scheduled.
        click(350,65);wait_for("sys_spawn: '/BIN/SNAKE.BIN'");time.sleep(2);shot('desktop-multitasking')
        click(92+470,136+12);time.sleep(.5) # Close top (Snake) window.
        click(100,180);type_command('echo focus-restored');wait_for('focus-restored')
        # Drag beyond the old 640x480 bounds and clip the window above the bottom panel.
        move(150,138);start=log.stat().st_size;cmd('mouse_button 1')
        wait_for('[desktop] drag started: ',after=start)
        move(WIDTH-1,HEIGHT-1)
        # Glass samples the backdrop, so its titlebar has no fixed flat color.
        # The close control remains a stable pixel at the negotiated window edge.
        wait_pixel('desktop-1080p-drag',(WIDTH-18,HEIGHT-32-316+14),(0xff,0x6b,0x79))
        cmd('mouse_button 0');time.sleep(.5)
        click(WIDTH-18,HEIGHT-32-316+12);time.sleep(.5)
        click(250,65);wait_for('[desktop] launched Terminal pid=');time.sleep(1)
        # Drag the terminal using its titlebar, then close it.
        move(150,138);start=log.stat().st_size;cmd('mouse_button 1')
        wait_for('[desktop] drag started: ',after=start)
        move(190,140);wait_pixel(None,(120+488-18,128+14),(0xff,0x6b,0x79))
        cmd('mouse_button 0');time.sleep(.5)
        click(120+470,128+12);time.sleep(.5)
        shot('desktop-after-close')
        for x,name,expected in [(50,'FORGE','self_rust_os forge'),(150,'HELLO','Hello from user space!'),(450,'SPIN','spin: starting')]:
            click(x,65);wait_for(expected);time.sleep(.7)
            width=640 if name=='HELLO' else 488
            click(80+width-18,126+14);time.sleep(.4)
        # The shell can die without taking the compositor or other apps down.
        shell_pid=re.search(r"sys_spawn: '/BIN/DE.BIN' -> pid (\d+)",log.read_text()).group(1)
        start=log.stat().st_size
        click(50,65);wait_for('self_rust_os forge',after=start)
        type_command('kill '+shell_pid)
        wait_for('[compositor] desktop disconnected',after=start)
        wait_pixel('desktop-compositor-alone',(1800,900),(0x0e,0x17,0x2b),timeout=5)
        start=log.stat().st_size
        type_command('echo compositor-alive');wait_for('\ncompositor-alive\n',after=start)
        click(80+488-18,126+14)
        move(1800,950) # Inspect the exposed scene, away from the software cursor.
        wait_pixel(None,(80+488-18,126+14),(0x0e,0x17,0x2b),timeout=5)
        assert 'Kernel panic' not in log.read_text()
        print('PASS: separate compositor/DE, space wallpaper, 1920x1080 edges/pointer, launchers, commands, concurrency, focus/drag/close, and compositor survival after DE exit')
    except Exception:
        if log.exists():print(log.read_text(errors='replace')[-8000:])
        raise
    finally:
        qemu.terminate();qemu.wait(timeout=5)
