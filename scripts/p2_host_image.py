#!/usr/bin/python3
"""Build and verify a Debian Host entirely in disposable file-backed KVM Guests."""
import argparse,hashlib,json,os,shutil,subprocess,time,uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
RT=Path('/tmp/elasticpc-integration-runtime')
INPUT=Path('/tmp/elasticpc-p2-inputs')
DEBS=Path('/tmp/elasticpc-p2-debs')
QEMU=RT/'root/usr/bin/qemu-system-x86_64'
IMG=RT/'root/usr/bin/qemu-img'
ENV={'PATH':'/usr/bin:/bin','LC_ALL':'C','LD_LIBRARY_PATH':str(RT/'root/usr/lib')}
def run(args,timeout=60):return subprocess.run([str(a) for a in args],env=ENV,check=True,capture_output=True,text=True,timeout=timeout)
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def seed(output,mode):
 folder=output/('seed-'+mode);folder.mkdir()
 (folder/'meta-data').write_text('instance-id: elasticpc-'+uuid.uuid4().hex+'\nlocal-hostname: elasticpc-host\n')
 (folder/'network-config').write_text('version: 2\nethernets: {}\n')
 if mode=='build':
  config={'users':[],'disable_root':True,'ssh_pwauth':False,'package_update':False,'package_upgrade':False,'runcmd':[['sh','-c','mkdir -p /mnt/bootstrap && mount -o ro /dev/disk/by-label/cidata /mnt/bootstrap && sh /mnt/bootstrap/install.sh']]}
  for p in (ROOT/'host/p2').iterdir():shutil.copyfile(p,folder/p.name)
  shutil.copyfile(ROOT/'target/debug/elasticctl',folder/'elasticctl')
  run(['/usr/bin/strip',folder/'elasticctl'])
 else:
  config={}
  (folder/'elasticpc-test-mode').write_text(mode+'\n')
 (folder/'user-data').write_text('#cloud-config\n'+json.dumps(config))
 image=output/(mode+'-seed.raw')
 with image.open('xb') as f:f.truncate(1024*1024*(1024 if mode=='build' else 32))
 run(['/usr/bin/mkfs.vfat','-n','cidata',image])
 mcopy=INPUT/'tools/usr/bin/mcopy'
 for p in folder.iterdir():run([mcopy,'-i',image,p,'::/'])
 if mode=='build':
  run([INPUT/'tools/usr/bin/mmd','-i',image,'::/debs'])
  for record in json.loads((DEBS/'provenance.json').read_text())['packages']:
   p=DEBS/'debs'/record['filename']
   if sha(p)!=record['sha256']:raise RuntimeError('Package hash mismatch: '+str(p))
   run([mcopy,'-i',image,p,'::/debs/'])
 return image

def boot(output,disk,varsfile,seedfile,label,expected):
 available=int(next(l.split()[1] for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:')))//1024
 if available<9472:raise RuntimeError('Host reserve + 1 GiB Guest + overhead cannot be met')
 args=[QEMU,'-no-user-config','-nodefaults','-machine','q35,accel=kvm,mem-merge=off,dump-guest-core=off','-cpu','host','-smp','1','-m','1024','-display','none','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-sandbox','on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny','-L',RT/'firmware','-drive',f'if=pflash,format=raw,readonly=on,file={INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={varsfile}','-drive',f'if=virtio,format=qcow2,file={disk}','-drive',f'if=virtio,format=raw,readonly=on,file={seedfile}']
 log=output/(label+'.log')
 with log.open('w') as f:
  child=subprocess.Popen([str(a) for a in args],env=ENV,stdin=subprocess.DEVNULL,stdout=f,stderr=subprocess.STDOUT)
  try:code=child.wait(timeout=600)
  except BaseException:
   child.kill();child.wait();raise
 text=log.read_text(errors='replace')
 if code!=0 or expected not in text or 'ELASTICPC_P2_BUILD_FAILED' in text:raise RuntimeError(f'{label}: failed, exit {code}; inspect {log}')
 return {'phase':label,'passed':True,'exit_code':code,'marker':expected,'log':str(log),'mem_available_mib':available,'log_sha256':sha(log)}
def main():
 p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--dry-run',action='store_true');a=p.parse_args()
 if a.dry_run:
  print(json.dumps({'status':'planned','memory_mib':1024,'vcpus':1,'network':False,'host_devices':False,'phases':['build','normal','recovery-arm','recovery','uninstall','fallback']}));return
 if os.geteuid()==0:raise SystemExit('Run as ordinary host user')
 output=a.output.resolve();output.mkdir(mode=0o700,parents=True,exist_ok=False)
 report={'schema_version':1,'status':'failed','checks':[],'physical_host_modified':False,'native_windows_manjaro_boot_verified':False}
 try:
  signature=json.loads((DEBS/'signature.json').read_text())
  if not signature['signature_verified'] or sha(DEBS/'Release')!=signature['release_sha256']:raise RuntimeError('Unauthenticated package Release')
  source=json.loads((INPUT/'provenance.json').read_text())['debian'];base=Path(source['path'])
  report['package_release_signature']=signature
  report['host_sources']={p.name:sha(p) for p in (ROOT/'host/p2').iterdir()}
  report['builder_sha256']=sha(Path(__file__))
  disk=output/'build.qcow2';run([IMG,'create','-f','qcow2','-F','qcow2','-b',base,disk]);run([IMG,'resize',disk,'12G'])
  variables=output/'build-vars.fd';shutil.copyfile(INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',variables)
  report['checks'].append(boot(output,disk,variables,seed(output,'build'),'build','ELASTICPC_P2_BUILD_OK'))
  host=output/'elasticpc-host.qcow2';run([IMG,'convert','-O','qcow2',disk,host],timeout=180)
  hostvars=output/'elasticpc-host-vars.fd';shutil.copyfile(variables,hostvars)
  trial=output/'verification.qcow2';run([IMG,'create','-f','qcow2','-F','qcow2','-b',host,trial])
  testvars=output/'verification-vars.fd';shutil.copyfile(hostvars,testvars)
  for mode,expected in [('normal','ELASTICPC_P2_NORMAL_OK'),('arm-recovery','ELASTICPC_P2_RECOVERY_ARMED'),('recovery','ELASTICPC_P2_RECOVERY_OK'),('uninstall','ELASTICPC_P2_UNINSTALL_OK'),('fallback','ELASTICPC_P2_FALLBACK_OK')]:
   report['checks'].append(boot(output,trial,testvars,seed(output,mode),mode,expected))
  run([IMG,'check',host]);run([IMG,'check',trial])
  report.update(status='passed',image=str(host),image_sha256=sha(host),firmware_vars_sha256=sha(hostvars),base_source=source,image_info=json.loads(run([IMG,'info','--output=json',host]).stdout))
 except Exception as e:
  report['error']=repr(e)
  if isinstance(e,subprocess.CalledProcessError):report['stderr']=e.stderr
 finally:(output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
 print(json.dumps(report,indent=2));raise SystemExit(0 if report['status']=='passed' else 1)
if __name__=='__main__':main()
