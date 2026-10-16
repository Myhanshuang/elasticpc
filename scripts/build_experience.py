#!/usr/bin/env python3
"""Build local-console-only preview image using the proven offline P2 recipe."""
import argparse
import json
from pathlib import Path
import shutil
import p2_host_image as p2


def main():
    parser=argparse.ArgumentParser(); parser.add_argument('--output',type=Path,required=True); args=parser.parse_args()
    output=args.output.absolute(); output.mkdir(mode=0o700,exist_ok=False)
    signature=json.loads((p2.DEBS/'signature.json').read_text())
    if not signature['signature_verified'] or p2.sha(p2.DEBS/'Release')!=signature['release_sha256']:
        raise RuntimeError('Unauthenticated package Release')
    source=json.loads((p2.INPUT/'provenance.json').read_text())['debian']
    disk=output/'build.qcow2'; p2.run([p2.IMG,'create','-f','qcow2','-F','qcow2','-b',source['path'],disk]); p2.run([p2.IMG,'resize',disk,'12G'])
    variables=output/'build-vars.fd'; shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',variables)
    seed=p2.seed(output,'build')
    install=output/'seed-build/install.sh'
    addition='''
# Local disposable console preview only. No SSH password or network is enabled.
mkdir -p /etc/systemd/system/serial-getty@ttyS0.service.d
cat > /etc/systemd/system/serial-getty@ttyS0.service.d/preview.conf <<'UNIT'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --noclear %I 115200,38400,9600 vt100
UNIT
systemctl disable ssh.service ssh.socket || true
cat > /etc/motd <<'MESSAGE'
ElasticPC Preview — isolated Debian console
Changes persist in your private Trial overlay. No network or physical disks.
Try: echo hello > /root/experience-note; cat /root/experience-note
Use poweroff to shut down. The client escapes with Ctrl-].
MESSAGE
printf '\\nexport PS1="elasticpc-preview:\\w# "\\n' >> /root/.bashrc
'''
    install.write_text(install.read_text().replace('echo ELASTICPC_P2_BUILD_OK',addition+'\necho ELASTICPC_P2_BUILD_OK'))
    p2.run([p2.INPUT/'tools/usr/bin/mcopy','-o','-i',seed,install,'::/install.sh'])
    check=p2.boot(output,disk,variables,seed,'build','ELASTICPC_P2_BUILD_OK')
    image=output/'experience.qcow2'; p2.run([p2.IMG,'convert','-O','qcow2',disk,image],timeout=180)
    firmware=output/'experience-vars.fd'; shutil.copyfile(variables,firmware)
    p2.run([p2.IMG,'check',image]); image.chmod(0o400); firmware.chmod(0o400)
    report=dict(status='passed',check=check,image_sha256=p2.sha(image),vars_sha256=p2.sha(firmware),
                local_console_autologin=True,network=False,physical_host_modified=False,
                install_sha256=p2.sha(install))
    (output/'report.json').write_text(json.dumps(report,indent=2)+'\n'); print(json.dumps(report))

if __name__=='__main__': main()
