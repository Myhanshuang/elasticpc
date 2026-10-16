#!/usr/bin/env python3
"""Repair only mismatching 4 MiB blocks, then verify all 12 GiB; explicit UDisk only."""
import os,json,mmap,hashlib,struct,subprocess,time
from pathlib import Path
root=Path(__file__).resolve().parents[1];candidate=root/'target/usb-host-mbr';size=12*1024**3;block=4*1024**2
proof=json.loads((candidate/'login-report.json').read_text());assert proof['status']=='passed'
with (candidate/'usb-host.qcow2').open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==proof['image_sha256']
source=candidate/'usb-host.raw';assert source.stat().st_size==size
with source.open('rb') as f:expected=hashlib.file_digest(f,'sha256').hexdigest()
with source.open('rb') as f:
 m=f.read(512);assert m[510:512]==b'\x55\xaa' and m[450]==0x83 and m[466]==0xef
 assert struct.unpack_from('<II',m,454)==(262144,24903647)
 assert struct.unpack_from('<II',m,470)==(8192,253952)
 assert f.read(512)==bytes(512)
p='/dev/disk/by-id/usb-General_UDisk-0:0'
r=json.loads(subprocess.check_output(['lsblk','-J','-b','-o','MODEL,SERIAL,SIZE,MOUNTPOINTS',p]))['blockdevices'][0]
assert r['model']=='UDisk' and r['serial']=='General_UDisk-0:0' and int(r['size'])==134217728000
assert not any(r['mountpoints']) and not r.get('children')
fd=os.open(p,os.O_RDWR|os.O_EXCL|os.O_DIRECT|os.O_SYNC);b=mmap.mmap(-1,block)
report=root/'reports/udisk-mbr-repair.json'
s={'status':'running','phase':'compare_and_repair','scope_bytes':size,'checked_bytes':0,'verified_bytes':0,'repaired_blocks':0,'partition_table':'MBR','expected_sha256':expected,'physical_boot_tested':False}
start=time.monotonic();last=0

def save(force=False):
 global last
 now=time.monotonic()
 if force or now-last>=150:
  s['seconds']=round(now-start,2);t=report.with_suffix('.tmp');t.write_text(json.dumps(s,indent=2)+'\n');t.replace(report);print(json.dumps(s),flush=True);last=now
try:
 save(True)
 with source.open('rb') as f:
  for off in range(0,size,block):
   want=f.read(block);assert len(want)==block
   assert os.preadv(fd,[b],off)==block
   if bytes(b)!=want:
    b[:]=want;assert os.pwrite(fd,b,off)==block;os.fsync(fd)
    assert os.preadv(fd,[b],off)==block and bytes(b)==want
    s['repaired_blocks']+=1
   s['checked_bytes']=off+block;save()
 os.fsync(fd);s['phase']='full_readback';save(True);digest=hashlib.sha256()
 for off in range(0,size,block):
  assert os.preadv(fd,[b],off)==block;digest.update(b);s['verified_bytes']=off+block;save()
 assert digest.hexdigest()==expected
 s.update(status='passed',phase='complete',actual_sha256=digest.hexdigest(),requires_replug=True)
except BaseException as e:s.update(status='failed',error=str(e));raise
finally:b.close();os.close(fd);save(True)
