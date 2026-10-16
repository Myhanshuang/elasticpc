import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('lab',ROOT/'scripts/private_guest_lab.py')
lab=importlib.util.module_from_spec(spec);spec.loader.exec_module(lab)

class HarnessTests(unittest.TestCase):
    def test_dry_run_creates_no_report_or_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            result=subprocess.run(['/usr/bin/python3',str(ROOT/'scripts/private_guest_lab.py'),'--runtime',str(root/'missing'),'--report',str(root/'report.json'),'--dry-run'],check=True,capture_output=True,text=True)
            self.assertEqual(json.loads(result.stdout)['status'],'planned')
            self.assertEqual(list(root.iterdir()),[])
    def test_cpio_preserves_module_and_regular_init(self):
        data=lab.archive([('init',0o100755,b'elf-bytes',0,0),('virtio_blk.ko',0o100400,b'module-bytes',0,0)])
        names=[];offset=0
        while offset<len(data):
            self.assertEqual(data[offset:offset+6],b'070701')
            fields=[int(data[offset+6+i*8:offset+14+i*8],16) for i in range(13)]
            size,namesize=fields[6],fields[11]
            name=data[offset+110:offset+110+namesize-1].decode();names.append(name)
            offset=(offset+110+namesize+3)&~3
            if name=='virtio_blk.ko':self.assertEqual(data[offset:offset+size],b'module-bytes')
            offset=(offset+size+3)&~3
        self.assertEqual(names,['init','virtio_blk.ko','TRAILER!!!'])

if __name__=='__main__':unittest.main()
