import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location(
    'qemu_test', Path(__file__).resolve().parents[1] / 'qemu_test.py')
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


class MonitorReadiness(unittest.TestCase):
    def test_retries_created_but_not_yet_listening_socket_and_closes_attempt(self):
        first, second = Mock(), Mock()
        first.connect.side_effect = ConnectionRefusedError()
        process = Mock()
        process.poll.return_value = None
        with patch.object(monitor.socket, 'socket', side_effect=[first, second]):
            result = monitor.connect('/tmp/qemu.sock', float('inf'), process)
        self.assertIs(result, second)
        first.close.assert_called_once()
        second.close.assert_not_called()

    def test_failed_startup_closes_socket_and_preserves_deadline(self):
        connection = Mock()
        connection.connect.side_effect = ConnectionRefusedError()
        process = Mock()
        process.poll.return_value = None
        with patch.object(monitor.socket, 'socket', return_value=connection), \
                patch.object(monitor.time, 'monotonic', side_effect=[0, 11]), \
                patch.object(monitor.time, 'sleep'):
            with self.assertRaises(TimeoutError):
                monitor.connect('/tmp/qemu.sock', 10, process)
        connection.close.assert_called_once()


class DiskIsolation(unittest.TestCase):
    def test_fixture_mutation_never_changes_the_template(self):
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'template.img'
            source.write_bytes(b'original disk')
            run = root / 'run'
            run.mkdir()
            clone = monitor.copy_disk(source, run)
            self.assertNotEqual(clone, source)
            self.assertEqual(clone.read_bytes(), b'original disk')
            clone.write_bytes(b'private changes')
            self.assertEqual(source.read_bytes(), b'original disk')
