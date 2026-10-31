#!/usr/bin/env python3
"""Measure guest HTTP throughput and TCP wire behavior with independent fixtures."""
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

ROOT=Path(__file__).resolve().parents[1]
sys.dont_write_bytecode=True
sys.path.insert(0,str(ROOT/'tools'))
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments
from qemu_test import disk_image


def trace(path):
    """Extract wire windows and phase delays; count retransmissions as wire bytes."""
    data = path.read_bytes()
    assert len(data) >= 24 and data[:4] in (b'\xd4\xc3\xb2\xa1', b'\xa1\xb2\xc3\xd4'), 'invalid pcap header'
    endian = '<' if data[:4] == b'\xd4\xc3\xb2\xa1' else '>'
    offset, rows, flows = 24, [], {}
    while offset < len(data):
        assert len(data) - offset >= 16, 'truncated pcap record header'
        sec, usec, size, original = struct.unpack_from(endian + 'IIII', data, offset)
        offset += 16
        assert size == original and offset + size <= len(data), 'truncated captured frame'
        frame = data[offset:offset+size]
        offset += size
        if len(frame) < 34 or frame[12:14] != b'\x08\x00':
            continue
        ip = frame[14:]
        ihl, total = (ip[0] & 15) * 4, int.from_bytes(ip[2:4], 'big')
        assert 20 <= ihl <= total <= len(ip), 'invalid captured IPv4 length'
        if ip[9] != 6:
            continue
        tcp = ip[ihl:total]
        assert len(tcp) >= 20, 'truncated captured TCP header'
        header = (tcp[12] >> 4) * 4
        assert 20 <= header <= len(tcp), 'invalid captured TCP data offset'
        source, destination = socket.inet_ntoa(ip[12:16]), socket.inet_ntoa(ip[16:20])
        sport, dport = struct.unpack_from('!HH', tcp)
        row = {'time': sec + usec/1000000, 'source': source,
               'flags': tcp[13], 'window': int.from_bytes(tcp[14:16], 'big'),
               'payload': len(tcp)-header}
        rows.append(row)
        outgoing = source == '10.0.2.15'
        if not outgoing and destination != '10.0.2.15':
            continue
        key = (sport, destination, dport) if outgoing else (dport, source, sport)
        flow = flows.setdefault(key, {})
        if not outgoing and tcp[13] & 18 == 18:
            flow.setdefault('synack', row['time'])
            flow.setdefault('base_sequence', (int.from_bytes(tcp[4:8], 'big')+1) & 0xffffffff)
        if outgoing and tcp[header:].startswith(b'GET '):
            flow.setdefault('request', row['time'])
        if not outgoing and row['payload']:
            seq = int.from_bytes(tcp[4:8], 'big')
            base = flow.setdefault('base_sequence', seq)
            left, right = (seq-base) & 0xffffffff, ((seq-base) & 0xffffffff)+row['payload']
            intervals = flow.setdefault('intervals', [])
            overlap = sum(max(0, min(right, b)-max(left, a)) for a, b in intervals)
            flow['retransmitted'] = flow.get('retransmitted', 0)+overlap
            if overlap < row['payload']:
                flow.setdefault('first', row['time'])
                flow['last_unique'] = row['time']
            merged = []
            for a, b in sorted(intervals+[(left, right)]):
                if merged and a <= merged[-1][1]:
                    merged[-1] = (merged[-1][0], max(b, merged[-1][1]))
                else:
                    merged.append((a, b))
            flow['intervals'] = merged
    measured = []
    for (local, peer, remote), flow in flows.items():
        if 'request' not in flow:
            continue
        item = {'local_port': local, 'peer': peer, 'remote_port': remote}
        for label, start, end in [('synack_to_request_ms', 'synack', 'request'),
                                  ('request_to_first_data_ms', 'request', 'first'),
                                  ('unique_data_span_ms', 'first', 'last_unique')]:
            if start in flow and end in flow:
                item[label] = round((flow[end]-flow[start])*1000, 3)
        item['retransmitted_wire_bytes'] = flow.get('retransmitted', 0)
        measured.append(item)
    windows = [r['window'] for r in rows if r['source'] == '10.0.2.15' and r['window']]
    received = [r for r in rows if r['source'] != '10.0.2.15' and r['payload']]
    return {'guest_max_window': max(windows, default=0), 'flows': measured,
            'received_payload_packets': len(received),
            'received_wire_bytes': sum(r['payload'] for r in received)}


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--label',default='current')
    parser.add_argument('--assert-fast',action='store_true',help='require 128 KiB in at most 25 PIT ticks')
    parser.add_argument('--site',help='also measure Portal curl against a public HTTP URL')
    args=parser.parse_args()
    assert re.fullmatch('[a-zA-Z0-9_-]+',args.label)
    out=ROOT/'target'/('net-perf-'+args.label);out.mkdir(parents=True,exist_ok=True)
    server=socket.socket();server.bind(('127.0.0.1',0));server.listen(2);server.settimeout(.2)
    stop=threading.Event(); errors=[];served=[]
    def peer():
        while not stop.is_set():
            try: connection,_=server.accept()
            except socket.timeout:continue
            except OSError:break
            try:
                with connection:
                    connection.settimeout(35);request=b''
                    while b'\r\n\r\n' not in request:
                        more=connection.recv(2048);assert more;request+=more;assert len(request)<=4096
                    assert request.startswith(b'GET /bulk HTTP/1.1\r\n')
                    start=time.monotonic()
                    connection.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 131072\r\nConnection: close\r\n\r\n'+b'x'*131072)
                    served.append(time.monotonic()-start)
            except Exception as error:errors.append(str(error))
    worker=threading.Thread(target=peer,daemon=True);worker.start()
    try:
        with tempfile.TemporaryDirectory(prefix='rustos-net-perf-') as tmp:
            tmp=Path(tmp);disk=tmp/'disk.img';shutil.copyfile(disk_image(ROOT),disk)
            config=tmp/'perf.url';config.write_text(f'http://10.0.2.2:{server.getsockname()[1]}/bulk')
            subprocess.run(['mcopy','-i',f'{disk}@@1048576',str(config),'::/TMP/PERF.URL'],check=True)
            log=out/'serial.log';capture=out/'ethernet.pcap';path=tmp/'qmp'
            qemu=subprocess.Popen(['qemu-system-x86_64','-machine','pc,i8042=on,vmport=on','-drive',f'format=raw,file={disk}','-vga','std','-display','none','-serial',f'file:{log}','-no-reboot','-qmp',f'unix:{path},server=on,wait=off']+platform_arguments()+network_arguments()+['-object',f'filter-dump,id=perf,netdev=net0,file={capture}'])
            qmp=None
            try:
                end=time.monotonic()+30
                while not path.exists():assert time.monotonic()<end and qemu.poll() is None;time.sleep(.1)
                qmp=socket.socket(socket.AF_UNIX);qmp.settimeout(5);qmp.connect(str(path));stream=qmp.makefile('rwb');json.loads(stream.readline())
                def call(name,arguments=None):
                    item={'execute':name}
                    if arguments is not None:item['arguments']=arguments
                    stream.write(json.dumps(item).encode()+b'\n');stream.flush()
                    while True:
                        result=json.loads(stream.readline());assert 'error' not in result,result
                        if 'return' in result:return result['return']
                def text():
                    raw=log.read_text(errors='replace') if log.exists() else ''
                    return raw+'\n'+''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n',raw,re.S))
                def wait(value,timeout=90):
                    end=time.monotonic()+timeout
                    while value not in text():
                        assert 'Kernel panic' not in text() and 'panic:' not in text(),text()[-4000:]
                        assert time.monotonic()<end and qemu.poll() is None,text()[-4000:]
                        time.sleep(.05)
                def click(x):
                    call('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':x*32767//1919}},{'type':'abs','data':{'axis':'y','value':65*32767//1079}}]});time.sleep(.2)
                    for down in (True,False):call('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':down}}]});time.sleep(.2)
                call('qmp_capabilities');wait('[desktop] ready: 7 applications',30);click(650);wait('[netperf] PASS')
                flat=text();rounds=[{'bytes':int(n),'ticks':int(t),'sum':int(c)} for n,t,c in re.findall(r'\[netperf\] round=\d+ bytes=(\d+) ticks=(\d+) sum=(\d+)',flat)]
                assert len(rounds)==2,flat[-3000:]
                if args.site:
                    click(350);wait('self_rust_os forge v1.0',30)
                    command='curl '+args.site
                    for char in command:
                        key={' ':'spc','/':'slash',':':'shift-semicolon','.':'dot','-':'minus'}.get(char,char)
                        call('human-monitor-command',{'command-line':'sendkey '+key});time.sleep(.075)
                    start=time.monotonic();call('human-monitor-command',{'command-line':'sendkey ret'})
                    end=start+60
                    while '[portal] output: curl: ' not in text():assert time.monotonic()<end and qemu.poll() is None,text()[-2000:];time.sleep(.05)
                    public={'url':args.site,'wall_seconds':time.monotonic()-start,'result':text()[text().rfind('[portal] output: curl: '):].splitlines()[0]}
                else:public=None
            finally:
                if qmp is not None:qmp.close()
                qemu.terminate()
                try:qemu.wait(timeout=5)
                except subprocess.TimeoutExpired:qemu.kill();qemu.wait(timeout=5)
        assert not errors,errors
        result={'rounds':rounds,'wire':trace(capture),'public':public}
        (out/'results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2),flush=True)
        if args.assert_fast:assert max(r['ticks'] for r in rounds)<=25,'128 KiB receive throughput below target'
    finally:
        stop.set();server.close();worker.join(timeout=2)

if __name__=='__main__':main()
