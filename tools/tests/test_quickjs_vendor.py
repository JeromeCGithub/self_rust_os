import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'qjs_vendor', Path(__file__).resolve().parents[1] / 'check_quickjs_vendor.py')
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class VendorIntegrity(unittest.TestCase):
    def test_pinned_source_passes_and_tampering_or_missing_source_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'engine.c'
            source.write_bytes(b'original')
            (root / 'VENDOR.json').write_text(json.dumps({
                'files': {'engine.c': hashlib.sha256(b'original').hexdigest()},
            }))
            CHECK.verify(root)
            source.write_bytes(b'tampered')
            with self.assertRaises(ValueError):
                CHECK.verify(root)
            source.unlink()
            with self.assertRaises(ValueError):
                CHECK.verify(root)

    def test_manifest_cannot_escape_the_vendor_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary)
            root = parent / 'vendor'
            root.mkdir()
            outside = parent / 'outside'
            outside.write_bytes(b'otherwise valid source')
            digest = hashlib.sha256(outside.read_bytes()).hexdigest()
            (root / 'VENDOR.json').write_text(json.dumps({
                'files': {'../outside': digest},
            }))
            with self.assertRaises(ValueError):
                CHECK.verify(root)
