#!/usr/bin/env python3
"""Exercise the installed guided runner in an offline disposable file-backed VM."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import uuid
import pexpect
import p2_host_image as p2

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(os.environ.get('EPC_GUIDED_OUT', str(ROOT / 'target/host-tests-v6')))
p2.RT = ROOT / '.elasticpc/runtime'
p2.INPUT = ROOT / '.elasticpc/inputs'
p2.ENV['LD_LIBRARY_PATH'] = str(p2.RT / 'root/usr/lib')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    build = json.loads((OUT / 'report.json').read_text())
    image = OUT / 'usb-host.qcow2'
    assert sha(image) == build['image_sha256']
    tag = 'guided-smoke-' + uuid.uuid4().hex[:8]
    disk = OUT / (tag + '.qcow2')
    vars_file = OUT / (tag + '-vars.fd')
    p2.run([p2.RT / 'root/usr/bin/qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', image, disk])
    shutil.copyfile(p2.INPUT / 'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd', vars_file)
    args = ['-no-user-config', '-nodefaults', '-machine', 'q35,accel=kvm', '-cpu', 'host',
            '-smp', '1', '-m', '1536', '-display', 'none', '-device', 'VGA', '-serial', 'stdio',
            '-monitor', 'none', '-nic', 'none', '-no-reboot', '-L', str(p2.RT / 'firmware'),
            '-drive', f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd',
            '-drive', f'if=pflash,format=raw,file={vars_file}',
            '-drive', f'file={disk},format=qcow2,if=virtio']
    child = pexpect.spawn(str(p2.RT / 'root/usr/bin/qemu-system-x86_64'), args, env=p2.ENV,
                          encoding='utf-8', codec_errors='replace', timeout=180, echo=False)
    report = {'status': 'failed', 'physical_disks': False, 'network': False}
    log = (OUT / (tag + '.log')).open('w')
    try:
        child.expect('elasticpc-host login:')
        child.sendline('root')
        child.expect('Password:')
        child.sendline((OUT / 'local-login.txt').read_text().split('Password: ', 1)[1].strip())
        child.expect(r'root@elasticpc-host:.*#')
        child.sendline('stty -echo')
        child.expect(r'root@elasticpc-host:.*#')
        child.logfile_read = log

        def command(value):
            child.sendline(value)
            child.expect(r'root@elasticpc-host:.*#')

        def upload(source, destination):
            encoded = base64.b64encode(source.read_bytes()).decode()
            command(': > /tmp/guided-upload.b64')
            for index in range(0, len(encoded), 1500):
                command("printf '%s' '" + encoded[index:index+1500] + "' >> /tmp/guided-upload.b64")
            command('base64 -d /tmp/guided-upload.b64 > ' + destination)

        for destination, expected in build['installed_files'].items():
            child.sendline('sha256sum ' + destination)
            child.expect(expected)
            child.expect(r'root@elasticpc-host:.*#')
        child.sendline("epc-test --help >/tmp/guided-help && epc-network-check --help >/tmp/network-help && printf '\\nGUIDED_HELP_OK\\n'")
        child.expect(r'\r+\nGUIDED_HELP_OK\r+\n')
        child.expect(r'root@elasticpc-host:.*#')
        command('mkdir -p /tmp/guided-fixture/scripts /tmp/guided-fixture/host/runtime')
        command('ln -s /usr/local/bin/epc-test /tmp/guided-fixture/host/runtime/epc-test')
        command('ln -s /usr/local/bin/epc /tmp/guided-fixture/host/runtime/epc')
        command('ln -s /usr/local/bin/epc-network-check /tmp/guided-fixture/host/runtime/check-network')
        for test in ('test_guided_test.py', 'test_network_check.py', 'test_host_runtime.py'):
            upload(ROOT / 'scripts' / test, '/tmp/guided-fixture/scripts/' + test)
            child.sendline('python3 /tmp/guided-fixture/scripts/' + test + " && printf '\\nGUIDED_OFFLINE_TEST_OK\\n'")
            child.expect(r'\r+\nGUIDED_OFFLINE_TEST_OK\r+\n')
            child.expect(r'root@elasticpc-host:.*#')
        # Production entry must reject this VM's ordinary file-backed root identity.
        child.sendline("epc-test </dev/null >/tmp/guided-refusal 2>&1; r=$?; test \"$r\" -ne 0 && grep -q 'approved USB Host' /tmp/guided-refusal && printf '\\nGUIDED_IDENTITY_REFUSED\\n'")
        child.expect(r'\r+\nGUIDED_IDENTITY_REFUSED\r+\n')
        child.expect(r'root@elasticpc-host:.*#')
        child.sendline('poweroff')
        child.expect(pexpect.EOF)
        child.close()
        assert child.exitstatus == 0
        report.update(status='passed', tested_image_sha256=sha(image), installed_files=build['installed_files'],
                      help=True, offline_flow_tests=True, wrong_host_refused=True)
    finally:
        if child.isalive():
            child.terminate(force=True)
        log.close()
        (OUT / 'guided-test-report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
