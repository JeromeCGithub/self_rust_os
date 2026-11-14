"""Fixture and strict evidence checks for supervised native filesystem execution."""
import re
PROBE_PASS = '[filetest] PASS: descriptors, offsets, usercopy, cwd, quotas, exit/kill cleanup, streaming'


def fixture():
    """Run the finite existing probe beside a continuously supervised sibling."""
    return {
        'A.UNIT': 'Version=1\nKind=service\nName=beacon\nExec=/BIN/GLOW.SVC\nRestart=never\n',
        'B.UNIT': 'Version=1\nKind=service\nName=fileio\nExec=/BIN/ZFILEIO.BIN\nRestart=never\nRequires=beacon\n',
        'BOOT.UNIT': 'Version=1\nKind=target\nName=boot\nRequires=fileio\n',
    }


def completed_probe(output):
    """Require complete serial records, one probe run and its exact completion."""
    lines = output.splitlines(keepends=True)
    complete = ''.join(line for line in lines if line.endswith('\n'))
    starts = re.findall(r'^\[initator\] started name=(beacon|fileio) unit=\d+ run=\d+ pid=(\d+) tick=\d+\r?$', complete, re.MULTILINE)
    probe = [int(pid) for name, pid in starts if name == 'fileio']
    sibling = [int(pid) for name, pid in starts if name == 'beacon']
    if len(probe) > 1 or len(sibling) > 1:
        raise ValueError('finite file probe or sibling restarted/duplicated')
    if not probe or not sibling:
        return False
    if probe[0] <= 1 or sibling[0] <= 1 or probe[0] == sibling[0]:
        raise ValueError('file probe and sibling require distinct non-init identities')
    completions = re.findall(r'^\[initator\] completion pid=(\d+) cause=(\w+) status=(\d+) tick=\d+\r?$', complete, re.MULTILINE)
    finished = False
    for pid, cause, status in completions:
        if int(pid) == sibling[0]:
            raise ValueError('independent sibling terminated')
        if int(pid) == probe[0]:
            if cause != 'Exited' or status != '0' or finished:
                raise ValueError('file probe did not complete once successfully')
            finished = True
    return (finished and '[initator] PID 1 entered Ring 3\n' in complete
            and PROBE_PASS + '\n' in complete and '[glow] beat=3\n' in complete)
