#!/usr/bin/env python3
"""Independent OpenSSL HTTPS fixtures and actual guest certificate rejection."""
import argparse,json,re,shutil,socket,ssl,struct,subprocess,sys,tempfile,threading,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True
sys.path.insert(0,str(ROOT/'tools'))
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments
from tls_fixtures import create


def main():
 parser=argparse.ArgumentParser();parser.add_argument('--site');parser.add_argument('--no-rng',action='store_true');parser.add_argument('--bad-clock',action='store_true');args=parser.parse_args()
 label='no-rng' if args.no_rng else 'bad-clock' if args.bad_clock else 'verified'
 out=ROOT/'target'/('tls-'+label);out.mkdir(parents=True,exist_ok=True)
 stop=threading.Event();servers=[];workers=[];errors=[];received=[];active=set();active_lock=threading.Lock()
 with tempfile.TemporaryDirectory(prefix='rustos-tls-') as tmp:
  tmp=Path(tmp);certs=create(tmp/'certs')
  try:
   for kind in ['valid','expired','future','rsa']:
    server=socket.socket();server.bind(('127.0.0.1',0));server.listen(8);server.settimeout(.2);servers.append(server)
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.minimum_version=ssl.TLSVersion.TLSv1_3;context.maximum_version=ssl.TLSVersion.TLSv1_3;context.load_cert_chain(certs/(kind+'.pem'),certs/(kind+'.key'))
    def serve(server=server,context=context,kind=kind):
     while not stop.is_set():
      try:connection,_=server.accept()
      except socket.timeout:continue
      except OSError:break
      wrapped=None
      try:
       connection.settimeout(25)
       wrapped=context.wrap_socket(connection,server_side=True,do_handshake_on_connect=False)
       with active_lock:active.add(wrapped)
       with wrapped as stream:
        stream.do_handshake()
        request=b''
        while b'\r\n\r\n' not in request:
         chunk=stream.recv(2048)
         if not chunk:break
         request+=chunk;assert len(request)<32768
        if not request:continue
        received.append((kind,request));assert kind in ('valid','rsa'), 'rejected certificate received application data'
        path=request.split(b' ')[1]
        if path==b'/large':data=b'HTTP/1.1 200 OK\r\nContent-Length: 65536\r\n\r\n'+b'x'*65536
        elif path==b'/expanded':data=b'HTTP/1.1 200 OK\r\nContent-Length: 17825792\r\n\r\n'+b'x'*17825792
        elif path==b'/headers':data=b'HTTP/1.1 200 OK\r\nContent-Security-Policy: '+b'a'*20000+b'\r\n'+(b'X-Padding: '+b'b'*800+b'\r\n')*200+b'Content-Length: 14\r\n\r\ntls-fixture-ok'
        elif path==b'/overline':data=b'HTTP/1.1 200 OK\r\nX-Large: '+b'a'*33000+b'\r\nContent-Length: 0\r\n\r\n'
        elif path==b'/overheaders':data=b'HTTP/1.1 200 OK\r\n'+(b'X-Large: '+b'a'*30000+b'\r\n')*9+b'\r\n'
        elif path==b'/overbody':data=b'HTTP/1.1 200 OK\r\nContent-Length: 268435457\r\n\r\n'
        elif path==b'/long-redirect':data=b'HTTP/1.1 302 Found\r\nLocation: /'+b'a'*8192+b'\r\nContent-Length: 0\r\n\r\n'
        elif path==b'/downgrade':data=b'HTTP/1.1 302 Found\r\nLocation: http://10.0.2.2/plain\r\nContent-Length: 0\r\n\r\n'
        elif path==b'/chunked':data=b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;x='+b'a'*4000+b'\r\ntls-\r\na\r\nfixture-ok\r\n0\r\nX-Trailer: '+b'b'*20000+b'\r\n\r\n'
        else:data=b'HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\ntls-fixture-ok'
        for at in range(0,len(data),777):stream.sendall(data[at:at+777])
      except (ssl.SSLError,ConnectionResetError,BrokenPipeError):pass
      except Exception as error:
       if not stop.is_set():errors.append(repr(error))
      finally:
       with active_lock:active.discard(wrapped)
       connection.close()
    worker=threading.Thread(target=serve,daemon=True);worker.start();workers.append(worker)
   dns=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);dns.bind(('127.0.0.1',0));dns.settimeout(.2);servers.append(dns)
   def resolve():
    while not stop.is_set():
     try:raw,peer=dns.recvfrom(512)
     except socket.timeout:continue
     except OSError:break
     dns.sendto(raw[:2]+b'\x81\x80\0\x01\0\x01\0\0\0\0'+raw[12:]+b'\xc0\x0c\0\x01\0\x01\0\0\0\x1e\0\x04\x0a\0\x02\x02',peer)
   worker=threading.Thread(target=resolve,daemon=True);worker.start();workers.append(worker)
   disk=tmp/'disk.img';shutil.copyfile(ROOT/'target/grub-disk.img',disk)
   config=tmp/'tls.cfg';config.write_bytes(struct.pack('<6H',*[s.getsockname()[1] for s in servers],1))
   mode=tmp/'tls.mode';mode.write_bytes(b'R' if args.no_rng else b'C' if args.bad_clock else b'V')
   for source,destination in [(config,'::/TMP/TLS.CFG'),(mode,'::/TMP/TLS.MOD'),(certs/'ca.der','::/ETC/TEST.CA')]:subprocess.run(['mcopy','-i',f'{disk}@@1048576',str(source),destination],check=True)
   platform=platform_arguments(not args.no_rng)
   if args.bad_clock:platform[-1]='base=1970-01-01T00:00:00,clock=vm'
   log=out/'serial.log';path=tmp/'qmp'
   qemu=subprocess.Popen(['qemu-system-x86_64','-machine','pc,i8042=on,vmport=on','-drive',f'format=raw,file={disk}','-vga','std','-display','none','-serial',f'file:{log}','-no-reboot','-qmp',f'unix:{path},server=on,wait=off']+platform+network_arguments())
   qmp=None
   try:
    end=time.monotonic()+30
    while not path.exists():assert time.monotonic()<end and qemu.poll() is None;time.sleep(.1)
    qmp=socket.socket(socket.AF_UNIX);qmp.settimeout(5);qmp.connect(str(path));stream=qmp.makefile('rwb');json.loads(stream.readline())
    def call(command,arguments=None):
     item={'execute':command}
     if arguments is not None:item['arguments']=arguments
     stream.write(json.dumps(item).encode()+b'\n');stream.flush()
     while True:
      result=json.loads(stream.readline());assert 'error' not in result,result
      if 'return' in result:return result['return']
    def text():
     raw=log.read_text(errors='replace') if log.exists() else ''
     return raw+'\n'+''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',raw,re.S))
    def wait(value,mark=0):
     end=time.monotonic()+90
     while value not in text()[mark:]:
      assert 'Kernel panic' not in text() and 'panic:' not in text(),text()[-5000:]
      assert time.monotonic()<end and qemu.poll() is None,f'missing {value}\n{text()[-5000:]}';time.sleep(.1)
    def click(x):
     call('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':x*32767//1919}},{'type':'abs','data':{'axis':'y','value':65*32767//1079}}]});time.sleep(.2)
     for down in (True,False):call('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.2)
    def type_line(line):
     for char in line+'\n':
      key={' ':'spc',':':'shift-semicolon','/':'slash','.':'dot','-':'minus','\n':'ret'}.get(char,char)
      call('human-monitor-command',{'command-line':'sendkey '+key});time.sleep(.075)
    call('qmp_capabilities');wait('[desktop] ready: 7 applications');click(350);wait('Portal / RustOS shell')
    type_line(f'curl https://10.0.2.2:{servers[0].getsockname()[1]}/')
    error='secure randomness unavailable' if args.no_rng else 'UTC clock unavailable' if args.bad_clock else 'TLS certificate rejected'
    wait('[portal] output: curl: '+error)
    type_line('echo tls-prompt-ok');wait('[portal] output: tls-prompt-ok')
    click(650)
    expected='unavailable entropy refused' if args.no_rng else 'invalid UTC refused' if args.bad_clock else 'verified HTTPS RSA hostname dates trust redirects streaming cleanup'
    wait('[tlstest] PASS: '+expected)
    if args.site:
     call('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':90*32767//1919}},{'type':'abs','data':{'axis':'y','value':170*32767//1079}}]})
     for down in (True,False):call('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.2)
     mark=len(text());type_line('curl '+args.site);wait('[portal] output: curl: HTTP ',mark)
    # Serial output precedes Portal's next surface publication/compositor frame.
    # Give the visual evidence time to show the already asserted response.
    time.sleep(.5)
    call('screendump',{'filename':str(out/'terminal.ppm')})
    assert not errors,errors
    if not args.no_rng and not args.bad_clock:assert len(received)>=10,received
    else:assert not received,'missing entropy/time must never send HTTP plaintext'
    print('PASS: '+expected,flush=True)
   finally:
    if qmp is not None:qmp.close()
    qemu.terminate()
    try:qemu.wait(timeout=5)
    except subprocess.TimeoutExpired:qemu.kill();qemu.wait(timeout=5)
  finally:
   stop.set()
   for server in servers:server.close()
   with active_lock:connections=list(active)
   for connection in connections:
    try:connection.shutdown(socket.SHUT_RDWR)
    except OSError:pass
    connection.close()
   for worker in workers:worker.join(timeout=2)

if __name__=='__main__':main()
