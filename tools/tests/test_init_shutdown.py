"""Serial record boundaries must not turn an observation race into a halt failure."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from init_shutdown import halt_record


class HaltRecordTests(unittest.TestCase):
    def test_partial_record_is_pending_at_every_byte_boundary(self):
        for line in (
            '[system-halt] action=Halt parked=true dma_disabled=true retained=true',
            '[system-halt] action=Halt parked=true dma_disabled=false retained=true',
        ):
            for boundary in range(len(line) + 1):
                with self.subTest(boundary=boundary, line=line):
                    self.assertIsNone(halt_record('boot complete\n' + line[:boundary]))
            self.assertIsNone(halt_record(line + '\r'))

    def test_complete_success_is_returned_with_lf_and_crlf(self):
        line = '[system-halt] action=Halt parked=true dma_disabled=true retained=true'
        for ending in ('\n', '\r\n'):
            self.assertEqual(halt_record('boot\n' + line + ending + 'tail'), line)

    def test_complete_failure_is_not_hidden_or_replaced_by_later_success(self):
        failed = '[system-halt] action=Halt parked=true dma_disabled=false'
        success = '[system-halt] action=Halt parked=true dma_disabled=true'
        self.assertEqual(halt_record(failed + '\n' + success + '\n'), failed)

    def test_nonterminal_output_and_quoted_marker_are_ignored(self):
        self.assertIsNone(halt_record(''))
        self.assertIsNone(halt_record('ordinary log\n'))
        self.assertIsNone(halt_record('[kernel] sys_write: "[system-halt] partial"\n'))


if __name__ == '__main__':
    unittest.main()
