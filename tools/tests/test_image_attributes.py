"""Keep the raw P6 wallpaper bytes out of Git text normalization and diffing."""
from pathlib import Path
import subprocess
import unittest
ROOT = Path(__file__).resolve().parents[2]


class BinaryWallpaper(unittest.TestCase):
    def test_native_desktop_wallpaper_is_a_binary_git_asset(self):
        asset = 'user_programs/de/wallpaper.ppm'
        self.assertTrue((ROOT / asset).read_bytes().startswith(b'P6\n'))
        value = subprocess.check_output(
            ['git', 'check-attr', 'binary', '--', asset], cwd=ROOT, text=True)
        self.assertEqual(value.strip().rsplit(': ', 1)[-1], 'set')
