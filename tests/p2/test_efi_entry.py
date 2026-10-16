from pathlib import Path
import subprocess
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / 'host/p2/owned-efi-entry'
LINE = 'Boot0005* ElasticPC\tHD(15,GPT,example,0x2000,0x3e000)/File(\\EFI\\ElasticPC\\shimx64.efi)\n'

class EfiEntryTests(unittest.TestCase):
    def parse(self, text):
        return subprocess.run(['/usr/bin/python3', str(SCRIPT)], input=text, capture_output=True, text=True)

    def test_device_path_is_not_part_of_label(self):
        result = self.parse('Boot0001* Debian\tHD(other)\n' + LINE)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, '0005\n')

    def test_duplicate_label_is_rejected(self):
        self.assertNotEqual(self.parse(LINE + LINE.replace('0005', '0006')).returncode, 0)

    def test_foreign_path_is_rejected(self):
        self.assertNotEqual(self.parse(LINE.replace('\\ElasticPC\\', '\\debian\\')).returncode, 0)

    def test_missing_entry_is_rejected(self):
        self.assertNotEqual(self.parse('Boot0005* ElasticPC-other\tHD(example)\n').returncode, 0)

if __name__ == '__main__':
    unittest.main()
