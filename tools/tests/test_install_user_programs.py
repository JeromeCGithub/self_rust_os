"""Packaging a test-only probe must not alter normal desktop launchers."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

PATH = Path(__file__).resolve().parents[1] / 'install_user_programs.py'
SPEC = importlib.util.spec_from_file_location('install_user_programs', PATH)
INSTALLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALLER)


class InstallUserProgramsTests(unittest.TestCase):
    def test_installs_normal_apps_and_excludes_test_only_probes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['hello', 'zfileio']:
                program = root / name
                program.mkdir()
                (program / f'{name}.bin').write_bytes(b'test image')
            (root / 'zfileio/.test-only').write_text('Host-driven probe\n')
            with patch.object(INSTALLER.subprocess, 'run') as run:
                INSTALLER.install(root, 'disposable.img@@1048576')
            self.assertEqual([call.args[0][-1] for call in run.call_args_list],
                             ['::/BIN/HELLO.BIN'])
            run.assert_called_once_with(
                ['mcopy', '-i', 'disposable.img@@1048576',
                 str(root / 'hello/hello.bin'), '::/BIN/HELLO.BIN'], check=True)

    def test_services_are_installed_without_desktop_bin_extension(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            service = root / 'jworker'
            service.mkdir()
            (service / 'jworker.bin').write_bytes(b'ELF fixture')
            (service / '.service').write_text('Private execution service')
            with patch.object(INSTALLER.subprocess, 'run') as run:
                INSTALLER.install(root, 'disposable.img@@1048576')
            self.assertEqual(run.call_args.args[0][-1], '::/BIN/JWORKER.SVC')
