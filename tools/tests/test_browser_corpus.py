import copy
import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from browser_corpus import assess,digest

def fixture():
    tasks=[dict(id=name,group='mandatory',checks=['full_task']) for name in ['google-search','youtube-video','facebook-session']]
    tasks += [dict(id=f'ordinary-{i}',group='ordinary',checks=['read','navigate']) for i in range(27)]
    manifest=dict(version=1,required_runs=3,ordinary_threshold=.9,tasks=tasks)
    observations=[dict(task_id=t['id'],status='pass',native=True,checks={c:True for c in t['checks']},evidence=dict(serial='serial.log',assertions='assertions.json')) for t in tasks]
    report=dict(manifest_sha256=digest(manifest),build_id='1'*64,runs=[dict(id=str(i),observations=copy.deepcopy(observations)) for i in range(3)])
    return manifest,report

class CompatibilityGate(unittest.TestCase):
    def test_a_required_site_cannot_be_averaged_away(self):
        manifest,report=fixture();self.assertTrue(assess(manifest,report,lambda _:True)['passed'])
        report['runs'][0]['observations'][0]['status']='fail'
        self.assertFalse(assess(manifest,report,lambda _:True)['passed'])
    def test_homepage_http200_or_missing_evidence_does_not_complete_a_task(self):
        manifest,report=fixture();report['runs'][0]['observations'][0]['checks']={'http200':True,'homepage':True}
        self.assertFalse(assess(manifest,report,lambda _:True)['passed'])
        manifest,report=fixture();self.assertFalse(assess(manifest,report,lambda _:False)['passed'])
    def test_missing_blocked_and_not_run_tasks_count_in_the_denominator(self):
        for state in ['blocked','not_run']:
            manifest,report=fixture()
            for observation in report['runs'][0]['observations'][3:6]:observation['status']=state
            self.assertFalse(assess(manifest,report,lambda _:True)['passed'])
        manifest,report=fixture();report['runs'][0]['observations']=report['runs'][0]['observations'][:3]
        result=assess(manifest,report,lambda _:True);self.assertFalse(result['passed']);self.assertEqual(result['runs'][0]['ordinary_total'],27)
    def test_unpinned_changed_duplicate_or_incomplete_runs_fail_closed(self):
        manifest,report=fixture();report['manifest_sha256']='wrong'
        with self.assertRaises(ValueError):assess(manifest,report,lambda _:True)
        manifest,report=fixture();report['runs'][0]['observations'].append(report['runs'][0]['observations'][0])
        with self.assertRaises(ValueError):assess(manifest,report,lambda _:True)
        manifest,report=fixture();report['runs']=report['runs'][:2]
        self.assertFalse(assess(manifest,report,lambda _:True)['passed'])
