#!/usr/bin/env python3
"""Real Ring 3 ping over QEMU Ethernet. Build the GRUB image before running.

Every run uses an independent disposable disk and preserves serial/pcap evidence.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import sys
import threading

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT / 'tools'))
from qemu_network import arguments as network_arguments
from qemu_platform import arguments as platform_arguments


def checksum(data):
    data += b'\0' * (len(data) % 2)
    value = sum(struct.unpack(f'!{len(data)//2}H', data))
    while value >> 16:
        value = (value & 65535) + (value >> 16)
    return (~value) & 65535


def packets(path):
    raw = path.read_bytes()
    assert raw[:4] in (b'\xd4\xc3\xb2\xa1', b'\xa1\xb2\xc3\xd4'), 'pcap magic'
    endian = '<' if raw[0] == 0xd4 else '>'
    offset = 24
    while offset < len(raw):
        assert len(raw) - offset >= 16, 'truncated pcap record'
        _, _, length, original = struct.unpack_from(endian + 'IIII', raw, offset)
        offset += 16
        assert length == original and offset + length <= len(raw), 'truncated capture'
        yield raw[offset:offset+length]
        offset += length


def verify_echo_capture(path, address, minimum):
    destination = socket.inet_aton(address)
    requests, replies = {}, {}
    for frame in packets(path):
        if len(frame) < 34 or frame[12:14] != b'\x08\x00':
            continue
        ip = frame[14:]
        ihl, total = (ip[0] & 15) * 4, int.from_bytes(ip[2:4], 'big')
        if ip[9] != 1:
            continue
        assert 20 <= ihl <= total <= len(ip), 'IPv4 lengths'
        assert checksum(ip[:ihl]) == 0, 'IPv4 checksum'
        icmp = ip[ihl:total]
        assert len(icmp) >= 8 and checksum(icmp) == 0, 'ICMP checksum'
        key = icmp[4:8]
        if icmp[0] == 8 and ip[16:20] == destination:
            requests[key] = icmp[8:]
        if icmp[0] == 0 and ip[12:16] == destination:
            replies[key] = icmp[8:]
    matched = sum(requests.get(key) == payload for key, payload in replies.items())
    assert matched >= minimum, f'only {matched} independently verified replies'
    return matched


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--address', default='10.0.2.2')
    parser.add_argument('--no-nic', action='store_true')
    parser.add_argument('--count', type=int, default=4)
    parser.add_argument('--repeat', type=int, default=1)
    parser.add_argument('--minimum-replies', type=int, default=1)
    parser.add_argument('--probe', action='store_true', help='run Ring 3 boundary/lifecycle, UDP and TCP/HTTP probe')
    args = parser.parse_args()
    output = ROOT / 'target' / ('network-probe' if args.probe else 'network-no-nic' if args.no_nic else f'network-{args.address}')
    output.mkdir(parents=True, exist_ok=True)
    log, capture = output / 'serial.log', output / 'ethernet.pcap'
    with tempfile.TemporaryDirectory(prefix='rustos-network-') as tmp:
        tmp = Path(tmp)
        disk = tmp / 'disk.img'
        shutil.copyfile(ROOT / 'target/grub-disk.img', disk)
        servers = []
        errors = []
        if args.probe:
            udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            for server in (udp, tcp):
                server.bind(('127.0.0.1', 0))
                server.settimeout(25)
                servers.append(server)
            tcp.listen(1)

            def udp_peer():
                try:
                    for _ in range(2):
                        data, peer = udp.recvfrom(2048)
                        udp.sendto(data, peer)
                except Exception as error:
                    errors.append(error)

            def http_peer():
                try:
                    connection, _ = tcp.accept()
                    with connection:
                        connection.settimeout(10)
                        data = b''
                        while b'\r\n\r\n' not in data:
                            data += connection.recv(2048)
                            assert len(data) <= 4096, 'HTTP request too large'
                        assert data.startswith(b'GET / HTTP/1.0\r\n'), data
                        connection.sendall(b'HTTP/1.0 200 OK\r\nContent-Length: 14\r\n\r\nrustos-')
                        time.sleep(.1)
                        connection.sendall(b'http-ok')
                except Exception as error:
                    errors.append(error)

            workers = [threading.Thread(target=udp_peer, daemon=True), threading.Thread(target=http_peer, daemon=True)]
            for worker in workers:
                worker.start()
            config = tmp / 'net.cfg'
            config.write_bytes(struct.pack('<HH', udp.getsockname()[1], tcp.getsockname()[1]))
            subprocess.run(['mcopy', '-i', f'{disk}@@1048576', str(config), '::/TMP/NET.CFG'], check=True)
        qmp_path = str(tmp / 'qmp')
        command = ['qemu-system-x86_64', '-machine', 'pc,i8042=on,vmport=on',
                   '-drive', f'format=raw,file={disk}', '-vga', 'std',
                   '-display', 'none', '-serial', f'file:{log}', '-no-reboot',
                   '-qmp', f'unix:{qmp_path},server=on,wait=off']
        command += platform_arguments()
        if args.no_nic:
            command += network_arguments('none')
        else:
            command += network_arguments()
            command += ['-object', f'filter-dump,id=capture,netdev=net0,file={capture}']
        qemu = subprocess.Popen(command, stderr=subprocess.PIPE)
        qmp = None
        try:
            end = time.monotonic() + 30
            while not Path(qmp_path).exists():
                assert qemu.poll() is None, qemu.stderr.read().decode()
                assert time.monotonic() < end, 'QMP socket timeout'
                time.sleep(.05)
            qmp = socket.socket(socket.AF_UNIX)
            qmp.settimeout(5)
            qmp.connect(qmp_path)
            stream = qmp.makefile('rwb')
            json.loads(stream.readline())

            def request(name, arguments=None):
                item = {'execute': name}
                if arguments is not None:
                    item['arguments'] = arguments
                stream.write(json.dumps(item).encode() + b'\n')
                stream.flush()
                while True:
                    result = json.loads(stream.readline())
                    assert 'error' not in result, result
                    if 'return' in result:
                        return result['return']

            def text(after=0):
                raw = log.read_text(errors='replace') if log.exists() else ''
                raw = raw[after:]
                return raw + '\n' + ''.join(re.findall(r'\[kernel\] sys_write: "(.*?)"\n', raw, re.S))

            def wait_for(expected, after=0, timeout=30):
                deadline = time.monotonic() + timeout
                while expected not in text(after):
                    assert qemu.poll() is None, 'QEMU exited unexpectedly'
                    assert time.monotonic() < deadline, f'missing {expected}\n{text()[-3000:]}'
                    if '[portal] output: unknown command\n' in text(after):
                        raise AssertionError('ping command is not implemented (unknown command)')
                    time.sleep(.1)

            def shell(command):
                for char in command + '\n':
                    key = {' ': 'spc', '.': 'dot', '-': 'minus', '\n': 'ret'}.get(char, char)
                    request('human-monitor-command', {'command-line': 'sendkey ' + key})
                    time.sleep(.075)

            request('qmp_capabilities')
            wait_for('[desktop] ready:')
            request('input-send-event', {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': (350*32767+1918)//1919}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': (65*32767+1078)//1079}}]})
            time.sleep(.2)
            for down in (True, False):
                request('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': down}}]})
                time.sleep(.2)
            wait_for('Portal / RustOS shell')
            if args.probe:
                # Build-user installs binaries alphabetically; CURL is first and
                # ZNETTEST is the seventh launcher.
                request('input-send-event', {'events': [
                    {'type': 'abs', 'data': {'axis': 'x', 'value': (650*32767+1918)//1919}},
                    {'type': 'abs', 'data': {'axis': 'y', 'value': (65*32767+1078)//1079}}]})
                time.sleep(.2)
                for down in (True, False):
                    request('input-send-event', {'events': [{'type': 'btn', 'data': {'button': 'left', 'down': down}}]})
                    time.sleep(.2)
                wait_for('[nettest] PASS: boundary lifecycle UDP TCP HTTP', timeout=35)
                assert not errors, errors
                print('PASS: actual Ring 3 user-buffer validation, ownership, cleanup, UDP and TCP HTTP request')
                return
            for _ in range(args.repeat):
                mark = len(log.read_text(errors='replace'))
                shell(f'ping -c {args.count} {args.address}')
                if args.no_nic:
                    wait_for('[portal] output: ping: network unavailable', mark)
                else:
                    wait_for(f'{args.count} transmitted,', mark)
                    replies = re.findall(r'\[portal\] output: reply from ' + re.escape(args.address) + r' seq=(\d+)', text(mark))
                    assert len(set(replies)) >= args.minimum_replies, f'no validated application replies: {text()[-3000:]}'
                    if args.minimum_replies == 0:
                        wait_for(f'{args.count} transmitted, 0 received, 100% loss', mark)
                        wait_for('seq=1 timeout', mark)
            shell('echo network-prompt-ok')
            wait_for('[portal] output: network-prompt-ok', mark)
            request('screendump', {'filename': str(output / 'terminal.ppm')})
        finally:
            if qmp is not None:
                qmp.close()
            qemu.terminate()
            try:
                qemu.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                qemu.kill()
                qemu.communicate(timeout=5)
            for server in servers:
                server.close()
    if not args.no_nic:
        matched = verify_echo_capture(capture, args.address, args.minimum_replies * args.repeat)
        print(f'PASS: Ring 3 ping {args.address}; {matched} packet-verified replies; prompt recovered')
    else:
        print('PASS: missing NIC reported; prompt recovered')
    print(f'Evidence: {output}')


if __name__ == '__main__':
    main()
