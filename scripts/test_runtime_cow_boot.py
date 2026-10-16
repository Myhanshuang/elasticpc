#!/usr/bin/env python3
"""Real QEMU NVMe/COW graph test using only owned files and no network."""
import importlib.util,importlib.machinery,sys,hashlib,json,os,shutil,uuid
from pathlib import Path
import pexpect
ROOT=Path(__file__).resolve().parents[1]
loader=importlib.machinery.SourceFileLoader('epc',str(ROOT/'host/runtime/epc'));spec=importlib.util.spec_from_loader(loader.name,loader);epc=importlib.util.module_from_spec(spec);loader.exec_module(epc)
import p2_host_image as p2
p2.RT=ROOT/'.elasticpc/runtime';p2.INPUT=ROOT/'.elasticpc/inputs';p2.IMG=p2.RT/'root/usr/bin/qemu-img';p2.ENV['LD_LIBRARY_PATH']=str(p2.RT/'root/usr/lib')
out=Path(sys.argv[1]).resolve();source=out/'usb-host.raw';session=out/('cow-test-'+uuid.uuid4().hex[:8]);session.mkdir(mode=0o700)
with source.open('rb') as f:before=hashlib.file_digest(f,'sha256').hexdigest()
overlay=session/'overlay.qcow2';p2.run([p2.IMG,'create','-f','qcow2','-F','raw','-b',source,overlay])
var=session/'vars.fd';shutil.copyfile(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_VARS.4m.fd',var)
fd=os.open(source,os.O_RDONLY)
profile={'vcpus':1,'ram_mib':1536,'uuid':'87f145c4-f03e-4e68-a5ba-e7483a813cbf'}
linux_mode='--manjaro-kernel' in sys.argv
args=epc.cmdline(profile,fd,overlay,var,session,'manjaro' if linux_mode else 'synthetic',{'manjaro':{'root_uuid':'aa2fd39f-c27d-4a91-ab92-ddc86f5c6930'}},network_on=False)[1:]
if linux_mode:
 # Debian fixture lacks matching Manjaro FAT modules; EFI mounting is outside this root-I/O test.
 args[args.index('-append')+1]+=' systemd.mask=boot-efi.mount'
 args[args.index('-kernel')+1]=str(ROOT/'.elasticpc/imports/manjaro-readonly/boot/vmlinuz-6.12-x86_64')
 args[args.index('-initrd')+1]=str(ROOT/'.elasticpc/imports/manjaro-readonly/boot/initramfs-6.12-x86_64.img')
# Serial stdio is only for this isolated test. The installed runner logs to a file.
args[args.index('-serial')+1]='stdio'
args=[x.replace('/usr/share/elasticpc/OVMF_CODE.fd',str(p2.INPUT/'tools/usr/share/edk2/x64/OVMF_CODE.4m.fd')) for x in args]
args+=['-L',str(p2.RT/'firmware')]
password=(out/'local-login.txt').read_text().split('Password: ',1)[1].strip()
def reopen_source():
 fresh=os.open(source,os.O_RDONLY)
 if fresh!=fd:os.dup2(fresh,fd);os.close(fresh)
 os.set_inheritable(fd,True)
child=pexpect.spawn(str(p2.RT/'root/usr/bin/qemu-system-x86_64'),args,env=p2.ENV,encoding='utf-8',codec_errors='replace',timeout=180,echo=False,preexec_fn=reopen_source)
report={'status':'failed','physical_disks':False,'network':False}
try:
 bootlog=(session/'boot.log').open('w');child.logfile_read=bootlog
 child.expect('elasticpc-host login:');child.logfile_read=None;bootlog.close();child.sendline('root');child.expect('Password:');child.sendline(password);child.expect(r'root@elasticpc-host:.*#')
 child.sendline('stty -echo');child.expect(r'root@elasticpc-host:.*#')
 child.sendline("echo trial-write > /root/epc-cow-test; sync; findmnt -n -o SOURCE /; printf '\\nEPC_COW_WRITE_OK\\n'")
 child.expect(r'\r+\nEPC_COW_WRITE_OK\r+\n');output=child.before;assert ('/dev/vda' if linux_mode else '/dev/nvme') in output
 if linux_mode:
  child.sendline("dd if=/dev/urandom of=/root/epc-load.bin bs=1M count=256 conv=fsync status=none && sha256sum /root/epc-load.bin > /root/epc-load.sha && sha256sum -c /root/epc-load.sha && printf '\\nEPC_IO_LOAD_OK\\n'")
  child.expect(r'\r+\nEPC_IO_LOAD_OK\r+\n')
 child.sendline('poweroff');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
 with source.open('rb') as f:after=hashlib.file_digest(f,'sha256').hexdigest()
 assert before==after;assert overlay.stat().st_size>196608
 report.update(status='passed',manjaro_kernel_virtio_boot=linux_mode,fixture_efi_mount_masked=linux_mode,nvme_boot=not linux_mode,trial_write=True,source_unchanged=True,source_sha256=after)
finally:
 if child.isalive():child.terminate(force=True)
 os.close(fd);(out/('manjaro-io-report.json' if linux_mode else 'runtime-cow-report.json')).write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
