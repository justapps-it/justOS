#!/usr/bin/env python3
"""A small Debian archive for test-compat.sh: what `deb install` and `deb get`
fetch on justOS, served from this machine.

    aptarchive.py <www> <bin> <local> <key.asc>

<bin> holds two static programs (aptapp, aptother). The archive goes into
<www>/debian: a suite "test", signed with a new RSA key (exported, armored, to
<key.asc>, for the image's /etc/apt/trusted.gpg.d), with a gzip index that is
also reachable by its hash; and a suite "untrusted" with the same index,
signed with a key justOS does not know. Its packages:

  - aptlib 1.0 and 1.2: /usr/share/aptlib/greeting.txt
  - aptpre: what aptapp pre-depends on (a second dpkg run)
  - aptprov: provides the virtual package apt-virtual
  - aptother: a program of the archive, for `deb get`
  - aptcorrupt: its file was changed after the index was made

and into <local>, the package files the session installs by hand:

  - aptapp: Pre-Depends: aptpre; Depends: aptlib (>= 1.1) | nothere,
    apt-virtual; its program prints aptlib's greeting
  - aptbad: conflicts with debhello, which the session installed before
"""
import email.utils
import gzip
import hashlib
import io
import os
import subprocess
import sys
import tarfile
import tempfile
import time


