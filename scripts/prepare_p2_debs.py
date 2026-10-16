#!/usr/bin/python3
"""Resolve Debian Depends/Pre-Depends into an offline package bundle.
APT inside the build Guest remains the final dependency/version validator.
"""
import concurrent.futures,hashlib,json,lzma,re,urllib.request,argparse,subprocess
p=argparse.ArgumentParser();p.add_argument("--resume",action="store_true");args=p.parse_args()
from pathlib import Path
base=Path('/tmp/elasticpc-p2-debs');base.mkdir(mode=0o700,exist_ok=args.resume)
url='https://deb.debian.org/debian/'
def fetch(u,p):
 subprocess.run(['/usr/bin/curl','--fail','--location','--max-time','120','--silent','--show-error','--output',str(p),u],check=True)
if not (base/'Release').exists():fetch(url+'dists/trixie/Release',base/'Release')
if not (base/'Packages.xz').exists():fetch(url+'dists/trixie/main/binary-amd64/Packages.xz',base/'Packages.xz')
lines=(base/'Release').read_text().split('SHA256:\n',1)[1].splitlines()
expected=next(l.split()[0] for l in lines if l.strip().endswith('main/binary-amd64/Packages.xz'))
assert hashlib.sha256((base/'Packages.xz').read_bytes()).hexdigest()==expected
packages={};providers={}
for paragraph in lzma.decompress((base/'Packages.xz').read_bytes()).decode().split('\n\n'):
 item={}
 for line in paragraph.splitlines():
  if line and not line[0].isspace() and ': ' in line:
   k,v=line.split(': ',1);item[k]=v
 if 'Package' not in item:continue
 packages[item['Package']]=item
 for p in item.get('Provides','').split(','):
  p=p.strip().split(' ')[0]
  if p:providers.setdefault(p,[]).append(item['Package'])
roots=['efibootmgr','qemu-system-x86','qemu-utils','libvirt-daemon-system','libvirt-daemon-driver-qemu','libvirt-clients']
selected={};queue=roots[:]
while queue:
 name=queue.pop()
 if name in selected:continue
 p=packages[name];selected[name]=p
 for group in (p.get('Pre-Depends','')+','+p.get('Depends','')).split(','):
  if not group.strip():continue
  options=[re.split(r'[ :([]',o.strip(),maxsplit=1)[0] for o in group.split('|')]
  chosen=next((r for r in roots if r in options or any(r in providers.get(n,[]) for n in options)),None) or next((n for n in options if n in packages),None)
  if chosen is None:
   chosen=next((sorted(providers[n])[0] for n in options if n in providers),None)
  if chosen is None:raise ValueError((name,group))
  queue.append(chosen)
folder=base/'debs';folder.mkdir(exist_ok=args.resume)
def download(p):
 target=folder/Path(p['Filename']).name
 if not target.exists() or hashlib.sha256(target.read_bytes()).hexdigest()!=p['SHA256']:fetch(url+p['Filename'],target)
 assert hashlib.sha256(target.read_bytes()).hexdigest()==p['SHA256']
 return {'name':p['Package'],'version':p['Version'],'filename':target.name,'sha256':p['SHA256'],'url':url+p['Filename']}
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:records=list(pool.map(download,selected.values()))
(base/'provenance.json').write_text(json.dumps({'release_url':url+'dists/trixie/Release','packages_index_sha256':expected,'roots':roots,'packages':records,'authentication':'HTTPS Release plus SHA256 chain; APT validates dependency versions in isolated Guest'},indent=2)+'\n')
print('Offline packages:',len(records),flush=True)
