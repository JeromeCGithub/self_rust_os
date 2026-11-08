"""Profile selection must be validated before any disk or build mutation."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

BUILDER = Path(__file__).resolve().parents[1] / 'build-grub-disk.sh'


class InitProfileTests(unittest.TestCase):
    def invoke(self, profile):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'tools').mkdir()
            (root / 'bin').mkdir()
            shutil.copyfile(BUILDER, root / 'tools/build-grub-disk.sh')
            user = root / 'tools/build-user.sh'
            user.write_text('#!/bin/sh\ntouch "$INIT_TEST_USER_CALLED"\n')
            user.chmod(0o755)
            cargo = root / 'bin/cargo'
            cargo.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$INIT_TEST_CARGO_ARGS"\nexit 17\n')
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(root / 'bin') + os.pathsep + os.environ['PATH'],
                       RUSTOS_BOOT_PROFILE=profile, INIT_TEST_CARGO_ARGS=str(root / 'args'),
                       INIT_TEST_USER_CALLED=str(root / 'user-called'))
            result = subprocess.run(['bash', str(root / 'tools/build-grub-disk.sh'),
                                     str(root / 'disposable.img')], env=env,
                                    text=True, capture_output=True)
            args = (root / 'args').read_text().strip() if (root / 'args').exists() else None
            return result, args, (root / 'user-called').exists(), (root / 'disposable.img').exists()

    def test_native_profile_selects_native_kernel_feature(self):
        result, args, user_called, image = self.invoke('native-init')
        self.assertEqual(result.returncode, 17)
        self.assertEqual(args, 'build --release --features grub,native-init')
        self.assertTrue(user_called)
        self.assertFalse(image)

    def test_desktop_profile_retains_current_feature_selection(self):
        result, args, _, image = self.invoke('desktop')
        self.assertEqual(result.returncode, 17)
        self.assertEqual(args, 'build --release --features grub')
        self.assertFalse(image)

    def test_invalid_profile_fails_before_build_or_disk_write(self):
        result, args, user_called, image = self.invoke('natvie-init')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Invalid boot profile', result.stderr)
        self.assertIsNone(args)
        self.assertFalse(user_called)
        self.assertFalse(image)
