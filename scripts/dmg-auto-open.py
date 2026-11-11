#!/usr/bin/env python3
"""Set Finder's open-folder hint on an unmounted, partitionless HFS+ image.

Equivalent to legacy bless --openfolder for the volume root, without relying on
bless's unsupported Apple Silicon option. Both HFS+ volume headers are updated.
See Apple's HFS Plus Volume Format: finderInfo[2] is the open-folder root CNID.
Only a task-owned writable image produced with hdiutil -layout NONE is accepted.
"""
import struct
import sys


def mark_open(image):
    with open(image, 'r+b') as disk:
        disk.seek(1024)
        header = disk.read(512)
        if len(header) != 512 or header[:4] != b'H+\x00\x04':
            raise ValueError('Expected a partitionless HFS+ version 4 image')
        block_size, blocks = struct.unpack_from('>II', header, 40)
        if block_size < 512 or block_size & (block_size - 1) or blocks < 4:
            raise ValueError('Invalid HFS+ volume dimensions')
        alternate = block_size * blocks - 1024
        disk.seek(0, 2)
        if alternate < 1536 or alternate + 512 > disk.tell():
            raise ValueError('HFS+ volume exceeds image bounds')
        disk.seek(alternate)
        backup = disk.read(512)
        if backup[:4] != header[:4] or backup[40:48] != header[40:48]:
            raise ValueError('Alternate HFS+ header does not match')
        for offset in (1024, alternate):
            disk.seek(offset + 88)  # finderInfo[2]; root directory CNID is 2.
            disk.write(struct.pack('>I', 2))


if __name__ == '__main__':
    mark_open(sys.argv[1])
