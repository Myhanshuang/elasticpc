#!/usr/bin/env python3
"""Build a physical-USB candidate in private file disks; no device writes."""
import argparse,hashlib,json,os,secrets,shutil,subprocess
from pathlib import Path
import p2_host_image as p2

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);a=parser.parse_args()
 output=a.output.resolve();output.mkdir(mode=0o700,exist_ok=False)
 base=p2.INPUT/'debian-13-generic-amd64-20261001-2618.qcow2'
 expected=next(l.split()[0] for l in (p2.INPUT/'SHA512SUMS').read_text().splitlines() if l.split()[-1]==base.name)
 with base.open('rb') as f:assert hashlib.file_digest(f,'sha512').hexdigest()==expected
 sig=json.loads((p2.DEBS/'signature.json').read_text())
 assert sig['signature_verified'] and p2.sha(p2.DEBS/'Release')==sig['release_sha256']
 report={'status':'failed','physical_device_written':False,'generic_image_sha512':expected,'checks':[]}
 try:
  disk=output/'build.qcow2';p2.run([p2.IMG,'create','-f','qcow2','-F','qcow2','-b',base,disk]);p2.run([p2.IMG,'resize',disk,'12G'])
  variables=output/'build-vars.fd';shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',variables)
  seed=p2.seed(output,'build');folder=output/'seed-build'
  password=secrets.token_urlsafe(15)
  credential=output/'local-login.txt';credential.write_text('Login: root\nPassword: '+password+'\n');credential.chmod(0o600)
  hashed=subprocess.run(['openssl','passwd','-6','-stdin'],input=password+'\n',text=True,capture_output=True,check=True).stdout.strip()
  (folder/'usb-root-password-hash').write_text('root:'+hashed+'\n')
  for name in ['configure.sh','usb-verify','usb-verify.service']:shutil.copyfile(p2.ROOT/'host/usb'/name,folder/name)
  installer=folder/'install.sh'
  installer.write_text(installer.read_text().replace('echo ELASTICPC_P2_BUILD_OK','. /mnt/seed/configure.sh\necho ELASTICPC_P2_BUILD_OK'))
  for name in ['install.sh','configure.sh','usb-verify','usb-verify.service','usb-root-password-hash']:
   p2.run([p2.INPUT/'tools/usr/bin/mcopy','-o','-i',seed,folder/name,'::/'+name])
  extra=Path('/tmp/elasticpc-usb-inputs')
  for r in json.loads((extra/'provenance.json').read_text()):
   path=extra/'debs'/r['filename'];assert p2.sha(path)==r['sha256']
   p2.run([p2.INPUT/'tools/usr/bin/mcopy','-i',seed,path,'::/debs/'])
  report['checks'].append(p2.boot(output,disk,variables,seed,'build','ELASTICPC_P2_BUILD_OK'))
  # Keep a production candidate untouched by verification; test a private COW.
  image=output/'usb-host.qcow2';p2.run([p2.IMG,'convert','-O','qcow2',disk,image],timeout=180)
  test=output/'verify.qcow2';p2.run([p2.IMG,'create','-f','qcow2','-F','qcow2','-b',image,test])
  for mode in ['usb-first','usb-second']:
   seed=p2.seed(output,mode)
   # Fresh NVRAM per boot proves removable-media fallback, no inherited EFI entry.
   var=output/(mode+'-vars.fd');shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
   args=[p2.QEMU,'-no-user-config','-nodefaults','-machine','q35,accel=kvm,mem-merge=off,dump-guest-core=off','-cpu','host','-smp','1','-m','1536','-display','none','-serial','stdio','-monitor','none','-nic','none','-no-reboot','-L',p2.RT/'firmware','-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd','-drive',f'if=pflash,format=raw,file={var}','-device','qemu-xhci,id=xhci','-drive',f'if=none,id=usb,format=qcow2,file={test}','-device','usb-storage,drive=usb,bootindex=1','-drive',f'if=virtio,format=raw,readonly=on,file={seed}']
   log=output/(mode+'.log')
   with log.open('w') as f:
    child=subprocess.Popen([str(x) for x in args],env=p2.ENV,stdin=subprocess.DEVNULL,stdout=f,stderr=subprocess.STDOUT)
    try:code=child.wait(timeout=600)
    except BaseException:child.kill();child.wait();raise
   text=log.read_text(errors='replace');assert code==0 and 'ELASTICPC_USB_OK_'+mode in text and 'ELASTICPC_USB_VERIFY_FAILED' not in text, str(log)
   report['checks'].append({'phase':mode,'passed':True,'log':str(log),'fresh_nvram':True,'usb_emulated':True})
  p2.run([p2.IMG,'check',image]);p2.run([p2.IMG,'convert','-O','raw',image,output/'usb-host.raw'],timeout=180)
  report.update(status='passed',image_sha256=p2.sha(image),raw_sha256=p2.sha(output/'usb-host.raw'),raw_bytes=(output/'usb-host.raw').stat().st_size,network_tested=False,physical_boot_tested=False)
 finally:(output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
 print(json.dumps(report))

if __name__=='__main__':main()
