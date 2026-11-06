#!/usr/bin/env python3
"""Run one focused Linux-like capability regression on a disposable disk."""
import argparse
import sys
sys.dont_write_bytecode=True
from file_io import main
parser=argparse.ArgumentParser()
parser.add_argument('--case',required=True)
args=parser.parse_args()
main('cap-'+args.case)
