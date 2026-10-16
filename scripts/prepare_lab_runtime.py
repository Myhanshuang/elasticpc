#!/usr/bin/env python3
"""Restore pinned, signed lab tools into a fresh private directory, never install."""
import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import subprocess
import urllib.request
import tempfile
import shutil


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    base = args.destination.absolute()
    base.mkdir(mode=0o700, exist_ok=False)
    home = base / 'gnupg'
    home.mkdir(mode=0o700)
    key = base / 'archlinux.gpg'
    subprocess.run(['/usr/bin/gpg', '--homedir', str(home), '--batch', '--dearmor', '--output', str(key), '/usr/share/pacman/keyrings/archlinux.gpg'], check=True)
    project = Path(__file__).resolve().parents[1]
    records = json.loads((project / 'reports/p1-guest-runtime-provenance.json').read_text())['packages']
    block = json.loads((project / 'reports/qemu-tools-provenance.json').read_text())
    records.append(json.loads((project / 'reports/p1-private-libnbd-provenance.json').read_text()))
    records.append({'name': 'qemu-img', 'resolved_url': 'https://mirror.moson.org/arch/extra/os/x86_64/qemu-img-11.1.2-1-x86_64.pkg.tar.zst', 'sha256': block['sha256']})

    def fetch(record):
        archive = base / (record['name'] + '.pkg.tar.zst')
        for url, path in [(record['resolved_url'], archive), (record['resolved_url'] + '.sig', Path(str(archive) + '.sig'))]:
            with urllib.request.urlopen(url, timeout=60) as response:
                path.write_bytes(response.read())
        if hashlib.sha256(archive.read_bytes()).hexdigest() != record['sha256']:
            raise ValueError('Digest mismatch: ' + record['name'])
        result = subprocess.run(['/usr/bin/gpgv', '--homedir', str(home), '--keyring', str(key), str(archive) + '.sig', str(archive)], check=True, text=True, capture_output=True)
        return dict(record, archive=str(archive), signature_verified=True, verification=result.stderr)

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        verified = list(pool.map(fetch, records))
    root = base / 'root'
    root.mkdir()
    for record in verified:
        listing = subprocess.check_output(['/usr/bin/bsdtar', '-tf', record['archive']], text=True).splitlines()
        selected = [p for p in listing if (p.startswith(('usr/bin/', 'usr/lib/', 'usr/share/qemu/', 'usr/share/seabios/', 'usr/share/libvirt/')) or p.startswith('etc/libvirt/')) and not p.endswith('/')]
        if any(p.startswith('/') or '..' in Path(p).parts for p in selected):
            raise ValueError('Unsafe archive member')
        if selected:
            subprocess.run(['/usr/bin/bsdtar', '-xf', record['archive'], '-C', str(root), *selected], check=True)
    firmware = base / 'firmware'
    firmware.mkdir()
    for name in ['bios-256k.bin', 'linuxboot_dma.bin', 'kvmvapic.bin']:
        candidates = [p for p in (root / 'usr/share').rglob(name) if p.is_file() and not p.is_symlink()]
        if not candidates:
            raise ValueError('Missing firmware ' + name)
        (firmware / name).write_bytes(candidates[0].read_bytes())
    transforms = []
    for name in ['qemu-system-x86_64', 'qemu-img', 'qemu-io', 'virsh', 'virtqemud', 'virtlogd']:
        binary = root / 'usr/bin' / name
        if binary.exists():
            before = hashlib.sha256(binary.read_bytes()).hexdigest()
            subprocess.run(['/usr/bin/patchelf', '--force-rpath', '--set-rpath', '$ORIGIN/../lib', str(binary)], check=True)
            transforms.append({'name': name, 'original_sha256': before, 'derived_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()})
    pc = root / 'usr/lib/pkgconfig/libvirt.pc'
    pc.write_text(pc.read_text().replace('libdir=/usr/lib', 'libdir=' + str(root / 'usr/lib')))
    cpu = Path(tempfile.mkdtemp(prefix='epc-cpu-'))
    shutil.copytree(root / 'usr/share/libvirt/cpu_map', cpu, dirs_exist_ok=True)
    library = (root / 'usr/lib/libvirt.so.0').resolve()
    original = library.read_bytes()
    old = b'/usr/share/libvirt/cpu_map\0'
    new = str(cpu).encode() + b'\0'
    if len(new) > len(old) or original.count(old) != 1:
        raise ValueError('Pinned libvirt CPU resource relocation no longer matches')
    modified = original.replace(old, new + b'\0' * (len(old)-len(new)))
    library.write_bytes(modified)
    transforms.append({'name':'libvirt CPU map path', 'private_path':str(cpu), 'original_sha256':hashlib.sha256(original).hexdigest(), 'derived_sha256':hashlib.sha256(modified).hexdigest()})
    result = {'packages': verified, 'transforms': transforms, 'root': str(root), 'installed_system_packages': False}
    (base / 'provenance.json').write_text(json.dumps(result, indent=2) + '\n')
    print('Verified runtime: ' + str(base), flush=True)

if __name__ == '__main__':
    main()
