"""Assess a pinned native task corpus. Load-only probes never imply full tasks."""
import argparse
import hashlib
import json
import math
from pathlib import Path

MANDATORY = {'google-search', 'youtube-video', 'facebook-session'}


def digest(manifest):
    """Pin tasks, required checks and limits, independent of whitespace."""
    serialized = json.dumps(manifest, sort_keys=True, separators=(',', ':'))
    return hashlib.sha256(serialized.encode()).hexdigest()


def assess(manifest, report, evidence_exists):
    """Evaluate observations without network requests or changing evidence.

    The caller resolves evidence references. A pass requires every named check,
    native execution, and serial plus assertion evidence. This validates reporting
    discipline, not the truth of an arbitrary report.
    """
    tasks = manifest.get('tasks', [])
    expected = {task['id']: task for task in tasks}
    mandatory = {task['id'] for task in tasks if task['group'] == 'mandatory'}
    if (manifest.get('version') != 1 or len(tasks) != 30
            or len(expected) != 30 or mandatory != MANDATORY):
        raise ValueError('expected the versioned30-task corpus and all three mandatory sites')
    if report.get('manifest_sha256') != digest(manifest):
        raise ValueError('corpus changed or was not pinned')
    build_id = report.get('build_id')
    if (not isinstance(build_id, str) or len(build_id) != 64
            or any(char not in '0123456789abcdef' for char in build_id)):
        raise ValueError('missing build image digest')
    if any(not task.get('checks') or len(task['checks']) != len(set(task['checks']))
           for task in tasks):
        raise ValueError('missing/duplicate required checks')
    ordinary = {task['id'] for task in tasks if task['group'] == 'ordinary'}
    if (len(ordinary) != 27 or manifest.get('required_runs') != 3
            or manifest.get('ordinary_threshold') != .9):
        raise ValueError('unratified acceptance limits')

    runs = []
    ids = set()
    for run in report.get('runs', []):
        if not run.get('id') or run['id'] in ids:
            raise ValueError('duplicate/missing run id')
        ids.add(run['id'])
        observed = set()
        passed = set()
        for item in run.get('observations', []):
            key = item.get('task_id')
            if key not in expected or key in observed:
                raise ValueError('unknown/duplicate task observation')
            if item.get('status') not in ('pass', 'fail', 'blocked', 'not_run'):
                raise ValueError('unknown task status')
            observed.add(key)
            evidence = item.get('evidence', {})
            verified = (
                item.get('native') is True
                and all(item.get('checks', {}).get(check) is True
                        for check in expected[key]['checks'])
                and all(isinstance(evidence.get(name), str) and evidence[name]
                        and evidence_exists(evidence[name])
                        for name in ('serial', 'assertions'))
            )
            if item['status'] == 'pass' and verified:
                passed.add(key)
        count = len(passed & ordinary)
        runs.append(dict(
            id=run['id'],
            passed=count >= math.ceil(len(ordinary) * .9) and MANDATORY <= passed,
            ordinary_passed=count,
            ordinary_total=len(ordinary),
            mandatory={key: key in passed for key in sorted(MANDATORY)},
            missing_tasks=sorted(set(expected) - observed),
        ))
    return dict(
        passed=len(runs) >= 3 and all(run['passed'] for run in runs),
        required_runs=3,
        recorded_runs=len(runs),
        manifest_sha256=digest(manifest),
        build_id=build_id,
        runs=runs,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('manifest', type=Path)
    parser.add_argument('report', type=Path)
    args = parser.parse_args()
    base = args.report.resolve().parent

    def exists(reference):
        path = (base / reference).resolve()
        return path.is_relative_to(base) and path.is_file() and path.stat().st_size > 0

    result = assess(json.loads(args.manifest.read_text()),
                    json.loads(args.report.read_text()), exists)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
