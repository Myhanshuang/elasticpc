#!/usr/bin/env python3
"""Experimental file-only Trial preparation. No physical-device or VM executor."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import uuid


def run(args, env):
    return subprocess.run([str(a) for a in args], env=env, check=True,
                          capture_output=True, text=True, timeout=120).stdout


@contextmanager
def regular(path, readonly=False):
    if not stat.S_ISREG(Path(path).lstat().st_mode):
        raise ValueError('Require a regular file before opening')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid() or st.st_nlink != 1:
            raise ValueError('Require an owned, single-link regular file')
        if readonly and st.st_mode & 0o222:
            raise ValueError('Base inputs must have no writable permission bits')
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        with os.fdopen(os.dup(fd), 'rb') as stream:
            yield stream
    finally:
        os.close(fd)


def fingerprint(path):
    with regular(path, readonly=True) as stream:
        before = os.fstat(stream.fileno())
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        after = os.fstat(stream.fileno())
        if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError('Base changed while hashing')
        return dict(size=after.st_size, sha256=digest, device=after.st_dev, inode=after.st_ino,
                    mtime_ns=after.st_mtime_ns, ctime_ns=after.st_ctime_ns)


def info(path, fmt, tool, env):
    return json.loads(run([tool, 'info', '-f', fmt, '--output=json', path], env))


def prepare(source, firmware, destination, fmt, tool, env):
    if fmt not in ('raw', 'qcow2'):
        raise ValueError('Explicit raw or qcow2 format required')
    destination = Path(destination).absolute()
    destination.mkdir(mode=0o700, exist_ok=False)
    try:
        # Copy from verified descriptors, never by reopening a possibly replaced source path.
        for original, name in [(source, 'base.img'), (firmware, 'vars.fd')]:
            with regular(original, readonly=True) as src, (destination/name).open('xb') as dst:
                before = os.fstat(src.fileno())
                shutil.copyfileobj(src, dst, 1024 * 1024)
                dst.flush(); os.fsync(dst.fileno())
                after = os.fstat(src.fileno())
                if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
                    raise ValueError('Input changed during private copy')
        base = destination/'base.img'
        base.chmod(0o400)
        metadata = info(base, fmt, tool, env)
        if metadata.get('backing-filename') or metadata.get('encrypted'):
            raise ValueError('Only standalone unencrypted bases are supported')
        specific = metadata.get('format-specific', {}).get('data', {})
        if specific.get('data-file'):
            raise ValueError('External qcow2 data files are not supported')
        if fmt == 'qcow2':
            run([tool, 'check', '-f', fmt, base], env)
        run([tool, 'create', '-f', 'qcow2', '-F', fmt, '-b', base, destination/'overlay.qcow2'], env)
        manifest = dict(schema_version=1, trial_id=str(uuid.uuid4()), base_format=fmt,
                        base_fingerprint=fingerprint(base), virtual_size=metadata['virtual-size'],
                        saved_memory_supported=False, source_kind='private_file_copy')
        with (destination/'manifest.json').open('x') as out:
            json.dump(manifest, out, indent=2); out.write('\n'); out.flush(); os.fsync(out.fileno())
        (destination/'session.lock').touch(mode=0o600, exist_ok=False)
        return manifest
    except BaseException:
        shutil.rmtree(destination)
        raise


@contextmanager
def session(directory, inherit=False):
    directory = Path(directory).absolute()
    if any(c == ',' or ord(c) < 32 for c in str(directory)):
        raise ValueError('Trial path cannot contain QEMU option delimiters or control characters')
    st = directory.lstat()
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.getuid() or st.st_mode & 0o077:
        raise ValueError('Trial directory must be owned and private (0700)')
    fd = os.open(directory/'session.lock', os.O_RDWR | os.O_NOFOLLOW)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError('Invalid lock file')
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield (directory, fd) if inherit else directory
    finally:
        os.close(fd)


def verify(directory, tool, env):
    """Caller must hold session() through verification AND any Guest lifetime."""
    directory = Path(directory).absolute()
    if os.path.lexists(directory/'native-invalidated.json'):
        raise ValueError('Native handoff invalidated this Trial; recreate from freshly inspected source')
    with regular(directory/'manifest.json') as stream:
        manifest = json.load(stream)
    if manifest['schema_version'] != 1 or manifest['base_format'] not in ('raw', 'qcow2'):
        raise ValueError('Unsupported Trial manifest')
    if fingerprint(directory/'base.img') != manifest['base_fingerprint']:
        raise ValueError('Stale base: reject overlay and saved-memory restore')
    for name in ('overlay.qcow2', 'vars.fd'):
        with regular(directory/name):
            pass
    overlay = info(directory/'overlay.qcow2', 'qcow2', tool, env)
    if (overlay.get('full-backing-filename') != str(directory/'base.img') or
            overlay.get('backing-filename-format') != manifest['base_format'] or
            overlay['virtual-size'] != manifest['virtual_size']):
        raise ValueError('Overlay backing chain or size changed')
    if overlay.get('format-specific', {}).get('data', {}).get('data-file'):
        raise ValueError('External overlay data file refused')
    return manifest


def invalidate_for_native(directory):
    """Invalidate ONE file profile under its lifetime lock. Does not schedule a reboot.

    Fail closed even after a partial marker write. Never remove this marker to
    resume a stale overlay: create a new profile after native use instead.
    A production adapter must inventory/invalidate every profile of an installation.
    """
    with session(directory) as root:
        with regular(root/'manifest.json') as stream:
            manifest = json.load(stream)
        if manifest.get('schema_version') != 1 or not manifest.get('trial_id'):
            raise ValueError('Invalid Trial identity')
        marker = root/'native-invalidated.json'
        result = dict(schema_version=1, trial_id=manifest['trial_id'],
                      reason='native_handoff', requires_new_trial=True,
                      schedules_reboot=False)
        if not os.path.lexists(marker):
            fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, 'w') as stream:
                json.dump(result, stream)
                stream.flush()
                os.fsync(stream.fileno())
        # An existing incomplete marker is also invalidation, not permission to resume.
        # Sync it again before returning success after an interrupted attempt.
        with regular(marker) as stream:
            os.fsync(stream.fileno())
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'verify', 'invalidate-for-native'])
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--format', choices=['raw', 'qcow2'])
    parser.add_argument('--runtime', type=Path, default=Path('/tmp/elasticpc-integration-runtime'))
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    if args.dry_run:
        print(json.dumps(dict(status='planned', action=args.action, file_only=True, launches_vm=False)))
        return
    env = dict(PATH='/usr/bin:/bin', LC_ALL='C', LD_LIBRARY_PATH=str(args.runtime/'root/usr/lib'))
    tool = args.runtime/'root/usr/bin/qemu-img'
    if args.action == 'prepare':
        if not all([args.source, args.firmware, args.format]):
            parser.error('prepare requires --source, --firmware and --format')
        result = prepare(args.source, args.firmware, args.directory, args.format, tool, env)
    elif args.action == 'invalidate-for-native':
        result = invalidate_for_native(args.directory)
    else:
        with session(args.directory):
            result = verify(args.directory, tool, env)
    print(json.dumps(dict(status='passed', manifest=result), indent=2))


if __name__ == '__main__':
    main()
