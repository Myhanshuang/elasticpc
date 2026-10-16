import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch, Mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('p2_host_image', ROOT / 'scripts/p2_host_image.py')
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)

class HostImageTests(unittest.TestCase):
    def test_dry_run_creates_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'new'
            result = subprocess.run(['/usr/bin/python3', str(ROOT/'scripts/p2_host_image.py'), '--dry-run', '--output', str(output)], check=True, capture_output=True, text=True)
            plan = json.loads(result.stdout)
            self.assertFalse(output.exists())
            self.assertFalse(plan['network'])
            self.assertIn('fallback', plan['phases'])

    def test_low_memory_refuses_before_start(self):
        with patch.object(Path, 'read_text', return_value='MemAvailable: 1000 kB\n'), patch.object(builder.subprocess, 'Popen') as launch:
            with self.assertRaisesRegex(RuntimeError, 'reserve'):
                builder.boot(Path('/unused'), 'disk', 'vars', 'seed', 'test', 'READY')
            launch.assert_not_called()

    def test_timeout_kills_and_reaps_guest(self):
        process = Mock()
        process.wait.side_effect = [subprocess.TimeoutExpired('qemu', 600), 0]
        with tempfile.TemporaryDirectory() as directory, patch.object(Path, 'read_text', return_value='MemAvailable: 16000000 kB\n'), patch.object(builder.subprocess, 'Popen', return_value=process):
            with self.assertRaises(subprocess.TimeoutExpired):
                builder.boot(Path(directory), 'disk', 'vars', 'seed', 'test', 'READY')
            process.kill.assert_called_once()
            self.assertEqual(process.wait.call_count, 2)

    def test_zero_exit_without_marker_fails(self):
        process = Mock()
        process.wait.return_value = 0
        with tempfile.TemporaryDirectory() as directory, patch.object(Path, 'read_text', side_effect=['MemAvailable: 16000000 kB\n', 'boot failed']), patch.object(builder.subprocess, 'Popen', return_value=process):
            with self.assertRaisesRegex(RuntimeError, 'failed, exit 0'):
                builder.boot(Path(directory), 'disk', 'vars', 'seed', 'test', 'READY')

    def test_build_failure_overrides_success_marker(self):
        process = Mock()
        process.wait.return_value = 0
        with tempfile.TemporaryDirectory() as directory, patch.object(Path, 'read_text', side_effect=['MemAvailable: 16000000 kB\n', 'READY\nELASTICPC_P2_BUILD_FAILED']), patch.object(builder.subprocess, 'Popen', return_value=process):
            with self.assertRaises(RuntimeError):
                builder.boot(Path(directory), 'disk', 'vars', 'seed', 'test', 'READY')

if __name__ == '__main__':
    unittest.main()
