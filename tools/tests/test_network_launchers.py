"""Network acceptance must locate actual applications after browser/service installs."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location('network_runner',Path(__file__).resolve().parents[2]/'e2e/network.py')
NETWORK=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NETWORK)
class NetworkLaunchers(unittest.TestCase):
    def test_browser_and_service_do_not_redirect_terminal_or_probe_launches(self):
        names=['CURL.BIN','FORGE.BIN','HELLO.BIN','JBROWSE.BIN','JWORKER.SVC','PORTAL.BIN','SNAKE.BIN','SPIN.BIN','ZNETTEST.BIN']
        listing='\n'.join('::/BIN/'+name for name in names).encode()
        with patch.object(NETWORK.subprocess,'check_output',return_value=listing):
            self.assertEqual(NETWORK.launcher_x('private.img','PORTAL'),450)
            self.assertEqual(NETWORK.launcher_x('private.img','ZNETTEST'),750)
