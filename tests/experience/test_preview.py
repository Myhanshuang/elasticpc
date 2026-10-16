import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'scripts'))
import experience
import trial_image

class PreviewTests(unittest.TestCase):
    def test_missing_socket_with_free_lock_means_stopped(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'session.lock').touch()
            with patch.object(experience,'rpc',side_effect=FileNotFoundError()):
                self.assertEqual(experience.observation(root)['state'],'Stopped')

    def test_busy_lock_without_socket_does_not_mean_stopped(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'session.lock').touch()
            with trial_image.session(root), patch.object(experience,'rpc',side_effect=FileNotFoundError()):
                with self.assertRaises(BlockingIOError):experience.observation(root)

    def test_unresponsive_socket_is_not_stopped(self):
        with patch.object(experience,'rpc',side_effect=TimeoutError('unresponsive')):
            with self.assertRaises(TimeoutError):experience.observation(Path('/unused'))

    def test_unsafe_paths_rejected(self):
        for value in ['/tmp/preview,unsafe','/tmp/preview\nunsafe']:
            with self.assertRaises(ValueError):experience.checked_root(value)

    def test_shared_directory_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);root.chmod(0o755)
            with self.assertRaises(ValueError):experience.checked_root(root)

    def test_inherited_lock_descriptor_available(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'session.lock').touch()
            with trial_image.session(root,inherit=True) as (path,fd):
                self.assertEqual(path,root)
                self.assertTrue(os.fstat(fd).st_size==0)

    def test_inherited_lock_survives_parent_context(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'session.lock').touch()
            with trial_image.session(root,inherit=True) as (_,fd):
                child=subprocess.Popen(['/usr/bin/python3','-c','import sys;sys.stdin.read(1)'],stdin=subprocess.PIPE,pass_fds=(fd,))
            try:
                with self.assertRaises(BlockingIOError):
                    with trial_image.session(root):pass
            finally:
                child.stdin.write(b'x');child.stdin.close();child.wait(timeout=5)
            with trial_image.session(root):pass

if __name__=='__main__':unittest.main()
