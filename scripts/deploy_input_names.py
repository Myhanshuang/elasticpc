#!/usr/bin/env python3
"""Incremental input-tool install, preserving current USB Host files and logs."""
import hashlib,json,os,re,runpy,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];DEV='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0';PART=DEV+'-part1'
REPORT=ROOT/'reports/kingston-input-names-v10.json'
def run(args):return subprocess.run([str(x) for x in args],check=True,capture_output=True,text=True,timeout=600)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def debug(cmd,write=False):
 r=run(['debugfs',*(['-w'] if write else []),'-R',cmd,PART])
 if any(s in r.stderr for s in ('not found','File not found','while ','Usage:','Permission denied','not open')):raise RuntimeError(r.stderr)
 return r.stdout
def main():
 check=json.loads((ROOT/'target/input-names-v10/report.json').read_text())
 assert check['status']=='passed' and check['source_sha256']==sha(ROOT/'host/runtime/epc-input')
 helpers=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='input_deploy_guard')
 helpers['source_guard']({'disk':DEV,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
 assert re.search(r'Filesystem state:\s*clean\s*$',run(['dumpe2fs','-h',PART]).stdout,re.M)
 run(['e2fsck','-f','-n',PART])
 cfg=json.loads(debug('cat /etc/elasticpc/runtime.json'));assert cfg['host_serial']=='1C6F6530298EB360392A31E6'
 report={'status':'running','device':DEV,'operation':'input-tool-and-instructions-only','backup':False,'full_readback':False,'installed_files':{},'physical_handoff_verified':False}
 def save():REPORT.write_text(json.dumps(report,indent=2)+'\n')
 save()
 try:
  for src,dest,mode in [(ROOT/'host/runtime/epc-input','/usr/local/bin/epc-input','0100755'),(ROOT/'docs/usb-input.txt','/root/USB-INPUT.txt','0100644')]:
   if Path(dest).name in debug('ls -l '+str(Path(dest).parent)):debug('rm '+dest,True)
   debug('write '+str(src)+' '+dest,True);debug('sif '+dest+' mode '+mode,True)
   report['installed_files'][dest]=sha(src);save()
  fd=os.open(PART,os.O_RDWR)
  try:os.fsync(fd)
  finally:os.close(fd)
  run(['e2fsck','-f','-n',PART]);report.update(status='written-unverified',phase='complete',filesystem_metadata_check='passed')
 except BaseException as e:report.update(status='failed',error=str(e));raise
 finally:save()
 print(json.dumps(report))
if __name__=='__main__':main()
