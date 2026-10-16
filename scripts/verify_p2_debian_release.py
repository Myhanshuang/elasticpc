#!/usr/bin/python3
"""Authenticate the package Release with a pinned Debian 13 archive signer."""
import hashlib,json,subprocess
from pathlib import Path
base=Path('/tmp/elasticpc-p2-debs');home=base/'gnupg';home.mkdir(mode=0o700,exist_ok=True)
fingerprint='04B54C3CDCA79751B16BC6B5225629DF75B188BD'
url='https://deb.debian.org/debian/'
def get(source,path):
 subprocess.run(['/usr/bin/curl','--fail','--location','--max-time','120','--silent','--show-error','--output',str(path),source],check=True)
key_url=url+'pool/main/d/debian-archive-keyring/debian-archive-keyring_2025.1_all.deb'
key=base/'keyring.deb'
if not key.exists():get(key_url,key)
assert hashlib.sha256(key.read_bytes()).hexdigest()=='9ea7778e443144ca490668737a8ab22dd3e748bb99e805e22ec055abeb3c7fac'
subprocess.run(['/usr/bin/ar','p',str(key),'data.tar.xz'],check=True,stdout=(base/'keyring.tar.xz').open('wb'))
subprocess.run(['/usr/bin/tar','-xJf',str(base/'keyring.tar.xz'),'-C',str(base),'./usr/share/keyrings'],check=True)
keyring=base/'usr/share/keyrings/debian-archive-trixie-automatic.gpg'
if not (base/'Release.gpg').exists():get(url+'dists/trixie/Release.gpg',base/'Release.gpg')
result=subprocess.run(['/usr/bin/gpgv','--homedir',str(home),'--status-fd','1','--keyring',str(keyring),str(base/'Release.gpg'),str(base/'Release')],capture_output=True,text=True)
# Other signatures may lack their keys. Require an exact cryptographically valid pinned primary signer.
valid=any(line.startswith('[GNUPG:] VALIDSIG ') and (line.split()[2]==fingerprint or line.split()[-1]==fingerprint) for line in result.stdout.splitlines())
assert valid,result.stderr
record={'key_fingerprint':fingerprint,'key_source':key_url,'release_sha256':hashlib.sha256((base/'Release').read_bytes()).hexdigest(),'verification':result.stdout,'signature_verified':True}
(base/'signature.json').write_text(json.dumps(record,indent=2)+'\n')
print('Debian package Release signature verified')
