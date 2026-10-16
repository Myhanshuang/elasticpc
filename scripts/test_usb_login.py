#!/usr/bin/env python3
"""Authenticate at the private USB-image serial console; no physical disks."""
import sys,json,shutil,uuid
from pathlib import Path
import pexpect
import p2_host_image as p2
if not p2.QEMU.exists():
 p2.RT=p2.ROOT/'.elasticpc/runtime';p2.INPUT=p2.ROOT/'.elasticpc/inputs'
 p2.QEMU=p2.RT/'root/usr/bin/qemu-system-x86_64';p2.IMG=p2.RT/'root/usr/bin/qemu-img'
 p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
out=Path(sys.argv[1]).resolve();base=out/'usb-host.qcow2'
if '--candidate' not in sys.argv:
 assert json.loads((out/'report.json').read_text())['status']=='passed'
tag='login-'+uuid.uuid4().hex[:8]
disk=out/(tag+'.qcow2');p2.run([p2.IMG,'create','-f','qcow2','-F','qcow2','-b',base,disk])
var=out/(tag+'-vars.fd');shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
args=['-no-user-config','-nodefaults','-machine','q35,accel=kvm,mem-merge=off,dump-guest-core=off','-cpu','host','-smp','1','-m','1536','-display','none','-device','VGA','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-L',str(p2.RT/'firmware'),'-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={var}','-device','qemu-xhci','-drive',f'if=none,id=usb,format=qcow2,file={disk}','-device','usb-storage,drive=usb,bootindex=1']
password=(out/'local-login.txt').read_text().split('Password: ',1)[1].strip()
report={'status':'failed','physical_devices':False,'network':False}
child=pexpect.spawn(str(p2.QEMU),args,env=p2.ENV,encoding='utf-8',codec_errors='replace',timeout=180,echo=False)
try:
 child.expect('elasticpc-host login:');child.sendline('root');child.expect('Password:');child.sendline(password);child.expect(r'root@elasticpc-host:.*#')
 # Disable shell input echo so the marker can only originate from execution.
 child.sendline('stty -echo');child.expect(r'root@elasticpc-host:.*#')
 child.sendline("test -f /etc/growroot-disabled && ! findmnt -no OPTIONS / | grep -qw discard && systemctl is-enabled fstrim.timer | grep -qx masked && systemctl is-enabled getty@tty1 | grep -qx enabled && command -v nmtui >/dev/null && test -s /boot/efi/EFI/BOOT/BOOTX64.EFI && printf '\\nELASTICPC_LOGIN_GUARDS_OK\\n'")
 child.expect(r'\r+\nELASTICPC_LOGIN_GUARDS_OK\r+\n')
 child.sendline('poweroff');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
 report.update(status='passed',password_login=True,local_getty_enabled=True,growth_disabled=True,discard_disabled=True,trim_masked=True,image_sha256=p2.sha(base))
finally:
 if child.isalive():child.terminate(force=True)
 (out/'login-report.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
