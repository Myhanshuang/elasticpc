#!/usr/bin/env python3
"""Authorized private KVM/libvirt lab; no system socket, network, or host disks.
All generated domains and files belong to one fresh private namespace. Protocol
waits are event driven; there is no recurring task-status polling.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import uuid
from xml.sax.saxutils import escape

PROJECT = Path(__file__).resolve().parents[1]

def run(args, env=None, timeout=30, check=True):
    return subprocess.run([str(a) for a in args], env=env, timeout=timeout, capture_output=True, text=True, check=check)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def archive(entries):
    result = bytearray()
    for ino, (name, mode, data, major, minor) in enumerate(entries + [('TRAILER!!!', 0, b'', 0, 0)], 1):
        fields = [ino, mode, 0, 0, 1, 0, len(data), 0, 0, major, minor, len(name)+1, 0]
        result += b'070701' + ''.join(f'{n:08x}' for n in fields).encode() + name.encode() + b'\0'
        result += b'\0' * (-len(result) % 4)
        result += data
        result += b'\0' * (-len(result) % 4)
    return result


class Lab:
    def __init__(self, runtime, report):
        self.root = Path(tempfile.mkdtemp(prefix='elasticpc-private-'))
        self.runtime, self.output = runtime, report
        self.bin = runtime/'root/usr/bin'
        self.ids = [str(uuid.uuid4()), str(uuid.uuid4())]
        self.tokens = {u:uuid.uuid4().hex for u in self.ids}
        self.daemon = None
        self.defined = []
        self.serial = {}
        self.logs = {}
        self.report = {'schema_version':1, 'status':'failed', 'checks':[], 'private_root':str(self.root), 'physical_device_writes':False, 'host_services_modified':False, 'evidence_kind':'real_private_libvirt_kvm_and_sqlite', 'limitations':['Private example backend only; not production host inventory or physical resource leases', 'Process crash tests are not host power-loss or kernel-crash tests']}
        for d in ['run/libvirt','config/libvirt','cache','data','logs']:
            (self.root/d).mkdir(parents=True,exist_ok=True)
        self.env = {'PATH':str(self.bin)+':/usr/bin:/bin','LC_ALL':'C','LD_LIBRARY_PATH':str(runtime/'root/usr/lib'), 'XDG_RUNTIME_DIR':str(self.root/'run'), 'XDG_CONFIG_HOME':str(self.root/'config'), 'XDG_CACHE_HOME':str(self.root/'cache'), 'XDG_DATA_HOME':str(self.root/'data'), 'LIBVIRT_DRIVER_DIR':str(runtime/'root/usr/lib/libvirt/connection-driver'), 'LIBVIRT_AUTOSTART':'0'}
        self.uri = 'qemu+unix:///session?socket='+str(self.root/'run/libvirt/virtqemud-sock')+'&no_autostart=1'
    def check(self, name, value, detail=''):
        self.report['checks'].append({'name':name,'passed':bool(value),'detail':detail})
        if not value: raise AssertionError(name+': '+detail)
    def virsh(self,*args,check=True):
        return run([self.bin/'virsh','-c',self.uri,*args],self.env,check=check)
    def worker(self,action,u,op='unused',crash=None,check=True):
        args=[PROJECT/'target/debug/examples/private_controller',self.root/'manifest.json',action,u,op]
        if crash: args.append(crash)
        result=run(args,self.env,check=False)
        self.report.setdefault('controller_steps',[]).append({'action':action,'uuid':u,'operation':op,'crash':crash,'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr})
        if check:result.check_returncode()
        return result
    def prepare(self):
        available=int(next(l.split()[1] for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:')))//1024
        self.check('memory_budget',available >= 9216,str(available)+' MiB available; 8192 reserve + 2*(256 guest + 256 overhead)')
        self.check('non_root',os.geteuid()!=0)
        shutil.copyfile('/boot/vmlinuz-6.12-x86_64',self.root/'kernel')
        module=Path('/usr/lib/modules/6.12.108-1-MANJARO/kernel/drivers/block/virtio_blk.ko.zst')
        result=subprocess.run(['/usr/bin/zstd','-dc',str(module)],check=True,capture_output=True)
        (self.root/'virtio_blk.ko').write_bytes(result.stdout)
        (self.root/'base.raw').write_bytes(b'\x11'*(8*1024*1024))
        (self.root/'base.raw').chmod(0o400)
        self.base_hash=digest(self.root/'base.raw')
        self.report['inputs']={'kernel_sha256':digest(self.root/'kernel'),'module_sha256':digest(self.root/'virtio_blk.ko'),'base_sha256':self.base_hash,'qemu_version':run([self.bin/'qemu-system-x86_64','--version'],self.env).stdout}
        for name in ['bios-256k.bin','linuxboot_dma.bin','kvmvapic.bin']:
            shutil.copyfile(self.runtime/'firmware'/name,self.root/name)
        for u in self.ids:
            source=(PROJECT/'crates/elastic-lab/assets/file-init.c').read_text().replace('@TOKEN@',self.tokens[u])
            (self.root/'init.c').write_text(source)
            run(['/usr/bin/cc','-static','-Os','-o',self.root/'init',self.root/'init.c'])
            data=archive([('dev',0o040755,b'',0,0),('dev/console',0o020600,b'',5,1),('init',0o100755,(self.root/'init').read_bytes(),0,0),('virtio_blk.ko',0o100400,result.stdout,0,0)])
            (self.root/(u+'.cpio')).write_bytes(data)
            run([self.bin/'qemu-img','create','-f','qcow2','-F','raw','-b',self.root/'base.raw',self.root/(u+'.qcow2')],self.env)
        (self.root/'config/libvirt/qemu.conf').write_text('security_driver = "none"\ndynamic_ownership = 0\nremember_owner = 0\ncgroup_controllers = []\nstdio_handler = "file"\n')
        conf=self.root/'config/libvirt/virtqemud.conf'
        conf.write_text('auth_unix_rw = "none"\nauth_unix_ro = "none"\nunix_sock_rw_perms = "0700"\nunix_sock_admin_perms = "0700"\n')
        with conf.open('a') as f:
            f.write('log_level = 1\nlog_outputs = "1:file:'+str(self.root/'logs/libvirt.log')+'"\n')
        # Provide systemd's documented socket activation protocol ourselves.
        # This does not contact systemd or any system/user service manager.
        listener=socket.socket(socket.AF_UNIX)
        listener.bind(str(self.root/'run/libvirt/virtqemud-sock'))
        os.chmod(self.root/'run/libvirt/virtqemud-sock',0o700)
        listener.listen(16)
        log=open(self.root/'logs/daemon.log','w')
        launcher="import os,sys; fd=int(sys.argv[1]); os.dup2(fd,3); os.set_inheritable(3,True); os.environ['LISTEN_PID']=str(os.getpid()); os.environ['LISTEN_FDS']='1'; os.environ['LISTEN_FDNAMES']='virtqemud.socket'; os.execve(sys.argv[2],sys.argv[2:],dict(os.environ))"
        self.daemon=subprocess.Popen(['/usr/bin/python3','-c',launcher,str(listener.fileno()),str(self.bin/'virtqemud'),'-f',str(conf),'-p',str(self.root/'daemon.pid'),'-t','300'],env=self.env,stdout=log,stderr=log,pass_fds=(listener.fileno(),))
        listener.close();log.close()
        self.check('empty_private_instance',not self.virsh('list','--all','--uuid').stdout.strip())
        for u in self.ids:
            xml=f'''<domain type="kvm" xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
<name>elasticpc-{u}</name><uuid>{u}</uuid><memory unit="MiB">256</memory><vcpu>1</vcpu>
<os><type arch="x86_64" machine="pc">hvm</type><kernel>{self.root}/kernel</kernel><initrd>{self.root}/{u}.cpio</initrd><cmdline>console=ttyS0,115200 rdinit=/init panic=-1 quiet nokaslr</cmdline></os>
<features><acpi/><apic/></features><cpu mode="custom" match="exact"><model fallback="forbid">qemu64</model></cpu>
<on_poweroff>destroy</on_poweroff><on_reboot>destroy</on_reboot><on_crash>destroy</on_crash>
<devices><emulator>{escape(str(self.bin/'qemu-system-x86_64'))}</emulator>
<disk type="file" device="disk"><driver name="qemu" type="qcow2"/><source file="{self.root}/{u}.qcow2"/><target dev="vda" bus="virtio"/></disk>
<serial type="unix"><source mode="bind" path="{self.root}/{u}.serial"/><target port="0"/></serial>
<graphics type="none"/><memballoon model="none"/></devices>
<qemu:commandline><qemu:arg value="-L"/><qemu:arg value="{self.root}"/></qemu:commandline></domain>'''
            # libvirt has no 'none' graphics type: omission means headless.
            xml=xml.replace('<graphics type="none"/>','')
            p=self.root/(u+'.xml');p.write_text(xml)
            self.virsh('define',p)
            self.defined.append(u)
        manifest={'root':str(self.root),'virsh':str(self.bin/'virsh'),'library':str(self.runtime/'root/usr/lib'),'domains':self.ids,'base_sha256':self.base_hash}
        (self.root/'manifest.json').write_text(json.dumps(manifest))
    def ready(self,u):
        sock=socket.socket(socket.AF_UNIX);sock.settimeout(30)
        sock.connect(str(self.root/(u+'.serial')))
        self.serial[u]=sock
        sock.sendall(b'h\n')
        # Guest responds to an explicit handshake even if early console output was lost.
        data=b''
        until=('ELASTICPC_DISK_READY:'+self.tokens[u]).encode()
        while until not in data:
            piece=sock.recv(4096)
            if not piece:raise RuntimeError('serial EOF: '+data.decode(errors='replace'))
            data+=piece
            if len(data)>65536:raise RuntimeError('serial limit')
        self.logs[u]=data.decode(errors='replace')
        self.check('guest_disk_ready',True,self.logs[u])
    def close_serial(self,u):
        if u in self.serial:self.serial.pop(u).close()
    def guest_pidfd(self,u):
        matches=[]
        for proc in Path('/proc').iterdir():
            if not proc.name.isdecimal():continue
            try:
                if proc.stat().st_uid!=os.geteuid():continue
                cmd=(proc/'cmdline').read_bytes().split(b'\0')
                if b'-uuid' in cmd and cmd[cmd.index(b'-uuid')+1]==u.encode():
                    fd=os.pidfd_open(int(proc.name))
                    if (proc/'exe').resolve()!=self.bin/'qemu-system-x86_64':
                        os.close(fd);raise RuntimeError('QEMU executable mismatch')
                    matches.append(fd)
            except (FileNotFoundError,ProcessLookupError,PermissionError):continue
        if len(matches)!=1:
            for fd in matches:os.close(fd)
            raise RuntimeError('Cannot uniquely identify owned QEMU')
        return matches[0]
    def perform(self):
        u,s=self.ids
        self.worker('start',u,'first')
        self.ready(u)
        self.check('guest_write', 'ELASTICPC_DISK_WRITE:' in self.logs[u])
        kvm=json.loads(self.virsh('qemu-monitor-command',u,'{"execute":"query-kvm"}').stdout)
        self.check('kvm_enabled',kvm['return']['enabled'] is True and kvm['return']['present'] is True)
        self.check('base_unchanged',digest(self.root/'base.raw')==self.base_hash)
        self.worker('recover',u)
        self.worker('start',u,'first')
        self.worker('stop',u,'first-stop');self.close_serial(u)
        self.worker('stop',u,'first-stop')
        self.worker('start',u,'second');self.ready(u)
        self.check('data_persisted','ELASTICPC_DISK_PERSISTED:' in self.logs[u])
        self.worker('start',s,'service');self.ready(s)
        # Real lock contention: a second Rust controller cannot enter SQLite.
        hold=subprocess.Popen([str(PROJECT/'target/debug/examples/private_controller'),str(self.root/'manifest.json'),'hold',u],env=self.env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            if not select.select([hold.stdout],[],[],15)[0] or hold.stdout.readline().strip()!='ready':raise RuntimeError('controller hold handshake')
            competing=self.worker('recover',u,check=False)
            self.check('controller_lock_conflict',competing.returncode!=0 and 'AlreadyRunning' in competing.stderr)
        finally:
            hold.kill();hold.communicate(timeout=10)
        self.worker('recover',u)
        # Make only this lab socket unreachable; no absence-to-stopped inference.
        sock=self.root/'run/libvirt/virtqemud-sock';hidden=sock.with_suffix('.hidden')
        sock.rename(hidden)
        try:
            unknown=self.worker('recover',u,check=False)
            self.check('unknown_observation_refused',unknown.returncode!=0 and 'UnknownObservation' in unknown.stderr)
        finally:hidden.rename(sock)
        self.worker('recover',u)
        # Two QEMU writers must not open the same overlay.
        conflict=run([self.bin/'qemu-io','-f','qcow2','-c','write -P 0xaa 0 512',self.root/(u+'.qcow2')],self.env,check=False)
        self.check('second_image_writer_refused',conflict.returncode!=0 and 'lock' in conflict.stderr.lower(),conflict.stderr)
        self.worker('stop',u,'second-stop');self.close_serial(u)
        self.check('service_isolation',self.virsh('domstate',s).stdout.strip()=='running')
        for point in ['AfterJournal','AfterLeases','AfterStart','AfterStopIntent','AfterStop','AfterRelease']:
            is_start=point in ['AfterJournal','AfterLeases','AfterStart']
            if not is_start:self.worker('start',u,'pre-'+point);self.ready(u)
            op='boundary-'+point
            outcome=self.worker('start' if is_start else 'stop',u,op,point,check=False)
            self.check(point+'_process_killed',outcome.returncode==-signal.SIGKILL,str(outcome.returncode))
            self.worker('recover',u)
            self.worker('start' if is_start else 'stop',u,op)
            if is_start:
                self.ready(u)
                self.worker('stop',u,'cleanup-'+point)
            self.close_serial(u)
            self.check(point+'_recovered',self.virsh('domstate',s).stdout.strip()=='running')
        # Guest poweroff is observed through a pidfd, not inferred from RPC loss.
        self.worker('start',u,'graceful');self.ready(u)
        fd=self.guest_pidfd(u)
        try:
            self.serial[u].sendall(b'q\n')
            self.check('graceful_guest_exit',bool(select.select([fd],[],[],20)[0]))
            output=b''
            while True:
                chunk=self.serial[u].recv(4096)
                if not chunk:break
                output+=chunk
                if len(output)>65536:raise RuntimeError('serial limit')
            self.check('graceful_poweroff_marker',('ELASTICPC_DISK_POWEROFF:'+self.tokens[u]).encode() in output and b'reboot: Power down' in output,output.decode(errors='replace'))
        finally:os.close(fd)
        self.close_serial(u)
        self.worker('recover',u)
        self.worker('stop',u,'graceful-accounted')
        self.worker('start',u,'abrupt');self.ready(u)
        fd=self.guest_pidfd(u)
        try:
            signal.pidfd_send_signal(fd,signal.SIGKILL)
            self.check('abrupt_guest_exit',bool(select.select([fd],[],[],20)[0]))
        finally:os.close(fd)
        self.close_serial(u)
        self.worker('recover',u)
        self.worker('stop',u,'abrupt-accounted')
        self.check('service_survived_guest_crash',self.virsh('domstate',s).stdout.strip()=='running')
        self.worker('start',u,'timeout-case');self.ready(u)
        fd=self.guest_pidfd(u)
        try:
            self.check('injected_deadline_reached',not select.select([fd],[],[],1)[0])
            signal.pidfd_send_signal(fd,signal.SIGKILL)
            self.check('deadline_process_reaped',bool(select.select([fd],[],[],20)[0]))
        finally:os.close(fd)
        self.close_serial(u)
        self.worker('recover',u)
        self.worker('stop',u,'timeout-accounted')
        self.check('service_survived_timeout_cleanup',self.virsh('domstate',s).stdout.strip()=='running')
        self.worker('stop',s,'service-stop');self.close_serial(s)
        self.check('base_still_unchanged',digest(self.root/'base.raw')==self.base_hash)
        # Actual QEMU verifies the overlay after Guest process exit.
        run([self.bin/'qemu-img','check',self.root/(u+'.qcow2')],self.env)
        raw=self.root/'readback.raw'
        run([self.bin/'qemu-img','convert','-f','qcow2','-O','raw',self.root/(u+'.qcow2'),raw],self.env)
        self.check('host_readback',raw.read_bytes()[:32]==self.tokens[u].encode())
        base=self.root/'base.raw';base.chmod(0o600)
        with base.open('r+b') as f:f.write(b'changed');f.flush();os.fsync(f.fileno())
        base.chmod(0o400)
        outcome=self.worker('start',u,'stale',check=False)
        self.check('stale_overlay_refused',outcome.returncode!=0 and 'stale base' in outcome.stderr,outcome.stderr)
    def cleanup(self):
        for sock in self.serial.values():sock.close()
        self.serial.clear()
        if self.daemon and self.daemon.poll() is None:
            for u in self.defined:
                self.virsh('destroy',u,check=False)
                self.virsh('undefine',u,check=False)
            remaining=self.virsh('list','--all','--uuid',check=False)
            if self.defined and (remaining.returncode or remaining.stdout.strip()):raise RuntimeError('private domain cleanup unconfirmed')
            self.daemon.terminate()
            self.daemon.wait(timeout=20)
        self.report['daemon_exit_code']=self.daemon.returncode if self.daemon else None
        self.report['daemon_log']='\n'.join(p.read_text(errors='replace')[-30000:] for p in (self.root/'logs').glob('*.log'))
        shutil.rmtree(self.root)
        self.report['cleanup_complete']=not self.root.exists()
    def execute(self):
        try:
            self.prepare();self.perform();self.report['status']='passed'
        except Exception as exc:
            self.report['error']=repr(exc)
            if isinstance(exc,subprocess.CalledProcessError):self.report['error_output']=exc.stderr
        finally:
            signal.setitimer(signal.ITIMER_REAL,0)
            try:self.cleanup()
            except Exception as exc:self.report['cleanup_error']=repr(exc);self.report['status']='failed'
            self.output.write_text(json.dumps(self.report,indent=2)+'\n')
        print(json.dumps({k:v for k,v in self.report.items() if k not in ['daemon_log','checks','controller_steps']},indent=2))
        return self.report['status']=='passed'

def main():
    p=argparse.ArgumentParser();p.add_argument('--runtime',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--dry-run',action='store_true');a=p.parse_args()
    if a.dry_run:
        print(json.dumps({'status':'planned','kvm':True,'max_guests':2,'memory_mib_each':256,'physical_devices':False,'system_libvirt':False}));return
    if os.geteuid()==0:raise SystemExit('non-root required')
    if not hasattr(os,'pidfd_open') or not hasattr(signal,'pidfd_send_signal'):raise SystemExit('Use /usr/bin/python3 with Linux pidfd support')
    def deadline(signum,frame):raise TimeoutError('private lab deadline or termination request')
    signal.signal(signal.SIGALRM,deadline)
    signal.signal(signal.SIGTERM,deadline)
    signal.setitimer(signal.ITIMER_REAL,240)
    raise SystemExit(0 if Lab(a.runtime.resolve(),a.report).execute() else 1)
if __name__=='__main__':main()
