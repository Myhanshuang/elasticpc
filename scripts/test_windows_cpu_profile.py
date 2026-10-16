#!/usr/bin/env python3
import json,os,runpy,subprocess,tempfile,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];epc=runpy.run_path(str(ROOT/'host/runtime/epc'),run_name='cpu_test');rt=ROOT/'.elasticpc/runtime'
with tempfile.TemporaryDirectory() as t:
 sock=Path(t)/'qmp.sock';cpu=epc['cpu_profile']('windows');env={**os.environ,'LD_LIBRARY_PATH':str(rt/'root/usr/lib')}
 cmd=[str(rt/'root/usr/bin/qemu-system-x86_64'),'-no-user-config','-nodefaults','-machine','q35,accel=kvm','-cpu',cpu,'-m','256','-display','none','-monitor','none','-nic','none','-S','-qmp','unix:'+str(sock)+',server=on,wait=off','-L',str(rt/'firmware')]
 p=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=env)
 try:
  for _ in range(100):
   if p.poll() is not None:raise RuntimeError(p.stderr.read().decode())
   try:epc['qmp'](sock,'query-status');break
   except (OSError,ValueError):time.sleep(.1)
  model=epc['qmp'](sock,'query-cpu-model-expansion',{'type':'full','model':{'name':'host','props':{'svm':False,'vmx':False,'hv-relaxed':True,'hv-vapic':True,'hv-time':True}}})['model']['props']
  assert model['svm']==False and model['vmx']==False and model['hv-time']==True
  epc['qmp'](sock,'quit');assert p.wait(timeout=10)==0
 finally:
  if p.poll() is None:p.kill();p.wait()
 report={'status':'passed','cpu_profile':cpu,'nested_svm':False,'nested_vmx':False,'windows_boot_verified':False}
 (ROOT/'target/host-repair-v5/windows-cpu-report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