def tar_gz(files):
    """A tar.gz of {path: (bytes, mode)}, owned by root."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        dirs = set()
        for path in files:
            parts = path.strip("/").split("/")[:-1]
            for i in range(1, len(parts) + 1):
                dirs.add("/".join(parts[:i]))
        for d in sorted(dirs):
            info = tarfile.TarInfo("./" + d)
            info.type, info.mode = tarfile.DIRTYPE, 0o755
            t.addfile(info)
        for path, (data, mode) in files.items():
            info = tarfile.TarInfo("./" + path.strip("/"))
            info.size, info.mode = len(data), mode
            t.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def ar(members):
    out = b"!<arch>\n"
    for name, data in members:
        out += f"{name:<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(data):<10}`\n".encode()
        out += data + (b"\n" if len(data) % 2 else b"")
    return out


def deb(control, files):
    ctl = tar_gz({"control": (control.encode(), 0o644)})
    return ar([("debian-binary", b"2.0\n"), ("control.tar.gz", ctl), ("data.tar.gz", tar_gz(files))])


def control(name, version, extra=""):
    return (f"Package: {name}\nVersion: {version}\nArchitecture: amd64\n"
            f"Maintainer: justOS tests <tests@justos.invalid>\nInstalled-Size: 20\n{extra}"
            f"Section: misc\nPriority: optional\nDescription: {name}, a package of justOS's apt test\n"
            f" Made by scripts/compat/aptarchive.py.\n")


def gpg(home, *args, data=None):
    return subprocess.run(["gpg", "--homedir", home, "--batch", "--pinentry-mode", "loopback", "--passphrase", "", *args],
                          input=data, capture_output=True, check=True).stdout


def main():
    www, bins, local, keyfile = sys.argv[1:5]
    prog = lambda n: (open(os.path.join(bins, n), "rb").read(), 0o755)
    pool = {
        "aptlib_1.0_amd64.deb": deb(control("aptlib", "1.0"), {"usr/share/aptlib/greeting.txt": (b"greeting from aptlib 1.0\n", 0o644)}),
        "aptlib_1.2_amd64.deb": deb(control("aptlib", "1.2"), {"usr/share/aptlib/greeting.txt": (b"greeting from aptlib 1.2\n", 0o644)}),
        "aptpre_1.0_amd64.deb": deb(control("aptpre", "1.0"), {"usr/share/aptpre/ok": (b"ok\n", 0o644)}),
        "aptprov_1.0_amd64.deb": deb(control("aptprov", "1.0", "Provides: apt-virtual\n"), {"usr/share/aptprov/ok": (b"ok\n", 0o644)}),
        "aptother_1.0_amd64.deb": deb(control("aptother", "1.0", "Depends: aptlib\n"), {"usr/bin/aptother": prog("aptother")}),
        "aptcorrupt_1.0_amd64.deb": deb(control("aptcorrupt", "1.0"), {"usr/share/aptcorrupt/ok": (b"ok\n", 0o644)}),
    }
    root = os.path.join(www, "debian")
    os.makedirs(os.path.join(root, "pool/main"), exist_ok=True)
    index = ""
    for name, data in pool.items():
        index += control_of(data) + f"Filename: pool/main/{name}\nSize: {len(data)}\nSHA256: {hashlib.sha256(data).hexdigest()}\n\n"
        if name.startswith("aptcorrupt"):
            # The same size, another byte: only the hash can tell.
            data = data[:-1] + bytes([data[-1] ^ 0xFF])
        open(os.path.join(root, "pool/main", name), "wb").write(data)
    packages = index.encode()
    packages_gz = gzip.compress(packages, mtime=0)
    now = time.time()
    with tempfile.TemporaryDirectory() as home:
        gpg(home, "--quick-gen-key", "justOS apt test <tests@justos.invalid>", "rsa3072", "sign", "never")
        open(keyfile, "wb").write(gpg(home, "--armor", "--export"))
        with tempfile.TemporaryDirectory() as other:
            gpg(other, "--quick-gen-key", "Somebody else <else@justos.invalid>", "ed25519", "sign", "never")
            for suite, signer in (("test", home), ("untrusted", other)):
                dist = os.path.join(root, "dists", suite)
                comp = os.path.join(dist, "main/binary-amd64")
                os.makedirs(os.path.join(comp, "by-hash/SHA256"), exist_ok=True)
                open(os.path.join(comp, "Packages"), "wb").write(packages)
                open(os.path.join(comp, "Packages.gz"), "wb").write(packages_gz)
                open(os.path.join(comp, "by-hash/SHA256", hashlib.sha256(packages_gz).hexdigest()), "wb").write(packages_gz)
                release = (f"Origin: justOS tests\nSuite: {suite}\nCodename: {suite}\n"
                           f"Date: {email.utils.formatdate(now, usegmt=True)}\n"
                           f"Valid-Until: {email.utils.formatdate(now + 7 * 86400, usegmt=True)}\n"
                           "Architectures: amd64\nComponents: main\nAcquire-By-Hash: yes\nSHA256:\n")
                for path, data in (("main/binary-amd64/Packages", packages), ("main/binary-amd64/Packages.gz", packages_gz)):
                    release += f" {hashlib.sha256(data).hexdigest()} {len(data):>8} {path}\n"
                open(os.path.join(dist, "InRelease"), "wb").write(gpg(signer, "--digest-algo", "SHA256", "--clearsign", data=release.encode()))
    os.makedirs(local, exist_ok=True)
    app = control("aptapp", "1.0", "Pre-Depends: aptpre\nDepends: aptlib (>= 1.1) | nothere, apt-virtual\n")
    open(os.path.join(local, "aptapp_1.0_amd64.deb"), "wb").write(deb(app, {"usr/bin/aptapp": prog("aptapp")}))
    bad = control("aptbad", "1.0", "Depends: aptlib\nConflicts: debhello\n")
    open(os.path.join(local, "aptbad_1.0_amd64.deb"), "wb").write(deb(bad, {"usr/share/aptbad/ok": (b"ok\n", 0o644)}))


def control_of(data):
    """The control file inside a .deb made by deb()."""
    i = data.index(b"control.tar.gz")
    size = int(data[i + 48:i + 58])
    ctl = data[i + 60:i + 60 + size]
    with tarfile.open(fileobj=io.BytesIO(ctl), mode="r:gz") as t:
        return t.extractfile("./control").read().decode().replace(" Made by scripts/compat/aptarchive.py.\n", "")


if __name__ == "__main__":
    main()
