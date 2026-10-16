#!/usr/bin/env python3
"""Copy already verified local artifacts into a durable, project-private bundle."""
from pathlib import Path
import hashlib,json,shutil
root=Path(__file__).resolve().parents[1];data=root/'.elasticpc';data.mkdir(mode=0o700,exist_ok=True)
for src,dst in [(Path('/tmp/elasticpc-integration-runtime'),data/'runtime'),(Path('/tmp/elasticpc-p2-inputs/tools/usr/share/edk2/x64'),data/'inputs/tools/usr/share/edk2/x64')]:
    if not dst.exists():
        dst.parent.mkdir(parents=True,exist_ok=True)
        shutil.copytree(src,dst,symlinks=True)
images=data/'images';images.mkdir(exist_ok=True)
report=json.loads((root/'target/experience-image/report.json').read_text())
for name,key in [('experience.qcow2','image_sha256'),('experience-vars.fd','vars_sha256')]:
    dst=images/name
    if not dst.exists():shutil.copyfile(root/'target/experience-image'/name,dst)
    with dst.open('rb') as stream:assert hashlib.file_digest(stream,'sha256').hexdigest()==report[key]
    dst.chmod(0o400)
(data/'bin').mkdir(exist_ok=True)
shutil.copy2(root/'target/debug/examples/experience_controller',data/'bin/experience_controller')
(data/'build-report.json').write_text(json.dumps(report,indent=2)+'\n')
print('Packaged preview at',data)
