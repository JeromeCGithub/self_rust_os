"""Verify packet-derived network performance measurements independently of QEMU."""
import importlib.util
from pathlib import Path
import socket
import struct
import tempfile
import unittest

PATH=Path(__file__).resolve().parents[2]/'e2e/net_perf.py'
spec=importlib.util.spec_from_file_location('net_perf',PATH)
net_perf=importlib.util.module_from_spec(spec)
spec.loader.exec_module(net_perf)


def tcp(source,destination,sport,dport,flags,window,payload=b''):
    ip=bytearray(20);ip[0]=0x45;ip[2:4]=(40+len(payload)).to_bytes(2,'big');ip[9]=6
    ip[12:16]=socket.inet_aton(source);ip[16:20]=socket.inet_aton(destination)
    header=struct.pack('!HHIIBBHHH',sport,dport,1 if payload else 0,0,5<<4,flags,window,0,0)
    return b'\0'*12+b'\x08\x00'+ip+header+payload


def capture(records):
    raw=struct.pack('<IHHIIII',0xa1b2c3d4,2,4,0,0,65535,1)
    for usec,frame in records:
        raw+=struct.pack('<IIII',1,usec,len(frame),len(frame))+frame
    return raw


class PacketMeasurements(unittest.TestCase):
    def parse(self,raw):
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp)/'wire.pcap';p.write_bytes(raw)
            return net_perf.trace(p)

    def test_window_directions_and_polling_delay(self):
        guest,peer='10.0.2.15','10.0.2.2'
        result=self.parse(capture([
            (0,tcp(guest,peer,49152,80,2,32768)),
            (2000,tcp(peer,guest,80,49152,18,65535)),
            (55000,tcp(guest,peer,49152,80,24,32768,b'GET / HTTP/1.1\r\n\r\n')),
            (65000,tcp(peer,guest,80,49152,24,65535,b'HTTP/1.1 200 OK\r\n\r\nx')),
        ]))
        self.assertEqual(result['guest_max_window'],32768)
        self.assertEqual(result['received_payload_packets'],1)
        self.assertAlmostEqual(result['flows'][0]['synack_to_request_ms'],53,places=3)
        self.assertAlmostEqual(result['flows'][0]['request_to_first_data_ms'],10,places=3)

    def test_late_retransmission_does_not_extend_unique_data_span(self):
        guest,peer='10.0.2.15','10.0.2.2'
        response=tcp(peer,guest,80,49152,24,65535,b'HTTP/1.1 200 OK\r\n\r\nx')
        result=self.parse(capture([
            (0,tcp(guest,peer,49152,80,2,32768)),
            (2000,tcp(peer,guest,80,49152,18,65535)),
            (55000,tcp(guest,peer,49152,80,24,32768,b'GET / HTTP/1.1\r\n\r\n')),
            (65000,response),(800000,response),
        ]))
        self.assertEqual(result['flows'][0]['unique_data_span_ms'],0)
        self.assertEqual(result['flows'][0]['retransmitted_wire_bytes'],len(b'HTTP/1.1 200 OK\r\n\r\nx'))

    def test_truncated_record_is_a_clear_capture_failure(self):
        with self.assertRaises(AssertionError):
            self.parse(capture([])+b'\0'*8)

    def test_invalid_global_header_is_rejected(self):
        with self.assertRaises(AssertionError):self.parse(b'invalid')
