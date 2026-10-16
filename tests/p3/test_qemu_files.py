"""Optional real QEMU file tests, offline and without KVM or devices."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('trial', ROOT/'scripts/trial_image.py')
trial = importlib.util.module_from_spec(spec); spec.loader.exec_module(trial)
RUNTIME = Path('/tmp/elasticpc-integration-runtime')
TOOL = RUNTIME/'root/usr/bin/qemu-img'
ENV = dict(PATH='/usr/bin:/bin', LC_ALL='C', LD_LIBRARY_PATH=str(RUNTIME/'root/usr/lib'))

@unittest.skipUnless(TOOL.exists(), 'Temporary QEMU runtime not prepared')
class RealFileTests(unittest.TestCase):
    def test_raw_overlay_write_and_stale_base(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); source = root/'source.raw'; firmware = root/'source.fd'
            source.write_bytes(b'\x00'*(1024*1024)); source.chmod(0o400)
            firmware.write_bytes(b'private-vars'); firmware.chmod(0o400)
            directory = root/'trial'
            trial.prepare(source, firmware, directory, 'raw', TOOL, ENV)
            with trial.session(directory):
                before = trial.verify(directory, TOOL, ENV)['base_fingerprint']
                trial.run([RUNTIME/'root/usr/bin/qemu-io', '-f', 'qcow2', '-c', 'write -P 0x5a 0 4096', directory/'overlay.qcow2'], ENV)
                self.assertEqual(before, trial.verify(directory, TOOL, ENV)['base_fingerprint'])
                self.assertEqual(source.read_bytes()[:4096], bytes(4096))
                self.assertEqual((directory/'base.img').read_bytes()[:4096], bytes(4096))
                trial.run([RUNTIME/'root/usr/bin/qemu-io', '-f', 'qcow2', '-r', '-c', 'read -P 0x5a 0 4096', directory/'overlay.qcow2'], ENV)
                base = directory/'base.img'; base.chmod(0o600)
                with base.open('r+b') as stream: stream.write(b'X')
                base.chmod(0o400)
                with self.assertRaisesRegex(ValueError, 'Stale base'):
                    trial.verify(directory, TOOL, ENV)

    def test_existing_backing_chain_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); base = root/'source.raw'; base.write_bytes(bytes(1024*1024)); base.chmod(0o400)
            chained = root/'chained.qcow2'
            trial.run([TOOL, 'create', '-f', 'qcow2', '-F', 'raw', '-b', base, chained], ENV); chained.chmod(0o400)
            firmware = root/'vars'; firmware.write_bytes(b'vars'); firmware.chmod(0o400)
            with self.assertRaisesRegex(ValueError, 'standalone'):
                trial.prepare(chained, firmware, root/'trial', 'qcow2', TOOL, ENV)
            self.assertFalse((root/'trial').exists())

if __name__ == '__main__': unittest.main()
