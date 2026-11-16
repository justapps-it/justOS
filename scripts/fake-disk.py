#!/usr/bin/env python3
"""Makes a disk file for installing justOS on in QEMU, with a made-up
Windows on it (only the partition tables and the boot sectors: what the
installer reads to say what a disk holds). Sparse: only those sectors are
written.

Usage: scripts/fake-disk.py <file> <size> empty|mbr|gpt
  empty  nothing on it
  mbr    an MBR with one NTFS partition (type 0x07) from 1 MiB to the end
  gpt    a GPT: an EFI system partition (FAT32, 100 MiB) and a Windows one
         (Microsoft basic data, NTFS) to the end; with the protective MBR
         and the backup table
<size> as truncate takes it: 3G, 256M...
"""
import os
import struct
import subprocess
import sys
import uuid
import zlib

path, size, kind = sys.argv[1:4]
if os.path.exists(path):
    os.remove(path)
subprocess.run(["truncate", "-s", size, path], check=True)
if kind not in ("empty", "mbr", "gpt"):
    raise SystemExit(f"{kind}: empty, mbr or gpt")
f = open(path, "r+b")
f.seek(0, 2)
sectors = f.tell() // 512


def put(lba, data):
    f.seek(lba * 512)
    f.write(data)


def boot_sector(oem, extra=b""):
    b = bytearray(512)
    b[0:3] = b"\xeb\x52\x90"
    b[3:11] = oem
    b[11:13] = struct.pack("<H", 512)
    b[0x47:0x47 + len(extra)] = extra
    b[510:512] = b"\x55\xaa"
    return bytes(b)


ntfs = boot_sector(b"NTFS    ")
if kind == "mbr":
    mbr = bytearray(512)
    mbr[0x1B8:0x1BC] = struct.pack("<I", 0x1234ABCD)
    mbr[0x1BE:0x1CE] = bytes([0x80, 0, 0, 0, 0x07, 0, 0, 0]) + struct.pack("<II", 2048, sectors - 2048)
    mbr[510:512] = b"\x55\xaa"
    put(0, mbr)
    put(2048, ntfs)
elif kind == "gpt":
    fat = boot_sector(b"MSDOS5.0", b"NO NAME    FAT32   ")
    esp_first, esp_last = 2048, 2048 + 204800 - 1
    win_first, win_last = esp_last + 1, sectors - 34

    def entry(type_guid, first, last, name):
        return (uuid.UUID(type_guid).bytes_le + uuid.uuid4().bytes_le + struct.pack("<QQQ", first, last, 0)
                + name.encode("utf-16-le").ljust(72, b"\0"))

    entries = entry("C12A7328-F81F-11D2-BA4B-00A0C93EC93B", esp_first, esp_last, "EFI system partition")
    entries += entry("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", win_first, win_last, "Basic data partition")
    entries = entries.ljust(128 * 128, b"\0")

    def header(this, other, table):
        h = bytearray(struct.pack("<8sIIIIQQQQ16sQIII", b"EFI PART", 0x10000, 92, 0, 0, this, other, 34, sectors - 34,
                                  uuid.uuid4().bytes_le, table, 128, 128, zlib.crc32(entries)))
        h[16:20] = struct.pack("<I", zlib.crc32(bytes(h)))
        return bytes(h).ljust(512, b"\0")

    mbr = bytearray(512)
    mbr[0x1BE:0x1CE] = bytes([0, 0, 2, 0, 0xEE, 0xFF, 0xFF, 0xFF]) + struct.pack("<II", 1, min(sectors - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xaa"
    put(0, mbr)
    put(1, header(1, sectors - 1, 2))
    put(2, entries)
    put(sectors - 33, entries)
    put(sectors - 1, header(sectors - 1, 1, sectors - 33))
    put(esp_first, fat)
    put(win_first, ntfs)
f.close()
