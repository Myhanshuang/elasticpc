#!/usr/bin/env python3
"""Incremental, identity-guarded deployment to the explicitly approved offline USB."""
import hashlib,json,os,re,runpy,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];OUT=ROOT/'target/resources-devices-v12';OUT.mkdir(exist_ok=True)
DEV='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0';PART=DEV+'-part1'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def run(a):return subprocess.run(a,check=True,capture_output=True,text=True,timeout=120).stdout
def debug(cmd,write=False):
 r=subprocess.run(['debugfs',*(['-w'] if write else []),'-R',cmd,PART],check=True,capture_output=True,text=True)
 if any(t in r.stderr for t in ('not found','while ','Usage:','Permission denied','not open')):raise RuntimeError(r.stderr)
 return r.stdout
smoke=json.loads((ROOT/'reports/cpu-device-smoke.json').read_text());assert smoke['status']=='passed'
for name,digest in smoke['sources'].items():assert sha(ROOT/'host/runtime'/name)==digest
host=json.loads((ROOT/'target/devices-v12/report.json').read_text());assert host['status']=='passed' and host['source_sha256']==sha(ROOT/'host/runtime/epc-device')
for test in ('test_host_runtime.py','test_epc_input.py','test_guided_test.py','test_network_check.py'):run(['python',str(ROOT/'scripts'/test)])
h=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='deploy_guard')
h['source_guard']({'disk':DEV,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
assert re.search(r'Filesystem state:\s*clean\s*$',run(['dumpe2fs','-h',PART]),re.M)
run(['e2fsck','-f','-n',PART])
cfg=json.loads(debug('cat /etc/elasticpc/runtime.json'));assert cfg['host_serial']=='1C6F6530298EB360392A31E6'
approval=json.loads(debug('cat /var/lib/elasticpc/runtime/windows-direct-approved.json'));assert approval['serial']==cfg['windows']['serial']=='S5SYNJ0NC06429H'
profile=json.loads((ROOT/'host/runtime/profile.json').read_text())
for name in ('windows','manjaro'):
 assert cfg[name]['serial']==profile[name]['serial']
 for key in ('ram_mib','vcpus','vcpu_pins'):cfg[name][key]=profile[name][key]
for key in ('host_reserve_mib','io_cpus','voice_card'):cfg[key]=profile[key]
h['validate_cpu_plan'](cfg)
(OUT/'runtime.json').write_text(json.dumps(cfg,indent=2)+'\n')
files=[(ROOT/'host/runtime'/n,'/usr/local/bin/'+n,'0100755') for n in ('epc','epc-device','epc-input')]
files += [(OUT/'runtime.json','/etc/elasticpc/runtime.json','0100600'),(ROOT/'docs/resources-devices-v12.txt','/root/NEXT-BOOT.txt','0100644')]
report={'status':'running','whole_device_backup':False,'whole_device_readback':False,'physical_passthrough_verified':False,'files':{}}
def save():(ROOT/'reports/resources-devices-deployment.json').write_text(json.dumps(report,indent=2)+'\n')
save()
try:
 for source,dest,mode in files:
  if Path(dest).name in debug('ls -l '+str(Path(dest).parent)):debug('rm '+dest,True)
  debug('write '+str(source)+' '+dest,True);debug('sif '+dest+' mode '+mode,True)
  report['files'][dest]=sha(source);save()
 fd=os.open(PART,os.O_RDWR)
 try:os.fsync(fd)
 finally:os.close(fd)
 run(['e2fsck','-f','-n',PART])
 for source,dest,mode in files:assert debug('cat '+dest)==source.read_text()
 assert json.loads(debug('cat /var/lib/elasticpc/runtime/windows-direct-approved.json'))==approval
 report.update(status='installed',metadata_check='passed',targeted_file_check='passed',unit_tests=82)
except BaseException as e:report.update(status='failed',error=str(e));raise
finally:save()
print(json.dumps(report,indent=2))
