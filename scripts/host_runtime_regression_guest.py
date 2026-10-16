#!/usr/bin/env python3
"""Run only INSIDE the disposable, offline Host VM; all loop devices back temporary files."""
import hashlib,importlib.machinery,importlib.util,json,os,subprocess,tempfile,time
from pathlib import Path
loader=importlib.machinery.SourceFileLoader('epc','/usr/local/bin/epc');spec=importlib.util.spec_from_loader(loader.name,loader);epc=importlib.util.module_from_spec(spec);loader.exec_module(epc)
def run(args):return subprocess.check_output([str(x) for x in args],text=True,stderr=subprocess.STDOUT)
def digest(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
assert Path('/run/elasticpc-disposable-test').exists(), 'Test harness marker required'
with tempfile.TemporaryDirectory(prefix='epc-regression-') as tmp:
 d=Path(tmp);source=d/'source.raw'
 with source.open('wb') as f:f.truncate(64*1024**2)
 original=digest(source);loop=run(['losetup','--find','--show','--read-only',source]).strip()
 fd=os.open(loop,os.O_RDONLY|os.O_EXCL)
 try:
  overlay=d/'trial.qcow2';run(['qemu-img','create','-f','qcow2','-F','raw','-b',loop,overlay])
  import shutil
  var=d/'vars.fd';shutil.copyfile('/usr/share/elasticpc/OVMF_VARS.fd',var)
  args=epc.cmdline({'vcpus':1,'ram_mib':256,'uuid':'c003aa6f-5bd7-4b61-bca4-83d1c8c23b15'},fd,overlay,var,d,'regression',{},network_on=False)
  args[args.index('-machine')+1]='q35,accel=tcg';args[args.index('-cpu')+1]='max';args+=['-S']
  blocks=[json.loads(args[i+1]) for i,x in enumerate(args) if x=='-blockdev'];assert blocks[0]['driver']=='host_device' and blocks[0]['read-only']
  # Negative control: reproduce the exact real-device/file-driver failure.
  bad=list(args);i=bad.index('-blockdev')+1;node=json.loads(bad[i]);node['driver']='file';bad[i]=json.dumps(node)
  old=subprocess.run(bad,pass_fds=(fd,),capture_output=True,text=True,timeout=30)
  assert old.returncode!=0 and 'regular file' in old.stderr,old.stderr
  with (d/'qemu.log').open('w') as log:
   q=subprocess.Popen(args,pass_fds=(fd,),stdout=log,stderr=log)
   try:
    for _ in range(100):
     if q.poll() is not None:raise RuntimeError((d/'qemu.log').read_text())
     try:
      status=epc.qmp(d/'qmp.sock','query-status');break
     except (OSError,ValueError):time.sleep(.1)
    else:raise RuntimeError('No QMP')
    assert status['status']=='prelaunch'
    epc.qmp(d/'qmp.sock','set_password',{'protocol':'vnc','password':'testonly'})
    epc.qmp(d/'qmp.sock','cont')
    epc.control('restart',True,d,{'os':'windows','status':'running'})
    epc.control('stop',True,d,{'os':'windows','status':'running'})
    assert q.wait(timeout=30)==0
    assert (d/'last-reset.json').exists() and (d/'forced-stop.json').exists()
   finally:
    if q.poll() is None:q.kill();q.wait()
  run(['qemu-io','-f','qcow2','-c','write -P 0xa5 0 4096',overlay])
  run(['qemu-io','-r','-f','qcow2','-c','read -P 0xa5 0 4096',overlay])
  assert digest(source)==original
 finally:os.close(fd);run(['losetup','-d',loop])
 # Exercise actual noswap tmpfs and verified archive persistence.
 ram=d/'ram-session';ram.mkdir();(ram/'store').mkdir()
 run(['mount','-t','tmpfs','-o','size=64M,noswap,mode=0700','tmpfs',ram/'store'])
 try:
  source=ram/'store/trial.qcow2';source.write_bytes(os.urandom(4*1024**2));original=digest(source)
  epc.archive_ram(ram);assert digest(ram/'trial-saved.qcow2')==original and source.exists()
  assert json.loads((ram/'archive.json').read_text())['status']=='saved'
 finally:run(['umount',ram/'store'])
 # No-save finalization on a real noswap tmpfs, with no Guest left running.
 discard=d/'discard-session';discard.mkdir();(discard/'store').mkdir()
 run(['mount','-t','tmpfs','-o','size=64M,noswap,mode=0700','tmpfs',discard/'store'])
 try:
  (discard/'store/trial.qcow2').write_bytes(os.urandom(1024**2))
  (discard/'trial-saving.qcow2').write_bytes(b'partial')
  state={'os':'windows','mode':'trial','storage':'ram','status':'save-failed','archive':'failed','session':str(discard)}
  oldbase=epc.BASE;epc.BASE=d
  try:epc.request_discard(discard,state)
  finally:epc.BASE=oldbase
  assert not (discard/'store').is_mount()
  assert not (discard/'trial-saving.qcow2').exists()
  assert not (discard/'trial-saved.qcow2').exists()
  assert state['status']=='discarded' and state['archive']=='discarded'
  assert json.loads((discard/'archive.json').read_text())['requested_by_user']
 finally:
  if (discard/'store').is_mount():run(['umount',discard/'store'])
 # Exercise installed ntfs-3g tools, including their unexpected zero exit code.
 ntfs=d/'ntfs.raw'
 with ntfs.open('wb') as f:f.truncate(64*1024**2)
 run(['mkntfs','-F','-Q',ntfs]);file=d/'dummy';file.write_text('test')
 run(['ntfscp',ntfs,file,'/Windows'])
 missing=subprocess.run(['ntfsinfo','-F','/hiberfil.sys',ntfs],capture_output=True,text=True)
 assert missing.returncode==0 and 'No such file or directory' in missing.stdout+missing.stderr
 epc.windows_clean(str(ntfs))
 run(['ntfscp',ntfs,file,'/hiberfil.sys'])
 try:epc.windows_clean(str(ntfs))
 except RuntimeError as e:assert 'hibernation' in str(e)
 else:raise AssertionError('Hibernation file not refused')
print('EPC_BLOCK_AND_NTFS_REGRESSION_OK',flush=True)
