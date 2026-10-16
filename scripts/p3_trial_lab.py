#!/usr/bin/env python3
"""Explicit isolated P3 KVM lab; file inputs only, no networking or host daemon."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import time
import trial_image as trial
import p2_host_image as p2

ROOT = Path(__file__).resolve().parents[1]


def boot(directory, seed, phase, crash=False):
    available = int(next(l.split()[1] for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:'))) // 1024
    if available < 9472:
        raise RuntimeError('Insufficient memory for reserve and 1 GiB Guest')
    args = [p2.QEMU, '-no-user-config', '-nodefaults', '-machine', 'q35,accel=kvm,mem-merge=off,dump-guest-core=off',
            '-cpu', 'host', '-smp', '1', '-m', '1024', '-display', 'none', '-monitor', 'none', '-serial', 'stdio',
            '-nic', 'none', '-no-reboot', '-sandbox', 'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny',
            '-L', p2.RT/'firmware', '-drive', f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd',
            '-drive', f'if=pflash,format=raw,file={directory}/vars.fd',
            '-drive', f'if=virtio,format=qcow2,file={directory}/overlay.qcow2',
            '-drive', f'if=virtio,format=raw,readonly=on,file={seed}']
    with trial.session(directory):
        trial.verify(directory, p2.IMG, p2.ENV)
        child = subprocess.Popen([str(a) for a in args], env=p2.ENV, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        data = bytearray(); killed = False
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                deadline = time.monotonic() + 300
                while True:
                    if time.monotonic() >= deadline:
                        raise TimeoutError('Guest boot timed out')
                    events = selector.select(min(30, deadline-time.monotonic()))
                    if not events:
                        continue
                    chunk = os.read(child.stdout.fileno(), 65536)
                    if not chunk:
                        break
                    data.extend(chunk)
                    if len(data) > 8*1024*1024:
                        raise RuntimeError('Guest log exceeded limit')
                    if crash and not killed and b'ELASTICPC_HOST_READY' in data:
                        child.kill(); killed = True
            code = child.wait(timeout=15)
            if crash:
                if not killed or code != -signal.SIGKILL:
                    raise RuntimeError('Guest crash injection not observed')
            elif code != 0 or b'ELASTICPC_P2_NORMAL_OK' not in data:
                raise RuntimeError('Guest did not complete normal shutdown')
            trial.verify(directory, p2.IMG, p2.ENV)
            return dict(phase=phase, passed=True, exit_code=code, memory_available_mib=available,
                        base_unchanged=True, log_sha256=hashlib.sha256(data).hexdigest())
        finally:
            if child.poll() is None:
                child.kill(); child.wait(timeout=15)
            child.stdout.close()
            (directory/(phase+'.log')).write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    if args.dry_run:
        print(json.dumps(dict(status='planned', file_only=True, network=False,
                             phases=['prepare', 'normal', 'crash', 'restart', 'stale-base-refusal'])))
        return
    if os.geteuid() == 0:
        raise SystemExit('Run as ordinary host user')
    output = args.output.absolute(); output.mkdir(mode=0o700, exist_ok=False)
    report = dict(schema_version=1, status='failed', checks=[], physical_host_modified=False,
                  real_windows_manjaro_import_verified=False,
                  limitations=['Full private copy of ordinary image; not physical partition mapping',
                               'SIGKILL after readiness is not host power-loss testing',
                               'No saved memory, Windows detection, vTPM or Manjaro EFI bridge'])
    try:
        inputs = ROOT/'target/p2-host-release'
        directory = output/'trial'
        manifest = trial.prepare(inputs/'elasticpc-host.qcow2', inputs/'elasticpc-host-vars.fd',
                                 directory, 'qcow2', p2.IMG, p2.ENV)
        report['trial_id'] = manifest['trial_id']
        normal = p2.seed(output, 'normal'); hold = p2.seed(output, 'hold')
        for phase, seed, crash in [('normal', normal, False), ('crash', hold, True), ('restart', normal, False)]:
            report['checks'].append(boot(directory, seed, phase, crash))
        trial.run([p2.IMG, 'check', directory/'overlay.qcow2'], p2.ENV)
        # Alter only the lab's private copy, preserving byte size; prior overlay must be refused.
        base = directory/'base.img'; base.chmod(0o600)
        with base.open('r+b') as stream:
            stream.seek(-1, os.SEEK_END); old = stream.read(1)
            stream.seek(-1, os.SEEK_END); stream.write(bytes([old[0]^1])); stream.flush(); os.fsync(stream.fileno())
        base.chmod(0o400)
        with trial.session(directory):
            try:
                trial.verify(directory, p2.IMG, p2.ENV)
            except ValueError as error:
                if 'Stale base' not in str(error):
                    raise
                report['checks'].append(dict(phase='stale-base-refusal', passed=True, reason=str(error)))
            else:
                raise AssertionError('Changed base accepted')
        report.update(status='passed', stale_trial_directory=str(directory), overlay_consistency='passed_before_base_mutation')
    except Exception as error:
        report['error'] = repr(error)
        if isinstance(error, subprocess.CalledProcessError):
            report['stderr'] = error.stderr
    (output/'report.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
    raise SystemExit(0 if report['status']=='passed' else 1)


if __name__ == '__main__':
    main()
