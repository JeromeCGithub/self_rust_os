#!/usr/bin/env python3
"""Explicit QEMU Ethernet device/backend shared by launchers and E2E tests."""
import sys


def arguments(mode='user'):
    """Return a complete NIC configuration; none must disable implicit networking."""
    if mode == 'user':
        return ['-netdev', 'user,id=net0,net=10.0.2.0/24,host=10.0.2.2,dhcpstart=10.0.2.15',
                '-device', 'e1000,netdev=net0,mac=52:54:00:12:34:56']
    if mode == 'none':
        return ['-nic', 'none']
    raise ValueError(f'unsupported RUSTOS_NETWORK profile: {mode}')


if __name__ == '__main__':
    print('\n'.join(arguments(sys.argv[1] if len(sys.argv) == 2 else 'user')))
