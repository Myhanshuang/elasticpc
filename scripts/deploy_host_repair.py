#!/usr/bin/env python3
"""Verify the whole preimage, apply only changed image blocks, verify the whole result."""
import argparse,fcntl,hashlib,json,mmap,os,runpy,struct,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];OUT=Path(os.environ.get('EPC_REPAIR_OUT',str(ROOT/'target/host-repair-v4')));REPORT=Path(os.environ.get('EPC_REPAIR_REPORT',str(ROOT/'reports/kingston-host-repair.json')))
DEVICE='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def main():
 p=argparse.ArgumentParser();p.add_argument('--apply',action='store_true');p.add_argument('--no-device-hash',action='store_true');a=p.parse_args()
 build=json.loads((OUT/'report.json').read_text());assert build['status']=='passed'
 for name in ['login-report.json','runtime-cow-report.json','candidate-regression.json']+(['manjaro-io-report.json','windows-cpu-report.json'] if OUT.name=='host-repair-v5' else [])+(['guided-test-report.json'] if OUT.name in ('host-tests-v6','host-repair-v7') else []):
  r=json.loads((OUT/name).read_text());assert r['status']=='passed'
  if name=='login-report.json':assert r['image_sha256']==build['image_sha256']
  if name in ('runtime-cow-report.json','manjaro-io-report.json'):assert r['source_sha256']==build['raw_sha256'] and r['source_unchanged']
  if name=='candidate-regression.json':assert r['tested_image_sha256']==build['image_sha256']
  if name=='guided-test-report.json':assert r['tested_image_sha256']==build['image_sha256'] and r['installed_files']==build['installed_files']
 before=OUT/'physical-before.raw';after=OUT/'usb-host.raw';size=14*1024**3;block=4*1024**2
 assert before.stat().st_size==after.stat().st_size==build['raw_bytes']==size
 assert sha(before)==build['before_sha256'] and sha(after)==build['raw_sha256']
 changed=[]
 with before.open('rb') as old,after.open('rb') as new:
  for off in range(0,size,block):
   if old.read(block)!=new.read(block):changed.append(off)
 assert changed and len(changed)*block<512*1024**2,'Unexpectedly large patch; inspect before applying'
 state={'status':'planned','phase':'precheck','scope_bytes':size,'device':DEVICE,'changed_blocks':len(changed),'patch_bytes':len(changed)*block,'written_bytes':0,'verified_bytes':0,'expected_sha256':build['raw_sha256'],'physical_guest_boot_verified':False}
 if not a.apply:print(json.dumps(state));return
 lock=open(OUT/'deploy.lock','w');fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 guards=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='deploy_guard');guards['source_guard']({'disk':DEVICE,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
 fd=os.open(DEVICE,os.O_RDWR|os.O_EXCL|os.O_DIRECT|os.O_SYNC);buf=mmap.mmap(-1,block)
 assert struct.unpack('Q',fcntl.ioctl(fd,0x80081272,bytes(8)))[0]==15483273216
 state['status']='running';started=time.monotonic();last=0
 def save(force=False):
  nonlocal last
  now=time.monotonic()
  if force or now-last>=150:
   state['seconds']=round(now-started,2);state['updated_at']=time.strftime('%Y-%m-%dT%H:%M:%S%z')
   tmp=REPORT.with_suffix('.tmp');tmp.write_text(json.dumps(state,indent=2)+'\n');tmp.replace(REPORT);print(json.dumps(state),flush=True);last=now
 def readhash():
  h=hashlib.sha256()
  for off in range(0,size,block):
   if os.preadv(fd,[buf],off)!=block:raise RuntimeError('Short read at '+str(off))
   h.update(buf);state['verified_bytes']=off+block;save()
  return h.hexdigest()
 try:
  save(True)
  if not a.no_device_hash:
   if readhash()!=build['before_sha256']:raise RuntimeError('USB differs from preserved snapshot; no repair writes performed')
  else:
   # Avoid applying a delta to changed metadata; inspect only the blocks to replace.
   with before.open('rb') as old:
    for off in changed:
     old.seek(off)
     if os.preadv(fd,[buf],off)!=block or bytes(buf)!=old.read(block):raise RuntimeError('Changed-block preimage differs; no writes performed')
   state['verification_skipped_by_user']=True
  state.update(phase='write',verified_bytes=0);save(True)
  with after.open('rb') as new:
   for off in changed:
    new.seek(off);data=new.read(block);assert len(data)==block;buf[:]=data
    if os.pwrite(fd,buf,off)!=block:raise RuntimeError('Short write at '+str(off))
    state['written_bytes']+=block;save()
  os.fsync(fd)
  if a.no_device_hash:
   state.update(status='written-unverified',phase='complete');return
  state['phase']='readback';save(True);actual=readhash();state['readback_sha256']=actual
  if actual!=build['raw_sha256']:raise RuntimeError('Repaired image hash mismatch')
  state.update(status='passed',phase='complete')
 except BaseException as e:state.update(status='failed',error=str(e));raise
 finally:buf.close();os.close(fd);save(True)
if __name__=='__main__':main()
