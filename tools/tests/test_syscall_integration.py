"""Prevent collisions between compositor IPC and the added network/platform ABI."""
from pathlib import Path
import re
import unittest
ROOT = Path(__file__).resolve().parents[2]


def constants(path):
    return dict((name, int(value)) for name, value in re.findall(
        r'pub const (SYS_\w+): u64 = (\d+);', (ROOT / path).read_text()))


class SyscallIntegration(unittest.TestCase):
    def test_network_and_platform_do_not_occupy_main_or_reserved_syscalls(self):
        message = constants('user_programs/userlib/src/sys.rs')
        self.assertEqual(message['SYS_SEND_MESSAGE'], 20)
        self.assertEqual(message['SYS_RECEIVE_MESSAGE'], 21)
        network = constants('crates/net-abi/src/lib.rs')
        platform = constants('crates/platform-abi/src/lib.rs')
        all_numbers = list(network.values()) + list(platform.values())
        self.assertEqual(len(all_numbers), len(set(all_numbers)))
        self.assertTrue(all(number >= 64 for number in all_numbers), all_numbers)
        self.assertTrue(all(number >= 80 for number in platform.values()))
