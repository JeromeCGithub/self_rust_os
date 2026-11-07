import sys
from pathlib import Path
import unittest
from PIL import Image, ImageDraw
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from browser_css import assert_rectangles

class NativeCSSReadback(unittest.TestCase):
    def test_exact_rectangles_reject_shifted_paint_even_with_the_right_color(self):
        image=Image.new('RGB',(100,80),'white')
        ImageDraw.Draw(image).rectangle((10,15,39,34),fill='#123456')
        expected=[dict(color=0x123456,box=[10,15,30,20])]
        assert_rectangles(image,expected)
        with self.assertRaises(AssertionError):
            assert_rectangles(image,[dict(color=0x123456,box=[11,15,30,20])])
        with self.assertRaises(AssertionError):
            assert_rectangles(Image.new('RGB',(100,80),'white'),expected)

    def test_native_decorations_exclude_only_the_declared_side_rims(self):
        image=Image.new('RGB',(100,80),'white')
        ImageDraw.Draw(image).rectangle((90,15,99,34),fill='#123456')
        ImageDraw.Draw(image).line((99,0,99,79),fill='gray')
        assert_rectangles(image,[dict(color=0x123456,box=[90,15,10,20])],inset=1)
        with self.assertRaises(AssertionError):
            assert_rectangles(image,[dict(color=0x123456,box=[89,15,11,20])],inset=1)

    def test_control_ink_bounds_distinguish_missing_or_small_glyphs(self):
        from browser_css import dark_bounds
        image=Image.new('RGB',(100,40),'#f7fbff')
        self.assertIsNone(dark_bounds(image))
        ImageDraw.Draw(image).rectangle((8,9,65,22),fill='black')
        self.assertEqual(dark_bounds(image),[8,9,58,14])
