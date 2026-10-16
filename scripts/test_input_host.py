#!/usr/bin/env python3
"""Test input tool on a disposable offline Host; never attaches physical USB."""
import base64,json,shutil,uuid,hashlib
from pathlib import Path
import pexpect
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1];OUT=ROOT/'target/input-names-v10';OUT.mkdir(exist_ok=True,mode=0o700)
p2.RT=ROOT/'.elasticpc/runtime';p2.INPUT=ROOT/'.elasticpc/inputs';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
base=ROOT/'target/host-nosave-v9/usb-host.qcow2';tag=uuid.uuid4().hex[:8];disk=OUT/(tag+'.qcow2');var=OUT/(tag+'-vars.fd')
p2.run([p2.RT/'root/usr/bin/qemu-img','create','-f','qcow2','-F','qcow2','-b',base,disk]);shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
args=['-no-user-config','-nodefaults','-machine','q35,accel=kvm','-cpu','host','-m','1536','-display','none','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-L',str(p2.RT/'firmware'),'-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={var}','-drive',f'file={disk},format=qcow2,if=virtio']
c=pexpect.spawn(str(p2.RT/'root/usr/bin/qemu-system-x86_64'),args,env=p2.ENV,encoding='utf-8',codec_errors='replace',timeout=180,echo=False)
r={'status':'failed','physical_usb_attached':False,'network':False}
try:
 c.expect('elasticpc-host login:');c.sendline('root');c.expect('Password:');c.sendline((ROOT/'target/host-nosave-v9/local-login.txt').read_text().split('Password: ',1)[1].strip());c.expect(r'root@elasticpc-host:.*#');c.sendline('stty -echo');c.expect(r'root@elasticpc-host:.*#')
 def cmd(s):c.sendline(s);c.expect(r'root@elasticpc-host:.*#')
 def upload(src,dst):
  encoded=base64.b64encode(src.read_bytes()).decode();cmd(': > /tmp/epc-input-upload')
  for i in range(0,len(encoded),1500):cmd("printf '%s' '"+encoded[i:i+1500]+"' >> /tmp/epc-input-upload")
  cmd('base64 -d /tmp/epc-input-upload > '+dst)
 cmd('mkdir -p /tmp/input-test/host/runtime /tmp/input-test/scripts')
 upload(ROOT/'host/runtime/epc-input','/tmp/input-test/host/runtime/epc-input');upload(ROOT/'scripts/test_epc_input.py','/tmp/input-test/scripts/test_epc_input.py')
 c.sendline("python3 /tmp/input-test/scripts/test_epc_input.py && python3 /tmp/input-test/host/runtime/epc-input --help >/dev/null && qemu-system-x86_64 -device usb-host,help >/tmp/usb-host-help && grep -q hostaddr /tmp/usb-host-help && grep -q vendorid /tmp/usb-host-help && printf '\\nINPUT_HOST_OK\\n'")
 c.expect(r'\r+\nINPUT_HOST_OK\r+\n');c.expect(r'root@elasticpc-host:.*#')
 c.sendline('poweroff');c.expect(pexpect.EOF);c.close();assert c.exitstatus==0
 r.update(status='passed',usb_host_module=True,offline_input_tests=10,source_sha256=hashlib.sha256((ROOT/'host/runtime/epc-input').read_bytes()).hexdigest())
finally:
 if c.isalive():c.terminate(force=True)
 (OUT/'report.json').write_text(json.dumps(r,indent=2)+'\n')
print(json.dumps(r))
