#!/usr/bin/env python3
"""Install regular apps into /BIN; keep host-driven regression probes private."""
from pathlib import Path
import subprocess
import sys


def install(programs, image):
    for binary in sorted(Path(programs).glob('*/*.bin')):
        if (binary.parent / '.test-only').is_file():
            continue
        subprocess.run(['mcopy', '-i', image, str(binary),
                        f'::/BIN/{binary.name.upper()}'], check=True)


if __name__ == '__main__':
    install(sys.argv[1], sys.argv[2])
