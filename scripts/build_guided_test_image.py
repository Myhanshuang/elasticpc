#!/usr/bin/env python3
"""Add guided acceptance tools to a private copy of the last deployed image."""
import hashlib
import json
import shutil
import struct
import subprocess
from pathlib import Path
import p2_host_image as p2

ROOT = Path(__file__).resolve().parents[1]
BASE = ROOT / 'target/host-repair-v5'
OUT = ROOT / 'target/host-tests-v6'


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, **kwargs)


def main():
    previous = json.loads((BASE / 'report.json').read_text())
    deployed = json.loads((ROOT / 'reports/kingston-host-repair-v5.json').read_text())
    assert deployed['status'] == 'written-unverified' and deployed['phase'] == 'complete'
    assert deployed['expected_sha256'] == previous['raw_sha256']
    assert sha(BASE / 'usb-host.raw') == previous['raw_sha256']
    assert sha(ROOT / 'host/runtime/epc') == previous['runtime_sha256']
    OUT.mkdir(mode=0o700)
    before = OUT / 'physical-before.raw'
    # This hardlink is immutable input; never open it for writing.
    before.hardlink_to(BASE / 'usb-host.raw')
    raw = OUT / 'usb-host.raw'
    root = OUT / 'root.img'
    with before.open('rb') as stream:
        start, count = struct.unpack_from('<II', stream.read(512), 454)
    assert start == 262144 and (start + count) * 512 == before.stat().st_size
    run(['cp', '--sparse=always', before, raw])
    run(['dd', 'if=' + str(before), 'of=' + str(root), 'bs=4M',
         'skip=' + str(start * 512), 'count=' + str(count * 512),
         'iflag=skip_bytes,count_bytes', 'conv=sparse', 'status=none'])
    root.chmod(0o600)
    run(['e2fsck', '-f', '-p', root])
    installed = {}
    for name, destination in [('epc-test', '/usr/local/bin/epc-test'),
                              ('check-network', '/usr/local/bin/epc-network-check'),
                              ('next-boot.txt', '/root/NEXT-BOOT.txt')]:
        source = ROOT / 'host/runtime' / name
        for command in ['rm ' + destination, 'write ' + str(source) + ' ' + destination,
                        'sif ' + destination + (' mode 0100644' if name.endswith('.txt') else ' mode 0100755')]:
            run(['debugfs', '-w', '-R', command, root], stdout=subprocess.DEVNULL)
        result = run(['debugfs', '-R', 'cat ' + destination, root], capture_output=True)
        assert result.stdout == source.read_bytes()
        installed[destination] = sha(source)
    run(['e2fsck', '-f', '-p', root])
    run(['dd', 'if=' + str(root), 'of=' + str(raw), 'bs=4M', 'seek=' + str(start * 512),
         'oflag=seek_bytes', 'conv=notrunc,sparse', 'status=none'])
    p2.RT = ROOT / '.elasticpc/runtime'
    p2.ENV['LD_LIBRARY_PATH'] = str(p2.RT / 'root/usr/lib')
    image = OUT / 'usb-host.qcow2'
    p2.run([p2.RT / 'root/usr/bin/qemu-img', 'convert', '-f', 'raw', '-O', 'qcow2', raw, image], timeout=300)
    shutil.copyfile(BASE / 'local-login.txt', OUT / 'local-login.txt')
    (OUT / 'local-login.txt').chmod(0o600)
    report = {'status': 'passed', 'before_sha256': previous['raw_sha256'],
              'raw_sha256': sha(raw), 'image_sha256': sha(image), 'raw_bytes': raw.stat().st_size,
              'runtime_sha256': previous['runtime_sha256'], 'installed_files': installed,
              'preserves_trial_data': True, 'preserves_tailscale_identity': True,
              'physical_guest_boot_verified': False}
    (OUT / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
