#!/usr/bin/env python3
"""A Wi-Fi access point simulator for justOS's tests.

justOS's test radio carries 802.11 frames inside Ethernet frames of type
0x88B5 (see system/libs/justwifi/src/sim.rs); QEMU sends a virtual network
card's frames to this program as UDP datagrams (-netdev dgram). The
simulator plays one or more access points, each on its own channel:
beacons, probe responses, open and SAE authentication, association, the
4-way and group key handshakes as authenticator, CCMP, protected management
frames (BIP), and behind each one a tiny network: a DHCP server, and a
gateway that answers ARP and ping.

It is written independently of justOS's Rust code, from IEEE 802.11-2020:
the ciphers come from Python's hashlib and the 'cryptography' package
(AES-CCM, AES key wrap, AES-CMAC); the elliptic curve and SAE are plain
Python integers.

Usage: apsim.py --listen PORT --peer PORT [--rekey S] [--log FILE]
Every event is a line on stdout: "ap <ssid>: ...".
"""

import argparse
import hashlib
import hmac
import os
import select
import socket
import struct
import sys
import time

from cryptography.hazmat.primitives.ciphers.aead import AESCCM
from cryptography.hazmat.primitives.keywrap import aes_key_wrap, aes_key_unwrap
from cryptography.hazmat.primitives.cmac import CMAC
from cryptography.hazmat.primitives.ciphers import algorithms

ETHERTYPE_SIM = 0x88B5
BROADCAST = b"\xff" * 6
OUI = b"\x00\x0f\xac"

def log(msg):
    print(msg, flush=True)

# ---------------------------------------------------------------------------
# Crypto helpers

def hmac_sha1(k, *parts):
    return hmac.new(k, b"".join(parts), hashlib.sha1).digest()

def hmac_sha256(k, *parts):
    return hmac.new(k, b"".join(parts), hashlib.sha256).digest()

def prf_sha1(k, label, data, n):
    out = b""
    i = 0
    while len(out) < n:
        out += hmac_sha1(k, label, b"\x00", data, bytes([i]))
        i += 1
    return out[:n]

