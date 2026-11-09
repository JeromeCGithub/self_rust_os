"""Check a bounded native loader experiment, not a hard real-time guarantee."""
import math
import re

def metrics(text, elapsed_wall, *, allow_cancelled=False):
    def require(pattern):
        match=re.search(pattern,text)
        if not match: raise ValueError(f'missing evidence: {pattern}')
        return match
    ready=require(r'\[initator\] ready name=wick unit=\d+ run=\d+ pid=(\d+) tick=(\d+)')
    launch=require(r'\[initator\] launching name=bulk unit=\d+ run=(\d+) tick=(\d+)')
    started_pattern=r'\[initator\] started name=bulk unit=\d+ run=(\d+) pid=\d+ tick=(\d+)'
    if allow_cancelled:
        if re.search(started_pattern,text):
            raise ValueError('dependent loader published instead of acknowledging cancellation')
        started=require(r'\[initator\] spawn failed name=bulk unit=\d+ run=(\d+) tick=(\d+)')
        failed=require(r'\[initator\] activation outcome=Failed')
        if not started.start()<failed.start():
            raise ValueError('failure before cancelled launch acknowledgement')
    else:
        started=require(started_pattern)
    stop=require(r'\[initator\] stopping name=wick unit=\d+ pid=(\d+) tick=(\d+) reason=Some\(Watchdog\)')
    completion=require(r'\[initator\] completion pid='+ready[1]+r' cause=Killed status=9 tick=(\d+)')
    if not ready.start()<launch.start()<stop.start()<completion.start() or stop[1]!=ready[1] or launch[1]!=started[1]:
        raise ValueError('inconsistent process/run/event ordering')
    start_tick=int(launch[2]);ready_tick=int(ready[2]);end_tick=int(started[2]);stop_tick=int(stop[2])
    delay=stop_tick-ready_tick-6
    if start_tick<ready_tick or end_tick<start_tick or not 0<=delay<=2:
        raise ValueError(f'deadline or clock violation: delay={delay} ticks, load={end_tick-start_tick} ticks')
    if not math.isfinite(elapsed_wall) or not 0<=elapsed_wall<=.5:
        raise ValueError(f'watchdog wall-time tolerance exceeded: {elapsed_wall}s')
    return dict(delay_ticks=delay,load_ticks=end_tick-start_tick,elapsed_wall=elapsed_wall)

def drive_options(image, read_bps=0):
    if type(read_bps) is not int or read_bps!=0 and not 2*1024*1024<=read_bps<=1024*1024*1024:
        raise ValueError('read rate must be zero or 2 MiB/s..1 GiB/s')
    options=f'format=raw,file={image}'
    return options+(f',throttling.bps-read={read_bps}' if read_bps else '')
