import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('trial', ROOT/'scripts/trial_image.py')
trial = importlib.util.module_from_spec(spec); spec.loader.exec_module(trial)

class TrialTests(unittest.TestCase):
    def test_native_invalidation_is_durable_idempotent_and_refuses_resume(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root/'session.lock').touch()
            (root/'manifest.json').write_text(json.dumps({'schema_version': 1, 'trial_id': 'fixture'}))
            first = trial.invalidate_for_native(root)
            self.assertFalse(first['schedules_reboot'])
            self.assertEqual(first, trial.invalidate_for_native(root))
            # A separate process must observe the marker before opening backing files/tools.
            code = 'import sys;sys.path.insert(0,sys.argv[1]);import trial_image as t\nwith t.session(sys.argv[2]): t.verify(sys.argv[2], "missing", {})'
            result = subprocess.run(['/usr/bin/python3', '-c', code, str(ROOT/'scripts'), str(root)], capture_output=True)
            self.assertIn(b'Native handoff invalidated', result.stderr)

    def test_native_invalidation_refuses_active_session(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root/'session.lock').touch()
            with trial.session(root):
                with self.assertRaises(BlockingIOError):
                    trial.invalidate_for_native(root)
            self.assertFalse((root/'native-invalidated.json').exists())

    def test_partial_native_marker_fails_closed(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root/'native-invalidated.json').touch()
            with self.assertRaisesRegex(ValueError, 'Native handoff invalidated'):
                trial.verify(root, 'missing', {})

    def test_dry_run_has_no_side_effects(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)/'absent'
            result = subprocess.run(['/usr/bin/python3', str(ROOT/'scripts/trial_image.py'), 'prepare', '--directory', str(output), '--dry-run'], capture_output=True, text=True, check=True)
            self.assertFalse(output.exists())
            self.assertEqual(json.loads(result.stdout)['status'], 'planned')

    def test_device_refused(self):
        with self.assertRaises(ValueError):
            with trial.regular('/dev/null'): pass

    def test_symlink_and_writable_base_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)/'base'; base.write_bytes(b'data')
            with self.assertRaises(ValueError): trial.fingerprint(base)
            base.chmod(0o400)
            link = Path(folder)/'link'; link.symlink_to(base)
            with self.assertRaises((OSError, ValueError)): trial.fingerprint(link)

    def test_same_size_content_change_detected(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)/'base'; base.write_bytes(b'old'); base.chmod(0o400)
            before = trial.fingerprint(base)
            base.chmod(0o600); base.write_bytes(b'new'); base.chmod(0o400)
            after = trial.fingerprint(base)
            self.assertEqual(before['size'], after['size'])
            self.assertNotEqual(before['sha256'], after['sha256'])

    def test_session_exclusion_across_processes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root/'session.lock').touch()
            with trial.session(root):
                code = 'import sys;sys.path.insert(0,sys.argv[1]);import trial_image as t\nwith t.session(sys.argv[2]): pass'
                result = subprocess.run(['/usr/bin/python3', '-c', code, str(ROOT/'scripts'), str(root)], capture_output=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'BlockingIOError', result.stderr)
            with trial.session(root): pass

    def test_qemu_option_delimiter_refused(self):
        with self.assertRaisesRegex(ValueError, 'delimiters'):
            with trial.session('/tmp/trial,file=/dev/example'): pass

    def test_existing_destination_is_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); marker = root/'keep'; marker.write_text('keep')
            with self.assertRaises(FileExistsError):
                trial.prepare('missing', 'missing', root, 'raw', 'missing', {})
            self.assertEqual(marker.read_text(), 'keep')

    def test_failed_prepare_removes_only_new_directory(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)/'new'
            with self.assertRaises(FileNotFoundError):
                trial.prepare('missing', 'missing', root, 'raw', 'missing', {})
            self.assertFalse(root.exists())

if __name__ == '__main__': unittest.main()