def kdf_sha256(k, label, context, bits):
    out = b""
    i = 1
    while len(out) * 8 < bits:
        out += hmac_sha256(k, struct.pack("<H", i), label, context, struct.pack("<H", bits))
        i += 1
    return out[: bits // 8]

def aes_cmac(k, data):
    c = CMAC(algorithms.AES(k))
    c.update(data)
    return c.finalize()

# ---------------------------------------------------------------------------
# P-256 and SAE (IEEE 802.11-2020 12.4)

P = 0xffffffff00000001000000000000000000000000ffffffffffffffffffffffff
A = P - 3
B = 0x5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b
N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551

def ec_add(p1, p2):
    if p1 is None:
        return p2
    if p2 is None:
        return p1
    (x1, y1), (x2, y2) = p1, p2
    if x1 == x2:
        if (y1 + y2) % P == 0:
            return None
        l = (3 * x1 * x1 + A) * pow(2 * y1, P - 2, P) % P
    else:
        l = (y2 - y1) * pow(x2 - x1, P - 2, P) % P
    x3 = (l * l - x1 - x2) % P
    return (x3, (l * (x1 - x3) - y1) % P)

def ec_mul(k, pt):
    r = None
    while k:
        if k & 1:
            r = ec_add(r, pt)
        pt = ec_add(pt, pt)
        k >>= 1
    return r

def ec_neg(pt):
    return (pt[0], (-pt[1]) % P)

def on_curve(x, y):
    return (y * y - (x ** 3 + A * x + B)) % P == 0

def is_qr(v):
    return pow(v, (P - 1) // 2, P) in (0, 1)

def sqrt(v):
    return pow(v, (P + 1) // 4, P)

def i2b(v, n=32):
    return v.to_bytes(n, "big")

def max_min(a, b):
    return (a + b) if a > b else (b + a)

def pwe_hunt_and_peck(a, b, password):
    key = max_min(a, b)
    found = None
    for counter in range(1, 41):
        seed = hmac_sha256(key, password, bytes([counter]))
        x = int.from_bytes(kdf_sha256(seed, b"SAE Hunting and Pecking", i2b(P), 256), "big")
        if x >= P:
            continue
        v = (x ** 3 + A * x + B) % P
        if not is_qr(v):
            continue
        if found is None:
            y = sqrt(v)
            if (y & 1) != (seed[-1] & 1):
                y = P - y
            found = (x, y)
    return found

def hkdf_expand(prk, info, n):
    out, t, i = b"", b"", 1
    while len(out) < n:
        t = hmac_sha256(prk, t, info, bytes([i]))
        out += t
        i += 1
    return out[:n]

def sswu(u):
    z = P - 10
    m = (z * z * pow(u, 4, P) + z * u * u) % P
    if m == 0:
        x1 = B * pow(z * A, P - 2, P) % P
    else:
        x1 = (-B * pow(A, P - 2, P)) * (1 + pow(m, P - 2, P)) % P
    gx1 = (x1 ** 3 + A * x1 + B) % P
    if is_qr(gx1):
        x, v = x1, gx1
    else:
        x = z * u * u * x1 % P
        v = (x ** 3 + A * x + B) % P
    y = sqrt(v)
    if (y & 1) != (u & 1):
        y = P - y
    return (x, y)

def sae_pt(ssid, password, ident=b""):
    seed = hmac_sha256(ssid, password + ident)  # HKDF-Extract(salt=ssid)
    pts = []
    for label in (b"SAE Hash to Element u1 P1", b"SAE Hash to Element u2 P2"):
        u = int.from_bytes(hkdf_expand(seed, label, 48), "big") % P
        pts.append(sswu(u))
    return ec_add(pts[0], pts[1])

def pwe_h2e(pt, a, b):
    val = int.from_bytes(hmac_sha256(b"\x00" * 32, max_min(a, b)), "big")
    val = val % (N - 1) + 1
    return ec_mul(val, pt)

class SaeAp:
    """The access point's side of SAE."""

    def __init__(self, pwe):
        self.pwe = pwe
        self.rand = int.from_bytes(os.urandom(40), "big") % (N - 2) + 2
        mask = int.from_bytes(os.urandom(40), "big") % (N - 2) + 2
        self.scalar = (self.rand + mask) % N
        self.element = ec_neg(ec_mul(mask, pwe))

    def commit(self):
        return struct.pack("<H", 19) + i2b(self.scalar) + i2b(self.element[0]) + i2b(self.element[1])

    def process(self, body):
        group = struct.unpack("<H", body[:2])[0]
        if group != 19 or len(body) < 98:
            return False
        ps = int.from_bytes(body[2:34], "big")
        ex, ey = int.from_bytes(body[34:66], "big"), int.from_bytes(body[66:98], "big")
        if not (1 < ps < N) or not on_curve(ex, ey):
            return False
        self.ps, self.pe = ps, (ex, ey)
        k = ec_mul(self.rand, ec_add(ec_mul(ps, self.pwe), self.pe))[0]
        keyseed = hmac_sha256(b"\x00" * 32, i2b(k))
        ctx = i2b((self.scalar + ps) % N)
        keys = kdf_sha256(keyseed, b"SAE KCK and PMK", ctx, 512)
        self.kck, self.pmk = keys[:32], keys[32:]
        return True

    def _confirm(self, sc, s1, e1, s2, e2):
        return hmac_sha256(self.kck, struct.pack("<H", sc), i2b(s1), i2b(e1[0]), i2b(e1[1]), i2b(s2), i2b(e2[0]), i2b(e2[1]))

    def check_confirm(self, body):
        sc = struct.unpack("<H", body[:2])[0]
        want = self._confirm(sc, self.ps, self.pe, self.scalar, self.element)
        return hmac.compare_digest(want, body[2:34])

    def confirm(self):
        return struct.pack("<H", 1) + self._confirm(1, self.scalar, self.element, self.ps, self.pe)

# ---------------------------------------------------------------------------
# 802.11

def mac_str(m):
    return ":".join("%02x" % b for b in m)

def ie(eid, data):
    return bytes([eid, len(data)]) + data

def mgmt_hdr(subtype, da, sa, bssid, seq, flags=0):
    return bytes([subtype << 4, flags, 0, 0]) + da + sa + bssid + struct.pack("<H", seq << 4)

def ccmp_aad_nonce(hdr, pn):
    fc0, fc1 = hdr[0], hdr[1]
    is_data = (fc0 >> 2) & 3 == 2
    qos = is_data and fc0 & 0x80
    m0 = fc0 & 0x8F if is_data else fc0
    m1 = (fc1 & ~0x38 & (0x7F if qos else 0xFF)) | 0x40
    aad = bytes([m0, m1]) + hdr[4:22] + bytes([hdr[22] & 0x0F, 0])
    at = 24
    if fc1 & 3 == 3:
        aad += hdr[24:30]
        at = 30
    prio = 0
    if qos:
        prio = hdr[at] & 0x0F
        aad += bytes([prio, 0])
    flags = prio | (0x10 if (fc0 >> 2) & 3 == 0 else 0)
    nonce = bytes([flags]) + hdr[10:16] + pn.to_bytes(6, "big")
    return aad, nonce

def ccmp_encrypt(tk, hdr, pn, keyid, body):
    aad, nonce = ccmp_aad_nonce(hdr, pn)
    p = pn.to_bytes(6, "little")
    ccmp_hdr = bytes([p[0], p[1], 0, 0x20 | (keyid << 6), p[2], p[3], p[4], p[5]])
    return ccmp_hdr + AESCCM(tk, tag_length=8).encrypt(nonce, body, aad)

def ccmp_decrypt(tk, hdr, body):
    if len(body) < 16:
        return None
    pn = int.from_bytes(body[0:2] + body[4:8], "little")
    aad, nonce = ccmp_aad_nonce(hdr, pn)
    try:
        return pn, AESCCM(tk, tag_length=8).decrypt(nonce, body[8:], aad)
    except Exception:
        return None

def bip_protect(igtk, keyid, ipn, frame):
    hdr, body = frame[:24], frame[24:]
    aad = bytes([hdr[0], hdr[1] & ~0x38]) + hdr[4:22]
    mmie = bytes([76, 16]) + struct.pack("<H", keyid) + ipn.to_bytes(6, "little")
    mic = aes_cmac(igtk, aad + body + mmie + b"\x00" * 8)[:8]
    return frame + mmie + mic

def rsn_ie(akms, caps):
    d = struct.pack("<H", 1) + OUI + b"\x04" + struct.pack("<H", 1) + OUI + b"\x04"
    d += struct.pack("<H", len(akms)) + b"".join(OUI + bytes([a]) for a in akms) + struct.pack("<H", caps)
    return ie(48, d)

LLC = b"\xaa\xaa\x03\x00\x00\x00"

def checksum(data):
    if len(data) % 2:
        data += b"\x00"
    s = sum(struct.unpack("!%dH" % (len(data) // 2), data))
    while s > 0xFFFF:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF

def ipv4(src, dst, proto, payload, ident=1):
    h = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(payload), ident, 0, 64, proto, 0, src, dst)
    h = h[:10] + struct.pack("!H", checksum(h)) + h[12:]
    return h + payload

def udp(sport, dport, data):
    return struct.pack("!HHHH", sport, dport, 8 + len(data), 0) + data

# ---------------------------------------------------------------------------
# The access points

class Sta:
    def __init__(self, mac):
        self.mac = mac
        self.state = "new"
        self.sae = None
        self.pmk = None
        self.ptk = None
        self.tk = None
        self.pn = 0
        self.rx_pn = -1
        self.replay = 0
        self.anonce = None
        self.assoc_rsn = None
        self.wmm = False
        self.tries = 0
        self.timer = None
        self.ip = None
        self.pings = 0
        self.rekeyed = False
        self.up_at = None
        self.pmf = False

class Bss:
    def __init__(self, sim, index, ssid, channel, security, password, rssi):
        self.sim = sim
        self.ssid = ssid.encode()
        self.channel = channel
        self.security = security  # open, wpa2, mixed, wpa3, wpa3-h2e
        self.password = password.encode() if password else None
        self.rssi = rssi
        self.bssid = bytes([0x02, 0xA0, 0, 0, 0, index + 1])
        self.gw_mac = bytes([0x02, 0xB0, 0, 0, 0, index + 1])
        self.net = bytes([192, 168, 10 + index])
        self.gw_ip = self.net + b"\x01"
        self.seq = 0
        self.stas = {}
        self.gtk_id = 1
        self.gtk = os.urandom(16)
        self.gtk_pn = 0
        self.igtk = os.urandom(16)
        self.igtk_id = 4
        self.igtk_pn = 0
        self.next_beacon = 0
        if security == "open":
            self.akms, self.caps = [], 0
        elif security == "wpa2":
            self.akms, self.caps = [2], 0
        elif security == "mixed":
            self.akms, self.caps = [2, 8], 0x80
        else:
            self.akms, self.caps = [8], 0xC0
        self.h2e = security == "wpa3-h2e"
        self.pt = sae_pt(self.ssid, self.password) if self.akms and 8 in self.akms else None

    def log(self, msg):
        log("ap %s: %s" % (self.ssid.decode(), msg))

    def next_seq(self):
        self.seq = (self.seq + 1) & 0xFFF
        return self.seq

    def tx(self, frame):
        self.sim.send(self.channel, self.rssi, frame)

    def rsn(self):
        return rsn_ie(self.akms, self.caps) if self.akms else b""

    def elements(self):
        rates = b"\x82\x84\x8b\x96\x0c\x12\x18\x24" if self.channel <= 14 else b"\x8c\x12\x98\x24\xb0\x48\x60\x6c"
        e = ie(0, self.ssid) + ie(1, rates)
        if self.channel <= 14:
            e += ie(3, bytes([self.channel])) + ie(50, b"\x30\x48\x60\x6c")
        e += self.rsn()
        e += ie(45, b"\x0c\x00\x17\xff" + b"\x00" * 22)
        e += ie(61, bytes([self.channel]) + b"\x00" * 21)
        if self.h2e:
            e += ie(244, b"\x20")
        # WMM parameter element.
        e += ie(221, b"\x00\x50\xf2\x02\x01\x01\x00\x00" + b"\x03\xa4\x00\x00\x27\xa4\x00\x00\x42\x43\x5e\x00\x62\x32\x2f\x00")
        return e

    def capability(self):
        return 0x0401 | (0x10 if self.akms else 0)

    def beacon(self, da=BROADCAST, probe=False):
        ts = int(time.monotonic() * 1e6)
        body = struct.pack("<QHH", ts, 100, self.capability()) + self.elements()
        return mgmt_hdr(5 if probe else 8, da, self.bssid, self.bssid, self.next_seq()) + body

    def tick(self, now):
        if now >= self.next_beacon:
            self.tx(self.beacon())
            self.next_beacon = now + 0.1
        for sta in list(self.stas.values()):
            if sta.timer and now >= sta.timer[0]:
                what = sta.timer[1]
                sta.timer = None
                what(sta)

    # -- management ---------------------------------------------------------

    def mgmt(self, sub, f, body):
        sa = f[10:16]
        if sub == 4:  # probe request
            ssid = None
            for eid, d in iter_ies(body):
                if eid == 0:
                    ssid = d
            if ssid in (b"", self.ssid):
                self.tx(self.beacon(sa, probe=True))
            return
        if f[4:10] != self.bssid:
            return
        sta = self.stas.get(sa)
        if sub == 11:
            self.auth(sa, body)
        elif sub == 0 and sta:
            self.assoc(sta, body)
        elif sub in (10, 12) and sta:
            if f[1] & 0x40:
                r = ccmp_decrypt(sta.tk, f[:24], body) if sta.tk else None
                if not r:
                    return
                body = r[1]
            self.log("deauthenticated by %s, reason %d" % (mac_str(sa), struct.unpack("<H", body[:2])[0]))
            del self.stas[sa]

    def auth_frame(self, da, alg, tseq, status, rest=b""):
        return mgmt_hdr(11, da, self.bssid, self.bssid, self.next_seq()) + struct.pack("<HHH", alg, tseq, status) + rest

    def auth(self, sa, body):
        alg, tseq, status = struct.unpack("<HHH", body[:6])
        rest = body[6:]
        if alg == 0 and tseq == 1:
            if self.akms and 2 not in self.akms:
                self.tx(self.auth_frame(sa, 0, 2, 13))  # SAE only
                return
            self.stas[sa] = Sta(sa)
            self.stas[sa].state = "auth"
            self.log("open authentication from %s" % mac_str(sa))
            self.tx(self.auth_frame(sa, 0, 2, 0))
        elif alg == 3 and tseq == 1:
            if 8 not in self.akms:
                self.tx(self.auth_frame(sa, 3, 2, 13))
                return
            if self.h2e and status != 126:
                self.log("SAE commit without hash-to-element refused")
                self.tx(self.auth_frame(sa, 3, 1, 126))
                return
            sta = self.stas.get(sa)
            if sta is None or sta.state not in ("sae-commit",):
                sta = Sta(sa)
                self.stas[sa] = sta
            if status == 126:
                pwe = pwe_h2e(self.pt, self.bssid, sa)
            else:
                pwe = pwe_hunt_and_peck(self.bssid, sa, self.password)
            sta.sae = SaeAp(pwe)
            if not sta.sae.process(rest):
                self.log("bad SAE commit")
                return
            sta.state = "sae-commit"
            self.log("SAE commit from %s (%s)" % (mac_str(sa), "hash-to-element" if status == 126 else "hunting and pecking"))
            self.tx(self.auth_frame(sa, 3, 1, status, sta.sae.commit()))
        elif alg == 3 and tseq == 2:
            sta = self.stas.get(sa)
            if not sta or not sta.sae:
                return
            if not sta.sae.check_confirm(rest):
                self.log("SAE confirm from %s wrong: wrong password" % mac_str(sa))
                return
            sta.pmk = sta.sae.pmk
            sta.state = "auth"
            self.log("SAE authenticated %s" % mac_str(sa))
            self.tx(self.auth_frame(sa, 3, 2, 0, sta.sae.confirm()))

    def assoc(self, sta, body):
        if sta.state not in ("auth", "assoc", "up"):
            return
        ies = dict()
        for eid, d in iter_ies(body[4:]):
            ies.setdefault(eid, d)
            if eid == 221 and d[:4] == b"\x00\x50\xf2\x02":
                sta.wmm = True
        status = 0
        rsn = ies.get(48)
        if self.akms:
            if rsn is None:
                status = 43
            else:
                akm = rsn[14 + 3]
                caps = struct.unpack("<H", rsn[18:20])[0] if len(rsn) >= 20 else 0
                if akm not in self.akms:
                    status = 43
                elif self.caps & 0x40 and not caps & 0x80:
                    status = 31  # PMF required
                sta.akm = akm
                sta.pmf = bool(self.caps & 0x80 and caps & 0x80)
                sta.assoc_rsn = ie(48, rsn)
                if akm == 2 and sta.pmk is None:
                    sta.pmk = hashlib.pbkdf2_hmac("sha1", self.password, self.ssid, 4096, 32)
                if akm == 8 and sta.pmk is None:
                    status = 30 if False else 1
        resp = mgmt_hdr(1, sta.mac, self.bssid, self.bssid, self.next_seq()) + struct.pack("<HHH", self.capability(), status, 0xC001)
        resp += ie(1, b"\x82\x84\x8b\x96\x0c\x12\x18\x24")
        self.tx(resp)
        if status:
            self.log("association of %s refused (%d)" % (mac_str(sta.mac), status))
            return
        self.log("associated %s%s%s" % (mac_str(sta.mac), " WMM" if sta.wmm else "", " PMF" if sta.pmf else ""))
        sta.state = "assoc"
        if not self.akms:
            self.station_up(sta)
        else:
            sta.anonce = os.urandom(32)
            sta.tries = 0
            self.send_msg1(sta)

    # -- EAPOL --------------------------------------------------------------

    def version(self, sta):
        return {2: 2, 6: 3, 8: 0}[sta.akm]

    def mic(self, sta, kck, data):
        if sta.akm == 2:
            return hmac_sha1(kck, data)[:16]
        return aes_cmac(kck, data)

    def eapol_key(self, sta, info, nonce, data=b"", rsc=0, mic_key=None):
        sta.replay += 1
        body = struct.pack("!BHHQ", 2, info, 16, sta.replay) + nonce + b"\x00" * 16 + rsc.to_bytes(8, "little") + b"\x00" * 8
        frame = struct.pack("!BBH", 2, 3, 95 + len(data)) + body + b"\x00" * 16 + struct.pack("!H", len(data)) + data
        if mic_key:
            m = self.mic(sta, mic_key, frame)
            frame = frame[:81] + m + frame[97:]
        return frame

    def send_msg1(self, sta):
        info = self.version(sta) | 0x08 | 0x80
        self.send_data(sta, self.bssid, 0x888E, self.eapol_key(sta, info, sta.anonce), encrypt=False)
        sta.state = "msg1"
        sta.timer = (time.monotonic() + 1.0, self.msg1_timeout)

    def msg1_timeout(self, sta):
        sta.tries += 1
        if sta.tries >= 3:
            self.log("4-way handshake with %s timed out: deauthenticating (reason 15)" % mac_str(sta.mac))
            self.tx(mgmt_hdr(12, sta.mac, self.bssid, self.bssid, self.next_seq()) + struct.pack("<H", 15))
            del self.stas[sta.mac]
            return
        self.send_msg1(sta)

    def derive_ptk(self, sta, snonce):
        a, b = sorted([self.bssid, sta.mac])
        n1, n2 = sorted([sta.anonce, snonce])
        data = a + b + n1 + n2
        if sta.akm == 2:
            k = prf_sha1(sta.pmk, b"Pairwise key expansion", data, 48)
        else:
            k = kdf_sha256(sta.pmk, b"Pairwise key expansion", data, 384)
        return k[:16], k[16:32], k[32:48]

    def gtk_kde(self):
        return b"\xdd" + bytes([6 + 16]) + OUI + b"\x01" + bytes([self.gtk_id, 0]) + self.gtk

    def igtk_kde(self):
        return b"\xdd" + bytes([4 + 8 + 16]) + OUI + b"\x09" + struct.pack("<H", self.igtk_id) + self.igtk_pn.to_bytes(6, "little") + self.igtk

    def wrap(self, kek, data):
        if len(data) % 8 or len(data) < 16:
            data += b"\xdd"
            while len(data) % 8 or len(data) < 16:
                data += b"\x00"
        return aes_key_wrap(kek, data)

    def eapol(self, sta, frame):
        if len(frame) < 99 or frame[1] != 3:
            return
        info = struct.unpack("!H", frame[5:7])[0]
        replay = struct.unpack("!Q", frame[9:17])[0]
        nonce = frame[17:49]
        mic = frame[81:97]
        kd_len = struct.unpack("!H", frame[97:99])[0]
        key_data = frame[99:99 + kd_len]
        zeroed = frame[:81] + b"\x00" * 16 + frame[97:4 + struct.unpack("!H", frame[2:4])[0]]
        if replay != sta.replay:
            self.log("EAPOL with old replay counter dropped")
            return
        if info & 0x08 and sta.state == "msg1":
            kck, kek, tk = self.derive_ptk(sta, nonce)
            if not hmac.compare_digest(self.mic(sta, kck, zeroed), mic):
                self.log("message 2 from %s has a wrong MIC: wrong password" % mac_str(sta.mac))
                return
            if key_data != sta.assoc_rsn:
                self.log("message 2 RSN element differs from the association")
                return
            sta.ptk = (kck, kek, tk)
            sta.timer = None
            data = self.rsn() + self.gtk_kde()
            if sta.pmf:
                data += self.igtk_kde()
            info = self.version(sta) | 0x08 | 0x40 | 0x80 | 0x100 | 0x200 | 0x1000
            m3 = self.eapol_key(sta, info, sta.anonce, self.wrap(kek, data), self.gtk_pn, kck)
            sta.state = "msg3"
            self.send_data(sta, self.bssid, 0x888E, m3, encrypt=False)
        elif info & 0x08 and sta.state == "msg3":
            kck, kek, tk = sta.ptk
            if not hmac.compare_digest(self.mic(sta, kck, zeroed), mic):
                self.log("message 4 has a wrong MIC")
                return
            sta.tk = tk
            sta.pn = 0
            self.log("4-way handshake done with %s (AKM %d)" % (mac_str(sta.mac), sta.akm))
            self.station_up(sta)
        elif not info & 0x08 and sta.state == "rekey":
            kck = sta.ptk[0]
            if not hmac.compare_digest(self.mic(sta, kck, zeroed), mic):
                self.log("group message 2 has a wrong MIC")
                return
            sta.state = "up"
            self.log("group key handshake done with %s (key %d)" % (mac_str(sta.mac), self.gtk_id))
            sta.timer = (time.monotonic() + 0.5, self.probe_after_rekey)

    def station_up(self, sta):
        sta.state = "up"
        sta.up_at = time.monotonic()
        if self.sim.rekey and self.akms:
            sta.timer = (time.monotonic() + self.sim.rekey, self.rekey)

    def rekey(self, sta):
        if sta.state != "up":
            return
        self.gtk_id = 2 if self.gtk_id == 1 else 1
        self.gtk = os.urandom(16)
        self.gtk_pn = 0
        data = self.gtk_kde()
        if sta.pmf:
            data += self.igtk_kde()
        kck, kek, _ = sta.ptk
        info = self.version(sta) | 0x80 | 0x100 | 0x200 | 0x1000
        m1 = self.eapol_key(sta, info, b"\x00" * 32, self.wrap(kek, data), 0, kck)
        sta.state = "rekey"
        self.log("new group key %d for %s" % (self.gtk_id, mac_str(sta.mac)))
        self.send_data(sta, self.bssid, 0x888E, m1)

    def probe_after_rekey(self, sta):
        """A broadcast ARP request for the station's address, under the new
        group key: its answer proves the key works."""
        if not sta.ip:
            return
        sta.rekeyed = True
        arp = struct.pack("!HHBBH", 1, 0x0800, 6, 4, 1) + self.gw_mac + self.gw_ip + b"\x00" * 6 + sta.ip
        self.send_data(sta, self.gw_mac, 0x0806, arp, broadcast=True)

    # -- data ---------------------------------------------------------------

    def send_data(self, sta, src, ethertype, payload, encrypt=True, broadcast=False, amsdu=False):
        da = BROADCAST if broadcast else sta.mac
        qos = sta.wmm
        fc0 = 0x88 if qos else 0x08
        hdr = bytes([fc0, 0x02, 0, 0]) + da + self.bssid + src + struct.pack("<H", self.next_seq() << 4)
        body = LLC + struct.pack("!H", ethertype) + payload
        if qos:
            tid = 7 if ethertype == 0x888E else 0
            hdr += bytes([tid | (0x80 if amsdu else 0), 0])
            if amsdu:
                sub = da + src + struct.pack("!H", len(body)) + body
                body = sub
        if encrypt and (sta.tk or broadcast) and self.akms:
            hdr = hdr[:1] + bytes([hdr[1] | 0x40]) + hdr[2:]
            if broadcast:
                self.gtk_pn += 1
                body = ccmp_encrypt(self.gtk, hdr, self.gtk_pn, self.gtk_id, body)
            else:
                sta.pn += 1
                body = ccmp_encrypt(sta.tk, hdr, sta.pn, 0, body)
        self.tx(hdr + body)

    def data(self, f, body):
        sa = f[10:16]
        sta = self.stas.get(sa)
        if not sta or f[1] & 3 != 1 or f[4:10] != self.bssid:
            return
        hlen = 24
        if f[0] & 0x80:
            hlen += 2
        if (f[0] >> 4) & 4:
            return  # null data (power save)
        body = f[hlen:]
        if f[1] & 0x40:
            if not sta.tk:
                return
            r = ccmp_decrypt(sta.tk, f[:hlen], body)
            if not r:
                self.log("CCMP MIC failure from %s" % mac_str(sa))
                return
            pn, body = r
            if pn <= sta.rx_pn:
                self.log("replayed frame dropped")
                return
            sta.rx_pn = pn
        elif sta.tk and self.akms:
            # Plain data from a station with keys: only EAPOL is allowed.
            pass
        if body[:6] != LLC:
            return
        ethertype = struct.unpack("!H", body[6:8])[0]
        payload = body[8:]
        da = f[16:22]
        if ethertype == 0x888E:
            self.eapol(sta, payload)
            return
        if self.akms and not f[1] & 0x40:
            self.log("unencrypted data from %s dropped" % mac_str(sa))
            return
        if ethertype == 0x0806:
            self.arp(sta, payload)
        elif ethertype == 0x0800:
            self.ip(sta, payload)

    def arp(self, sta, p):
        op = struct.unpack("!H", p[6:8])[0]
        sender_mac, sender_ip, target_ip = p[8:14], p[14:18], p[24:28]
        if op == 1 and target_ip == self.gw_ip:
            reply = struct.pack("!HHBBH", 1, 0x0800, 6, 4, 2) + self.gw_mac + self.gw_ip + sender_mac + sender_ip
            self.send_data(sta, self.gw_mac, 0x0806, reply)
        elif op == 2 and sta.rekeyed and sender_ip == sta.ip:
            sta.rekeyed = False
            self.log("ARP reply from %s after the new group key: the group key works" % mac_str(sta.mac))

    def ip(self, sta, p):
        proto = p[9]
        src, dst = p[12:16], p[16:20]
        ihl = (p[0] & 0xF) * 4
        body = p[ihl:struct.unpack("!H", p[2:4])[0]]
        if proto == 17 and struct.unpack("!H", body[2:4])[0] == 67:
            self.dhcp(sta, body[8:])
        elif proto == 1 and body[0] == 8 and dst == self.gw_ip:
            sta.pings += 1
            reply = b"\x00\x00\x00\x00" + body[4:]
            reply = reply[:2] + struct.pack("!H", checksum(reply)) + reply[4:]
            self.log("ping %d from %s" % (sta.pings, socket.inet_ntoa(src)))
            # Every other answer in an A-MSDU (when the station has WMM).
            self.send_data(sta, self.gw_mac, 0x0800, ipv4(self.gw_ip, src, 1, reply), amsdu=sta.wmm and sta.pings % 2 == 0)

    def dhcp(self, sta, d):
        if len(d) < 240 or d[0] != 1:
            return
        xid = d[4:8]
        chaddr = d[28:34]
        kind = None
        i = 240
        while i < len(d) and d[i] != 255:
            if d[i] == 0:
                i += 1
                continue
            if d[i] == 53:
                kind = d[i + 2]
            i += 2 + d[i + 1]
        yiaddr = self.net + b"\x64"
        reply_kind = {1: 2, 3: 5}.get(kind)
        if not reply_kind:
            return
        r = bytearray(240)
        r[0], r[1], r[2] = 2, 1, 6
        r[4:8] = xid
        r[10] = 0x80
        r[16:20] = yiaddr
        r[20:24] = self.gw_ip
        r[28:34] = chaddr
        r[236:240] = bytes([99, 130, 83, 99])
        opts = bytes([53, 1, reply_kind, 54, 4]) + self.gw_ip + bytes([51, 4]) + struct.pack("!I", 3600)
        opts += bytes([1, 4, 255, 255, 255, 0, 3, 4]) + self.gw_ip + bytes([6, 4]) + self.gw_ip + b"\xff"
        packet = ipv4(self.gw_ip, b"\xff" * 4, 17, udp(67, 68, bytes(r) + opts))
        if reply_kind == 5:
            sta.ip = yiaddr
            self.log("DHCP: %s for %s" % (socket.inet_ntoa(yiaddr), mac_str(sta.mac)))
        # Broadcast: sent under the group key.
        self.send_data(sta, self.gw_mac, 0x0800, packet, broadcast=True)

    def deauth_all(self, protected):
        """A broadcast deauthentication, signed with BIP when `protected`."""
        f = mgmt_hdr(12, BROADCAST, self.bssid, self.bssid, self.next_seq()) + struct.pack("<H", 3)
        if protected:
            self.igtk_pn += 1
            f = bip_protect(self.igtk, self.igtk_id, self.igtk_pn, f)
        self.tx(f)

def iter_ies(b):
    i = 0
    while i + 2 <= len(b):
        n = b[i + 1]
        yield b[i], b[i + 2:i + 2 + n]
        i += 2 + n

class Sim:
    def __init__(self, listen, peer, rekey):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", listen))
        self.peer = ("127.0.0.1", peer)
        self.rekey = rekey
        self.bss = []
        self.src = bytes([0x02, 0xA0, 0, 0, 0, 0])

    def send(self, channel, rssi, frame):
        e = BROADCAST + self.src + struct.pack("!H", ETHERTYPE_SIM) + b"JWSM" + bytes([1, channel, rssi & 0xFF, 0])
        e += struct.pack("!H", len(frame)) + frame
        try:
            self.sock.sendto(e, self.peer)
        except OSError:
            pass

    def receive(self, e):
        if len(e) < 24 or struct.unpack("!H", e[12:14])[0] != ETHERTYPE_SIM or e[14:18] != b"JWSM":
            return
        channel = e[19]
        n = struct.unpack("!H", e[22:24])[0]
        f = e[24:24 + n]
        if len(f) < 24:
            return
        ftype, sub = (f[0] >> 2) & 3, f[0] >> 4
        for bss in self.bss:
            if bss.channel != channel:
                continue
            if ftype == 0:
                bss.mgmt(sub, f, f[24:])
            elif ftype == 2 and f[4:10] == bss.bssid:
                bss.data(f, f[24:])

    def control(self, line):
        """Commands on stdin: 'deauth <ssid> forged|bip|plain'."""
        parts = line.split()
        if len(parts) == 3 and parts[0] == "deauth":
            for bss in self.bss:
                if bss.ssid.decode() == parts[1]:
                    if parts[2] == "forged":
                        # Unprotected, unicast to every station: must be ignored under PMF.
                        for sta in bss.stas.values():
                            bss.tx(mgmt_hdr(12, sta.mac, bss.bssid, bss.bssid, bss.next_seq()) + struct.pack("<H", 7))
                        bss.log("forged deauthentication sent")
                    else:
                        bss.deauth_all(parts[2] == "bip")
                        bss.log("broadcast deauthentication sent (%s)" % parts[2])

    def run(self):
        while True:
            now = time.monotonic()
            for bss in self.bss:
                bss.tick(now)
            r, _, _ = select.select([self.sock, sys.stdin], [], [], 0.01)
            if self.sock in r:
                try:
                    data, _ = self.sock.recvfrom(4096)
                    self.receive(data)
                except OSError:
                    pass
            if sys.stdin in r:
                line = sys.stdin.readline()
                if not line:
                    sys.stdin = open(os.devnull)
                else:
                    self.control(line)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", type=int, required=True)
    ap.add_argument("--peer", type=int, required=True)
    ap.add_argument("--rekey", type=float, default=0, help="new group key this many seconds after joining")
    ap.add_argument("--bss", action="append", help="ssid,channel,security,password,rssi")
    args = ap.parse_args()
    sim = Sim(args.listen, args.peer, args.rekey)
    specs = args.bss or [
        "justOS-aperta,1,open,,-48",
        "justOS-wpa2,6,wpa2,password-wpa2,-61",
        "justOS-misto,11,mixed,password-misto,-70",
        "justOS-wpa3,36,wpa3,password-wpa3,-55",
        "justOS-h2e,44,wpa3-h2e,password-h2e,-66",
    ]
    for i, s in enumerate(specs):
        ssid, ch, sec, pw, rssi = s.split(",")
        sim.bss.append(Bss(sim, i, ssid, int(ch), sec, pw, int(rssi)))
        log("ap %s: channel %s, %s" % (ssid, ch, sec))
    sim.run()

if __name__ == "__main__":
    main()
