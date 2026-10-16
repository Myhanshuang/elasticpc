#!/usr/bin/env python3
"""Explicit, narrowly scoped deployment to the user's identity-pinned Kingston USB."""
import runpy,argparse,fcntl,hashlib,json,mmap,os,struct,subprocess,time,zlib
from pathlib import Path

def main():
 p=argparse.ArgumentParser();p.add_argument('--image-directory',type=Path,required=True);p.add_argument('--confirm-erase-kingston',action='store_true');a=p.parse_args()
 if not a.confirm_erase_kingston:raise SystemExit('Explicit erase confirmation required')
 root=Path(__file__).resolve().parents[1];reports=root/'reports'
 directory=a.image_directory.resolve();image=directory/'usb-host.raw';build=json.loads((directory/'report.json').read_text())
 assert build['status']=='passed' and build['check']['passed']
 login=json.loads((directory/'login-report.json').read_text());assert login['status']=='passed' and login['image_sha256']==build['image_sha256']
 size=image.stat().st_size
 assert size==build['raw_bytes']==14*1024**3
 with image.open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==build['raw_sha256']
 cow=json.loads((directory/'runtime-cow-report.json').read_text())
 assert cow['status']=='passed' and cow['source_unchanged'] and cow['source_sha256']==build['raw_sha256']
 # MBR partition bounds; reject protective GPT or partitions beyond this image.
 with image.open('rb') as f: header=f.read(512)
 assert header[510:512]==b'\x55\xaa'
 parts=[]
 for i in range(4):
  entry=header[446+i*16:462+i*16];kind=entry[4]
  if kind:
   first,count=struct.unpack_from('<II',entry,8)
   assert kind!=0xee and first>0 and count>0 and (first+count)*512<=size
   parts.append((first,first+count))
 assert len(parts)==2
 parts.sort();assert parts[0][1]<=parts[1][0]
 name='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0'
 rows=json.loads(subprocess.check_output(['lsblk','-J','-b','-o','MODEL,SERIAL,SIZE,MOUNTPOINTS',name]))['blockdevices']
 r=rows[0];assert r['serial']=='1C6F6530298EB360392A31E6' and int(r['size'])==15483273216
 assert all(not any(x.get('mountpoints',[])) for x in rows+r.get('children',[]))
 guard_source=(root/'host/runtime/epc').read_text()
 # Exiting tasks may retain proc entries after their mount namespace is gone.
 guard_source=guard_source.replace('  except FileNotFoundError:pass', '''  except FileNotFoundError:pass
  except OSError as e:
   if e.errno==22 and not (proc/'ns/mnt').exists():continue
   raise''')
 guards={'__name__':'deploy_guards'};exec(compile(guard_source,'deployment-guards','exec'),guards)
 guards['source_guard']({'disk':name,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
 fd=os.open(name,os.O_RDWR|os.O_EXCL|os.O_DIRECT|os.O_SYNC)
 assert struct.unpack('Q',fcntl.ioctl(fd,0x80081272,bytes(8)))[0]==15483273216
 block=4*1024**2;buf=mmap.mmap(-1,block)
 status={'status':'running','phase':'write','scope_bytes':size,'written_bytes':0,'verified_bytes':0,'device':name,'image_sha256':build['raw_sha256'],'physical_boot_tested':False}
 started=time.monotonic();last=0;report=reports/'kingston-host-deployment.json'
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
