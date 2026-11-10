"""Native filesystem validation must include the exact service completion."""
from pathlib import Path
import sys
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from init_file_io import completed_probe, fixture, PROBE_PASS
TRACE = '''[initator] PID 1 entered Ring 3
[initator] started name=beacon unit=0 run=1 pid=2 tick=1
[initator] started name=fileio unit=1 run=2 pid=3 tick=2
''' + PROBE_PASS + '''
[initator] completion pid=3 cause=Exited status=0 tick=25
[glow] beat=3
'''
class NativeFileIOEvidence(unittest.TestCase):
    def test_fixture_supervises_single_finite_probe_with_surviving_sibling(self):
        files=fixture()
        self.assertEqual(set(files),{'A.UNIT','B.UNIT','BOOT.UNIT'})
        self.assertIn('Exec=/BIN/ZFILEIO.BIN\n',files['B.UNIT'])
        self.assertIn('Restart=never\n',files['B.UNIT'])
        self.assertIn('Requires=beacon\n',files['B.UNIT'])
        self.assertIn('Requires=fileio\n',files['BOOT.UNIT'])
    def test_completed_probe_requires_native_init_marker_and_owned_success(self):
        self.assertTrue(completed_probe(TRACE))
        for marker in ('[initator] PID 1 entered Ring 3\n',PROBE_PASS+'\n','[glow] beat=3\n'):
            self.assertFalse(completed_probe(TRACE.replace(marker,'')))
        self.assertFalse(completed_probe(TRACE.replace('completion pid=3','completion pid=9')))
    def test_marker_or_partial_terminal_record_is_not_success(self):
        self.assertFalse(completed_probe(PROBE_PASS+'\n'))
        prefix=TRACE.split('[initator] completion')[0]+'[glow] beat=3\n'
        terminal='[initator] completion pid=3 cause=Exited status=0 tick=25'
        for boundary in range(len(terminal)+1):
            self.assertFalse(completed_probe(prefix+terminal[:boundary]))
    def test_wrong_termination_cause_and_status_are_failures(self):
        for terminal in ('cause=Exited status=1','cause=Killed status=9','cause=Fault status=6'):
            with self.assertRaises(ValueError):completed_probe(TRACE.replace('cause=Exited status=0',terminal))
    def test_restart_duplicate_or_reused_service_identity_is_rejected(self):
        for extra in ('[initator] started name=fileio unit=1 run=3 pid=4 tick=30\n',
                      '[initator] started name=fileio unit=1 run=2 pid=3 tick=2\n'):
            with self.assertRaises(ValueError):completed_probe(TRACE+extra)
        with self.assertRaises(ValueError):completed_probe(TRACE.replace('name=fileio unit=1 run=2 pid=3','name=fileio unit=1 run=2 pid=2').replace('completion pid=3','completion pid=2'))
    def test_sibling_completion_is_not_a_surviving_service(self):
        with self.assertRaises(ValueError):completed_probe(TRACE+'[initator] completion pid=2 cause=Exited status=0 tick=26\n')
if __name__=='__main__':unittest.main()
