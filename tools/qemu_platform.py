"""Explicit guest platform prerequisites for userspace cryptography."""
import os
import sys

def arguments(rng=True, cpus=None, rtc='base=utc,clock=host'):
    if cpus is None and 'RUSTOS_CPUS' in os.environ:
        cpus = int(os.environ['RUSTOS_CPUS'])
    result = ['-m', '1024', '-cpu', 'qemu64,rdrand='+('on' if rng else 'off'), '-rtc', rtc]
    if cpus is not None:
        if type(cpus) is not int or not 1 <= cpus <= 32:
            raise ValueError('CPU count must be an integer from 1 to 32')
        result += ['-smp', str(cpus)]
    return result

if __name__ == '__main__':
    print('\n'.join(arguments(cpus=int(os.environ.get('RUSTOS_CPUS', '8')))))
