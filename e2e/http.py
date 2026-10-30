#!/usr/bin/env python3
"""HTTP acceptance: independent peers, actual curl command and Ring 3 SDK probe."""
import argparse
import json
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--site', help='also fetch a public plain HTTP URL')
    args = parser.parse_args()
    out = ROOT / 'target/http'
    out.mkdir(parents=True, exist_ok=True)
    errors, requests = [], []
    stop = threading.Event()
    tcp = socket.socket()
    dns = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    for server in (tcp, dns):
        server.bind(('127.0.0.1', 0))
        server.settimeout(.2)
    tcp.listen(8)
    port, dns_port = tcp.getsockname()[1], dns.getsockname()[1]

    def serve():
        while not stop.is_set():
            try:
                connection, _ = tcp.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                with connection:
                    connection.settimeout(3)
                    raw = b''
                    while b'\r\n\r\n' not in raw:
                        more = connection.recv(2048)
                        assert more, 'EOF in request'
                        raw += more
                        assert len(raw) <= 4096
                    requests.append(raw)
                    path = raw.split(b' ')[1]
                    assert raw.startswith(b'GET ') and b' HTTP/1.1\r\nHost: ' in raw
                    assert b'Accept-Encoding: identity\r\n' in raw
                    if path == b'/redirect':
                        data = b'HTTP/1.1 302 Found\r\nLocation: /chunked\r\nContent-Length: 0\r\n\r\n'
                    elif path == b'/chunked':
                        data = b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7;v=1\r\nhttp-fi\r\n8\r\nxture-ok\r\n0\r\nX-End: yes\r\n\r\n'
                    elif path == b'/truncated':
                        data = b'HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort'
                    elif path == b'/close':
                        data = b'HTTP/1.0 404 Missing\r\n\r\neof-ok'
                    elif path == b'/large':
                        data = b'HTTP/1.1 200 OK\r\nContent-Length: 20000\r\n\r\n' + b'x' * 20000
                    else:
                        data = b'HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\nhttp-fixture-ok'
                    # Headers and body deliberately cross TCP receive boundaries.
                    for offset in range(0, len(data), 31):
                        connection.sendall(data[offset:offset+31])
                        if len(data) < 200: time.sleep(.01)
            except (BrokenPipeError, ConnectionResetError):
                pass  # Redirect/error cleanup may discard the remainder.
            except Exception as error:
                errors.append(error)

    def resolve():
        while not stop.is_set():
            try:
                raw, peer = dns.recvfrom(512)
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                assert raw[2:12] == b'\x01\0\0\x01\0\0\0\0\0\0'
                dns.sendto(raw[:2] + b'\x81\x80\0\x01\0\x01\0\0\0\0' + raw[12:] + b'\xc0\x0c\0\x01\0\x01\0\0\0\x1e\0\x04\x0a\0\x02\x02', peer)
            except Exception as error:
                errors.append(error)

    workers = [threading.Thread(target=f, daemon=True) for f in (serve, resolve)]
    for worker in workers: worker.start()
    try:
        with tempfile.TemporaryDirectory(prefix='rustos-http-') as tmp:
            tmp = Path(tmp)
            disk = tmp / 'disk.img'
            shutil.copyfile(ROOT / 'target/grub-disk.img', disk)
            config = tmp / 'http.cfg'
            config.write_bytes(struct.pack('<HH', port, dns_port))
            subprocess.run(['mcopy', '-i', f'{disk}@@1048576', str(config), '::/TMP/HTTP.CFG'], check=True)
            path = tmp / 'qmp'
            log = out / 'serial.log'
            qemu = subprocess.Popen(['qemu-system-x86_64', '-machine', 'pc,i8042=on,vmport=on', '-drive', f'format=raw,file={disk}', '-vga', 'std', '-display', 'none', '-serial', f'file:{log}', '-no-reboot', '-qmp', f'unix:{path},server=on,wait=off'] + platform_arguments() + network_arguments())
            qmp = None
            try:
                end = time.monotonic() + 30
                while not path.exists():
                    assert time.monotonic() < end and qemu.poll() is None
                    time.sleep(.1)
                qmp = socket.socket(socket.AF_UNIX)
                qmp.settimeout(5)
                qmp.connect(str(path))
                stream = qmp.makefile('rwb')
                json.loads(stream.readline())
                def call(command, arguments=None):
                    item = {'execute': command}
                    if arguments is not None: item['arguments'] = arguments
                    stream.write(json.dumps(item).encode() + b'\n'); stream.flush()
                    while True:
                        result = json.loads(stream.readline())
                        assert 'error' not in result, result
                        if 'return' in result: return result['return']
                def wait(value, mark=0):
                    end = time.monotonic() + 40
                    while True:
                        raw = log.read_text(errors='replace')[mark:]
                        raw += '\n' + ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))
                        if value in raw: return
                        assert 'output: unknown command' not in raw, 'curl command missing'
                        assert 'Kernel panic' not in raw and 'panic:' not in raw, raw[-4000:]
                        assert time.monotonic() < end and qemu.poll() is None, f'missing {value}\n{raw[-4000:]}'
                        time.sleep(.1)
                def click(x):
                    call('input-send-event', {'events': [{'type':'abs','data':{'axis':'x','value':x*32767//1919}}, {'type':'abs','data':{'axis':'y','value':65*32767//1079}}]})
                    time.sleep(.2)
                    for down in (True, False):
                        call('input-send-event', {'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.2)
                def type_line(line):
                    for char in line + '\n':
                        key = {' ':'spc', '.':'dot', '/':'slash', ':':'shift-semicolon', '-':'minus', '\n':'ret'}.get(char,char)
                        call('human-monitor-command', {'command-line':'sendkey '+key});time.sleep(.075)
                call('qmp_capabilities')
                wait('[desktop] ready: 7 applications')
                # Before CURL is installed, Portal is third; afterwards fourth.
                click(350 if (ROOT/'user_programs/curl/curl.bin').exists() else 250)
                wait('Portal / RustOS shell')
                mark=log.stat().st_size
                type_line(f'curl http://10.0.2.2:{port}/chunked')
                wait('[portal] output: http-fixture-ok', mark)
                wait('[portal] output: curl: HTTP 200, 15 bytes',mark)
                type_line('curl ftp://example.com/')
                wait('[portal] output: curl: unsupported URL scheme')
                type_line('echo http-prompt-ok')
                wait('[portal] output: http-prompt-ok')
                call('screendump', {'filename':str(out/'terminal.ppm')})
                click(50)
                wait('curl: enter an http:// or https:// URL')
                mark=log.stat().st_size
                type_line(f'http://10.0.2.2:{port}/redirect')
                wait('curl: HTTP 200, 15 bytes',mark)
                click(650)
                wait('[httptest] PASS: DNS HTTP framing redirects errors streaming cleanup')
                if args.site:
                    # Focus Portal's exposed content before typing another command.
                    call('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':90*32767//1919}}, {'type':'abs','data':{'axis':'y','value':170*32767//1079}}]})
                    for down in (True,False):
                        call('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.2)
                    mark=log.stat().st_size
                    type_line('curl '+args.site)
                    wait('[portal] output: curl: HTTP ',mark)
                assert not errors, errors
                assert any(b'Host: fixture.test:' in request for request in requests), requests
                print('PASS: Portal curl, standalone CURL, Ring 3 DNS/HTTP library and cleanup')
            finally:
                if qmp is not None: qmp.close()
                qemu.terminate()
                try: qemu.wait(timeout=5)
                except subprocess.TimeoutExpired: qemu.kill(); qemu.wait(timeout=5)
    finally:
        stop.set()
        for server in (tcp,dns): server.close()
        for worker in workers: worker.join(timeout=2)

if __name__ == '__main__': main()
