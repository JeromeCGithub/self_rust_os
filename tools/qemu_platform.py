"""Explicit guest platform prerequisites for userspace cryptography."""
import sys

def arguments(rng=True):
    return ['-m', '1024', '-cpu', 'qemu64,rdrand='+('on' if rng else 'off'), '-rtc', 'base=utc,clock=host']

if __name__ == '__main__':
    print('\n'.join(arguments()))
