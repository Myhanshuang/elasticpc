#!/usr/bin/env python3
"""Explicit end-to-end preview acceptance on one private disposable Trial."""
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import subprocess
import sys
import time
import uuid
import experience as exp

ROOT=Path(__file__).resolve().parents[1]
DIRECTORY=Path(sys.argv[1]).absolute() if len(sys.argv)>1 else ROOT/'target/experience-test'
checks=[]

def cli(action,*extra,check=True):
    result=subprocess.run([str(ROOT/'elasticpc'),action,'--directory',str(DIRECTORY),*extra],capture_output=True,text=True,timeout=65)
    if check and result.returncode:raise RuntimeError(result.stdout+result.stderr)
    return result

def check(name,condition):
    checks.append({'name':name,'passed':bool(condition)})
    if not condition:raise AssertionError(name)

def until(stream,needle,timeout=120):
    data=bytearray();deadline=time.monotonic()+timeout
    with selectors.DefaultSelector() as selector:
        selector.register(stream,selectors.EVENT_READ)
        while needle not in data:
            remaining=deadline-time.monotonic()
            if remaining<=0:raise TimeoutError('Console marker missing: '+repr(needle)+' tail='+repr(data[-500:]))
            if not selector.select(remaining):continue
            chunk=os.read(stream.fileno(),65536)
            if not chunk:raise RuntimeError('Console closed: '+repr(data[-500:]))
            data.extend(chunk)
            if len(data)>2*1024*1024:raise RuntimeError('Console limit')
    return bytes(data)

def terminal():
    conn=socket.socket(socket.AF_UNIX);conn.connect(str(DIRECTORY/'console.sock'));conn.sendall(b'\n')
    until(conn,b'elasticpc-preview:')
    return conn

def worker(action,point):
    return subprocess.run([str(exp.WORKER),str(DIRECTORY),action,str(uuid.uuid4()),'normal',point],capture_output=True,text=True,timeout=65)

report={'status':'failed','checks':checks,'physical_host_modified':False}
try:
    if not DIRECTORY.exists():cli('init')
    check('initial_stopped',json.loads(cli('status').stdout)['result']=='Stopped')
    check('start',json.loads(cli('start').stdout)['result']=='Running')
    original=exp.rpc(DIRECTORY,'status')
    cli('start')
    check('duplicate_start_same_guest',exp.rpc(DIRECTORY,'status')['guest_pid']==original['guest_pid'])
    client=subprocess.Popen([str(ROOT/'elasticpc'),'console','--directory',str(DIRECTORY)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    try:
        until(client.stdout,b'elasticpc-preview:')
        client.stdin.write(b"printf 'preview-persistent-value\\n' > /root/preview-note; sync; printf '\\nWRITE_%s\\n' OK\n");client.stdin.flush()
        written=until(client.stdout,b'\r\nWRITE_OK\r\n')
        (DIRECTORY/'console-write.log').write_bytes(written)
        client.stdin.write(b'\x1d');client.stdin.flush();check('console_detach',client.wait(timeout=10)==0)
    finally:
        if client.poll() is None:client.kill();client.wait()
        client.stdin.close();client.stdout.close()
    check('detach_keeps_guest_running',exp.rpc(DIRECTORY,'status')['guest_pid']==original['guest_pid'])
    check('graceful_stop',json.loads(cli('stop').stdout)['result']=='Stopped')
    cli('start')
    with terminal() as conn:
        conn.sendall(b"cat /root/preview-note; printf '\\nREAD_%s\\n' OK\n")
        data=until(conn,b'\r\nREAD_OK\r\n');(DIRECTORY/'console-read.log').write_bytes(data)
        check('file_persists_across_shutdown',b'preview-persistent-value\r\n' in data)
    stopped=worker('stop','AfterStop');check('controller_killed_after_stop',stopped.returncode==-signal.SIGKILL)
    cli('recover');check('recover_stopped',json.loads(cli('status').stdout)['result']=='Stopped')
    started=worker('start','AfterStart');check('controller_killed_after_start',started.returncode==-signal.SIGKILL)
    before=exp.rpc(DIRECTORY,'status');cli('recover')
    check('recover_running_same_guest',exp.rpc(DIRECTORY,'status')['guest_pid']==before['guest_pid'])
    with terminal() as conn:conn.sendall(b'sync\n')
    guestfd=os.pidfd_open(before['guest_pid'])
    try:
        os.kill(before['supervisor_pid'],signal.SIGKILL)
        with selectors.DefaultSelector() as selector:
            selector.register(guestfd,selectors.EVENT_READ)
            check('supervisor_death_reaps_guest_io',bool(selector.select(10)))
    finally:os.close(guestfd)
    cli('recover');check('supervisor_death_recovers_stopped',json.loads(cli('status').stdout)['result']=='Stopped')
    cli('start')
    with terminal() as conn:
        conn.sendall(b"cat /root/preview-note; printf '\\nAFTER_CRASH_%s\\n' OK\n")
        check('restart_after_supervisor_crash',b'preview-persistent-value\r\n' in until(conn,b'\r\nAFTER_CRASH_OK\r\n'))
    cli('stop');cli('stop');check('duplicate_stop',json.loads(cli('status').stdout)['result']=='Stopped')
    base=DIRECTORY/'base.img';base.chmod(0o600)
    with base.open('r+b') as stream:stream.seek(-1,os.SEEK_END);byte=stream.read(1);stream.seek(-1,os.SEEK_END);stream.write(bytes([byte[0]^1]))
    base.chmod(0o400)
    rejected=cli('start',check=False)
    check('stale_base_start_refused',rejected.returncode!=0 and 'Stale base' in rejected.stdout)
    report['status']='passed'
except Exception as error:
    report['error']=repr(error)
finally:
    try:exp.backend(DIRECTORY,'force')
    except Exception:pass
    report['sources']={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['scripts/experience.py','scripts/trial_image.py','crates/elastic-controller/examples/experience_controller.rs']}
    (ROOT/'reports/experience-integration.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
raise SystemExit(0 if report['status']=='passed' else 1)
