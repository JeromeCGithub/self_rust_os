import struct
import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from audio_capture import finalize
def header(size=0,data=0):
    return struct.pack('<4sI4s4sIHHIIHH4sI',b'RIFF',size,b'WAVE',b'fmt ',16,1,2,48000,192000,4,16,b'data',data)
class AudioCapture(unittest.TestCase):
    def test_qemu_zero_lengths_are_completed_without_changing_one_pcm_byte(self):
        payload=struct.pack('<hhhh',6000,6000,-6000,-6000)
        result=finalize(header()+payload)
        self.assertEqual(result,header(36+len(payload),len(payload))+payload)
        self.assertEqual(finalize(result),result)
    def test_unknown_formats_bad_lengths_and_incomplete_frames_are_rejected(self):
        for raw in [b'',header()[:-1],header()+b'x',header(99,4)+b'1234',header(40,8)+b'1234',b'NOPE'+header()[4:]]:
            with self.assertRaises(ValueError):finalize(raw)
