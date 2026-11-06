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
import socket
import sys
import tempfile
import threading
import time
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_test import launchers, connect, copy_disk, disk_image, console_text
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments

if os.environ.get('RUSTOS_JBROWSE_REQUIRE_GOOGLE_RESULTS') == '1':
    os.environ['RUSTOS_JBROWSE_GOOGLE']='1'

requests = []
stalled_started=threading.Event()
stalled_closed=threading.Event()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_GET(self):
        requests.append(self.path)
        if self.path == '/stalled':
            self.send_response(200)
            self.send_header('Content-Length', '100')
            self.end_headers()
            self.wfile.flush()
            stalled_started.set()
            self.connection.settimeout(5)
            try:
                if self.connection.recv(1) == b'': stalled_closed.set()
            except ConnectionResetError: stalled_closed.set()
            except TimeoutError: pass
            return
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
            '/after-cancel': b'<title>HTTP cancellation recovered</title><style>body {background:#e2f3e4}</style><h1>HTTP cancellation recovered</h1>',
            '/after-tls': b'<title>TLS cancellation recovered</title><style>body {background:#dbf0fa}</style><h1>TLS cancellation recovered</h1>',
            '/big': b'x' * 262145,
            '/pixels.png': (ROOT/'crates/web-image/tests/fixtures/two-pixels.png').read_bytes(),
            '/image': b"<title>Image fixture</title><a href='/dir/next.html'><img src='/pixels.png' width=120 alt='Image fallback'></a>",
            '/bad.png': b'not an image',
            '/bad-image': b"<title>Broken image fixture</title><img src='/bad.png' alt='Readable fallback'><p>Image failure recovery</p>",
            '/typography': "<title>Typography fixture</title><style>body{font-family:sans-serif;font-size:16px;background:#f9f9f9}</style><h1>Readable text</h1><p><span style='color:#0000ff'>WWW</span> <span style='color:#ff0000'>iii</span> café Ω Ж</p><p style='font-family:serif'>A proportional serif paragraph.</p><pre>Monospace code: 0123456789</pre>".encode(),
            '/form': b"<title>Search fixture</title><form action='/find'><input type=hidden name=hl value=en><input name=q title='Search'><input type=submit name=btnG value=Search></form>",
            '/script': b"<title>Before script</title><p id='result'>Waiting</p><script>const n=6*7; document.getElementById('result').textContent='Answer: '+n; document.getElementById('result').style.color='#123456'; document.body.style.backgroundColor='#e8dcff'; document.title='Script ready';</script>",
            '/native-script': b"<title>Before native</title><p id='result'>Waiting</p><script>class Counter {#n=40;next(){return ++this.#n}} const c=new Counter(); const p=document.getElementById('result'); if(p!==document.getElementById('result') || typeof os!=='undefined' || typeof __rustosHost!=='undefined')throw Error('boundary'); p.style.color='#0c5a42'; Promise.resolve([c.next(),c.next()].map(x=>x*2)).then(xs=>{p.textContent='Native functions: '+xs.join(',');document.body.style.backgroundColor='#c9eaff';document.title='Native engine ready';}); ({toString(){throw Error('unused completion')}});</script>",
            '/native-queue': b"<title>Native queue bounded</title><style>body {background:#fff0dd}</style><p>Still readable after a microtask loop</p><script>function again(){Promise.resolve().then(again)};again()</script>",
            '/blank-script': b"<title>Blank script</title><script>while(true) {}</script>",
            '/bad-script': b"<title>Unsupported script</title><style>body {background:#fce8e6}</style><script>while(true) {}</script><p>Still readable</p>",
        }
        if self.path.startswith('/find?'):
            from urllib.parse import parse_qs,urlsplit
            query=parse_qs(urlsplit(self.path).query).get('q',[''])[0]
            pages[self.path]=("<title>Search results</title><style>body {background:#ddf3ff}</style><h1>Search results</h1><p>"+query+"</p><a href='/dir/next.html'>A matching result</a>").encode()
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
tls_started=threading.Event();tls_closed=threading.Event()
tls_listener=None
tls_connections=[]
if os.environ.get('RUSTOS_JBROWSE_LIFECYCLE') == '1':
    tls_listener=socket.socket()
    tls_listener.bind(('127.0.0.1',0));tls_listener.listen(1);tls_listener.settimeout(.2)
    def stalled_tls():
        while tls_listener.fileno()>=0:
            try: connection,_=tls_listener.accept()
            except socket.timeout: continue
            except OSError: return
            tls_connections.append(connection)
            with connection:
                connection.settimeout(5)
                try:
                    if connection.recv(16384): tls_started.set()
                    while connection.recv(16384): pass
                    tls_closed.set()
                except ConnectionResetError: tls_closed.set()
                except (TimeoutError,OSError): pass
            return
    threading.Thread(target=stalled_tls,daemon=True).start()
