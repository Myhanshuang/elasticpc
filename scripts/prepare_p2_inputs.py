#!/usr/bin/python3
"""Fetch Debian cloud base (HTTPS + published checksum) and signed lab tools."""
import hashlib,json,subprocess,urllib.request
from pathlib import Path
BASE=Path('/tmp/elasticpc-p2-inputs')
BASE.mkdir(mode=0o700,exist_ok=False)
report={'debian_authentication':'HTTPS and published SHA512 manifest; no detached signature claimed','system_installation':False,'packages':[]}
def get(url,p):
    with urllib.request.urlopen(url,timeout=90) as r:
        resolved=r.url
        with p.open('wb') as out:
            while chunk:=r.read(1024*1024):out.write(chunk)
    return resolved
url='https://cloud.debian.org/images/cloud/trixie/20261001-2618/'
name='debian-13-genericcloud-amd64-20261001-2618.qcow2'
get(url+'SHA512SUMS',BASE/'SHA512SUMS')
line=next(l for l in (BASE/'SHA512SUMS').read_text().splitlines() if l.split()[-1].lstrip('*')==name)
get(url+name,BASE/name)
assert hashlib.file_digest((BASE/name).open('rb'),'sha512').hexdigest()==line.split()[0]
report['debian']={'url':url+name,'sha512':line.split()[0],'path':str(BASE/name)}
rt=Path('/tmp/elasticpc-integration-runtime')
for name in ['edk2-ovmf','mtools']:
    p=BASE/(name+'.pkg.tar.zst')
    resolved=get('https://archlinux.org/packages/extra/any/'+name+'/download/' if name=='edk2-ovmf' else 'https://archlinux.org/packages/extra/x86_64/'+name+'/download/',p)
    get(resolved+'.sig',Path(str(p)+'.sig'))
    r=subprocess.run(['/usr/bin/gpgv','--homedir',str(rt/'gnupg'),'--keyring',str(rt/'archlinux.gpg'),str(p)+'.sig',str(p)],check=True,capture_output=True,text=True)
    root=BASE/'tools';root.mkdir(exist_ok=True)
    subprocess.run(['/usr/bin/bsdtar','-xf',str(p),'-C',str(root)],check=True)
    report['packages'].append({'name':name,'url':resolved,'sha256':hashlib.file_digest(p.open('rb'),'sha256').hexdigest(),'signature_verified':True,'verification':r.stderr})
(BASE/'provenance.json').write_text(json.dumps(report,indent=2)+'\n')
print('P2 inputs verified',flush=True)
