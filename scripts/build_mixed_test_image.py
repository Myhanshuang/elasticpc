#!/usr/bin/env python3
"""Prepare a private mixed-mode candidate from a recovered USB root snapshot.
Never opens a real device. Does not claim this is a fresh deployment preimage.
"""
import hashlib, json, shutil, subprocess
from pathlib import Path
import p2_host_image as p2
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'target/host-mixed-v8'
def run(args, **kw):
    return subprocess.run([str(x) for x in args], check=True, **kw)
def sha(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def main():
    OUT.mkdir(mode=0o700,exist_ok=False)
    root=OUT/'root.img'
    run(['cp','--sparse=always',ROOT/'target/host-repair-v7/root.img',root])
    def read(dest):
        return run(['debugfs','-R','cat '+dest,root],capture_output=True).stdout
    installed={}
    def install(source,dest,mode='0100600'):
        for cmd in ['rm '+dest,'write '+str(source)+' '+dest,'sif '+dest+' mode '+mode]:
            run(['debugfs','-w','-R',cmd,root],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        assert read(dest)==source.read_bytes()
        installed[dest]=sha(source)
    for name,dest,mode in [
        ('epc','/usr/local/bin/epc','0100755'),
        ('epc-test','/usr/local/bin/epc-test','0100755'),
        ('check-network','/usr/local/bin/epc-network-check','0100755'),
        ('next-boot.txt','/root/NEXT-BOOT.txt','0100644')]:
        install(ROOT/'host/runtime'/name,dest,mode)
    # Owner explicitly authorized Manjaro direct testing; this is NOT a fabricated
    # clean-shutdown attestation and does not accept or merge any previous trial.
    state=json.loads(read('/var/lib/elasticpc/runtime/manjaro-trial-8e24b6c4234f/state.json'))
    approval={k:state[k] for k in ('serial','source_header_sha256')}
    approval.update(user_authorized_direct_test=True,trial_changes_merged=False,
                    authorization='Owner requested Manjaro direct / Windows trial on 2026-10-15',
                    clean_shutdown_verified=False,native_fallback_verified=False)
    f=OUT/'manjaro-direct-approved.json';f.write_text(json.dumps(approval,indent=2))
    install(f,'/var/lib/elasticpc/runtime/manjaro-direct-approved.json')
    # Reconstruct latest pointers from recovered per-session states. Original
    # files and incomplete archives remain in their session directories.
    for name,session in [('manjaro','manjaro-trial-8e24b6c4234f'),('windows','windows-trial-502f57f85d5e')]:
        s=json.loads(read('/var/lib/elasticpc/runtime/'+session+'/state.json'))
        s.update(status='interrupted',archive='incomplete',pid=None,
                 recovery_note='Physical power loss during RAM archive; original session evidence retained. No trial changes merged.')
        f=OUT/(name+'-recovered-latest.json');f.write_text(json.dumps(s,indent=2))
        install(f,'/var/lib/elasticpc/runtime/'+name+'-latest.json')
    run(['e2fsck','-f','-p',root])
    raw=OUT/'usb-host.raw'
    # New empty output: sparse seeks cannot leave stale bytes behind.
    with raw.open('xb') as dst:
        with (ROOT/'target/host-tests-v6/usb-host.raw').open('rb') as src:
            prefix=src.read(128*1024**2)
            assert len(prefix)==128*1024**2
            dst.write(prefix)
        with root.open('rb') as src:
            while data:=src.read(4*1024**2):
                if data.count(0)==len(data):dst.seek(len(data),1)
                else:dst.write(data)
        dst.truncate()
    raw.chmod(0o600)
    assert raw.stat().st_size==14*1024**3
    p2.RT=ROOT/'.elasticpc/runtime'
    p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
    image=OUT/'usb-host.qcow2'
    p2.run([p2.RT/'root/usr/bin/qemu-img','convert','-f','raw','-O','qcow2',raw,image],timeout=600)
    shutil.copyfile(ROOT/'target/host-tests-v6/local-login.txt',OUT/'local-login.txt')
    (OUT/'local-login.txt').chmod(0o600)
    report={'status':'built-not-tested','raw_sha256':sha(raw),'image_sha256':sha(image),
            'raw_bytes':raw.stat().st_size,'runtime_sha256':sha(ROOT/'host/runtime/epc'),
            'installed_files':installed,'modes':{'manjaro':'direct','windows':'trial'},
            'preserves_trial_data':True,'physical_guest_boot_verified':False,
            'usb_written':False,'requires_fresh_usb_snapshot_before_deployment':True}
    (OUT/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
if __name__=='__main__':main()
