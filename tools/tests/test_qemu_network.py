"""Execute the host networking profile, including its command-line interface."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'qemu_network.py'
spec = importlib.util.spec_from_file_location('qemu_network', MODULE)
qemu_network = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qemu_network)


class NetworkProfileTests(unittest.TestCase):
    def test_user_profile_selects_exactly_one_supported_nic_and_backend(self):
        args = qemu_network.arguments('user')
        self.assertEqual(args.count('-device'), 1)
        self.assertEqual(args.count('-netdev'), 1)
        self.assertIn('e1000,netdev=net0,mac=52:54:00:12:34:56', args)
        self.assertIn('user,id=net0,net=10.0.2.0/24,host=10.0.2.2,dhcpstart=10.0.2.15', args)

    def test_no_nic_disables_qemu_implicit_network(self):
        self.assertEqual(qemu_network.arguments('none'), ['-nic', 'none'])

    def test_unknown_profile_fails_instead_of_silently_using_defaults(self):
        result = subprocess.run([sys.executable, str(MODULE), 'invalid'], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_cli_outputs_one_argument_per_line(self):
        result = subprocess.run([sys.executable, str(MODULE), 'user'], check=True, text=True, capture_output=True)
        self.assertEqual(result.stdout.splitlines(), qemu_network.arguments())


if __name__ == '__main__':
    unittest.main()
