#!/usr/bin/env python3
"""Derive the page-aligned read-only prefix from linker ELF load permissions."""
import pathlib
import struct
import sys


def readonly_size(binary, base=0x400000):
    if len(binary) < 64 or binary[:7] != b'\x7fELF\x02\x01\x01':
        raise ValueError('expected little-endian ELF64 output')
    offset = struct.unpack_from('<Q', binary, 32)[0]
    size, count = struct.unpack_from('<HH', binary, 54)
    if size != 56 or not 1 <= count <= 128 or offset + size * count > len(binary):
        raise ValueError('invalid program header table')
    writable = []
    ends = []
    for index in range(count):
        kind, flags, at, address, _, stored, memory, _ = struct.unpack_from(
            '<IIQQQQQQ', binary, offset + index * size)
        if kind != 1 or memory == 0:
            continue
        if address < base or stored > memory or at + stored > len(binary):
            raise ValueError('invalid load segment')
        ends.append((address + memory + 4095) & ~4095)
        if flags & 2:
            writable.append(address & ~4095)
    if not ends:
        raise ValueError('missing load segments')
    return (min(writable) if writable else max(ends)) - base


if __name__ == '__main__':
    print(readonly_size(pathlib.Path(sys.argv[1]).read_bytes()))
