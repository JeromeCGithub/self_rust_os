#!/usr/bin/env python3
"""Run only the Ring 3 process-argument regression on a private GRUB disk."""
import sys
sys.dont_write_bytecode = True
from file_io import main
if __name__ == '__main__':
    main('arguments')
