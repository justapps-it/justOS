#!/usr/bin/env python3
"""Checks JustOS Connect with a pretend phone, written from the protocol
(system/kernel/src/connect/mod.rs) as the Android app speaks it.

justOS runs in QEMU with its Connect ports forwarded to this machine. The
pretend phone finds it with a datagram, pairs with the code justOS writes
in its log, sends a file (checked with sha256 on justOS), watches the
screen (a JPEG frame), moves the pointer, gets a file sent with
"connect send", then connects again with its token, and once with a wrong
one (refused).

Everything the phone needs is here, in plain Python: P-256 for the key
exchange and AES-GCM for the frames (slow, so the data is kept small).

Usage: scripts/test-connect.py <image>     (VERBOSE=1 prints the session)
"""
import hashlib
import hmac
import json
import os
import re
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

# --------------------------------------------------------------------------
# P-256

P = 2**256 - 2**224 + 2**192 + 2**96 - 1
A = P - 3
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
G = (0x6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296,
     0x4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5)


def ec_add(p, q):
    if p is None:
        return q
    if q is None:
        return p
    if p[0] == q[0] and (p[1] + q[1]) % P == 0:
        return None
    if p == q:
        l = (3 * p[0] * p[0] + A) * pow(2 * p[1], -1, P) % P
    else:
        l = (q[1] - p[1]) * pow(q[0] - p[0], -1, P) % P
    x = (l * l - p[0] - q[0]) % P
    return (x, (l * (p[0] - x) - p[1]) % P)


def ec_mul(k, p):
    r = None
    while k:
        if k & 1:
            r = ec_add(r, p)
        p = ec_add(p, p)
        k >>= 1
    return r


def point_bytes(p):
    return b"\x04" + p[0].to_bytes(32, "big") + p[1].to_bytes(32, "big")


def bytes_point(b):
    assert len(b) == 65 and b[0] == 4
    return (int.from_bytes(b[1:33], "big"), int.from_bytes(b[33:], "big"))

# --------------------------------------------------------------------------
# AES-256-GCM

SBOX = [0] * 256


def _sbox():
    p = q = 1
    while True:
        p = p ^ ((p << 1) & 0xFF) ^ (0x1B if p & 0x80 else 0)
        q ^= q << 1
        q ^= q << 2
        q ^= q << 4
        q &= 0xFF
        if q & 0x80:
            q ^= 0x09
        x = q ^ ((q << 1) | (q >> 7)) ^ ((q << 2) | (q >> 6)) ^ ((q << 3) | (q >> 5)) ^ ((q << 4) | (q >> 4))
        SBOX[p] = (x ^ 0x63) & 0xFF
        if p == 1:
            break
    SBOX[0] = 0x63


_sbox()


def xtime(a):
    return ((a << 1) ^ 0x1B) & 0xFF if a & 0x80 else a << 1


class Aes:
    def __init__(self, key):
        nk = len(key) // 4
        self.rounds = nk + 6
        w = [list(key[4 * i:4 * i + 4]) for i in range(nk)]
        rcon = 1
        for i in range(nk, 4 * (self.rounds + 1)):
            t = list(w[i - 1])
            if i % nk == 0:
                t = [SBOX[t[1]] ^ rcon, SBOX[t[2]], SBOX[t[3]], SBOX[t[0]]]
                rcon = xtime(rcon)
            elif nk > 6 and i % nk == 4:
                t = [SBOX[b] for b in t]
            w.append([w[i - nk][j] ^ t[j] for j in range(4)])
        self.rk = [sum(w[4 * r:4 * r + 4], []) for r in range(self.rounds + 1)]

    def encrypt(self, block):
        s = [b ^ k for b, k in zip(block, self.rk[0])]
        for r in range(1, self.rounds + 1):
            s = [SBOX[b] for b in s]
            s = [s[(i + 4 * (i % 4)) % 16] for i in range(16)]
            if r != self.rounds:
                m = []
                for c in range(4):
                    a = s[4 * c:4 * c + 4]
                    t = a[0] ^ a[1] ^ a[2] ^ a[3]
                    m += [a[i] ^ t ^ xtime(a[i] ^ a[(i + 1) % 4]) for i in range(4)]
                s = m
            s = [b ^ k for b, k in zip(s, self.rk[r])]
        return bytes(s)


