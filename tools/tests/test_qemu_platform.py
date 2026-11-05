import importlib.util
from pathlib import Path
import unittest
spec=importlib.util.spec_from_file_location('qemu_platform',Path(__file__).resolve().parents[1]/'qemu_platform.py')
platform=importlib.util.module_from_spec(spec);spec.loader.exec_module(platform)
class Platform(unittest.TestCase):
 def test_cpu_counts_cover_the_supported_range(self):
  for count in range(1, 33):
   args=platform.arguments(cpus=count)
   self.assertEqual(args[args.index('-smp')+1], str(count))
 def test_invalid_cpu_counts_are_rejected(self):
  for count in [0, -1, 33, 256, True, '8', 1.5]:
   with self.assertRaises(ValueError):
    platform.arguments(cpus=count)
 def test_clock_override_preserves_smp_topology(self):
  args=platform.arguments(cpus=8, rtc='base=1970-01-01T00:00:00,clock=vm')
  self.assertEqual(args[args.index('-rtc')+1], 'base=1970-01-01T00:00:00,clock=vm')
  self.assertEqual(args[args.index('-smp')+1], '8')
 def test_hardware_rng_and_utc_are_explicit(self):
  self.assertEqual(platform.arguments(),['-m','1024','-cpu','qemu64,rdrand=on','-rtc','base=utc,clock=host'])
 def test_rng_failure_profile_changes_only_the_rng_bit(self):
  self.assertEqual(platform.arguments(False),['-m','1024','-cpu','qemu64,rdrand=off','-rtc','base=utc,clock=host'])

 def test_modern_memory_budget(self):
  args=platform.arguments()
  self.assertEqual(args[args.index('-m')+1], '1024')
