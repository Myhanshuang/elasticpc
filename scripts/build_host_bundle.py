#!/usr/bin/env python3
"""Upgrade the verified MBR image in an isolated file-backed Guest; no physical writes."""
import hashlib,json,os,shutil,struct,subprocess,tarfile,sys
from pathlib import Path
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1]
p2.RT=ROOT/'.elasticpc/runtime';p2.INPUT=ROOT/'.elasticpc/inputs';p2.QEMU=p2.RT/'root/usr/bin/qemu-system-x86_64';p2.IMG=p2.RT/'root/usr/bin/qemu-img';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
out=ROOT/'target/host-bundle-v3';out.mkdir(mode=0o700,exist_ok=False)
deps=Path('/tmp/elasticpc-console-debs');sig=json.loads((deps/'signature.json').read_text());assert sig['signature_verified'] and p2.sha(deps/'Release')==sig['release_sha256']
payload=out/'payload';payload.mkdir();(payload/'debs').mkdir()
for r in json.loads((deps/'provenance.json').read_text())['packages']:
 src=deps/'debs'/r['filename'];assert p2.sha(src)==r['sha256'];shutil.copyfile(src,payload/'debs'/src.name)
for name in ['epc','epc-test','epc-input','epc-device','epc-usb-plan','check-network','profile.json','tailscaled.service','install-bundle.sh','driver-packages.txt','next-boot.txt']:
 shutil.copyfile(ROOT/'host/runtime'/name,payload/name)
shutil.copyfile(ROOT/'host/usb/collect-diagnostics',payload/'collect-diagnostics')
for src,name in [(Path('/usr/bin/tailscale'),'tailscale'),(Path('/usr/bin/tailscaled'),'tailscaled'),(Path('/tmp/elasticpc-windows-bridge.efi'),'windows-bridge.efi'),(ROOT/'.elasticpc/imports/manjaro-readonly/boot/vmlinuz-6.12-x86_64','manjaro-vmlinuz'),(ROOT/'.elasticpc/imports/manjaro-readonly/boot/initramfs-6.12-x86_64.img','manjaro-initrd')]:shutil.copyfile(src,payload/name)
assert p2.sha(ROOT/'.elasticpc/imports/manjaro-readonly/boot/vmlinuz-6.12-x86_64')==p2.sha(Path('/boot/vmlinuz-6.12-x86_64'))
for src,name in [('OVMF_CODE.4m.fd','OVMF_CODE.fd'),('OVMF_VARS.4m.fd','OVMF_VARS.fd')]:shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64'/src,payload/name)
password=(ROOT/'target/usb-host-v2/local-login.txt').read_text().split('Password: ',1)[1].strip()
assert password and '\n' not in password
hashed=subprocess.run(['openssl','passwd','-6','-stdin'],input=password+'\n',capture_output=True,text=True,check=True).stdout.strip()
(payload/'password-hash').write_text('root:'+hashed+'\n');(payload/'password-hash').chmod(0o600)
(out/'local-login.txt').write_text('Login: root\nPassword: '+password+'\n');(out/'local-login.txt').chmod(0o600)
files=[p for p in payload.rglob('*') if p.is_file()]
(payload/'SHA256SUMS').write_text(''.join(p2.sha(p)+'  '+str(p.relative_to(payload))+'\n' for p in files))
with tarfile.open(out/'payload.tar','w') as tar:
 for p in payload.iterdir():tar.add(p,arcname=p.name)
# Extract only the existing owned ext4 partition into an ordinary file.
rootimg=out/'root.img';run=lambda args,**kw: subprocess.run([str(x) for x in args],check=True,**kw)
run(['dd','if='+str(ROOT/'target/usb-host-mbr/usb-host.raw'),'of='+str(rootimg),'bs=4M','skip=134217728','count=12750667264','iflag=skip_bytes,count_bytes','conv=sparse','status=none'])
run(['e2fsck','-f','-p',rootimg]);os.truncate(rootimg,14*1024**3-134217728);run(['resize2fs',rootimg])
def debug(command):run(['debugfs','-w','-R',command,rootimg],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
unit=out/'elasticpc-bundle-install.service';unit.write_text('[Unit]\nDescription=Explicit file-image bundle installer\nAfter=local-fs.target\nBefore=elasticpc-host.service\nConditionPathExists=/dev/disk/by-label/EPCSETUP\n[Service]\nType=oneshot\nExecStart=/usr/local/sbin/epc-bundle-install\nTimeoutStartSec=900\n[Install]\nWantedBy=multi-user.target\n')
debug('write '+str(unit)+' /etc/systemd/system/elasticpc-bundle-install.service')
debug('write '+str(payload/'install-bundle.sh')+' /usr/local/sbin/epc-bundle-install');debug('sif /usr/local/sbin/epc-bundle-install mode 0100755')
debug('symlink /etc/systemd/system/multi-user.target.wants/elasticpc-bundle-install.service /etc/systemd/system/elasticpc-bundle-install.service')
run(['e2fsck','-f','-p',rootimg])
raw=out/'build.raw';run(['cp','--reflink=auto','--sparse=always',ROOT/'target/usb-host-mbr/usb-host.raw',raw]);os.truncate(raw,14*1024**3)
with raw.open('r+b') as f:
 f.seek(458);f.write(struct.pack('<I',(14*1024**3-134217728)//512))
run(['dd','if='+str(rootimg),'of='+str(raw),'bs=4M','seek=134217728','oflag=seek_bytes','conv=notrunc,sparse','status=none'])
# Use verified Debian mtools locally, never install it into the current host.
tools=out/'mtools';tools.mkdir();deb=next((deps/'debs').glob('mtools_*.deb'))
member=next(x for x in subprocess.check_output(['ar','t',str(deb)],text=True).splitlines() if x.startswith('data.tar'))
with (tools/member).open('wb') as f:run(['ar','p',deb,member],stdout=f)
run(['tar','-xf',tools/member,'-C',tools])
seed=out/'setup.img';seed.touch();os.truncate(seed,512*1024**2);run(['mkfs.vfat','-n','EPCSETUP',seed]);run([tools/'usr/bin/mcopy','-i',seed,out/'payload.tar','::/payload.tar'])
# The installer needs its script before extracting payload; it already lives inside root.img.
variables=out/'build-vars.fd';shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',variables)
p2.run([p2.IMG,'convert','-f','raw','-O','qcow2',raw,out/'build.qcow2'],timeout=180)
check=p2.boot(out,out/'build.qcow2',variables,seed,'bundle-build','ELASTICPC_BUNDLE_OK')
p2.run([p2.IMG,'convert','-O','qcow2',out/'build.qcow2',out/'usb-host.qcow2'],timeout=180)
p2.run([p2.IMG,'convert','-O','raw',out/'usb-host.qcow2',out/'usb-host.raw'],timeout=180)
report={'status':'passed','check':check,'raw_sha256':p2.sha(out/'usb-host.raw'),'image_sha256':p2.sha(out/'usb-host.qcow2'),'raw_bytes':(out/'usb-host.raw').stat().st_size,'physical_guest_boot_verified':False,'gpu_passthrough_verified':False,'tailscale_direct_verified':False,'runtime_sha256':p2.sha(payload/'epc')}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
