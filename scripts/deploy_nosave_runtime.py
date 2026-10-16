#!/usr/bin/env python3
"""Install tested runtime files into the owner's unmounted clean Kingston.
No full-device backup or readback. Preserve current trial files and test logs.
"""
import hashlib,json,os,re,runpy,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];OUT=ROOT/'target/host-nosave-v9'
DEV='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0'
REPORT=ROOT/'reports/kingston-host-nosave-v9.json'
def sha(p):
 with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def run(args):return subprocess.run([str(x) for x in args],capture_output=True,text=True,check=True)
def debug(cmd,write=False):
 result=run(['debugfs',*(['-w'] if write else []),'-R',cmd,DEV+'-part1'])
 # debugfs returns zero even for command failures.
 if any(t in result.stderr for t in ('not found','File not found','while ','Usage:','Permission denied','not open')):
  raise RuntimeError(result.stderr)
 return result.stdout
def main():
 build=json.loads((OUT/'report.json').read_text());assert build['status']=='passed'
 for name in ('guided-test-report.json','candidate-regression.json','login-report.json'):
  r=json.loads((OUT/name).read_text());assert r['status']=='passed'
  assert r.get('tested_image_sha256',r.get('image_sha256'))==build['image_sha256']
 guard=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='deploy_guard')
 guard['source_guard']({'disk':DEV,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
 metadata=run(['dumpe2fs','-h',DEV+'-part1']).stdout
 assert re.search(r'Filesystem state:\s*clean\s*$',metadata,re.M)
 # Read-only metadata check, not a whole-device hash or backup.
 run(['e2fsck','-f','-n',DEV+'-part1'])
 cfg=json.loads(debug('cat /etc/elasticpc/runtime.json'))
 assert cfg['host_serial']=='1C6F6530298EB360392A31E6'
 assert cfg['write_policy']=='validated-drivers-installed-native'
 state=json.loads(debug('cat /var/lib/elasticpc/runtime/manjaro-latest.json'))
 assert state['serial']==cfg['manjaro']['serial']=='YMB51T0JA242440G36'
 approval=json.loads((ROOT/'target/host-mixed-v8/manjaro-direct-approved.json').read_text())
 assert state['source_header_sha256']==approval['source_header_sha256']
 payload=[]
 for name,dest in [('epc','/usr/local/bin/epc'),('epc-test','/usr/local/bin/epc-test'),('check-network','/usr/local/bin/epc-network-check'),('next-boot.txt','/root/NEXT-BOOT.txt')]:
  source=ROOT/'host/runtime'/name
  assert sha(source)==build['installed_files'][dest]
  payload.append((source,dest,'0100644' if name.endswith('.txt') else '0100755'))
 candidate_cfg=json.loads((OUT/'runtime.json').read_text())
 cfg['windows']['ram_mib']=8192;cfg['manjaro']['ram_mib']=6144
 assert candidate_cfg==cfg
 assert sha(OUT/'runtime.json')==build['installed_files']['/etc/elasticpc/runtime.json']
 payload.append((OUT/'runtime.json','/etc/elasticpc/runtime.json','0100600'))
 payload.append((ROOT/'target/host-mixed-v8/manjaro-direct-approved.json','/var/lib/elasticpc/runtime/manjaro-direct-approved.json','0100600'))
 report={'status':'running','device':DEV,'operation':'runtime-files-only','backup':False,'full_readback':False,'preserves_current_trial_files':True,'installed_files':{}}
 def save():REPORT.write_text(json.dumps(report,indent=2)+'\n')
 save()
 try:
  for source,dest,mode in payload:
   listing=debug('ls -l '+str(Path(dest).parent))
   if Path(dest).name in listing:debug('rm '+dest,True)
   debug('write '+str(source)+' '+dest,True)
   debug('sif '+dest+' mode '+mode,True)
   report['installed_files'][dest]=sha(source);save()
  fd=os.open(DEV+'-part1',os.O_RDWR)
  try:os.fsync(fd)
  finally:os.close(fd)
  run(['e2fsck','-f','-n',DEV+'-part1'])
  report.update(status='written-unverified',phase='complete',filesystem_metadata_check='passed')
 except BaseException as e:
  report.update(status='failed',error=str(e));raise
 finally:save()
 print(json.dumps(report))
if __name__=='__main__':main()
