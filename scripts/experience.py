#!/usr/bin/env python3
"""ElasticPC local, file-only console preview. No physical disk executor."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import struct
import subprocess
import sys
import termios
import time
import tty
import uuid
import trial_image as trial
import p2_host_image as p2

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT/'.elasticpc'
WORKER = DATA/'bin/experience_controller'
p2.RT = DATA/'runtime'
p2.INPUT = DATA/'inputs'
p2.QEMU = p2.RT/'root/usr/bin/qemu-system-x86_64'
p2.IMG = p2.RT/'root/usr/bin/qemu-img'
p2.ENV = dict(PATH='/usr/bin:/bin', LC_ALL='C', LD_LIBRARY_PATH=str(p2.RT/'root/usr/lib'))


def checked_root(value):
    root = Path(value).absolute()
    if root.resolve() != root or len(str(root).encode()) > 80 or any(c == ',' or ord(c)<32 for c in str(root)):
        raise ValueError('Use a short canonical path without comma/control characters')
    st = root.lstat()
    if not root.is_dir() or st.st_uid != os.getuid() or st.st_mode & 0o077:
        raise ValueError('Preview directory must be owned and private')
    return root


def rpc(root, action, timeout=40):
    with socket.socket(socket.AF_UNIX) as conn:
        conn.settimeout(timeout); conn.connect(str(root/'control.sock'))
        conn.sendall(json.dumps({'action':action}).encode()+b'\n')
        data = bytearray()
        while b'\n' not in data:
            part=conn.recv(4096)
            if not part: raise RuntimeError('Supervisor disconnected; state unknown')
            data.extend(part)
            if len(data)>65536: raise RuntimeError('Oversized supervisor reply')
        result=json.loads(data.split(b'\n',1)[0])
        if result.get('error'): raise RuntimeError(result['error'])
        return result


def observation(root):
    try:
        return rpc(root,'status',3)
    except (FileNotFoundError,ConnectionRefusedError):
        # Only a free lifetime lock establishes stopped I/O, never a stale PID.
        with trial.session(root):
            return {'state':'Stopped'}


def backend(root, action):
    if action=='status': return observation(root)
    if action in ('stop','force'):
        if observation(root)['state']=='Stopped': return {'state':'Stopped'}
        return rpc(root,action)
    if action!='start': raise ValueError('Unknown backend action')
    if observation(root)['state']=='Running': return {'state':'Running'}
    with (root/'supervisor.log').open('ab') as log:
        proc=subprocess.Popen(['/usr/bin/python3',str(Path(__file__).resolve()),'_supervise','--directory',str(root)],
                              stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=log,start_new_session=True)
        with selectors.DefaultSelector() as selector:
            selector.register(proc.stdout,selectors.EVENT_READ)
            if not selector.select(20):
                proc.terminate(); proc.wait(timeout=10)
                raise TimeoutError('Supervisor startup timeout')
            line=proc.stdout.readline(); proc.stdout.close()
        if not line: raise RuntimeError('Supervisor failed; inspect supervisor.log')
        reply=json.loads(line)
        if reply.get('error'): raise RuntimeError(reply['error'])
        return reply


def supervise(root):
    pending=None
    with trial.session(root,inherit=True) as (_,lockfd):
        trial.verify(root,p2.IMG,p2.ENV)
        available=int(next(l.split()[1] for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:')))//1024
        if available<9472: raise RuntimeError('Need 9472 MiB available including 8 GiB reserve')
        for name in ['control.sock','console.sock']:
            path=root/name
            if path.exists():
                if not path.is_socket(): raise ValueError('Unexpected object at socket path')
                path.unlink()
        parent=os.getpid()
        def guard():
            # QEMU dies if the owning supervisor disappears. Inherited flock closes the race.
            if ctypes.CDLL(None).prctl(1,signal.SIGKILL,0,0,0)!=0: os._exit(125)
            if os.getppid()!=parent: os._exit(125)
        args=[p2.QEMU,'-no-user-config','-nodefaults','-machine','q35,accel=kvm,mem-merge=off,dump-guest-core=off',
              '-cpu','host','-smp','1','-m','1024','-display','none','-nic','none','-no-reboot',
              '-qmp','stdio','-chardev',f'socket,id=console,path={root}/console.sock,server=on,wait=off',
              '-serial','chardev:console','-add-fd',f'fd={lockfd},set=99,opaque=trial-lifetime-lock',
              '-sandbox','on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny',
              '-L',p2.RT/'firmware','-drive',f'if=pflash,format=raw,readonly=on,file={p2.INPUT}/tools/usr/share/edk2/x64/OVMF_CODE.4m.fd',
              '-drive',f'if=pflash,format=raw,file={root}/vars.fd',
              '-drive',f'if=virtio,format=qcow2,file={root}/overlay.qcow2']
        listener=socket.socket(socket.AF_UNIX); pending=None; child=None; pidfd=None
        try:
            listener.bind(str(root/'control.sock')); listener.listen(4)
            with (root/'qemu.log').open('ab') as log:
                child=subprocess.Popen([str(a) for a in args],env=p2.ENV,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
                                       stderr=log,pass_fds=(lockfd,),preexec_fn=guard)
            pidfd=os.pidfd_open(child.pid)
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout,selectors.EVENT_READ,'qmp')
                selector.register(pidfd,selectors.EVENT_READ,'exit')
                selector.register(listener,selectors.EVENT_READ,'client')
                buffer=b''; ready=False; deadline=time.monotonic()+15; stopping=False
                while True:
                    if child.poll() is not None: break
                    timeout=max(0,deadline-time.monotonic()) if (not ready or stopping) else None
                    events=selector.select(timeout)
                    if not events:
                        if not ready: raise TimeoutError('QMP greeting timeout')
                        if pending:
                            pending.sendall(b'{"error":"Graceful shutdown timed out; Guest remains protected. Use stop --force if needed."}\n')
                            pending.close();pending=None;stopping=False
                        continue
                    for key,_ in events:
                        if key.data=='exit': break
                        if key.data=='qmp':
                            chunk=os.read(child.stdout.fileno(),65536)
                            if not chunk: break
                            buffer+=chunk
                            if len(buffer)>1024*1024: raise RuntimeError('QMP buffer exceeded limit')
                            while b'\n' in buffer:
                                line,buffer=buffer.split(b'\n',1)
                                message=json.loads(line)
                                if 'QMP' in message:
                                    child.stdin.write(b'{"execute":"qmp_capabilities","id":"hello"}\n');child.stdin.flush()
                                if message.get('id')=='hello':
                                    if 'error' in message: raise RuntimeError(str(message['error']))
                                    ready=True
                                    print(json.dumps({'state':'Running','supervisor_pid':os.getpid()}),flush=True)
                        if key.data=='client':
                            conn,_=listener.accept(); conn.settimeout(2)
                            try:
                                _,uid,_=struct.unpack('3i',conn.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
                                if uid!=os.getuid(): raise PermissionError('Foreign peer')
                                request=b''
                                while b'\n' not in request:
                                    part=conn.recv(4096)
                                    if not part or len(request)+len(part)>4096: raise ValueError('Invalid request length')
                                    request+=part
                                action=json.loads(request.split(b'\n',1)[0])['action']
                                if action=='status':
                                    conn.sendall(json.dumps({'state':'Running','supervisor_pid':os.getpid(),'guest_pid':child.pid}).encode()+b'\n')
                                elif action in ('stop','force') and pending is None:
                                    command='quit' if action=='force' else 'system_powerdown'
                                    child.stdin.write(json.dumps({'execute':command}).encode()+b'\n');child.stdin.flush()
                                    pending=conn;conn=None;stopping=True;deadline=time.monotonic()+30
                                else: raise ValueError('Unknown or concurrent request')
                            except Exception as error:
                                if conn: conn.sendall(json.dumps({'error':str(error)}).encode()+b'\n')
                            finally:
                                if conn: conn.close()
                child.wait(timeout=10)
            trial.verify(root,p2.IMG,p2.ENV)
            (root/'last-exit.json').write_text(json.dumps({'exit_code':child.returncode,'base_unchanged':True})+'\n')
        finally:
            if child:
                if child.poll() is None: child.kill();child.wait(timeout=10)
                child.stdin.close();child.stdout.close()
            if pidfd is not None: os.close(pidfd)
            listener.close()
            for name in ['control.sock','console.sock']:
                path=root/name
                if path.is_socket(): path.unlink()

    if pending:
        pending.sendall(b'{"state":"Stopped"}\n');pending.close()


def console(root):
    if observation(root)['state']!='Running': raise RuntimeError('Start the preview first')
    saved=None
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(str(root/'console.sock'))
        print('Connected to isolated Guest. Ctrl-] detaches; poweroff shuts down.',flush=True)
        try:
            if sys.stdin.isatty():
                saved=termios.tcgetattr(sys.stdin.fileno());tty.setraw(sys.stdin.fileno())
            conn.sendall(b'\n')
            with selectors.DefaultSelector() as selector:
                selector.register(conn,selectors.EVENT_READ,'guest');selector.register(sys.stdin,selectors.EVENT_READ,'input')
                while True:
                    for key,_ in selector.select():
                        if key.data=='guest':
                            data=conn.recv(65536)
                            if not data:return
                            os.write(sys.stdout.fileno(),data)
                        else:
                            data=os.read(sys.stdin.fileno(),4096)
                            if not data or b'\x1d' in data:return
                            conn.sendall(data)
        finally:
            if saved:termios.tcsetattr(sys.stdin.fileno(),termios.TCSADRAIN,saved)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action',choices=['init','start','status','stop','recover','console','_backend','_supervise'])
    parser.add_argument('--directory',type=Path,default=DATA/'preview')
    parser.add_argument('--force',action='store_true')
    parser.add_argument('--backend-action',choices=['start','stop','force','status'])
    args=parser.parse_args()
    if os.geteuid()==0: raise RuntimeError('Run as ordinary host user')
    if args.action=='init':
        DATA.mkdir(mode=0o700,exist_ok=True)
        image=DATA/'images'
        result=trial.prepare(image/'experience.qcow2',image/'experience-vars.fd',args.directory,'qcow2',p2.IMG,p2.ENV)
    else:
        root=checked_root(args.directory)
        if args.action=='_supervise': supervise(root);return
        if args.action=='_backend': result=backend(root,args.backend_action)
        elif args.action=='console': console(root);return
        else:
            command=[str(WORKER),str(root),args.action,str(uuid.uuid4())]
            if args.force: command.append('force')
            completed=subprocess.run(command,capture_output=True,text=True,timeout=60)
            if completed.returncode: raise RuntimeError(completed.stderr.strip() or completed.stdout)
            print(completed.stdout,end='');return
    print(json.dumps(result,indent=2))


if __name__=='__main__':
    try:main()
    except Exception as error:
        print(json.dumps({'error':str(error)}),flush=True);sys.exit(1)
