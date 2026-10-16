#!/usr/bin/env python3
import hashlib,json,os,re,runpy,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
DEV='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0';PART=DEV+'-part1'
def run(a):return subprocess.run(a,check=True,capture_output=True,text=True).stdout
def debug(cmd,write=False):
 r=subprocess.run(['debugfs',*(['-w'] if write else []),'-R',cmd,PART],capture_output=True,text=True,check=True)
 if any(t in r.stderr for t in ('not found','while ','Usage:','Permission denied','not open')):raise RuntimeError(r.stderr)
 return r.stdout
h=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='deploy_guard')
h['source_guard']({'disk':DEV,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
assert re.search(r'Filesystem state:\s*clean\s*$',run(['dumpe2fs','-h',PART]),re.M)
run(['e2fsck','-f','-n',PART])
approval=json.loads(debug('cat /var/lib/elasticpc/runtime/windows-direct-approved.json'))
assert approval['serial']=='S5SYNJ0NC06429H'
source=ROOT/'host/runtime/epc';dest='/usr/local/bin/epc'
debug('rm '+dest,True);debug('write '+str(source)+' '+dest,True);debug('sif '+dest+' mode 0100755',True)
fd=os.open(PART,os.O_RDWR)
try:os.fsync(fd)
finally:os.close(fd)
run(['e2fsck','-f','-n',PART])
# Targeted configuration/source check only, not a whole-device verification.
assert debug('cat '+dest)==source.read_text()
assert json.loads(debug('cat /var/lib/elasticpc/runtime/windows-direct-approved.json'))==approval
report={'status':'installed','file':dest,'sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'metadata_check':'passed','targeted_file_check':'passed','full_device_readback':False,'physical_usb_handoff_tested':False}
(ROOT/'reports/start-input-deployment.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
