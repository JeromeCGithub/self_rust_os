#!/usr/bin/env python3
"""Boot the desktop, launch apps, verify shell commands, focus and close windows.
Build first: ./tools/build-grub-disk.sh
Run: python3 e2e/desktop.py
"""
import json, os, pathlib, re, socket, subprocess, tempfile, time, sys
from PIL import Image
ROOT=pathlib.Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT/'tools'))
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments
from qemu_test import launchers, disk_image, connect as connect_monitor, copy_disk
WIDTH,HEIGHT=1920,1080
OUT=ROOT/'docs/screenshots'
OUT.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='rustos-desktop-') as tmp:
    disk=copy_disk(disk_image(ROOT),pathlib.Path(tmp))
    # Disposable read fixture exercises cat beyond its former 1024-byte cap.
    long_file=pathlib.Path(tmp)/'long.txt'
    long_file.write_bytes(b'A'*1500+b'\ncat-stream-tail\n')
    subprocess.run(['mcopy','-o','-i',f'{disk}@@1048576',str(long_file),'::/LONG.TXT'],check=True)
    launcher_positions=launchers(disk)
    def launcher_x(name):
        return launcher_positions[name]
    monpath=f'{tmp}/monitor'; qmppath=f'{tmp}/qmp'; log=pathlib.Path(tmp)/'serial.log'
    qemu=subprocess.Popen(['qemu-system-x86_64','-drive',f'format=raw,file={disk}','-machine','pc,i8042=on,vmport=on','-vga','std','-serial',f'file:{log}','-display','none','-no-reboot','-monitor',f'unix:{monpath},server,nowait','-qmp',f'unix:{qmppath},server,nowait'] + platform_arguments() + network_arguments())
    try:
        deadline=time.monotonic()+60
        while not pathlib.Path(monpath).exists():
            assert time.monotonic()<deadline,'monitor timeout'
            time.sleep(.1)
        mon=connect_monitor(monpath,deadline,qemu);mon.recv(4096)
        qmp=connect_monitor(qmppath,deadline,qemu)
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
            keys={' ':'spc','/':'slash','.':'dot','-':'minus',chr(34):'shift-apostrophe',chr(39):'apostrophe','=':'equal','_':'shift-minus'}
            for c in text:cmd('sendkey '+keys.get(c,('shift-'+c.lower()) if c.isupper() else c),.08)
            cmd('sendkey ret',.3)
        def check_arguments():
            start=log.stat().st_size
            type_command('export TEST_ENV="hello world"')
            wait_for('[portal] output: exported TEST_ENV',after=start)
            type_command("./hello 'unfinished")
            wait_for('[portal] output: parse error: unterminated quote or escape',after=start)
            type_command('./hello alpha "two words" ""')
            for expected in ['[hello] TEST_ENV=hello world','[hello] argc=4','[hello] argv[0]=/bin/HELLO.BIN','[hello] argv[1]=alpha','[hello] argv[2]=two words','[hello] argv[3]=\n']:
                wait_for(expected,after=start)
            wait_for('[hello-ui] ready: 640x400 glass',after=start)
            # Show the typed command and its running child side by side.
            move(150,140);cmd('mouse_button 1');move(900,220);cmd('mouse_button 0')
            shot('desktop-capabilities' if os.environ.get('RUSTOS_DESKTOP_CASE') == 'capabilities' else 'desktop-arguments')
            click(92+640-18+750,136+14+80);time.sleep(.4)
            click(100,180)
            start=log.stat().st_size
            type_command('echo argv-shell-alive');wait_for('[portal] output: argv-shell-alive',after=start)
            type_command('unset TEST_ENV');wait_for('[portal] output: unset TEST_ENV',after=start)
            for command,expected in [('mkdir "/tmp/directory with spaces"','created /tmp/directory with spaces'),
                                     ('cd "/tmp/directory with spaces"','cwd: /TMP/directory with spaces'),
                                     ('write long-filename.txt saved','wrote 5 bytes'),
                                     ('ls','long-filename.txt'),('cat long-filename.txt','saved'),('cd /','cwd: /')]:
                mark=log.stat().st_size;type_command(command);wait_for('[portal] output: '+expected+'\n',after=mark)

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
        click(launcher_x('PORTAL'),65);wait_for('[portal] shell pid=');wait_for('self_rust_os forge v1.0');time.sleep(1)
        if os.environ.get('RUSTOS_DESKTOP_CASE') in ('arguments','capabilities'):
            check_arguments()
            print('PASS: Forge quoting/errors, argument delivery to main(argc, argv), Hello UI and shell survival')
            sys.exit(0)
        for command,expected in [('help','Commands:'),('echo desktop-ok','desktop-ok'),('pwd','/'),('ls /bin','PORTAL.BIN'),('write /test.txt saved','wrote 5 bytes'),('cat /test.txt','saved'),('mkdir /demo','created /demo'),('time','ticks:')]:
            type_command(command);wait_for("[portal] output: "+expected+"\n" if command not in ("time",) else "ticks:")
        # Working directory belongs to Forge, and failed changes preserve it.
        for command,expected in [('cd /demo','cwd: /DEMO'),('pwd','/DEMO'),
                                 ('mkdir sub','created sub'),('cd sub','cwd: /DEMO/SUB'),
                                 ('write note.txt relative-data','wrote 13 bytes'),
                                 ('cat ./note.txt','relative-data'),('cd note.txt','cd failed'),
                                 ('cd /missing','cd failed'),('pwd','/DEMO/SUB'),
                                 ('cd ..','cwd: /DEMO'),('cd /','cwd: /')]:
            start=log.stat().st_size
            type_command(command);wait_for('[portal] output: '+expected+'\n',after=start)
        # A modal editor must save and reopen real disk contents, not command echoes.
        start=log.stat().st_size
        type_command('medit /demo/edit.txt');wait_for('medit: /demo/edit.txt',after=start)
        for c in 'iedited':cmd('sendkey '+c,.08)
        cmd('sendkey esc');shot('desktop-medit');cmd('sendkey shift-semicolon')
        type_command('wq');wait_for('medit: saved 6 bytes',after=start)
        start=log.stat().st_size
        type_command('cat /demo/edit.txt');wait_for('[portal] output: edited\n',after=start)
        shot('desktop-medit-saved')
        start=log.stat().st_size
        type_command('medit /demo/edit.txt');wait_for('medit: /demo/edit.txt',after=start)
        for c in 'ix':cmd('sendkey '+c,.08)
        cmd('sendkey esc');cmd('sendkey shift-semicolon');type_command('q')
        wait_for('unsaved changes; use :w or :q!',after=start)
        cmd('sendkey shift-semicolon');cmd('sendkey q');cmd('sendkey shift-1');cmd('sendkey ret')
        wait_for('medit: closed',after=start)
        start=log.stat().st_size
        type_command('cat /demo/edit.txt');wait_for('[portal] output: edited\n',after=start)
        start=log.stat().st_size
        type_command('cat /long.txt');wait_for('cat-stream-tail\n',after=start)
        # Forge-only commands must execute in the hosted process, not Portal.
        type_command('kill x');wait_for('[portal] output: invalid pid')
        type_command('cat /missing.txt');wait_for('[portal] output: cat failed')
        type_command('./missing');wait_for('[portal] output: failed to spawn /bin/MISSING.BIN')
        hosted = re.findall(r"\[portal\] shell pid=(\d+)", ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',log.read_text(),re.S)))
        assert hosted, 'no independently spawned Forge'
        wait_for("sys_spawn: '/BIN/FORGE.BIN' -> pid "+hosted[0])
        shot('desktop-terminal')
        # Opening Snake must leave the terminal intact and both processes scheduled.
        click(launcher_x('SNAKE'),65);wait_for("sys_spawn: '/BIN/SNAKE.BIN'");time.sleep(2);shot('desktop-multitasking')
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
        click(launcher_x('PORTAL'),65);wait_for('[desktop] launched Terminal pid=');time.sleep(1)
        # Drag the terminal using its titlebar, then close it.
        move(150,138);start=log.stat().st_size;cmd('mouse_button 1')
        wait_for('[desktop] drag started: ',after=start)
        move(190,140);wait_pixel(None,(120+488-18,128+14),(0xff,0x6b,0x79))
        cmd('mouse_button 0');time.sleep(.5)
        click(120+470,128+12);time.sleep(.5)
        shot('desktop-after-close')
        # Every hosted shell must be reaped when its terminal closes.
        output=''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',log.read_text(),re.S))
        for pid in re.findall(r"\[portal\] shell pid=(\d+)",output):
            wait_for('[sched] reaped zombie '+pid)
        start=log.stat().st_size
        click(launcher_x('PORTAL'),65);wait_for('[portal] shell pid=',after=start)
        type_command('exit');wait_for('[portal] shell disconnected',after=start)
        wait_for('reaped zombie',after=start)
        for x,name,expected in [(launcher_x('FORGE'),'FORGE','self_rust_os forge'),(launcher_x('HELLO'),'HELLO','Hello from user space!'),(launcher_x('SPIN'),'SPIN','spin: starting')]:
            click(x,65);wait_for(expected);time.sleep(.7)
            width=640 if name=='HELLO' else 488
            click(80+width-18,126+14);time.sleep(.4)
        start=log.stat().st_size
        click(launcher_x('PORTAL'),65);wait_for('[portal] shell pid=',after=start)
        check_arguments()
        click(80+488-18,126+14);time.sleep(.4)
        # The shell can die without taking the compositor or other apps down.
        shell_pid=re.search(r"sys_spawn: '/BIN/DE.BIN' -> pid (\d+)",log.read_text()).group(1)
        start=log.stat().st_size
        click(launcher_x('FORGE'),65);wait_for('self_rust_os forge',after=start)
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
        shot('desktop-failure')
        if log.exists():(ROOT/'target'/'desktop-failure.log').write_bytes(log.read_bytes())
        if log.exists():print(log.read_text(errors='replace')[-8000:])
        raise
    finally:
        qemu.terminate();qemu.wait(timeout=5)