server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory(prefix='jbrowse-') as tmp:
    tmp = Path(tmp)
    disk = copy_disk(disk_image(ROOT), tmp)
    terminal_x = launchers(disk)['PORTAL']
    descriptor, serial_path = tempfile.mkstemp(prefix='serial-', suffix='.log', dir=out)
    os.close(descriptor)
    serial = Path(serial_path)
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
            return console_text(raw)
        last_navigation=0
        def wait(value, after=None):
            if after is None: after=last_navigation
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
        start_path='/stalled' if os.environ.get('RUSTOS_JBROWSE_STOP_STARTUP') == '1' else '/start'
        type_text('./jbrowse' if os.environ.get('RUSTOS_JBROWSE_GOOGLE') == '1' else f'./jbrowse http://10.0.2.2:{server.server_port}{start_path}')
        key('ret')
        wait('[jbrowse] ready')
        # Second native window: x=92, titlebar y=128, content y=156.
        ox,oy=92,156
        if os.environ.get('RUSTOS_JBROWSE_STOP_STARTUP') == '1':
            assert stalled_started.wait(5), 'startup request not sent'
            mark=len(output());click(ox+608,oy+42);wait('[jbrowse] stopped',mark)
            assert stalled_closed.wait(.5), 'startup stop did not close its socket'
            print('PASS: Stop cancels the first navigation before any page commits')
        if os.environ.get('RUSTOS_JBROWSE_GOOGLE') == '1':
            wait('[jbrowse] loaded: Google')
            # Wait for the actual native input rectangle, excluding the loading
            # placeholder and chrome. The published screenshot must show Google.
            deadline=time.monotonic()+20
            entry=None
            while entry is None:
                image=shot('google-home')
                for y in range(oy+65,oy+346):
                    for x in range(ox+1,ox+239):
                        if image.getpixel((x,y))==(247,251,255) and image.getpixel((x+180,y))==(247,251,255):
                            entry=(x+20,y+8);break
                    if entry is not None:break
                assert time.monotonic()<deadline,'Google homepage has no rendered search input'
            click(*entry);type_text('rust operating system')
            mark=len(output());key('ret')
            wait('[jbrowse] loaded:',mark)
            # The current public endpoint may return a JavaScript gate. Capture
            # the response as evidence; this does not assert usable search results.
            deadline=time.monotonic()+20
            while True:
                image=shot('google-search-response')
                colors=image.crop((ox,oy+64,ox+640,oy+372)).getcolors(640*308)
                if not any(color==(247,251,255) and count>100 for count,color in colors):break
                assert time.monotonic()<deadline,'Google search response did not replace homepage'
                time.sleep(.1)
            if os.environ.get('RUSTOS_JBROWSE_REQUIRE_GOOGLE_RESULTS') == '1':
                assert '[jbrowse] unsupported script page' not in output()[mark:], 'Google results remain blocked by unsupported JavaScript'
        def navigate(path):
            global last_navigation
            last_navigation=len(output())
            click(ox+120,oy+42)
            type_text(f'http://10.0.2.2:{server.server_port}{path}')
            key('ret')
        if os.environ.get('RUSTOS_JBROWSE_NATIVE_SCRIPTS') == '1':
            navigate('/native-script');wait('[jbrowse] loaded: Native engine ready')
            image=pixel((ox+500,oy+200),(201,234,255),'native-script')
            colors=image.crop((ox,oy+64,ox+500,oy+150)).getcolors(500*86)
            assert sum(count for count,color in colors if color==(12,90,66))>40, 'native DOM text mutation was not painted'
            navigate('/native-queue');wait('[jbrowse] loaded: Native queue bounded')
            pixel((ox+500,oy+200),(255,240,221),'native-queue')
            assert '[jbrowse] script errors: 1' in output()[last_navigation:], 'unbounded microtask queue was not rejected'
            print('PASS: native restricted QuickJS, classes/functions/promises, DOM pixels and bounded microtask recovery')
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
        navigate('/form');wait('[jbrowse] loaded: Search fixture')
        # Entry x=8,y=8 in document coordinates; submit is on the next row.
        click(ox+100,oy+84);type_text('rust os');key('ret')
        wait('[jbrowse] loaded: Search results')
        assert '/find?hl=en&q=rust+os' in requests,requests
        pixel((ox+500,oy+200),(221,243,255),'form-search')
        navigate('/form');wait('[jbrowse] loaded: Search fixture')
        click(ox+100,oy+84);type_text('browser');click(ox+40,oy+118)
        wait('[jbrowse] loaded: Search results')
        assert '/find?hl=en&q=browser&btnG=Search' in requests,requests
        pixel((ox+500,oy+200),(221,243,255),'form-button')
        if os.environ.get('RUSTOS_JBROWSE_SCRIPTS') == '1':
            navigate('/script');wait('[jbrowse] loaded: Script ready')
            image=pixel((ox+500,oy+200),(232,220,255),'scripted-page')
            colors=image.crop((ox,oy+64,ox+320,oy+150)).getcolors(320*86)
            assert any(color==(18,52,86) and count>20 for count,color in colors), 'scripted text color absent'
            navigate('/bad-script');wait('[jbrowse] loaded: Unsupported script')
            pixel((ox+500,oy+200),(252,232,230),'unsupported-script')
            navigate('/blank-script');wait('[jbrowse] loaded: Blank script')
            image=pixel((ox+500,oy+200),(255,255,255),'script-fallback')
            colors=image.crop((ox,oy+64,ox+640,oy+200)).getcolors(640*136)
            assert sum(count for count,color in colors if color==(32,33,36))>100,'unsupported-script fallback is blank'
            # The bounded interpreter must reject loops and return to navigation.
            navigate('/dir/next.html');wait('[jbrowse] loaded: Second page')
            pixel((ox+500,oy+200),(212,237,218),'script-recovery')
        if os.environ.get('RUSTOS_JBROWSE_PUBLIC') == '1':
            before_public=shot('before-public').crop((ox,oy+64,ox+640,oy+200)).tobytes()
            click(ox+120,oy+42)
            type_text('http://info.cern.ch/hypertext/WWW/TheProject.html')
            key('ret')
            wait('[jbrowse] loaded: The World Wide Web project')
            # The old background probe now intersects proportional text.
            # Probe the empty margin and require fresh document pixels as well.
            deadline=time.monotonic()+20
            while True:
                image=pixel((ox+2,oy+100),(255,255,255),'cern-public')
                if image.crop((ox,oy+64,ox+640,oy+200)).tobytes()!=before_public: break
                assert time.monotonic()<deadline, 'public document did not replace old pixels'
                time.sleep(.1)
            # Real page text occupies the document area; toolbar is excluded.
            colors=image.crop((ox,oy+64,ox+640,oy+200)).getcolors(640*136)
            assert sum(count for count,color in colors if color != (255,255,255)) > 100, 'public page has no rendered text'
        if os.environ.get('RUSTOS_JBROWSE_IMAGES') == '1':
            navigate('/image');wait('[jbrowse] loaded: Image fixture')
            pixel((ox+20,oy+84),(255,0,0),'decoded-image')
            pixel((ox+100,oy+84),(127,255,127),'alpha-image')
            pixel((ox+100,oy+130),(127,255,127),'image-no-text-decoration')
            click(ox+20,oy+84);wait('[jbrowse] loaded: Second page')
            mark=len(output());click(ox+21,oy+42);wait('[jbrowse] loaded: Image fixture',mark)
            pixel((ox+20,oy+84),(255,0,0),'history-image')
            mark=len(output());click(ox+53,oy+42);wait('[jbrowse] loaded: Second page',mark)
            pixel((ox+500,oy+200),(212,237,218),'history-forward')
            navigate('/bad-image');wait('[jbrowse] loaded: Broken image fixture')
            image=pixel((ox+500,oy+200),(255,255,255),'bad-image-fallback')
            assert any(color!=(255,255,255) and count>20 for count,color in image.crop((ox,oy+64,ox+400,oy+150)).getcolors(400*86)), 'image failure erased document fallback'
            print('PASS: native PNG pixels, alpha/scaling, image link hits and failure fallback')
        if os.environ.get('RUSTOS_JBROWSE_TYPOGRAPHY') == '1':
            navigate('/typography');wait('[jbrowse] loaded: Typography fixture')
            image=pixel((ox+500,oy+200),(249,249,249),'typography')
            crop=image.crop((ox,oy+64,ox+640,oy+300))
            colors=crop.getcolors(crop.width*crop.height)
            assert len(colors)>20, 'page text has no antialiased coverage'
            blue=[];red=[]
            for y in range(crop.height):
                for x in range(crop.width):
                    r,g,b=crop.getpixel((x,y))
                    if b>200 and r<120 and g<120:blue.append(x)
                    if r>200 and g<120 and b<120:red.append(x)
            assert blue and red, 'proportional glyph fixtures absent'
            assert max(blue)-min(blue)>2*(max(red)-min(red)), 'WWW and iii still use a fixed glyph cell'
            print('PASS: native proportional antialiased fonts, CSS families and Unicode fixture')
        if os.environ.get('RUSTOS_JBROWSE_LIFECYCLE') == '1':
            stalled_started.clear();stalled_closed.clear()
            navigate('/stalled')
            assert stalled_started.wait(5), 'stalled HTTP fixture not requested'
            began=time.monotonic()
            navigate('/after-cancel');wait('[jbrowse] loaded: HTTP cancellation recovered')
            assert stalled_closed.wait(.5), 'superseded request socket stayed open'
            assert time.monotonic()-began<5, 'navigation waited for old connection timeout'
            pixel((ox+500,oy+200),(226,243,228),'cancelled-navigation')
            click(ox+120,oy+42);type_text(f'https://10.0.2.2:{tls_listener.getsockname()[1]}/');key('ret')
            assert tls_started.wait(5), 'TLS fixture did not receive handshake bytes'
            began=time.monotonic()
            navigate('/after-tls');wait('[jbrowse] loaded: TLS cancellation recovered')
            assert tls_closed.wait(.5), 'cancelled TLS handshake socket stayed open'
            assert time.monotonic()-began<5, 'navigation waited for old TLS timeout'
            pixel((ox+500,oy+200),(219,240,250),'cancelled-tls')
            print('PASS: stalled HTTP/TLS cancellation, fresh destinations and framebuffer commits')
        # Close, return to the independently running terminal, then relaunch.
        click(ox+622,128+14)
        time.sleep(.5)
        click(100,180)
        type_text('echo browser-closed');key('ret')
        wait('[portal] output: browser-closed')
        mark=len(output());type_text('./jbrowse');key('ret');wait('[jbrowse] ready',mark)
        click(ox+622,128+14)
        print('PASS: HTML/CSS pixels, links, editable GET forms, Enter/button search submission, scrolling, failure recovery, terminal survival and relaunch')
        (out/'requests.json').write_text(json.dumps(requests,indent=2))
    finally:
        qemu.terminate()
        try:
            qemu.wait(timeout=5)
        except subprocess.TimeoutExpired:
            qemu.kill();qemu.wait()
        server.shutdown();server.server_close()
        if tls_listener is not None: tls_listener.close()
        for connection in tls_connections: connection.close()
