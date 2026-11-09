import importlib.util
from pathlib import Path
import unittest
SPEC=importlib.util.spec_from_file_location('init_rollback',Path(__file__).resolve().parents[1]/'init_rollback.py')
MODULE=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(MODULE)
ADMIT='[initator] launching name=ember unit=2 run=2 tick=2\n'
START='[initator] started name=ember unit=2 run=2 pid=3 tick=3\n'
FAULT='[initator] completion pid=2 cause=Fault status=6 tick=3\n'
STOP='[initator] stopping name=ember unit=2 pid=3 tick=3 reason=None\n'
KILL='[initator] completion pid=3 cause=Killed status=9 tick=4\n'
FAILED='[initator] activation outcome=Failed\n'
CANCEL='[initator] spawn failed name=ember unit=2 run=2 tick=3\n'
class RollbackEvidence(unittest.TestCase):
    def test_synchronous_or_late_publication_requires_exact_reaped_child(self):
        for trace in (ADMIT+START+FAULT+STOP+KILL+FAILED,ADMIT+FAULT+START+STOP+KILL+FAILED):
            self.assertTrue(MODULE.verify_early_exit(trace))
    def test_no_admission_or_cancelled_reservation_needs_no_child(self):
        for trace in (FAULT+FAILED,ADMIT+FAULT+CANCEL+FAILED):
            self.assertTrue(MODULE.verify_early_exit(trace))
    def test_observed_fault_prevents_new_admission(self):
        with self.assertRaises(ValueError):MODULE.verify_early_exit(FAULT+ADMIT+START+STOP+KILL+FAILED)
    def test_wrong_child_missing_stop_or_premature_failure_is_rejected(self):
        for trace in (ADMIT+FAULT+START+STOP+KILL.replace('pid=3','pid=9')+FAILED,ADMIT+FAULT+START+KILL+FAILED,ADMIT+FAULT+START+STOP+FAILED+KILL,ADMIT+FAULT+FAILED):
            with self.assertRaises(ValueError):MODULE.verify_early_exit(trace)
    def test_cancelled_dependent_cannot_become_running_or_publish_twice(self):
        for trace in (ADMIT+FAULT+START+'[initator] state name=ember unit=2 state=Running(ProcessKey { pid: 3, generation: 1 })\n'+STOP+KILL+FAILED,ADMIT+FAULT+START+START+STOP+KILL+FAILED,ADMIT+FAULT+CANCEL.replace('run=2','run=3')+FAILED):
            with self.assertRaises(ValueError):MODULE.verify_early_exit(trace)
