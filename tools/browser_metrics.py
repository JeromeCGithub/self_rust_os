"""Fixed browser wall-clock budgets; all trials must pass, including cold processes."""
import math
import statistics

BUDGETS={'window':1.0,'first_page':2.0,'navigation':1.0}

def assess(samples):
    if len(samples)<3:
        raise ValueError('at least three complete browser launches are required')
    phases={}
    for phase,budget in BUDGETS.items():
        values=[]
        for sample in samples:
            value=sample.get(phase)
            if type(value) not in (int,float) or not math.isfinite(value) or value<0:
                raise ValueError('missing or invalid phase duration: '+phase)
            values.append(value)
        phases[phase]={'median_seconds':statistics.median(values),'maximum_seconds':max(values),'budget_seconds':budget}
    return {'passed':all(p['maximum_seconds']<=p['budget_seconds'] for p in phases.values()),'trials':len(samples),'phases':phases}
