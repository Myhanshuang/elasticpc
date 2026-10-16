#!/usr/bin/env python3
"""Read-only, direct-I/O verification of the pinned Kingston deployment."""
import fcntl,hashlib,json,mmap,os,struct,subprocess,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
REPORT=ROOT/'reports/kingston-host-deployment.json'
DEVICE='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0'
def main():
 lock=open(ROOT/'reports/kingston-verification.lock','w');fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 build=json.loads((ROOT/'target/host-bundle-v3/report.json').read_text())
 prior=json.loads(REPORT.read_text());size=14*1024**3
 assert build['status']=='passed' and build['raw_bytes']==size
 assert prior['device']==DEVICE and prior['written_bytes']==size and prior['image_sha256']==build['raw_sha256']
 rows=json.loads(subprocess.check_output(['lsblk','-J','-b','-o','SERIAL,SIZE,MOUNTPOINTS',DEVICE]))['blockdevices']
 assert rows[0]['serial']=='1C6F6530298EB360392A31E6' and int(rows[0]['size'])==15483273216
 def flat(rows):
  for row in rows:
   yield row
   yield from flat(row.get('children',[]))
 assert not any(any(r.get('mountpoints',[])) for r in flat(rows))
 fd=os.open(DEVICE,os.O_RDONLY|os.O_EXCL|os.O_DIRECT)
 try:
  assert struct.unpack('Q',fcntl.ioctl(fd,0x80081272,bytes(8)))[0]==15483273216
  (ROOT/'reports'/('kingston-before-verification-'+str(time.time_ns())+'.json')).write_text(json.dumps(prior,indent=2)+'\n')
  state=dict(prior);state.update(status='running',phase='readback',verified_bytes=0,verification_read_only=True,verification_started_at=time.strftime('%Y-%m-%dT%H:%M:%S%z'),pid=os.getpid())
  state.pop('error',None);state.pop('readback_sha256',None)
  started=time.monotonic();last=0
  def save(force=False):
   nonlocal last
   now=time.monotonic()
   if force or now-last>=150:
    state['verification_seconds']=round(now-started,2)
    state['updated_at']=time.strftime('%Y-%m-%dT%H:%M:%S%z')
    tmp=REPORT.with_suffix('.tmp');tmp.write_text(json.dumps(state,indent=2)+'\n');tmp.replace(REPORT)
    print(json.dumps(state),flush=True);last=now
  try:
   save(True);digest=hashlib.sha256();block=4*1024**2
   with mmap.mmap(-1,block) as buf:
    for off in range(0,size,block):
     if os.preadv(fd,[buf],off)!=block:raise RuntimeError('Short read at '+str(off))
     digest.update(buf);state['verified_bytes']=off+block;save()
   actual=digest.hexdigest();state['readback_sha256']=actual
   if actual!=build['raw_sha256']:raise RuntimeError('Full image SHA256 mismatch')
   state.update(status='passed',phase='complete')
  except BaseException as e:
   state.update(status='failed',error=str(e));raise
  finally:save(True)
 finally:os.close(fd)
if __name__=='__main__':main()
