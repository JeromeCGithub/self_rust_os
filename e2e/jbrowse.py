#!/usr/bin/env python3
"""Real Ring 3 browser: local HTTP fixture, QMP input and framebuffer assertions."""
import sys
from pathlib import Path
sys.path = [p for p in sys.path if Path(p or ".").resolve() != Path(__file__).resolve().parent]
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import time
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_test import launchers, connect, copy_disk, disk_image
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments

requests = []
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_GET(self):
        requests.append(self.path)
        if self.path == '/start':
            self.send_response(302)
            self.send_header('Location', '/dir/index.html')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        pages = {
            '/dir/index.html': b"<title>Browser fixture</title><link rel='stylesheet' href='theme.css'><div id='banner'><a href='next.html'>Follow link</a></div><p>HTML &amp; CSS from a real socket</p>" + b'<p>Scroll content</p>' * 40,
            '/dir/theme.css': b'body { background-color: #f5f0df; } #banner {background-color:#2468ac; padding:12px} a {color:white}',
            '/dir/next.html': b"<title>Second page</title><style>body {background:#d4edda}</style><h1>Navigation works</h1>",
            '/big': b'x' * 262145,
        }
        body = pages.get(self.path, b'<h1>Missing page</h1>')
        self.send_response(200 if self.path in pages else 404)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

out = ROOT / 'target/jbrowse-e2e'
out.mkdir(parents=True, exist_ok=True)
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory(prefix='jbrowse-') as tmp:
    tmp = Path(tmp)
    disk = copy_disk(disk_image(ROOT), tmp)
    terminal_x = launchers(disk)['PORTAL']
    serial = out / 'serial.log'
    qemu = subprocess.Popen(['qemu-system-x86_64', '-drive', f'format=raw,file={disk}', '-machine', 'pc,i8042=on,vmport=on', '-vga','std','-serial',f'file:{serial}','-display','none','-no-reboot','-qmp',f'unix:{tmp}/qmp,server,nowait'] + platform_arguments() + network_arguments())
    try:
        qmp = connect(tmp/'qmp', time.monotonic()+60, qemu)
        stream = qmp.makefile('rwb')
        stream.readline()
        def command(execute, arguments=None):
            request = {'execute': execute}
            if arguments is not None:
                request['arguments'] = arguments
            stream.write(json.dumps(request).encode()+b'\n')
            stream.flush()
            while True:
                response = json.loads(stream.readline())
                assert 'error' not in response, response
                if 'return' in response:
                    return response['return']
        command('qmp_capabilities')
        def hmp(text):
            return command('human-monitor-command', {'command-line':text})
        def output():
            raw = serial.read_text(errors='replace')
            return raw + ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))
        def wait(value, after=0):
            deadline = time.monotonic()+45
            while value not in output()[after:]:
                assert qemu.poll() is None, 'QEMU exited'
                assert time.monotonic()<deadline, f'missing {value}\n{output()[-5000:]}'
                time.sleep(.1)
        def move(x,y):
            command('input-send-event', {'events':[
                {'type':'abs','data':{'axis':'x','value':(x*32767+1918)//1919}},
                {'type':'abs','data':{'axis':'y','value':(y*32767+1078)//1079}}]})
            time.sleep(.12)
        def click(x,y):
            move(x,y)
            hmp('mouse_button 1');time.sleep(.12)
            hmp('mouse_button 0');time.sleep(.12)
        def key(code):
            hmp('sendkey '+code);time.sleep(.08)
        def type_text(text):
            keys={' ':'spc','/':'slash',':':'shift-semicolon','.':'dot','-':'minus','?':'shift-slash','=':'equal'}
            for c in text:
                key(keys.get(c, ('shift-'+c.lower()) if c.isupper() else c))
        def shot(name):
            command('screendump', {'filename':str(tmp/'frame.ppm')})
            image = Image.open(tmp/'frame.ppm').convert('RGB')
            image.save(out/(name+'.png'))
            return image
        def pixel(point, color, name):
            deadline=time.monotonic()+20
            while True:
                image=shot(name)
                if image.getpixel(point)==color:
                    return image
                assert time.monotonic()<deadline, f'{point}: {image.getpixel(point)} != {color}'
                time.sleep(.2)
        wait('[desktop] ready:')
        click(terminal_x,65)
        wait('[portal] shell pid=')
        type_text('./jbrowse')
        key('ret')
        wait('[jbrowse] ready')
        # The native window is placed after Portal at (92,128), content y=156.
        ox,oy=92,156
        def navigate(path):
            click(ox+120,oy+42)
            type_text(f'http://10.0.2.2:{server.server_port}{path}')
            key('ret')
        navigate('/start')
        wait('[jbrowse] loaded: Browser fixture')
        assert '/dir/theme.css' in requests, requests
        pixel((ox+500,oy+200),(245,240,223),'html-css')
        pixel((ox+12,oy+76),(36,104,172),'html-css')
        # Release inside an anchor navigates relative to the redirect target.
        click(ox+40,oy+90)
        wait('[jbrowse] loaded: Second page')
        assert '/dir/next.html' in requests, requests
        pixel((ox+500,oy+200),(212,237,218),'linked-page')
        navigate('/start');wait('[jbrowse] loaded: Browser fixture', after=0)
        pixel((ox+12,oy+76),(36,104,172),'before-scroll')
        click(ox+606,oy+386)
        pixel((ox+12,oy+76),(245,240,223),'scrolled')
        # Invalid schemes and oversized downloads recover without terminating.
        click(ox+120,oy+42);type_text('file:///etc/passwd');key('ret')
        wait('[jbrowse] error: invalid address')
        navigate('/big');wait('[jbrowse] error: response or URL exceeds limits')
        navigate('/dir/next.html');wait('[jbrowse] loaded: Second page')
        pixel((ox+500,oy+200),(212,237,218),'recovered')
        if os.environ.get('RUSTOS_JBROWSE_PUBLIC') == '1':
            click(ox+120,oy+42)
            type_text('http://info.cern.ch/hypertext/WWW/TheProject.html')
            key('ret')
            wait('[jbrowse] loaded: The World Wide Web project')
            image=pixel((ox+500,oy+200),(255,255,255),'cern-public')
            # Real page text occupies the document area; toolbar is excluded.
            colors=image.crop((ox,oy+64,ox+640,oy+200)).getcolors(640*136)
            assert sum(count for count,color in colors if color != (255,255,255)) > 100, 'public page has no rendered text'
        # Close, return to the independently running terminal, then relaunch.
        click(ox+622,128+14)
        time.sleep(.5)
        click(100,180)
        type_text('echo browser-closed');key('ret')
        wait('[portal] output: browser-closed')
        mark=len(output());type_text('./jbrowse');key('ret');wait('[jbrowse] ready',mark)
        click(ox+622,128+14)
        print('PASS: HTML/CSS pixels, redirected stylesheet/link, URL editing, scrolling, resource failure/recovery, independent terminal and relaunch')
        (out/'requests.json').write_text(json.dumps(requests,indent=2))
    finally:
        qemu.terminate()
        try:
            qemu.wait(timeout=5)
        except subprocess.TimeoutExpired:
            qemu.kill();qemu.wait()
        server.shutdown();server.server_close()
