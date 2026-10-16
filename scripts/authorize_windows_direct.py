#!/usr/bin/env python3
"""Apply explicit owner direct-write authorization to the offline Kingston only."""
import hashlib,json,os,re,runpy,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
DEV='/dev/disk/by-id/usb-Kingston_DataTraveler_2.0_1C6F6530298EB360392A31E6-0:0'
PART=DEV+'-part1';BASE='/var/lib/elasticpc/runtime'
OUT=ROOT/'target/windows-direct-authorization';OUT.mkdir(exist_ok=True)
def run(args):return subprocess.run(args,check=True,text=True,capture_output=True,timeout=120).stdout
def debug(cmd,write=False):
 r=subprocess.run(['debugfs',*(['-w'] if write else []),'-R',cmd,PART],check=True,text=True,capture_output=True)
 if any(t in r.stderr for t in ('not found','while ','Usage:','Permission denied','not open')):raise RuntimeError(r.stderr)
 return r.stdout
def put(name,dest,data):
 p=OUT/name;p.write_text(json.dumps(data,indent=2)+'\n')
 if Path(dest).name in debug('ls -l '+str(Path(dest).parent)):debug('rm '+dest,True)
 debug('write '+str(p)+' '+dest,True);debug('sif '+dest+' mode 0100600',True)
helpers=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='authorize_only')
helpers['source_guard']({'disk':DEV,'serial':'1C6F6530298EB360392A31E6','bytes':15483273216})
assert re.search(r'Filesystem state:\s*clean\s*$',run(['dumpe2fs','-h',PART]),re.M)
run(['e2fsck','-f','-n',PART])
cfg=json.loads(debug('cat /etc/elasticpc/runtime.json'))
assert cfg['host_serial']=='1C6F6530298EB360392A31E6'
assert cfg['write_policy']=='validated-drivers-installed-native'
w=cfg['windows'];assert w['serial']=='S5SYNJ0NC06429H'
helpers['source_guard'](w)
with open(w['disk'],'rb') as f:fingerprint=hashlib.sha256(f.read(1048576)).hexdigest()
state=json.loads(debug('cat '+BASE+'/windows-latest.json'))
assert state['source_header_sha256']==fingerprint and state['serial']==w['serial']
session=state['session'];assert Path(session).parent==Path(BASE) and Path(session).name.startswith('windows-trial-')
assert state['boot_id']!=Path('/proc/sys/kernel/random/boot_id').read_text().strip()
assert state['status'] in ('running','saving','interrupted','stopped')
assert state.get('mode')=='trial'
# Confirm the old session cannot still own QEMU or its transient service.
for unit in ('elasticpc-windows','elasticpc-console-windows'):
 r=subprocess.run(['systemctl','is-active',unit],text=True,capture_output=True)
 assert r.returncode in (3,4) and r.stdout.strip() in ('inactive','unknown'),r.stdout
for proc in Path('/proc').iterdir():
 if not proc.name.isdigit():continue
 try:cmd=(proc/'cmdline').read_bytes().split(b'\0')
 except FileNotFoundError:continue
 except PermissionError:raise RuntimeError('Cannot verify process ownership')
 if cmd and b'qemu-system' in Path(os.fsdecode(cmd[0])).name.encode():
  assert not any(session.encode() in arg or b'elasticpc-windows' in arg for arg in cmd),'Windows QEMU still alive'
# Record authorization truthfully: no claim that clean shutdown or drivers passed.
approval={'serial':w['serial'],'source_header_sha256':fingerprint,'authorization':'explicit-owner-request-2026-10-16','trial':session,'user_attested_boot_and_drivers':False,'clean_shutdown_verified':False,'trial_changes_merged':False,'force_stop_risk_acknowledged':True,'user_reported_windows_validation':'passed'}
put('windows-direct-approved.json',BASE+'/windows-direct-approved.json',approval)
put('previous-state.json',session+'/pre-direct-authorization.json',state)
state.update(status='interrupted',archive='incomplete',reason='Previous USB Host boot ended; owner authorized future direct writes')
put('state.json',session+'/state.json',state);put('latest.json',BASE+'/windows-latest.json',state)
listing=debug('ls -l '+BASE)
removed=[]
for name in ('windows-last-error.json','windows-last-error.tmp','manjaro-last-error.json','manjaro-last-error.tmp','windows-console.json','manjaro-console.json'):
 if name in listing:debug('rm '+BASE+'/'+name,True);removed.append(name)
fd=os.open(PART,os.O_RDWR)
try:os.fsync(fd)
finally:os.close(fd)
run(['e2fsck','-f','-n',PART])
report={'status':'completed','approval':approval,'removed_current_state_files':removed,'old_trial_logs_retained':True,'windows_disk_written':False,'whole_device_readback':False,'metadata_check':'passed','start_command':'epc start windows --mode direct --gpu'}
(ROOT/'reports/windows-direct-authorization.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
