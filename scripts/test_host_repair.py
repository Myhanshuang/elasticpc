#!/usr/bin/env python3
"""Offline disposable Host boot, then real loop-device/QMP/NTFS regression."""
import base64,json,shutil,sys,uuid,os
from pathlib import Path
import pexpect
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1];p2.RT=ROOT/'.elasticpc/runtime';p2.INPUT=ROOT/'.elasticpc/inputs';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
out=Path(os.environ.get('EPC_TEST_OUT',str(ROOT/'target/host-repair-v4')));base=Path(sys.argv[1]).resolve() if len(sys.argv)>1 else Path(os.environ.get('EPC_TEST_BASE',str(ROOT/'target/host-bundle-v3/usb-host.qcow2')))
tag='regression-'+uuid.uuid4().hex[:8];disk=out/(tag+'.qcow2');var=out/(tag+'-vars.fd')
p2.run([p2.RT/'root/usr/bin/qemu-img','create','-f','qcow2','-F','qcow2','-b',base,disk]);shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
args=['-no-user-config','-nodefaults','-machine','q35,accel=kvm','-cpu','host','-smp','2','-m','2048','-display','none','-device','VGA','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-L',str(p2.RT/'firmware'),'-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={var}','-drive',f'file={disk},format=qcow2,if=virtio']
child=pexpect.spawn(str(p2.RT/'root/usr/bin/qemu-system-x86_64'),args,env=p2.ENV,encoding='utf-8',codec_errors='replace',timeout=240,echo=False)
report={'status':'failed','real_disks':False,'network':False};log=(out/(tag+'.log')).open('w')
try:
 child.expect('elasticpc-host login:');child.sendline('root');child.expect('Password:')
 child.sendline((ROOT/'target/host-bundle-v3/local-login.txt').read_text().split('Password: ',1)[1].strip());child.expect(r'root@elasticpc-host:.*#')
 child.sendline('stty -echo');child.expect(r'root@elasticpc-host:.*#');child.logfile_read=log
 def command(c):child.sendline(c);child.expect(r'root@elasticpc-host:.*#')
 def upload(source,destination):
  data=base64.b64encode(source.read_bytes()).decode();command(': > /tmp/upload.b64')
  for i in range(0,len(data),1500):command("printf '%s' '"+data[i:i+1500]+"' >> /tmp/upload.b64")
  command('base64 -d /tmp/upload.b64 > '+destination)
 # On the candidate run, exercise the installed code, checking its exact hash.
 if len(sys.argv)==1:upload(ROOT/'host/runtime/epc','/usr/local/bin/epc')
 else:
  import hashlib
  wanted=hashlib.sha256((ROOT/'host/runtime/epc').read_bytes()).hexdigest()
  child.sendline('sha256sum /usr/local/bin/epc');child.expect(wanted);child.expect(r'root@elasticpc-host:.*#')
 upload(ROOT/'scripts/host_runtime_regression_guest.py','/tmp/regression.py')
 command('touch /run/elasticpc-disposable-test')
 child.sendline('python3 /tmp/regression.py');child.expect(r'\r+\nEPC_BLOCK_AND_NTFS_REGRESSION_OK\r+\n');child.expect(r'root@elasticpc-host:.*#')
 child.sendline('poweroff');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
 import hashlib
 with base.open('rb') as f:report['tested_image_sha256']=hashlib.file_digest(f,'sha256').hexdigest()
 report.update(status='passed',block_device_negative_control=True,block_device_qmp=True,cow_source_unchanged=True,ntfs_missing_hiber_passes=True,ntfs_present_hiber_refused=True)
finally:
 if child.isalive():child.terminate(force=True)
 log.close();(out/('candidate-regression.json' if len(sys.argv)>1 else 'regression.json')).write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
