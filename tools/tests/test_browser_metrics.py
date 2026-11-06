import math
import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from browser_metrics import assess

class BrowserMetrics(unittest.TestCase):
    def test_every_trial_and_phase_must_meet_its_budget(self):
        samples=[dict(window=0.4,first_page=0.9,navigation=0.3) for _ in range(3)]
        result=assess(samples)
        self.assertTrue(result['passed'])
        self.assertEqual(result['phases']['window']['maximum_seconds'],0.4)
        samples[-1]['window']=1.001
        self.assertFalse(assess(samples)['passed'], 'a slow trial cannot disappear in an average')
        samples[-1]['window']=1.0
        self.assertTrue(assess(samples)['passed'])

    def test_missing_nonfinite_negative_or_insufficient_observations_are_not_passes(self):
        for samples in [[],[dict(window=.2,first_page=.3,navigation=.1)]]:
            with self.assertRaises(ValueError):assess(samples)
        for value in [-1,math.nan,math.inf,True,'0.1']:
            with self.assertRaises(ValueError):assess([dict(window=value,first_page=.3,navigation=.1)]*3)
        with self.assertRaises(ValueError):assess([dict(window=.1,first_page=.2)]*3)
