import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from browser_probe import rgb_digest
class FrameReadback(unittest.TestCase):
    def test_same_pixel_region_has_an_independent_fixed_digest(self):
        self.assertEqual(rgb_digest([(0x11,0x22,0x33),(0x44,0x55,0x66)]*2),0xd04d521e87241a11)
        self.assertNotEqual(rgb_digest([(0,0,0)]*4),rgb_digest([(0,0,0)]*3+[(0,0,1)]))
