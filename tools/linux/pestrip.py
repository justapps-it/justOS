#!/usr/bin/env python3
"""Takes the debugging information out of Wine's Windows DLLs and programs.

Debian builds Wine's PE files (lib/x86_64-linux-gnu/wine/*-windows) with
DWARF sections, 812 of their 1302 MiB. Wine never reads them, but the
first start of a Windows program copies all those files into the C: drive
(winprep), so they were written to the disk twice: 1.4 GiB at the first
start instead of half a GiB.

The sections go only when they are the file's last ones (they always are
in Debian's files): everything before them stays byte for byte, the DOS
header with Wine's "Wine builtin DLL" mark included (strip and objcopy
rewrite it, and Wine would take its own DLLs for native ones). Then the
headers say so: fewer sections, a smaller image, no COFF symbols; the
long section names that remain (".eh_frame") keep a string table of their
own after the last section.

Usage: pestrip.py <folder>...   (every file under them, in place; files
that are not PE, or have no such sections, are left as they are)
"""
import os
import struct
import sys


def strip(d):
    """The file without its trailing .debug sections, or None."""
    if len(d) < 0x40 or d[:2] != b"MZ":
        return None
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    if pe + 24 > len(d) or d[pe:pe + 4] != b"PE\0\0":
        return None
    n = struct.unpack_from("<H", d, pe + 6)[0]
    symp, nsym = struct.unpack_from("<II", d, pe + 12)
    optsize = struct.unpack_from("<H", d, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", d, opt)[0]
    if magic not in (0x10B, 0x20B):
        return None
    strtab = symp + 18 * nsym if symp else 0
    heads = opt + optsize
    secs = []
    for i in range(n):
        o = heads + 40 * i
        raw = d[o:o + 8].rstrip(b"\0")
        name = raw
        if raw.startswith(b"/") and strtab:
            off = int(raw[1:])
            name = d[strtab + off:d.index(b"\0", strtab + off)]
        vsize, va, rsize, rptr = struct.unpack_from("<IIII", d, o + 8)
        secs.append((name, va, vsize, rptr, rsize))
    keep = len(secs)
    while keep and secs[keep - 1][0].startswith(b".debug"):
        keep -= 1
    if keep == len(secs) or any(s[0].startswith(b".debug") for s in secs[:keep]):
        return None
    end = max((rp + rs for (_, _, _, rp, rs) in secs[:keep] if rs), default=0)
    # Nothing after the kept sections may be needed: a certificate table
    # (data directory 4, a file offset) would be cut off.
    ndirs = struct.unpack_from("<I", d, opt + (92 if magic == 0x10B else 108))[0]
    dirs = opt + (96 if magic == 0x10B else 112)
    if ndirs > 4 and struct.unpack_from("<I", d, dirs + 8 * 4)[0]:
        return None
    out = bytearray(d[:end])
    # Long names of the kept sections, in a string table of their own.
    table = bytearray()
    for i, (name, *_rest) in enumerate(secs[:keep]):
        o = heads + 40 * i
        if out[o:o + 1] == b"/":
            ref = b"/%d" % (4 + len(table))
            table += name + b"\0"
            out[o:o + 8] = ref.ljust(8, b"\0")
    for i in range(keep, len(secs)):
        o = heads + 40 * i
        out[o:o + 40] = bytes(40)
    struct.pack_into("<H", out, pe + 6, keep)
    if table:
        struct.pack_into("<II", out, pe + 12, len(out), 0)
        out += struct.pack("<I", 4 + len(table)) + table
    else:
        struct.pack_into("<II", out, pe + 12, 0, 0)
    align = struct.unpack_from("<I", out, opt + 32)[0] or 0x1000
    last = max(va + vs for (_, va, vs, _, _) in secs[:keep])
    struct.pack_into("<I", out, opt + 56, (last + align - 1) // align * align)
    # The debug directory (6), if it pointed into what is gone.
    if ndirs > 6:
        rva, size = struct.unpack_from("<II", out, dirs + 8 * 6)
        if rva and rva + size > last:
            struct.pack_into("<II", out, dirs + 8 * 6, 0, 0)
    struct.pack_into("<I", out, opt + 64, 0)  # the checksum no longer holds
    return bytes(out)


def main():
    before = after = files = 0
    for top in sys.argv[1:]:
        for root, _, names in os.walk(top):
            for f in names:
                p = os.path.join(root, f)
                if os.path.islink(p) or not os.path.isfile(p):
                    continue
                with open(p, "rb") as fh:
                    d = fh.read()
                s = strip(d)
                if s is None:
                    continue
                st = os.stat(p)
                with open(p, "wb") as fh:
                    fh.write(s)
                os.utime(p, ns=(st.st_atime_ns, st.st_mtime_ns))
                before += len(d)
                after += len(s)
                files += 1
    if files:
        print(f"pestrip: {files} files, {before >> 20} -> {after >> 20} MiB")


if __name__ == "__main__":
    main()
