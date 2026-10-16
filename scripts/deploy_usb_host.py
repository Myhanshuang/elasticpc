#!/usr/bin/env python3
"""Explicit, narrowly scoped deployment to the user's capacity-tested UDisk."""
import argparse,fcntl,hashlib,json,mmap,os,struct,subprocess,time,zlib
from pathlib import Path

def main():
 p=argparse.ArgumentParser();p.add_argument('--image-directory',type=Path,required=True);p.add_argument('--confirm-erase-udisk',action='store_true');a=p.parse_args()
 if not a.confirm_erase_udisk:raise SystemExit('Explicit erase confirmation required')
 root=Path(__file__).resolve().parents[1];reports=root/'reports'
 directory=a.image_directory.resolve();image=directory/'usb-host.raw';build=json.loads((directory/'report.json').read_text())
 assert build['status']=='passed' and len(build['checks'])==3 and all(c['passed'] for c in build['checks'])
 login=json.loads((directory/'login-report.json').read_text());assert login['status']=='passed' and login['image_sha256']==build['image_sha256']
 size=image.stat().st_size
 assert size==build['raw_bytes']==12*1024**3
 with image.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==build['raw_sha256']
 capacity=json.loads((reports/'udisk-powercycle-verification.json').read_text())
 assert capacity['status']=='passed' and capacity['power_cycle_verified'] and size<capacity['verified_bytes']
 # Check GPT integrity and that every partition/backup header stays in the tested range.
 with image.open('rb') as f:
  f.seek(512);header=f.read(512);assert header[:8]==b'EFI PART'
  header_size=struct.unpack_from('<I',header,12)[0];assert 92<=header_size<=512
  expected_crc=struct.unpack_from('<I',header,16)[0];h=bytearray(header[:header_size]);h[16:20]=bytes(4);assert zlib.crc32(h)==expected_crc
  backup=struct.unpack_from('<Q',header,32)[0];entries=struct.unpack_from('<Q',header,72)[0];count,entrysize=struct.unpack_from('<II',header,80)
  assert backup*512<size and count<=1024 and 128<=entrysize<=1024
  f.seek(entries*512);table=f.read(count*entrysize);assert zlib.crc32(table)==struct.unpack_from('<I',header,88)[0]
  for i in range(count):
   item=table[i*entrysize:(i+1)*entrysize]
   if item[:16]!=bytes(16):
    first,last=struct.unpack_from('<QQ',item,32);assert 0<first<=last and (last+1)*512<=size
 name='/dev/disk/by-id/usb-General_UDisk-0:0'
 rows=json.loads(subprocess.check_output(['lsblk','-J','-b','-o','MODEL,SERIAL,SIZE,MOUNTPOINTS',name]))['blockdevices']
 r=rows[0];assert r['model']=='UDisk' and r['serial']=='General_UDisk-0:0' and int(r['size'])==134217728000
 assert all(not any(x.get('mountpoints',[])) for x in rows+r.get('children',[]))
 fd=os.open(name,os.O_RDWR|os.O_EXCL|os.O_DIRECT|os.O_SYNC)
 assert struct.unpack('Q',fcntl.ioctl(fd,0x80081272,bytes(8)))[0]==134217728000
 block=4*1024**2;buf=mmap.mmap(-1,block)
 status={'status':'running','phase':'write','scope_bytes':size,'written_bytes':0,'verified_bytes':0,'device':name,'image_sha256':build['raw_sha256'],'physical_boot_tested':False}
 started=time.monotonic();last=0;report=reports/'udisk-host-deployment.json'
 def save(force=False):
  nonlocal last
  now=time.monotonic()
  if force or now-last>=150:
   status['seconds']=round(now-started,2);temp=report.with_suffix('.tmp');temp.write_text(json.dumps(status,indent=2)+'\n');temp.replace(report);print(json.dumps(status),flush=True);last=now
 try:
  save(True)
  with image.open('rb') as source:
   for off in range(0,size,block):
    data=source.read(block);assert len(data)==block;buf[:]=data
    if os.pwrite(fd,buf,off)!=block:raise RuntimeError('Short write at '+str(off))
    status['written_bytes']=off+block;save()
  os.fsync(fd);status['phase']='readback';save(True)
  digest=hashlib.sha256()
  for off in range(0,size,block):
   if os.preadv(fd,[buf],off)!=block:raise RuntimeError('Short read at '+str(off))
   digest.update(buf);status['verified_bytes']=off+block;save()
  assert digest.hexdigest()==build['raw_sha256'],'Deployment readback hash mismatch'
  status.update(status='passed',phase='complete',readback_sha256=digest.hexdigest())
 except BaseException as e:status.update(status='failed',error=str(e));raise
 finally:buf.close();os.close(fd);save(True)

if __name__=='__main__':main()