def gmul(x, y):
    r = 0
    R = 0xE1 << 120
    for i in range(127, -1, -1):
        if (y >> i) & 1:
            r ^= x
        x = (x >> 1) ^ R if x & 1 else x >> 1
    return r


class Gcm:
    def __init__(self, key):
        self.aes = Aes(key)
        self.h = int.from_bytes(self.aes.encrypt(bytes(16)), "big")

    def _ctr(self, nonce, data):
        out = bytearray()
        for i in range(0, len(data), 16):
            ks = self.aes.encrypt(nonce + (2 + i // 16).to_bytes(4, "big"))
            out += bytes(a ^ b for a, b in zip(data[i:i + 16], ks))
        return bytes(out)

    def _tag(self, nonce, ct):
        y = 0
        for i in range(0, len(ct), 16):
            y = gmul(y ^ int.from_bytes(ct[i:i + 16].ljust(16, b"\0"), "big"), self.h)
        y = gmul(y ^ (len(ct) * 8), self.h)
        j0 = int.from_bytes(self.aes.encrypt(nonce + b"\0\0\0\1"), "big")
        return (y ^ j0).to_bytes(16, "big")

    def seal(self, nonce, data):
        ct = self._ctr(nonce, data)
        return ct + self._tag(nonce, ct)

    def open(self, nonce, data):
        ct, tag = data[:-16], data[-16:]
        if not hmac.compare_digest(self._tag(nonce, ct), tag):
            raise ValueError("bad seal")
        return self._ctr(nonce, ct)


def hkdf(salt, ikm, info, n):
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    out, t, i = b"", b"", 1
    while len(out) < n:
        t = hmac.new(prk, t + info + bytes([i]), hashlib.sha256).digest()
        out += t
        i += 1
    return out[:n]

# --------------------------------------------------------------------------
# The phone


class Phone:
    def __init__(self, port, ident, name):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=60)
        self.ident, self.name = ident, name
        self.buf = b""
        self.seal = self.open = None
        self.sent = self.received = 0

    def send(self, ty, payload=b""):
        body = bytes([ty]) + payload
        if self.seal:
            body = self.seal.seal(bytes(4) + self.sent.to_bytes(8, "big"), body)
            self.sent += 1
        self.s.sendall(struct.pack(">I", len(body)) + body)

    def send_json(self, ty, v):
        self.send(ty, json.dumps(v).encode())

    def recv(self, timeout=60):
        self.s.settimeout(timeout)
        while True:
            if len(self.buf) >= 4:
                n = struct.unpack(">I", self.buf[:4])[0]
                if len(self.buf) >= 4 + n:
                    body, self.buf = self.buf[4:4 + n], self.buf[4 + n:]
                    if self.open:
                        body = self.open.open(bytes(4) + self.received.to_bytes(8, "big"), body)
                        self.received += 1
                    return body[0], body[1:]
            d = self.s.recv(65536)
            if not d:
                raise EOFError("closed")
            self.buf += d

    def expect(self, want, timeout=60, skip=(0x42, 0x51)):
        while True:
            ty, p = self.recv(timeout)
            if ty in skip:
                continue
            if ty != want:
                raise AssertionError(f"expected {want:#x}, got {ty:#x} {p[:80]!r}")
            return p

    def handshake(self):
        k = secrets.randbelow(N - 1) + 1
        pub = point_bytes(ec_mul(k, G))
        self.send_json(0x01, {"id": self.ident, "name": self.name, "key": pub.hex()})
        ch = json.loads(self.expect(0x02))
        pc_key = bytes.fromhex(ch["key"])
        shared = ec_mul(k, bytes_point(pc_key))[0].to_bytes(32, "big")
        self.th = hashlib.sha256(pub + pc_key).digest()
        keys = hkdf(b"justos-connect 1", shared, self.th, 64)
        self.seal, self.open = Gcm(keys[:32]), Gcm(keys[32:])
        return ch

    def mac(self, key, label):
        return hmac.new(key, label + self.th, hashlib.sha256).hexdigest()


def main():
    image = os.path.realpath(sys.argv[1])
    verbose = os.environ.get("VERBOSE") == "1"
    work = tempfile.mkdtemp()
    failures = []

    def check(ok, what):
        print(("ok   " if ok else "FAIL ") + what, flush=True)
        if not ok:
            failures.append(what)

    def free_port(kind):
        s = socket.socket(socket.AF_INET, kind)
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        s.close()
        return p

    tcp_port, udp_port = free_port(socket.SOCK_STREAM), free_port(socket.SOCK_DGRAM)
    disk = os.path.join(work, "disk.img")
    subprocess.run(["cp", "--sparse=always", image, disk], check=True)
    ser = os.path.join(work, "serial.sock")
    log_path = os.path.join(work, "serial.log")
    args = [os.environ.get("QEMU", "qemu-system-x86_64"), "-m", "768M", "-display", "none", "-no-reboot",
            "-chardev", f"socket,id=ser,path={ser},server=on,wait=off,logfile={log_path}", "-serial", "chardev:ser",
            "-drive", f"file={disk},format=raw,if=ide",
            "-nic", f"user,hostfwd=tcp:127.0.0.1:{tcp_port}-:47801,hostfwd=udp:127.0.0.1:{udp_port}-:47800"]
    proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    console = None

    def log():
        try:
            text = open(log_path, "rb").read().decode("utf-8", "replace")
        except OSError:
            return ""
        return re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", text).replace("\r", "")

    def wait_log(pattern, timeout, start=0):
        end = time.time() + timeout
        while time.time() < end:
            m = re.search(pattern, log()[start:])
            if m:
                return m
            time.sleep(0.3)
        return None

    def serial(line):
        nonlocal console
        if console is None:
            console = socket.socket(socket.AF_UNIX)
            console.connect(ser)
            # What justOS writes goes to the log file too; read it off the
            # socket all the same, or once its buffer is full QEMU stops
            # passing the serial port's output on.
            def drain(s):
                try:
                    while s.recv(65536):
                        pass
                except OSError:
                    pass
            threading.Thread(target=drain, args=(console,), daemon=True).start()
        console.sendall(line.encode() + b"\r")

    try:
        # Found on the network.
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        u.settimeout(1)
        info = None
        end = time.time() + 180
        while time.time() < end and info is None:
            u.sendto(b"JUSTOS-CONNECT?", ("127.0.0.1", udp_port))
            try:
                data, _ = u.recvfrom(2048)
                if data.startswith(b"JUSTOS-CONNECT "):
                    info = json.loads(data[15:])
            except (socket.timeout, OSError):
                time.sleep(1)
        check(info is not None and info.get("port") == 47801, f"discovery answered: {info}")
        if info is None:
            return 1
        time.sleep(3)  # the desktop settles

        # Pairing with the code from the log.
        ident = secrets.token_hex(8)
        ph = Phone(tcp_port, ident, "Pixel di prova")
        ch = ph.handshake()
        check(ch.get("known") is False, "a new phone is not known")
        mark = len(log())
        ph.send(0x04)
        m = wait_log(r"pairing code for Pixel di prova \([^)]*\): (\d{6})", 30, mark)
        check(m is not None, "pairing code shown")
        code = m.group(1)
        ph.send_json(0x05, {"mac": ph.mac(code.encode(), b"pair")})
        welcome = json.loads(ph.expect(0x06))
        token = bytes.fromhex(welcome.get("token", ""))
        check(len(token) == 32, "paired: a token came")
        check(welcome.get("mac") == ph.mac(token, b"pc"), "the computer proved the token")

        # A file to the computer.
        data = secrets.token_bytes(150_000)
        ph.send_json(0x20, {"name": "prova connect.bin", "size": len(data)})
        for i in range(0, len(data), 65536):
            ph.send(0x21, data[i:i + 65536])
        ph.send(0x22)
        done = json.loads(ph.expect(0x23, 60))
        check(done.get("ok") is True, f"file received: {done}")
        mark = len(log())
        serial('sha256 "sys:/home/Scaricati/prova connect.bin"')
        want = hashlib.sha256(data).hexdigest()
        check(wait_log(want, 20, mark) is not None, "file the same on justOS (sha256)")

        # The screen.
        ph.send_json(0x30, {"on": True, "w": 640, "q": 60})
        p = ph.expect(0x31, 60)
        w, h, sw, sh = struct.unpack(">4H", p[:8])
        jpeg = p[8:]
        check(jpeg[:2] == b"\xff\xd8" and jpeg[-2:] == b"\xff\xd9" and w > 0 and h > 0,
              f"a frame of the screen: {w}x{h} of {sw}x{sh}, {len(jpeg)} bytes")
        open(os.path.join(work, "frame.jpg"), "wb").write(jpeg)
        try:
            from PIL import Image
            img = Image.open(os.path.join(work, "frame.jpg"))
            img.load()
            check(img.size == (w, h), f"the frame decodes ({img.size})")
            if os.environ.get("FRAME_OUT"):
                shutil.copy(os.path.join(work, "frame.jpg"), os.environ["FRAME_OUT"])
        except ImportError:
            print("skip decoding the frame (no Pillow)")
        # The pointer moves, a new frame comes once the last was shown. The
        # tap goes elsewhere than where the move ends: read together, the two
        # must not cancel out.
        ph.send(0x32)
        ph.send(0x10, struct.pack(">hh", 120, 80))
        ph.send(0x15, struct.pack(">HH", 16384, 49152))
        p = ph.expect(0x31, 30)
        check(len(p) > 8, "another frame after moving the pointer")
        ph.send(0x32)
        ph.send_json(0x30, {"on": False})
        ph.send_json(0x41, {"level": 57, "charging": True})
        ph.send_json(0x40, {"app": "Messaggi", "title": "Anna", "text": "Ciao!"})
        ph.send(0x42, "dal telefono".encode())

        # A file to the phone.
        time.sleep(0.5)
        mark = len(log())
        serial("connect")
        check(wait_log(r"batteria 57% in carica", 10, mark) is not None, "battery shown in connect")
        serial('connect send "sys:/home/Scaricati/prova connect.bin"')
        begin = json.loads(ph.expect(0x20, 30, skip=(0x42, 0x51, 0x31)))
        got = b""
        while True:
            ty, p = ph.recv(30)
            if ty == 0x21:
                got += p
            elif ty == 0x22:
                break
            elif ty in (0x42, 0x51, 0x31):
                continue
            else:
                raise AssertionError(f"unexpected {ty:#x}")
        ph.send_json(0x23, {"name": begin["name"], "ok": True})
        check(begin.get("name") == "prova connect.bin" and got == data, f"file sent to the phone: {begin}")

        ph.send(0x50)
        ph.expect(0x51, 10, skip=(0x42,))
        check(True, "ping")
        ph.s.close()
        time.sleep(1)

        # Again, with the token.
        ph = Phone(tcp_port, ident, "Pixel di prova")
        ch = ph.handshake()
        check(ch.get("known") is True, "the phone is known now")
        ph.send_json(0x03, {"mac": ph.mac(token, b"auth")})
        welcome = json.loads(ph.expect(0x06))
        check("token" not in welcome and welcome.get("mac") == ph.mac(token, b"pc"), "connected again with the token")
        ph.s.close()
        time.sleep(1)

        # A wrong token is refused.
        ph = Phone(tcp_port, ident, "Pixel di prova")
        ph.handshake()
        ph.send_json(0x03, {"mac": ph.mac(b"x" * 32, b"auth")})
        try:
            ty, _ = ph.recv(15)
        except (EOFError, OSError):
            ty = None
        check(ty == 0x07, "a wrong token is refused")
        ph.s.close()

        mark = len(log())
        serial("connect forget Pixel di prova")
        time.sleep(1)
        serial("connect")
        time.sleep(2)
        check("abbinato: Pixel di prova" not in log()[mark:], "forgotten")
        check("justOS stopped" not in log(), "justOS still running")
    finally:
        proc.kill()
        if verbose or failures:
            print(log()[-6000:])
        shutil.rmtree(work, ignore_errors=True)
    print("ok connect" if not failures else f"FAILED: {len(failures)}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
