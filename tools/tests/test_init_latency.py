import importlib.util
from pathlib import Path
import unittest
SPEC = importlib.util.spec_from_file_location('init_latency', Path(__file__).resolve().parents[1]/'init_latency.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TRACE = '''[initator] ready name=wick unit=5 run=4 pid=5 tick=10
[initator] launching name=bulk unit=1 run=5 tick=10
[initator] started name=bulk unit=1 run=5 pid=6 tick=14
[initator] stopping name=wick unit=5 pid=5 tick=16 reason=Some(Watchdog)
[initator] completion pid=5 cause=Killed status=9 tick=16
'''
class LatencyEvidence(unittest.TestCase):
    def test_complete_typed_evidence_measures_loader_and_deadline_delay(self):
        result=MODULE.metrics(TRACE,.34)
        self.assertEqual(result['delay_ticks'],0)
        self.assertEqual(result['load_ticks'],4)
    def test_incomplete_wrong_identity_early_or_late_stop_fails(self):
        for text in [TRACE.split('[initator] completion')[0],TRACE.replace('completion pid=5','completion pid=9'),TRACE.replace('tick=16 reason','tick=15 reason'),TRACE.replace('tick=16 reason','tick=19 reason')]:
            with self.subTest(text=text):
                with self.assertRaises(ValueError): MODULE.metrics(text,.34)
    def test_wall_time_and_bad_clock_values_cannot_hide_stopped_guest_ticks(self):
        for elapsed in (.51,float('inf'),float('nan'),-1):
            with self.assertRaises(ValueError): MODULE.metrics(TRACE,elapsed)
    def test_async_loader_may_publish_after_watchdog_is_serviced(self):
        result=MODULE.metrics(TRACE.replace('pid=6 tick=14','pid=6 tick=30'),.34)
        self.assertEqual(result['delay_ticks'],0)

class StoragePressure(unittest.TestCase):
    def test_throttle_is_explicit_and_bounded_without_changing_the_source_image(self):
        self.assertEqual(MODULE.drive_options('/tmp/private.img',8388608),'format=raw,file=/tmp/private.img,throttling.bps-read=8388608')
        self.assertEqual(MODULE.drive_options('/tmp/private.img',0),'format=raw,file=/tmp/private.img')
    def test_invalid_throttles_fail_before_launch(self):
        for rate in (-1,1,True,2**40):
            with self.assertRaises(ValueError):MODULE.drive_options('/tmp/private.img',rate)

class CancelledLoader(unittest.TestCase):
    def test_cancelled_run_requires_matching_acknowledgement_and_failure_order(self):
        trace=TRACE.replace('[initator] started name=bulk unit=1 run=5 pid=6 tick=14\n','')
        trace+='[initator] spawn failed name=bulk unit=1 run=5 tick=17\n[initator] activation outcome=Failed\n'
        result=MODULE.metrics(trace,.34,allow_cancelled=True)
        self.assertEqual(result['load_ticks'],7)
        for bad in (trace.replace('run=5 tick=17','run=6 tick=17'),trace.replace('tick=17','tick=9'),trace.replace('activation outcome=Failed','activation outcome=Running')):
            with self.assertRaises(ValueError):MODULE.metrics(bad,.34,allow_cancelled=True)
    def test_cancellation_cannot_substitute_for_success_in_independent_loader_experiment(self):
        trace=TRACE.replace('[initator] started name=bulk unit=1 run=5 pid=6 tick=14','[initator] spawn failed name=bulk unit=1 run=5 tick=14')
        with self.assertRaises(ValueError):MODULE.metrics(trace,.34)
