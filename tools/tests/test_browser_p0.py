"""The guest probe must never accept diagnostics from an earlier VM."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'browser_p0', Path(__file__).resolve().parents[2] / 'e2e/browser_p0.py')
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


class ProbeEvidence(unittest.TestCase):
    def test_every_vm_gets_a_fresh_serial_log(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            previous = directory / 'serial.log'
            previous.write_text('[qjsprobe] PASS')
            first = PROBE.reserve_log(directory)
            first.write_text('panic: previous execution')
            second = PROBE.reserve_log(directory)
            self.assertNotEqual(first, second)
            self.assertEqual(second.read_text(), '')
            self.assertEqual(previous.read_text(), '[qjsprobe] PASS')
