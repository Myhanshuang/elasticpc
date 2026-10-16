#!/usr/bin/env python3
"""Read exported OS trees and stage private boot artifacts; never mount or edit a source.

The result is a staging directory, NOT a bootable disk or an authorization to boot.
Windows BCD semantics/encryption and Linux initramfs compatibility stay unverified.
"""
import argparse
import errno
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat


@contextmanager
def tree_file(root, name):
    """Resolve every relative component with openat/O_NOFOLLOW (no symlink escape)."""
    parts = PurePosixPath(name).parts
    if not parts or name.startswith('/') or any(p in ('.', '..') for p in name.split('/')):
        raise ValueError('Require a relative file path without dot components')
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        child = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        with os.fdopen(child, 'rb') as stream:
            st = os.fstat(stream.fileno())
            if not stat.S_ISREG(st.st_mode) or st.st_size > 512 * 1024 * 1024:
                raise ValueError('Require a regular artifact no larger than 512 MiB')
            yield stream
    finally:
        os.close(fd)


def read_artifact(root, name):
    with tree_file(root, name) as stream:
        before = os.fstat(stream.fileno())
        data = stream.read(512 * 1024 * 1024 + 1)
        after = os.fstat(stream.fileno())
        if len(data) != before.st_size or (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError('Artifact changed during inspection')
        return data


def record(data):
    return {'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data)}


def pe_image(data):
    if len(data) < 64 or data[:2] != b'MZ':
        return False
    offset = int.from_bytes(data[60:64], 'little')
    return data[offset:offset + 4] == b'PE\x00\x00'


def collect(family, root, esp=None, kernel=None, initrd=None, root_uuid=None):
    artifacts = {}
    if family == 'windows':
        if esp is None:
            raise ValueError('Windows requires an explicitly selected exported ESP tree')
        for label, base, name in [
            ('system', root, 'Windows/System32/ntoskrnl.exe'),
            ('loader', esp, 'EFI/Microsoft/Boot/bootmgfw.efi'),
            ('bcd', esp, 'EFI/Microsoft/Boot/BCD'),
        ]:
            data = read_artifact(base, name)
            if (label != 'bcd' and not pe_image(data)) or (label == 'bcd' and data[:4] != b'regf'):
                raise ValueError(f'Invalid {label} signature; OS identity remains unverified')
            artifacts[label] = (name, data)
        gaps = ['BCD hive signature does not establish OS-volume mapping; decode BCD devices before boot',
                'BitLocker/device encryption, recovery availability, clean shutdown and storage driver compatibility unknown',
                'Private persistent OVMF vars/vTPM and full boot-volume assets still required']
    elif family == 'linux':
        if not kernel or not initrd or not root_uuid or not re.fullmatch(r'[A-Za-z0-9-]{1,64}', root_uuid):
            raise ValueError('Linux requires explicit kernel, initrd and a safe filesystem UUID')
        release_path = 'etc/os-release'
        try:
            os_release = read_artifact(root, release_path)
        except OSError as error:
            if error.errno not in (errno.ELOOP, errno.ENOENT):
                raise
            # Standard fallback, never follow the symlink's supplied target.
            release_path = 'usr/lib/os-release'
            os_release = read_artifact(root, release_path)
        if not re.search(rb'^ID=(?:"[a-zA-Z0-9._-]+"|[a-zA-Z0-9._-]+)\s*$', os_release, re.M):
            raise ValueError('No OS ID in os-release')
        for label, name in [('os-release', release_path), ('fstab', 'etc/fstab'), ('kernel', kernel), ('initrd', initrd)]:
            data = os_release if label == 'os-release' else read_artifact(root, name)
            if not data:
                raise ValueError(f'Empty {label}')
            artifacts[label] = (name, data)
        kernel_data = artifacts['kernel'][1]
        if kernel_data[0x202:0x206] != b'HdrS':
            raise ValueError('Require an x86 Linux boot-protocol kernel')
        # Private GRUB fragment: never source native grub.cfg, which may refer to host swap/ESP.
        cfg = ("set timeout=3\nmenuentry 'ElasticPC imported Linux trial' {\n"
               f"  linux /elasticpc/vmlinuz root=UUID={root_uuid} ro noresume\n"
               "  initrd /elasticpc/initrd\n}\n").encode()
        artifacts['grub-config'] = ('generated', cfg)
        gaps = ['Staged kernel/initrd/config are not an EFI disk: a verified GRUB EFI loader and private FAT image are required',
                'Verify root UUID against selected image and virtio/filesystem modules in initramfs before boot',
                'noresume does not disable fstab swap: use guest-only fstab/service overrides in COW and never expose Host swap/ESP',
                'Refresh bundle after native kernel/initramfs/fstab updates; stop affected Guest before refresh']
    else:
        raise ValueError('Unsupported OS family')
    manifest = {
        'schema_version': 1, 'family': family, 'executable': False,
        'source_kind': 'explicit_file_tree', 'root_uuid': root_uuid,
        'artifacts': {k: {'source_relative_path': name, **record(data)} for k, (name, data) in artifacts.items()},
        'preserve_native_drivers': True, 'capability_gaps': gaps,
    }
    manifest['generation'] = hashlib.sha256(json.dumps(manifest, sort_keys=True).encode()).hexdigest()
    return manifest, artifacts


def stage(destination, manifest, artifacts):
    destination = Path(destination)
    destination.mkdir(mode=0o700, exist_ok=False)
    try:
        # Linux boot paths are fixed; Windows files are evidence only, not a partial boot ESP.
        names = {'kernel': 'elasticpc/vmlinuz', 'initrd': 'elasticpc/initrd',
                 'grub-config': 'boot/grub/grub.cfg'}
        for key, (_, data) in artifacts.items():
            target = destination / names.get(key, f'evidence/{key}')
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with target.open('xb') as stream:
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            target.chmod(0o400)
        with (destination / 'manifest.json').open('x') as stream:
            json.dump(manifest, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        shutil.rmtree(destination)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['inspect', 'stage', 'verify'])
    parser.add_argument('--family', choices=['windows', 'linux'], required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--esp', type=Path)
    parser.add_argument('--kernel')
    parser.add_argument('--initrd')
    parser.add_argument('--root-uuid')
    parser.add_argument('--destination', type=Path)
    args = parser.parse_args()
    try:
        manifest, artifacts = collect(args.family, args.root, args.esp, args.kernel, args.initrd, args.root_uuid)
        if args.action != 'inspect' and args.destination is None:
            raise ValueError('stage/verify requires --destination')
        if args.action == 'stage':
            stage(args.destination, manifest, artifacts)
        elif args.action == 'verify':
            old = json.loads(read_artifact(args.destination, 'manifest.json'))
            if old != manifest:
                raise ValueError('Boot evidence changed; reject old bridge and stage a new bundle')
            names = {'kernel': 'elasticpc/vmlinuz', 'initrd': 'elasticpc/initrd', 'grub-config': 'boot/grub/grub.cfg'}
            for key, (_, data) in artifacts.items():
                if record(read_artifact(args.destination, names.get(key, f'evidence/{key}'))) != record(data):
                    raise ValueError('Staged boot artifact changed; reject bundle')
        print(json.dumps(manifest, indent=2))
    except (OSError, ValueError) as error:
        print(json.dumps({'error': str(error), 'executable': False}))
        raise SystemExit(1)


if __name__ == '__main__':
    main()
