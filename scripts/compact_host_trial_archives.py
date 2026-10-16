#!/usr/bin/env python3
"""Reclaim zero-filled blocks in retained trial files inside an offline private image."""
import base64,hashlib,json,shutil,sys
from pathlib import Path
import pexpect
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1];out=Path(sys.argv[1]).resolve();p2.RT=ROOT/'.elasticpc/runtime';p2.INPUT=ROOT/'.elasticpc/inputs';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
image=out/'usb-host.qcow2';var=out/'compact-vars.fd';shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
args=['-no-user-config','-nodefaults','-machine','q35,accel=kvm','-cpu','host','-smp','2','-m','2048','-display','none','-device','VGA','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-L',str(p2.RT/'firmware'),'-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={var}','-drive',f'file={image},format=qcow2,if=virtio']
program="""import hashlib,json,subprocess,os
from pathlib import Path
rows=[]
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
for p in Path('/var/lib/elasticpc/runtime').glob('*/store.ext4'):
 assert not os.path.ismount(p.parent/'store')
 before=sha(p);blocks=p.stat().st_blocks
 subprocess.run(['fallocate','-d',str(p)],check=True)
 assert before==sha(p)
 rows.append({'session':p.parent.name,'sha256':before,'freed_bytes':(blocks-p.stat().st_blocks)*512})
Path('/root/archive-compaction.json').write_text(json.dumps(rows,indent=2))
print('ARCHIVES_IDENTICAL '+json.dumps(rows),flush=True)
"""
child=pexpect.spawn(str(p2.RT/'root/usr/bin/qemu-system-x86_64'),args,env=p2.ENV,encoding='utf-8',codec_errors='replace',timeout=600,echo=False)
try:
 child.expect('elasticpc-host login:');child.sendline('root');child.expect('Password:');child.sendline((out/'local-login.txt').read_text().split('Password: ',1)[1].strip());child.expect(r'root@elasticpc-host:.*#');child.sendline('stty -echo');child.expect(r'root@elasticpc-host:.*#')
 command="python3 -c \"import base64;exec(base64.b64decode('"+base64.b64encode(program.encode()).decode()+"'))\""
 child.sendline(command);child.expect(r'ARCHIVES_IDENTICAL ([^\r\n]+)\r+\n');rows=json.loads(child.match.group(1));child.expect(r'root@elasticpc-host:.*#')
 child.sendline('poweroff');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
finally:
 if child.isalive():child.terminate(force=True)
p2.run([p2.RT/'root/usr/bin/qemu-img','convert','-O','raw',image,out/'usb-host.raw'],timeout=300)
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
report=json.loads((out/'report.json').read_text());report.update(raw_sha256=sha(out/'usb-host.raw'),image_sha256=sha(image),archive_compaction=rows)
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'status':'passed','archives':rows}))
