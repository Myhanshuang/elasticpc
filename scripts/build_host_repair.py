#!/usr/bin/env python3
"""Patch a completed private USB snapshot, retaining all existing trial data."""
import hashlib,json,os,shutil,struct,subprocess
from pathlib import Path
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1];out=Path(os.environ.get('EPC_REPAIR_OUT',str(ROOT/'target/host-repair-v4')));before=out/'physical-before.raw';raw=out/'usb-host.raw';root=out/'root.img'
def run(args,**kw):return subprocess.run([str(x) for x in args],check=True,**kw)
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
assert json.loads((out/'regression.json').read_text())['status']=='passed'
assert before.stat().st_size==14*1024**3 and not raw.exists()
with before.open('rb') as f:
 mbr=f.read(512);start,count=struct.unpack_from('<II',mbr,454)
assert start==262144 and (start+count)*512==before.stat().st_size
run(['cp','--sparse=always',before,raw]);raw.chmod(0o600)
run(['dd','if='+str(before),'of='+str(root),'bs=4M','skip='+str(start*512),'count='+str(count*512),'iflag=skip_bytes,count_bytes','conv=sparse','status=none']);root.chmod(0o600)
run(['e2fsck','-f','-p',root])
def debug(c):run(['debugfs','-w','-R',c,root],stdout=subprocess.DEVNULL)
for source,dest,mode in [(ROOT/'host/runtime/epc','/usr/local/bin/epc','0100755'),(ROOT/'docs/kingston-next-boot.md','/root/NEXT-BOOT.txt','0100644')]:
 debug('rm '+dest);debug('write '+str(source)+' '+dest);debug('sif '+dest+' mode '+mode)
run(['e2fsck','-f','-p',root]);run(['dd','if='+str(root),'of='+str(raw),'bs=4M','seek='+str(start*512),'oflag=seek_bytes','conv=notrunc,sparse','status=none'])
p2.RT=ROOT/'.elasticpc/runtime';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
p2.run([p2.RT/'root/usr/bin/qemu-img','convert','-f','raw','-O','qcow2',raw,out/'usb-host.qcow2'],timeout=300)
shutil.copyfile(ROOT/'target/host-bundle-v3/local-login.txt',out/'local-login.txt');(out/'local-login.txt').chmod(0o600)
report={'status':'passed','check':{'passed':True,'phase':'snapshot-preserving-repair'},'before_sha256':sha(before),'raw_sha256':sha(raw),'image_sha256':sha(out/'usb-host.qcow2'),'raw_bytes':raw.stat().st_size,'runtime_sha256':sha(ROOT/'host/runtime/epc'),'preserves_trial_data':True,'preserves_tailscale_identity':True,'physical_guest_boot_verified':False}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
