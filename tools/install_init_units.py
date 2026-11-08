#!/usr/bin/env python3
"""Install native service templates into a newly built disposable FAT image."""
from pathlib import Path
import subprocess
import sys


def install(root, image):
    sources = []
    for scope in ('system', 'user'):
        directory = Path(root) / scope
        files = sorted(directory.iterdir())
        total = 0
        seen = set()
        if len(files) > 64:
            raise ValueError(f'{scope}: more than 64 unit files')
        for source in files:
            name = source.name
            folded = name.casefold()
            if (source.is_symlink() or not source.is_file()
                    or not folded.endswith('.unit') or folded in seen
                    or len(name.encode()) > 255 or any(ord(c) < 32 or c in ':\\' for c in name)):
                raise ValueError(f'Invalid or duplicate unit source: {source}')
            seen.add(folded)
            size = source.stat().st_size
            total += size
            if size > 8192 or total > 128 * 1024:
                raise ValueError(f'{scope}: configuration byte budget exceeded')
            sources.append((source, f'::/ETC/INIT/{scope.upper()}/{name}'))
    # Validate both scopes before mutating the image. Runtime performs schema and
    # graph validation before any service activation; packaging never executes units.
    subprocess.run(['mmd', '-i', image, '::/ETC/INIT',
                    '::/ETC/INIT/SYSTEM', '::/ETC/INIT/USER'], check=True)
    for source, destination in sources:
        subprocess.run(['mcopy', '-i', image, str(source), destination], check=True)


if __name__ == '__main__':
    install(sys.argv[1], sys.argv[2])
