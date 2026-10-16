import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / 'scripts/boot_bundle.py'
spec = importlib.util.spec_from_file_location('boot_bundle', SCRIPT)
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / 'source'
        self.root.mkdir()
        self.dest = Path(self.tmp.name) / 'bundle'

    def tearDown(self):
        self.tmp.cleanup()

    def put(self, name, data):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def linux(self):
        self.put('etc/os-release', b'ID=manjaro\n')
        self.put('etc/fstab', b'UUID=original-swap none swap defaults 0 0\n')
        self.put('boot/kernel', bytes(0x202) + b'HdrS' + bytes(64))
        self.put('boot/initrd', b'fixture-initrd')
        return bundle.collect('linux', self.root, kernel='boot/kernel', initrd='boot/initrd', root_uuid='abcd-1234')

    def cli_verify(self):
        return subprocess.run([sys.executable, str(SCRIPT), 'verify', '--family', 'linux', '--root', str(self.root), '--kernel', 'boot/kernel', '--initrd', 'boot/initrd', '--root-uuid', 'abcd-1234', '--destination', str(self.dest)], capture_output=True)

    def test_stage_and_verify_then_reject_kernel_drift(self):
        manifest, artifacts = self.linux()
        before = {str(p): p.read_bytes() for p in self.root.rglob('*') if p.is_file()}
        bundle.stage(self.dest, manifest, artifacts)
        self.assertEqual(self.cli_verify().returncode, 0)
        self.assertEqual(before, {str(p): p.read_bytes() for p in self.root.rglob('*') if p.is_file()})
        self.put('boot/kernel', bytes(0x202) + b'HdrSnew-kernel')
        self.assertNotEqual(self.cli_verify().returncode, 0)

    def test_bundle_tamper_rejected(self):
        manifest, artifacts = self.linux()
        bundle.stage(self.dest, manifest, artifacts)
        path = self.dest / 'elasticpc/initrd'
        path.chmod(0o600)
        path.write_bytes(b'tampered')
        self.assertNotEqual(self.cli_verify().returncode, 0)

    def test_standard_os_release_fallback_does_not_follow_arbitrary_link(self):
        self.linux()
        path = self.root / 'etc/os-release'
        path.unlink()
        path.symlink_to('/etc/passwd')
        self.put('usr/lib/os-release', b'ID=manjaro\n')
        manifest, _ = self.linux_invalid()
        self.assertEqual(manifest['artifacts']['os-release']['source_relative_path'], 'usr/lib/os-release')

    def test_noresume_private_config_and_original_fstab_preserved(self):
        manifest, artifacts = self.linux()
        self.assertIn(b'ro noresume', artifacts['grub-config'][1])
        self.assertIn(b'original-swap', (self.root / 'etc/fstab').read_bytes())
        self.assertFalse(manifest['executable'])
        self.assertTrue(any('fstab swap' in gap for gap in manifest['capability_gaps']))

    def test_uuid_injection_and_invalid_kernel_rejected(self):
        self.linux()
        with self.assertRaises(ValueError):
            bundle.collect('linux', self.root, kernel='boot/kernel', initrd='boot/initrd', root_uuid='x\nreboot')
        self.put('boot/kernel', b'MZ-not-linux')
        with self.assertRaises(ValueError):
            self.linux_invalid()

    def linux_invalid(self):
        return bundle.collect('linux', self.root, kernel='boot/kernel', initrd='boot/initrd', root_uuid='abcd')

    def test_symlink_components_and_traversal_rejected(self):
        self.put('outside/file', b'secret')
        (self.root / 'alias').symlink_to(self.root / 'outside', target_is_directory=True)
        (self.root / 'link').symlink_to(self.root / 'outside/file')
        for name in ['alias/file', 'link', '../source/outside/file', '/etc/passwd']:
            with self.assertRaises((ValueError, OSError)):
                bundle.read_artifact(self.root, name)

    def test_existing_destination_not_overwritten(self):
        manifest, artifacts = self.linux()
        self.dest.mkdir()
        (self.dest / 'keep').write_text('keep')
        with self.assertRaises(FileExistsError):
            bundle.stage(self.dest, manifest, artifacts)
        self.assertEqual((self.dest / 'keep').read_text(), 'keep')

    def test_windows_requires_more_than_ntfs_or_file_names(self):
        self.put('Windows/System32/ntoskrnl.exe', b'not PE')
        with self.assertRaises(ValueError):
            bundle.collect('windows', self.root, self.root)
        pe = b'MZ' + bytes(58) + (64).to_bytes(4, 'little') + b'PE\x00\x00'
        self.put('Windows/System32/ntoskrnl.exe', pe)
        self.put('EFI/Microsoft/Boot/bootmgfw.efi', pe)
        self.put('EFI/Microsoft/Boot/BCD', b'regffixture')
        manifest, _ = bundle.collect('windows', self.root, self.root)
        self.assertFalse(manifest['executable'])
        self.assertTrue(any('decode BCD' in gap for gap in manifest['capability_gaps']))
        self.put('EFI/Microsoft/Boot/BCD', b'broken')
        with self.assertRaises(ValueError):
            bundle.collect('windows', self.root, self.root)


if __name__ == '__main__':
    unittest.main()
