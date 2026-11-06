"""Verify pinned upstream source files before compiling the engine probe."""
import hashlib
import json
from pathlib import Path
import sys


def verify(directory):
    root = Path(directory).resolve()
    manifest = json.loads((root / 'VENDOR.json').read_text())
    if not manifest.get('files'):
        raise ValueError('empty vendor manifest')
    for name, expected in manifest['files'].items():
        source = (root / name).resolve()
        if not source.is_relative_to(root) or not source.is_file():
            raise ValueError(f'invalid or missing vendor source: {name}')
        actual = hashlib.sha256(source.read_bytes()).hexdigest()
        if actual != expected:
            raise ValueError(f'vendor checksum mismatch: {name}')
    return True


if __name__ == '__main__':
    verify(sys.argv[1])
