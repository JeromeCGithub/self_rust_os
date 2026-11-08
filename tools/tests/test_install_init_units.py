"""Native unit packaging is deterministic and rejects unsafe sources before writes."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

PATH = Path(__file__).resolve().parents[1] / 'install_init_units.py'
SPEC = importlib.util.spec_from_file_location('install_init_units', PATH)
INSTALLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALLER)

class InstallInitUnitsTests(unittest.TestCase):
    def test_installs_system_units_and_separate_user_template_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            (root/'system').mkdir(); (root/'user').mkdir()
            (root/'system/Z.UNIT').write_text('fixture z')
            (root/'system/A.UNIT').write_text('fixture a')
            (root/'user/SESSION.UNIT').write_text('fixture session')
            with patch.object(INSTALLER.subprocess,'run') as run:
                INSTALLER.install(root,'disposable.img@@1048576')
            args=[call.args[0] for call in run.call_args_list]
            self.assertEqual(args[0],['mmd','-i','disposable.img@@1048576','::/ETC/INIT','::/ETC/INIT/SYSTEM','::/ETC/INIT/USER'])
            self.assertEqual([cmd[-1] for cmd in args[1:]],['::/ETC/INIT/SYSTEM/A.UNIT','::/ETC/INIT/SYSTEM/Z.UNIT','::/ETC/INIT/USER/SESSION.UNIT'])
    def test_bad_sources_fail_before_any_image_mutation(self):
        for bad in ['notes.txt','duplicate','symlink','oversize','too-many']:
            with self.subTest(bad=bad), tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary); (root/'system').mkdir(); (root/'user').mkdir()
                (root/'system/A.UNIT').write_text('fixture')
                if bad=='notes.txt': (root/'system/notes.txt').write_text('not a unit')
                if bad=='duplicate': (root/'system/a.unit').write_text('case collision')
                if bad=='symlink': (root/'system/B.UNIT').symlink_to(root/'system/A.UNIT')
                if bad=='oversize': (root/'system/A.UNIT').write_bytes(b'x'*8193)
                if bad=='too-many':
                    for n in range(64): (root/f'system/U{n}.UNIT').write_text('fixture')
                with patch.object(INSTALLER.subprocess,'run') as run:
                    with self.assertRaises(ValueError): INSTALLER.install(root,'disposable.img@@1048576')
                    run.assert_not_called()
