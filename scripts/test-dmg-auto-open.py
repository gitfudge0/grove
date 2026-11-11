#!/usr/bin/env python3
"""Validate the guarded HFS+ open-folder hint edit without touching host volumes."""
import importlib.util
from pathlib import Path
import struct
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('dmg_auto_open', Path(__file__).with_name('dmg-auto-open.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class OpenFolderTests(unittest.TestCase):
    def fixture(self):
        data = bytearray(8192)
        header = bytearray(512)
        header[:4] = b'H+\x00\x04'
        struct.pack_into('>II', header, 40, 512, 16)
        data[1024:1536] = header
        data[7168:7680] = header
        return data

    def test_only_open_folder_words_change(self):
        before = self.fixture()
        with tempfile.NamedTemporaryFile() as file:
            file.write(before)
            file.flush()
            module.mark_open(file.name)
            after = Path(file.name).read_bytes()
        expected = before[:]
        struct.pack_into('>I', expected, 1112, 2)
        struct.pack_into('>I', expected, 7256, 2)
        self.assertEqual(after, expected)

    def test_invalid_images_are_preserved(self):
        for kind in ('signature', 'dimensions', 'truncated', 'alternate'):
            with self.subTest(kind=kind):
                data = self.fixture()
                if kind == 'signature':
                    data[1024:1026] = b'XX'
                elif kind == 'dimensions':
                    struct.pack_into('>I', data, 1064, 513)
                elif kind == 'truncated':
                    data = data[:2000]
                else:
                    data[7168] = 0
                with tempfile.NamedTemporaryFile() as file:
                    file.write(data)
                    file.flush()
                    with self.assertRaises(ValueError):
                        module.mark_open(file.name)
                    self.assertEqual(Path(file.name).read_bytes(), data)


if __name__ == '__main__':
    unittest.main()
