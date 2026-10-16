#!/usr/bin/env python3
"""Build an offline no-save candidate from the already tested local v8 image.
Physical deployment installs only the listed runtime files, never this old data.
"""
import hashlib,json,shutil,subprocess
from pathlib import Path
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1];OUT=ROOT/'target/host-nosave-v9';BASE=ROOT/'target/host-mixed-v8'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def run(args,**kw):return subprocess.run([str(x) for x in args],check=True,**kw)
root=OUT/'root.img'
run(['cp','--sparse=always',BASE/'root.img',root])
installed={}
for name,dest in [('epc','/usr/local/bin/epc'),('epc-test','/usr/local/bin/epc-test'),('check-network','/usr/local/bin/epc-network-check'),('next-boot.txt','/root/NEXT-BOOT.txt')]:
 source=ROOT/'host/runtime'/name
 for c in ['rm '+dest,'write '+str(source)+' '+dest,'sif '+dest+' mode '+('0100644' if name.endswith('.txt') else '0100755')]:
  run(['debugfs','-w','-R',c,root],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 assert run(['debugfs','-R','cat '+dest,root],capture_output=True).stdout==source.read_bytes()
 installed[dest]=sha(source)
source=OUT/'runtime.json';dest='/etc/elasticpc/runtime.json'
for c in ['rm '+dest,'write '+str(source)+' '+dest,'sif '+dest+' mode 0100600']:
 run(['debugfs','-w','-R',c,root],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
assert run(['debugfs','-R','cat '+dest,root],capture_output=True).stdout==source.read_bytes()
installed[dest]=sha(source)
run(['e2fsck','-f','-p',root])
raw=OUT/'usb-host.raw'
with raw.open('xb') as dst:
 with (BASE/'usb-host.raw').open('rb') as src:dst.write(src.read(128*1024**2))
 with root.open('rb') as src:
  while data:=src.read(4*1024**2):
   if data.count(0)==len(data):dst.seek(len(data),1)
   else:dst.write(data)
 dst.truncate()
raw.chmod(0o600)
p2.RT=ROOT/'.elasticpc/runtime';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
image=OUT/'usb-host.qcow2'
p2.run([p2.RT/'root/usr/bin/qemu-img','convert','-f','raw','-O','qcow2',raw,image],timeout=600)
shutil.copyfile(BASE/'local-login.txt',OUT/'local-login.txt');(OUT/'local-login.txt').chmod(0o600)
r={'status':'built-not-tested','raw_sha256':sha(raw),'image_sha256':sha(image),'raw_bytes':raw.stat().st_size,'installed_files':installed,'runtime_sha256':sha(ROOT/'host/runtime/epc'),'deployment':'runtime-files-only','usb_written':False}
(OUT/'report.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r))
root.unlink() # Disposable intermediate only; retain tested raw and qcow2 images.
