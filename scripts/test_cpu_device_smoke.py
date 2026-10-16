#!/usr/bin/env python3
"""Two disposable, diskless/networkless TCG machines; verifies real thread affinity."""
import json,os,runpy,subprocess,tempfile,time,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
e=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='smoke')
cfg=json.loads((ROOT/'host/runtime/profile.json').read_text())
e['validate_cpu_plan'](cfg)
binary=ROOT/'.elasticpc/runtime/root/usr/bin/qemu-system-x86_64'
env={**os.environ,'LD_LIBRARY_PATH':str(ROOT/'.elasticpc/runtime/root/usr/lib')}
report={'status':'failed','accelerator':'tcg','real_disks':False,'network':False,'physical_devices':False,'guests':{}}
children=[]
try:
 with tempfile.TemporaryDirectory(prefix='epc-pin-',dir='/tmp') as tmp:
  for name in ('windows','manjaro'):
   p=cfg[name];session=Path(tmp)/name;session.mkdir();log=(session/'qemu.log').open('w')
   args=[str(binary),'-no-user-config','-nodefaults','-machine','q35','-accel','tcg,thread=multi','-cpu','max','-smp',f"{p['vcpus']},sockets=1,cores={p['vcpus']//2},threads=2",'-m','128','-S','-display','none','-monitor','none','-serial','none','-nic','none','-L',str(ROOT/'.elasticpc/runtime/firmware'),'-device','qemu-xhci','-device','usb-mouse,id=testmouse','-qmp',f'unix:{session}/qmp.sock,server=on,wait=off']
   previous=os.sched_getaffinity(0)
   try:
    os.sched_setaffinity(0,set(cfg['io_cpus']));child=subprocess.Popen(args,env=env,stdout=log,stderr=log);children.append((child,session,log))
   finally:os.sched_setaffinity(0,previous)
   time.sleep(1)
   assert child.poll() is None,(session/'qemu.log').read_text()
   result=e['pin_qemu'](child.pid,session,p,cfg['io_cpus'])
   tids={row['tid'] for row in result['vcpus']}
   for task in (Path('/proc')/str(child.pid)/'task').iterdir():
    if int(task.name) not in tids:assert os.sched_getaffinity(int(task.name))==set(cfg['io_cpus'])
   assert e['qmp'](session/'qmp.sock','qom-get',{'path':'/machine/peripheral/testmouse','property':'attached'}) is True
   report['guests'][name]=result
  report['status']='passed'
finally:
 for child,session,log in children:
  if child.poll() is None:
   try:e['qmp'](session/'qmp.sock','quit');child.wait(timeout=10)
   except Exception:child.kill();child.wait()
  log.close()
 report['sources']={n:hashlib.sha256((ROOT/'host/runtime'/n).read_bytes()).hexdigest() for n in ('epc','epc-device','profile.json')}
 (ROOT/'reports/cpu-device-smoke.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
