"""Validate admission/publication ordering and exact raced-child rollback."""
import re


def verify_early_exit(text):
    def require(pattern):
        match = re.search(pattern, text)
        if match is None:
            raise ValueError(f'missing rollback evidence: {pattern}')
        return match

    fault = require(r'\[initator\] completion pid=\d+ cause=Fault status=6 tick=\d+')
    failed = require(r'\[initator\] activation outcome=Failed')
    admissions = list(re.finditer(r'\[initator\] launching name=ember unit=(\d+) run=(\d+) tick=\d+', text))
    starts = list(re.finditer(r'\[initator\] started name=ember unit=(\d+) run=(\d+) pid=(\d+) tick=\d+', text))
    if failed.start() <= fault.start() or len(admissions) > 1 or len(starts) > 1:
        raise ValueError('invalid rollback ordering or duplicate launch')
    if not admissions:
        if starts:
            raise ValueError('publication without admission')
        return True
    admission = admissions[0]
    if admission.start() >= fault.start():
        raise ValueError('new dependent admitted after observed prerequisite exit')
    if not starts:
        cancelled = require(r'\[initator\] spawn failed name=ember unit='+admission[1]+r' run='+admission[2]+r' tick=\d+')
        if not admission.start() < cancelled.start() < failed.start():
            raise ValueError('failure before cancelled reservation acknowledged')
        return True
    start = starts[0]
    if (start[1], start[2]) != (admission[1], admission[2]) or start.start() <= admission.start():
        raise ValueError('publication does not match admitted token')
    stop = require(r'\[initator\] stopping name=ember unit='+start[1]+r' pid='+start[3]+r' tick=\d+')
    killed = require(r'\[initator\] completion pid='+start[3]+r' cause=Killed status=9 tick=\d+')
    if not max(fault.start(), start.start()) < stop.start() < killed.start() < failed.start():
        raise ValueError('raced child not stopped and reaped before failure')
    if re.search(r'\[initator\] state name=ember unit=\d+ state=Running', text[fault.end():]):
        raise ValueError('cancelled dependent became running after observed exit')
    return True
