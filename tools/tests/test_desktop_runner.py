"""Execute the Cargo desktop runner with disposable build/QEMU stand-ins."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


TOOLS = Path(__file__).resolve().parents[1]


class DesktopRunnerTests(unittest.TestCase):
    def run_profile(self, cpus):
        with tempfile.TemporaryDirectory(prefix='rustos-runner-') as directory:
            root = Path(directory)
            (root / 'tools').mkdir()
            (root / 'bin').mkdir()
            for name in ['run-desktop.sh', 'qemu_platform.py', 'qemu_network.py']:
                shutil.copyfile(TOOLS / name, root / 'tools' / name)
            builder = root / 'tools/build-grub-disk.sh'
            builder.write_text('#!/bin/sh\ntouch built\n')
            builder.chmod(0o755)
            qemu = root / 'bin/qemu-system-x86_64'
            qemu.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > qemu-args\n')
            qemu.chmod(0o755)
            env = dict(os.environ, PATH=str(root / 'bin') + os.pathsep + os.environ['PATH'])
            env.pop('RUSTOS_CPUS', None)
            if cpus is not None:
                env['RUSTOS_CPUS'] = cpus
            result = subprocess.run(
                ['bash', str(root / 'tools/run-desktop.sh'), 'target/release/self_rust_os'],
                cwd=root, env=env, capture_output=True, text=True, timeout=10)
            arguments = (root / 'qemu-args').read_text().splitlines() if (root / 'qemu-args').exists() else []
            return result, arguments, (root / 'built').exists()

    def test_default_desktop_uses_eight_cpus(self):
        result, arguments, built = self.run_profile(None)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(built)
        self.assertEqual(arguments[arguments.index('-smp') + 1], '8')

    def test_supported_boundary_profiles_reach_qemu(self):
        for count in ['1', '32']:
            with self.subTest(cpus=count):
                result, arguments, built = self.run_profile(count)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(built)
                self.assertEqual(arguments[arguments.index('-smp') + 1], count)

    def test_invalid_profile_stops_before_build_or_qemu(self):
        for count in ['0', '33', 'invalid']:
            with self.subTest(cpus=count):
                result, arguments, built = self.run_profile(count)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(built)
                self.assertEqual(arguments, [])
