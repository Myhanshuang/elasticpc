#!/usr/bin/env python3
import importlib.machinery,importlib.util,unittest,json,tempfile,stat,types
from pathlib import Path
from unittest.mock import patch
loader=importlib.machinery.SourceFileLoader('epc_runtime',str(Path(__file__).resolve().parents[1]/'host/runtime/epc'))
spec=importlib.util.spec_from_loader(loader.name,loader);epc=importlib.util.module_from_spec(spec);loader.exec_module(epc)
class RuntimeGuards(unittest.TestCase):
 def test_discard_refuses_direct_and_saved_trials(self):
  with tempfile.TemporaryDirectory() as tmp,patch.object(epc,'RUN',Path(tmp)):
   for state in ({'mode':'direct'},{'mode':'trial','storage':'ram','archive':'saved'}):
    with self.assertRaises(RuntimeError):epc.request_discard(Path(tmp),state)
   self.assertEqual(list(Path(tmp).iterdir()),[])
 def test_discard_before_archive_never_opens_destination(self):
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp)/'session';session.mkdir();(session/'store').mkdir()
   (session/'store/trial.qcow2').write_bytes(b'important')
   with patch.object(epc,'RUN',Path(tmp)):
    epc.atomic(Path(tmp)/(session.name+'-discard.json'),{})
    with self.assertRaises(epc.DiscardTrial):epc.archive_ram(session)
   self.assertFalse((session/'trial-saving.qcow2').exists())
   self.assertEqual((session/'store/trial.qcow2').read_bytes(),b'important')
 def test_discard_during_flush_cancels_commit(self):
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp)/'session';session.mkdir();(session/'store').mkdir()
   (session/'store/trial.qcow2').write_bytes(b'x'*4096)
   real=epc.atomic
   def record(path,data):
    real(path,data)
    if data.get('phase')=='flushing':real(Path(tmp)/(session.name+'-discard.json'),{})
   with patch.object(epc,'RUN',Path(tmp)),patch.object(epc,'atomic',side_effect=record):
    with self.assertRaises(epc.DiscardTrial):epc.archive_ram(session)
   self.assertFalse((session/'trial-saved.qcow2').exists())
   self.assertFalse((session/'archive.json').exists())
 def test_discard_cleanup_removes_only_current_archive(self):
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp)/'session';session.mkdir()
   for name in ('trial-saving.qcow2','trial-saved.qcow2','qemu.log'):(session/name).write_bytes(b'x')
   other=Path(tmp)/'other-saved.qcow2';other.write_bytes(b'keep')
   state={'mode':'trial'}
   epc.discard_ram(session,state)
   self.assertEqual(state['archive'],'discarded')
   self.assertTrue((session/'qemu.log').exists());self.assertEqual(other.read_bytes(),b'keep')
   self.assertFalse((session/'trial-saving.qcow2').exists())
 def test_concurrent_mixed_guests_fit_but_overcommit_refused(self):
  epc.memory_budget(27648,21504,8192,6144,6144,0)
  epc.memory_budget(27648,13312,6144,8192,0,6144)
  with self.assertRaisesRegex(RuntimeError,'Combined'):epc.memory_budget(27648,27648,12288,8192,6144,0)
  with self.assertRaisesRegex(RuntimeError,'available'):epc.memory_budget(27648,10000,8192,0,6144,0)
 def test_stale_state_reconciles_without_deleting_logs(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);session=base/'windows-trial-old';session.mkdir()
   state={'session':str(session),'status':'saving','mode':'trial','archive':'saving'}
   (base/'windows-latest.json').write_text(json.dumps(state));(session/'qemu.log').write_text('keep')
   with patch.object(epc,'BASE',base),patch.object(epc,'RUN',base),patch.object(epc.subprocess,'run',return_value=types.SimpleNamespace(returncode=3)),patch.object(epc,'qmp',side_effect=OSError()):
    self.assertTrue(epc.reconcile_stale('windows'))
   fixed=json.loads((base/'windows-latest.json').read_text())
   self.assertEqual(fixed['status'],'interrupted');self.assertEqual(fixed['archive'],'incomplete')
   self.assertEqual((session/'qemu.log').read_text(),'keep')
   self.assertEqual(len(list(session.glob('pre-reconcile-*.json'))),1)
 def test_reconcile_never_clears_active_guest(self):
  with patch.object(epc.subprocess,'run',return_value=types.SimpleNamespace(returncode=0)),patch.object(epc.os,'open') as op:
   self.assertFalse(epc.reconcile_stale('windows'));op.assert_not_called()
 def test_stale_reconcile_refuses_live_qmp_even_without_service(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);session=base/'windows-trial-live';session.mkdir()
   state={'session':str(session),'status':'running','mode':'trial'}
   latest=base/'windows-latest.json';latest.write_text(json.dumps(state));before=latest.read_bytes()
   with patch.object(epc,'BASE',base),patch.object(epc,'RUN',base),patch.object(epc.subprocess,'run',return_value=types.SimpleNamespace(returncode=3)),patch.object(epc,'qmp',return_value={'status':'running'}):
    self.assertFalse(epc.reconcile_stale('windows'))
   self.assertEqual(latest.read_bytes(),before)
 def test_discard_while_saving_requests_cancel_without_killing_worker(self):
  import fcntl,os
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);session=base/'windows-trial-saving';session.mkdir()
   lock=os.open(base/'windows.lock',os.O_CREAT|os.O_RDWR,0o600);fcntl.flock(lock,fcntl.LOCK_EX)
   try:
    with patch.object(epc,'RUN',base),patch.object(epc,'control') as control:
     epc.request_discard(session,{'os':'windows','mode':'trial','storage':'ram','status':'saving','archive':'saving'})
     control.assert_not_called();self.assertTrue(epc.discard_requested(session))
   finally:os.close(lock)
 def test_virtio_network_uses_vhost_and_keeps_mac(self):
  profile={'vcpus':1,'ram_mib':512,'uuid':'test','tap':'epctest','mac':'52:54:00:00:00:01'}
  with patch.object(epc.os,'fstat',return_value=types.SimpleNamespace(st_mode=stat.S_IFREG)):
   old=epc.cmdline(profile,9,None,Path('/tmp/vars'),Path('/tmp/session'),'synthetic',{})
   new=epc.cmdline(dict(profile,net_model='virtio'),9,None,Path('/tmp/vars'),Path('/tmp/session'),'synthetic',{})
   offline=epc.cmdline(dict(profile,net_model='virtio'),9,None,Path('/tmp/vars'),Path('/tmp/session'),'synthetic',{},network_on=False)
  self.assertIn('e1000e,netdev=net,mac=52:54:00:00:00:01',old)
  self.assertIn('virtio-net-pci,netdev=net,mac=52:54:00:00:00:01',new)
  self.assertIn('vhost=on',new[new.index('-netdev')+1])
  self.assertNotIn('-netdev',offline)
 def test_full_trial_refuses_acpi_instead_of_waiting_forever(self):
  with patch.object(epc,'qmp') as q:
   with self.assertRaisesRegex(RuntimeError,'normal shutdown cannot run'):
    epc.control('stop',False,Path('/tmp/test'),{'os':'windows','status':'paused-space-limit'})
   q.assert_not_called()
 def test_stop_during_archive_does_not_kill_or_resend_acpi(self):
  with patch.object(epc,'qmp') as q,patch.object(epc.os,'pidfd_open',create=True) as kill:
   epc.control('stop',True,Path('/tmp/test'),{'os':'windows','status':'saving'})
   q.assert_not_called();kill.assert_not_called()
 def test_missing_overlay_cannot_be_marked_saved(self):
  with tempfile.TemporaryDirectory() as tmp:
   with self.assertRaisesRegex(RuntimeError,'missing'):epc.archive_ram(Path(tmp))
   self.assertFalse((Path(tmp)/'archive.json').exists())
 def test_sparse_archive_progress_and_shutdown_signal_protection(self):
  import hashlib,signal,os
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp)/'session';session.mkdir();(session/'store').mkdir();runtime=Path(tmp)/'run';runtime.mkdir()
   data=bytes(8*1024**2)+b'a'*(4*1024**2)
   source=session/'store/trial.qcow2';source.write_bytes(data)
   phases=[];received=[];original_atomic=epc.atomic;original_sync=os.fsync
   def record(path,value):
    if 'phase' in value:phases.append(value['phase'])
    original_atomic(path,value)
   def sync(fd):
    if os.readlink('/proc/self/fd/'+str(fd)).endswith('trial-saving.qcow2'):signal.raise_signal(signal.SIGTERM)
    original_sync(fd)
   handler=lambda *_:received.append('unexpected interrupt')
   previous=signal.signal(signal.SIGTERM,handler)
   try:
    with patch.object(epc,'RUN',runtime),patch.object(epc,'atomic',side_effect=record),patch.object(epc.os,'fsync',side_effect=sync):
     epc.archive_ram(session);progress=epc.archive_progress(session)
    self.assertIs(signal.getsignal(signal.SIGTERM),handler)
   finally:signal.signal(signal.SIGTERM,previous)
   self.assertEqual(received,[]);self.assertEqual(source.read_bytes(),data)
   saved=session/'trial-saved.qcow2';self.assertEqual(saved.read_bytes(),data)
   self.assertEqual(json.loads((session/'archive.json').read_text())['sha256'],hashlib.sha256(data).hexdigest())
   self.assertEqual(progress['zero_bytes_skipped'],8*1024**2)
   self.assertEqual(progress['written_bytes'],4*1024**2)
   self.assertEqual(progress['verified_bytes'],len(data))
   self.assertEqual(phases,['copying','flushing','verifying','committing','saved'])
 def test_host_rejects_native_root(self):
  def run(args):
   if args[0]=='findmnt':return '/dev/nvme0n1p1\n'
   return json.dumps({'blockdevices':[{'serial':'internal','size':1000}]})
  with patch.object(epc.os,'geteuid',return_value=0),patch.object(epc,'run',side_effect=run):
   with self.assertRaisesRegex(RuntimeError,'approved USB Host'):epc.host_guard({'host_serial':'usb','host_bytes':2000})
 def test_unprivileged_rejected_before_commands(self):
  with patch.object(epc.os,'geteuid',return_value=1000),patch.object(epc,'run') as run:
   with self.assertRaises(RuntimeError):epc.host_guard({})
   run.assert_not_called()
 def test_source_identity_rejected(self):
  with patch.object(epc,'disk_info',return_value=[{'serial':'different','size':2000}]):
   with self.assertRaisesRegex(RuntimeError,'identity'):epc.source_guard({'disk':'unused','serial':'expected','bytes':2000})
 def test_source_mounted_child_rejected(self):
  rows=[{'serial':'x','size':2000,'maj:min':'8:0','mountpoints':[None],'children':[{'maj:min':'8:1','mountpoints':['/other-namespace-root']}]}]
  with patch.object(epc,'disk_info',return_value=rows):
   with self.assertRaisesRegex(RuntimeError,'mounted'):epc.source_guard({'disk':'unused','serial':'x','bytes':2000})
 def test_trial_backing_readonly_and_direct_writable(self):
  profile={'vcpus':1,'ram_mib':512,'uuid':'9b341284-1284-41bf-b899-aa7154312b92'}
  for overlay,readonly in [(Path('/tmp/overlay'),True),(None,False)]:
   with patch.object(epc.os,'fstat',return_value=types.SimpleNamespace(st_mode=stat.S_IFREG)):
    args=epc.cmdline(profile,9,overlay,Path('/tmp/vars'),Path('/tmp/session'),'synthetic',{},network_on=False)
   blocks=[json.loads(args[i+1]) for i,x in enumerate(args) if x=='-blockdev']
   self.assertEqual(blocks[0]['filename'],'/proc/self/fd/9');self.assertEqual(blocks[0]['read-only'],readonly)
   self.assertIn('none',args);self.assertNotIn('-netdev',args)
   if overlay:self.assertEqual(next(x for x in blocks if x['node-name']=='trial')['backing'],'native')
 def test_physical_block_uses_host_device(self):
  with patch.object(epc.os,'fstat',return_value=types.SimpleNamespace(st_mode=stat.S_IFBLK)):
   args=epc.cmdline({'vcpus':1,'ram_mib':512,'uuid':'test'},9,Path('/tmp/cow'),Path('/tmp/vars'),Path('/tmp/session'),'synthetic',{},network_on=False)
  block=json.loads(args[args.index('-blockdev')+1]);self.assertEqual(block['driver'],'host_device');self.assertTrue(block['read-only'])
 def test_windows_missing_hiberfile_is_clean(self):
  with patch.object(epc,'run',side_effect=['Volume Flags: 0x0000','Windows\nUsers\npagefile.sys\n']):epc.windows_clean('/test')
 def test_windows_hiberfile_refused(self):
  with patch.object(epc,'run',side_effect=['Volume Flags: 0x0000','Windows\nHIBERFIL.SYS\n']):
   with self.assertRaisesRegex(RuntimeError,'hibernation'):epc.windows_clean('/test')
 def test_windows_failed_listing_refused(self):
  with patch.object(epc,'run',side_effect=['Volume Flags: 0x0000',RuntimeError('read error')]):
   with self.assertRaisesRegex(RuntimeError,'read error'):epc.windows_clean('/test')
 def test_windows_dirty_refused(self):
  with patch.object(epc,'run',return_value='Volume Flags: 0x0001'):
   with self.assertRaisesRegex(RuntimeError,'clean'):epc.windows_clean('/test')
 def test_preflight_failure_is_persisted(self):
  import sys,types
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);conf=base/'config.json';conf.write_text('{}')
   with patch.object(epc,'BASE',base),patch.object(epc,'RUN',base),patch.object(epc,'CONF',conf),patch.object(epc,'host_guard'),patch.object(epc,'execute',side_effect=RuntimeError('test preflight rejection')),patch.object(sys,'argv',['epc','_run','windows']):
    with self.assertRaisesRegex(RuntimeError,'preflight'):epc.main()
   error=json.loads((base/'windows-last-error.json').read_text());self.assertEqual(error['status'],'failed');self.assertIn('preflight',error['error'])
 def test_force_controls_and_same_session_reset(self):
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp);state={'os':'windows','status':'running'}
   with patch.object(epc,'qmp',return_value={'status':'running'}) as q:
    epc.control('restart',True,session,state);self.assertEqual(q.call_args.args[1],'system_reset')
    epc.control('stop',True,session,state);self.assertEqual(q.call_args.args[1],'quit')
   self.assertTrue((session/'forced-stop.json').exists())
 def test_restart_requires_force_and_refuses_io_error(self):
  with tempfile.TemporaryDirectory() as tmp:
   with self.assertRaisesRegex(RuntimeError,'--force'):epc.control('restart',False,Path(tmp),{'os':'manjaro','status':'running'})
   with patch.object(epc,'qmp',return_value={'status':'io-error'}):
    with self.assertRaisesRegex(RuntimeError,'storage'):epc.control('restart',True,Path(tmp),{'os':'manjaro','status':'running'})
 def test_password_available_without_qmp(self):
  import sys,io,contextlib
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);conf=base/'cfg';conf.write_text('{}');session=base/'session';session.mkdir();(session/'console-password.txt').write_text('testpass')
   epc.atomic(base/'windows-latest.json',{'session':str(session)});epc.atomic(base/'windows-last-error.json',{'error':'old error'})
   with patch.object(epc,'BASE',base),patch.object(epc,'RUN',base),patch.object(epc,'CONF',conf),patch.object(epc,'host_guard'),patch.object(epc,'qmp') as q,patch.object(sys,'argv',['epc','password','windows']),contextlib.redirect_stdout(io.StringIO()) as output:
    epc.main();self.assertIn('testpass',output.getvalue());q.assert_not_called()
 def test_manjaro_disk_and_swap_profile(self):
  with patch.object(epc.os,'fstat',return_value=types.SimpleNamespace(st_mode=stat.S_IFREG)):
   args=epc.cmdline({'vcpus':1,'ram_mib':512,'uuid':'test'},9,Path('/tmp/cow'),Path('/tmp/vars'),Path('/tmp/session'),'manjaro',{'manjaro':{'root_uuid':'test'}},network_on=False)
  self.assertTrue(any(x.startswith('virtio-blk-pci,') for x in args));self.assertIn('systemd.swap=no',args[args.index('-append')+1]);self.assertIn('svm=off',args[args.index('-cpu')+1])
 def test_ram_archive_failure_preserves_source(self):
  with tempfile.TemporaryDirectory() as tmp:
   session=Path(tmp);(session/'store').mkdir();source=session/'store/trial.qcow2';source.write_bytes(b'preserve')
   with patch.object(epc.shutil,'disk_usage',return_value=types.SimpleNamespace(free=0)):
    with self.assertRaisesRegex(RuntimeError,'RAM copy retained'):epc.archive_ram(session)
   self.assertEqual(source.read_bytes(),b'preserve');self.assertFalse((session/'trial-saved.qcow2').exists())
 def test_console_repeat_reuses_service_and_prints_password(self):
  import sys,io,contextlib
  with tempfile.TemporaryDirectory() as tmp:
   base=Path(tmp);conf=base/'cfg';conf.write_text(json.dumps({'windows':{'vnc':1}}));session=base/'session';session.mkdir();(session/'console-password.txt').write_text('repeatpass');epc.atomic(base/'windows-latest.json',{'session':str(session)})
   active=False;starts=[]
   def run(args):
    nonlocal active
    if args[0]=='tailscale':return '100.64.1.1\n'
    if args[0]=='systemd-run':active=True;starts.append(args)
    return ''
   def subprocess_run(args,**kw):return types.SimpleNamespace(returncode=(0 if active else 3) if args[0]=='systemctl' else 0)
   with patch.object(epc,'BASE',base),patch.object(epc,'RUN',base),patch.object(epc,'CONF',conf),patch.object(epc,'host_guard'),patch.object(epc,'qmp'),patch.object(epc,'run',side_effect=run),patch.object(epc.subprocess,'run',side_effect=subprocess_run),patch.object(sys,'argv',['epc','console','windows']),contextlib.redirect_stdout(io.StringIO()) as output:
    epc.main();epc.main();self.assertEqual(output.getvalue().count('repeatpass'),2);self.assertEqual(len(starts),1);self.assertIn('host=100.64.1.1&port=6081&encrypt=false&path=websockify',output.getvalue())
 def test_start_input_preflight_rejects_wrong_guest_and_missing_name(self):
  with self.assertRaisesRegex(RuntimeError,'Unsupported Guest'):epc.validate_input_names('other',['mouse'])
  with patch.object(epc.runpy,'run_path',return_value={'discover':lambda:[], 'select':lambda d,n: (_ for _ in ()).throw(RuntimeError('missing'))}):
   with self.assertRaisesRegex(RuntimeError,'missing'):epc.validate_input_names('windows',['mouse'])
 def test_start_input_failure_preserves_running_guest(self):
  with tempfile.TemporaryDirectory() as tmp,patch.object(epc,'BASE',Path(tmp)),patch.object(epc,'run',side_effect=RuntimeError('USB unplugged')):
   state={'status':'running','session':tmp}
   epc.attach_start_inputs(state,['mouse'])
   self.assertEqual(state['status'],'running');self.assertEqual(state['input_status'],'failed')
   self.assertIn('USB unplugged',state['input_error'])
 def test_start_input_success_passes_names_as_separate_arguments(self):
  with tempfile.TemporaryDirectory() as tmp,patch.object(epc,'BASE',Path(tmp)),patch.object(epc,'run') as run:
   state={'status':'running','session':tmp}
   epc.attach_start_inputs(state,['A87 Keyboard','G502 HERO SE'])
   run.assert_called_once_with(['/usr/local/bin/epc-device','give','windows','A87 Keyboard','G502 HERO SE'])
   self.assertEqual(state['input_status'],'assigned')
 def test_16_10_budget_keeps_four_gib_host_reserve(self):
  epc.memory_budget(30885,30885,16384,10240,0,0,4096)
  with self.assertRaisesRegex(RuntimeError,'Combined'):epc.memory_budget(27648,27648,16384,10240,0,0,4096)
  with self.assertRaisesRegex(RuntimeError,'Combined'):epc.memory_budget(30885,30885,16384,10240,6144,0,4096)
  with self.assertRaisesRegex(RuntimeError,'available'):epc.memory_budget(30885,17000,16384,0,0,0,4096)
  with self.assertRaisesRegex(RuntimeError,'at least'):epc.memory_budget(30885,30885,16384,12288,0,0,2048)
 def test_host_reserve_includes_existing_host_usage(self):
  epc.memory_budget(30885,12600,10240,16384,0,0,4096)
 def test_cpu_plan_rejects_overlap_and_split_smt(self):
  import copy
  cfg=json.loads((Path(__file__).resolve().parents[1]/'host/runtime/profile.json').read_text())
  epc.validate_cpu_plan(cfg,online=range(16))
  bad=copy.deepcopy(cfg);bad['manjaro']['vcpu_pins'][0]=8
  with self.assertRaisesRegex(RuntimeError,'overlapping'):epc.validate_cpu_plan(bad,online=range(16))
  bad=copy.deepcopy(cfg);bad['windows']['vcpu_pins'][1:3]=[4,3]
  with self.assertRaisesRegex(RuntimeError,'SMT'):epc.validate_cpu_plan(bad,online=range(16))
  with self.assertRaisesRegex(RuntimeError,'Offline'):epc.validate_cpu_plan(cfg,online=range(14))
 def test_voice_card_refuses_shared_group_and_busy_host(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=Path(tmp);b='0000:65:00.6';p=root/'bus/pci/devices'/b;p.mkdir(parents=True)
   for name,value in [('vendor','0x1022'),('device','0x15e3'),('class','0x040300'),('driver_override','')]: (p/name).write_text(value)
   group=p/'iommu_group/devices';group.mkdir(parents=True);(group/b).mkdir();(group/'0000:65:00.5').mkdir()
   (root/'class/sound').mkdir(parents=True)
   cfg={'voice_card':{'bdf':b,'vendor':'0x1022','device':'0x15e3'}}
   with patch.object(epc,'run') as run:
    with self.assertRaisesRegex(RuntimeError,'isolated'):epc.voice_card_prepare(cfg,root,root,root)
    run.assert_not_called();self.assertFalse((root/'voice-card-restore.json').exists())
   (group/'0000:65:00.5').rmdir();(root/'class/sound/controlC3').symlink_to(p/'sound/controlC3')
   (root/'snd').mkdir();(root/'snd/controlC3').touch()
   with patch.object(epc.subprocess,'run',return_value=types.SimpleNamespace(returncode=0)),patch.object(epc,'run') as run:
    with self.assertRaisesRegex(RuntimeError,'using voice'):epc.voice_card_prepare(cfg,root,root,root)
    run.assert_not_called()
 def test_atomic_state_roundtrip(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp)/'s.json';epc.atomic(p,{'status':'paused'});self.assertEqual(json.loads(p.read_text()),{'status':'paused'})
if __name__=='__main__':unittest.main()
