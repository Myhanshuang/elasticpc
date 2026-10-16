#!/usr/bin/env python3
import importlib.machinery,importlib.util,json,tempfile,unittest
from pathlib import Path
from unittest.mock import patch
loader=importlib.machinery.SourceFileLoader('epc_input',str(Path(__file__).resolve().parents[1]/'host/runtime/epc-device'))
spec=importlib.util.spec_from_loader(loader.name,loader);m=importlib.util.module_from_spec(spec);loader.exec_module(m)
class InputTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.base=Path(self.tmp.name);self.session=self.base/'windows-trial-test';self.session.mkdir();self.ids=set();self.calls=[];self.pending=False
  for key in ('BASE','RUN'):
   p=patch.object(m,key,self.base);p.start();self.addCleanup(p.stop)
  sleeper=patch.object(m.time,'sleep');sleeper.start();self.addCleanup(sleeper.stop)
  self.devices=[{'id':'epc-input-'+n,'hostbus':1,'hostaddr':i,'vid':'046d','product_id':'c08b','name':n,'selector':n+'@1-'+str(i),'port':'1-'+str(i)} for n,i in [('mouse',5),('keyboard',7)]]
  self.epc={'qmp':self.qmp,'atomic':lambda p,v:p.write_text(json.dumps(v))}
  self.state={'os':'windows','status':'running','session':str(self.session),'pid':12345}
 def qmp(self,sock,cmd,args=None):
  self.calls.append((cmd,args))
  if cmd=='query-status':return {'status':'running'}
  if cmd=='qom-list':return [{'name':i} for i in self.ids]
  if cmd=='qom-get':return args['path'].split('/')[-1] in self.ids
  if cmd=='device_add':self.ids.add(args['id'])
  if cmd=='device_del' and not self.pending:self.ids.discard(args['id'])
  return {}
 def test_pair_give_and_release_keeps_host_input(self):
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True),patch.object(m,'restore_hid') as restore:
   m.give(self.epc,self.state,['mouse','keyboard']);lease=json.loads((self.base/'input-lease.json').read_text())
   self.assertEqual(len(self.ids),2);self.assertEqual(lease['state'],'assigned')
   self.assertTrue(m.release(self.epc,lease));restore.assert_called_once_with(self.devices)
   self.assertFalse((self.base/'input-lease.json').exists())
  self.assertEqual([x[1]['driver'] for x in self.calls if x[0]=='device_add'],['usb-host','usb-host'])
 def test_missing_laptop_rescue_refuses_before_usb_claim(self):
  with patch.object(m,'rescue_present',return_value=False):
   with self.assertRaises(RuntimeError):m.give(self.epc,self.state,['mouse','keyboard'])
  self.assertEqual(self.calls,[])
 def test_existing_lease_refuses_second_owner(self):
  (self.base/'input-lease.json').write_text('{}')
  with patch.object(m,'rescue_present',return_value=True):
   with self.assertRaises(RuntimeError):m.give(self.epc,self.state,['mouse','keyboard'])
  self.assertEqual(self.calls,[])
 def test_pending_delete_never_rebinds_or_clears_lease(self):
  self.ids={'epc-input-mouse'};self.pending=True
  lease={'session':str(self.session),'devices':self.devices};(self.base/'input-lease.json').write_text(json.dumps(lease))
  with patch.object(m,'restore_hid') as restore:
   self.assertFalse(m.release(self.epc,lease));restore.assert_not_called()
  self.assertTrue((self.base/'input-lease.json').exists())
 def test_second_device_failure_rolls_back_first(self):
  original=self.qmp
  def fail(sock,cmd,args=None):
   if cmd=='device_add' and args['id'].endswith('keyboard'):raise RuntimeError('claim failed')
   return original(sock,cmd,args)
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True),patch.object(m,'restore_hid'):
   with self.assertRaises(RuntimeError):m.give(dict(self.epc,qmp=fail),self.state,['mouse','keyboard'])
  self.assertEqual(self.ids,set());self.assertFalse((self.base/'input-lease.json').exists())
 def test_names_match_exactly_and_reject_ambiguity(self):
  items=[dict(self.devices[0],name='Gaming Mouse',selector='Gaming Mouse@1-5'),dict(self.devices[1],name='Gaming Mouse',selector='Gaming Mouse@1-7')]
  with self.assertRaisesRegex(RuntimeError,'Ambiguous'):m.select(items,['Gaming Mouse'])
  self.assertEqual(m.select(items,['Gaming Mouse@1-7']),[items[1]])
  with self.assertRaisesRegex(RuntimeError,'not found'):m.select(items,['Gaming'])
  with self.assertRaisesRegex(RuntimeError,'twice'):m.select(items,['Gaming Mouse@1-5','Gaming Mouse@1-5'])
  with self.assertRaisesRegex(RuntimeError,'Specify'):m.select(items,[])
 def test_selective_release_and_incremental_assignment(self):
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True),patch.object(m,'restore_hid') as restore:
   m.give(self.epc,self.state,['mouse']);m.give(self.epc,self.state,['keyboard'])
   lease=json.loads((self.base/'input-lease.json').read_text())
   self.assertTrue(m.release(self.epc,lease,['mouse']))
   self.assertEqual(self.ids,{'epc-input-keyboard'});restore.assert_called_once_with([self.devices[0]])
   remaining=json.loads((self.base/'input-lease.json').read_text())
   self.assertEqual([d['name'] for d in remaining['devices']],['keyboard'])
 def test_failed_append_preserves_previous_assignment(self):
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True),patch.object(m,'restore_hid'):
   m.give(self.epc,self.state,['mouse'])
   original=self.qmp
   def fail(sock,cmd,args=None):
    if cmd=='device_add':raise RuntimeError('new device unavailable')
    return original(sock,cmd,args)
   with self.assertRaises(RuntimeError):m.give(dict(self.epc,qmp=fail),self.state,['keyboard'])
   self.assertEqual(self.ids,{'epc-input-mouse'})
   lease=json.loads((self.base/'input-lease.json').read_text())
   self.assertEqual(lease['state'],'assigned');self.assertEqual([d['name'] for d in lease['devices']],['mouse'])
 def test_pending_delete_retry_does_not_repeat_request(self):
  self.ids={'epc-input-mouse'};self.pending=True
  lease={'session':str(self.session),'devices':[self.devices[0]]}
  with patch.object(m,'restore_hid'):
   self.assertFalse(m.release(self.epc,lease));self.assertFalse(m.release(self.epc,lease))
   self.assertEqual(len([c for c in self.calls if c[0]=='device_del']),1)
   self.ids.clear()
   self.assertTrue(m.release(self.epc,lease))
 def test_non_hid_device_is_refused(self):
  sys=self.base/'sys';d=sys/'bus/usb/devices/1-1';d.mkdir(parents=True)
  for n,v in [('idVendor','046d'),('idProduct','c08b'),('serial','195F31693138'),('product','Fake Storage'),('busnum','1'),('devnum','2')]: (d/n).write_text(v)
  i=d.parent/'1-1:1.0';i.mkdir();(i/'bInterfaceClass').write_text('08')
  with patch.object(m,'SYS',sys):
   with self.assertRaisesRegex(RuntimeError,'USB HID/audio'):m.select(m.discover(),['Fake Storage'])
 def test_qom_object_without_attachment_is_not_success(self):
  original=self.qmp
  def detached(sock,cmd,args=None):
   if cmd=='qom-get':return False
   return original(sock,cmd,args)
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True),patch.object(m,'restore_hid'):
   with self.assertRaisesRegex(RuntimeError,'not attached'):m.give(dict(self.epc,qmp=detached),self.state,['mouse'])
   self.assertFalse(self.ids);self.assertTrue((self.session/'device-last-error.json').exists())
 def test_no_reset_compatibility_flags(self):
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True):
   m.give(self.epc,self.state,['mouse'],True)
   props=next(a for c,a in self.calls if c=='device_add')
   self.assertFalse(props['guest-reset']);self.assertFalse(props['guest-resets-all'])
 def test_cross_guest_lease_blocks_claim(self):
  m.lease_path('manjaro').write_text(json.dumps({'devices':[self.devices[0]]}))
  with patch.object(m,'discover',return_value=self.devices),patch.object(m,'rescue_present',return_value=True):
   with self.assertRaisesRegex(RuntimeError,'another Guest'):m.give(self.epc,self.state,['mouse'])
  self.assertFalse(self.ids)
 def test_composite_audio_hid_allowed_storage_denied(self):
  sys=self.base/'sys';d=sys/'bus/usb/devices/1-1';d.mkdir(parents=True)
  for n,v in [('idVendor','35d8'),('idProduct','012e'),('product','USB Sound'),('busnum','1'),('devnum','2')]: (d/n).write_text(v)
  for index,klass in enumerate(['01','01','03']):
   i=d.parent/('1-1:1.'+str(index));i.mkdir();(i/'bInterfaceClass').write_text(klass)
  with patch.object(m,'SYS',sys):
   self.assertEqual(m.select(m.discover(),['USB Sound'])[0]['kind'],'usb-audio')
   (i/'bInterfaceClass').write_text('08')
   with self.assertRaises(RuntimeError):m.select(m.discover(),['USB Sound'])
if __name__=='__main__':unittest.main()
