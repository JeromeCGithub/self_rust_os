import sys
from pathlib import Path
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from browser_css import assert_rectangles, dark_bounds


class PixelFixture:
    """Small RGB test surface; ordinary tooling CI needs no image dependency."""
    def __init__(self, width, height, color=(255, 255, 255)):
        self.width, self.height = width, height
        self.rows = [[color] * width for _ in range(height)]

    def convert(self, mode):
        assert mode == 'RGB'
        return self

    def getpixel(self, point):
        x, y = point
        return self.rows[y][x]

    def paint(self, box, color):
        left, top, right, bottom = box
        for y in range(top, bottom + 1):
            self.rows[y][left:right + 1] = [color] * (right - left + 1)


class NativeCSSReadback(unittest.TestCase):
    def test_exact_rectangles_reject_shifted_paint_even_with_the_right_color(self):
        image = PixelFixture(100, 80)
        image.paint((10, 15, 39, 34), (0x12, 0x34, 0x56))
        expected = [dict(color=0x123456, box=[10, 15, 30, 20])]
        assert_rectangles(image, expected)
        with self.assertRaises(AssertionError):
            assert_rectangles(image, [dict(color=0x123456, box=[11, 15, 30, 20])])
        with self.assertRaises(AssertionError):
            assert_rectangles(PixelFixture(100, 80), expected)

    def test_native_decorations_exclude_only_the_declared_side_rims(self):
        image = PixelFixture(100, 80)
        image.paint((90, 15, 99, 34), (0x12, 0x34, 0x56))
        image.paint((99, 0, 99, 79), (128, 128, 128))
        assert_rectangles(image, [dict(color=0x123456, box=[90, 15, 10, 20])], inset=1)
        with self.assertRaises(AssertionError):
            assert_rectangles(image, [dict(color=0x123456, box=[89, 15, 11, 20])], inset=1)

    def test_control_ink_bounds_distinguish_missing_or_small_glyphs(self):
        image = PixelFixture(100, 40, (247, 251, 255))
        self.assertIsNone(dark_bounds(image))
        image.paint((8, 9, 65, 22), (0, 0, 0))
        self.assertEqual(dark_bounds(image), [8, 9, 58, 14])
