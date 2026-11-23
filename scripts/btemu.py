#!/usr/bin/env python3
"""An emulated USB Bluetooth controller for QEMU, so that justOS's
Bluetooth can be tested without Bluetooth hardware (QEMU has none, and no
host device is passed through).

It speaks the usbredir protocol (the one of usbredirhost) on a Unix socket;
QEMU attaches it with

    -chardev socket,id=bt,path=SOCKET -device usb-redir,chardev=bt,bus=u.0

and the guest sees a Realtek RTL8852BU (13d3:3571, the Bluetooth half of
an RTL8852BE combo card) at full speed: HCI commands on endpoint 0, events
on interrupt IN 0x81, ACL data on bulk 0x02/0x82, an isochronous interface
1 nobody uses. Like the real chip it starts on its ROM code: it answers
the Realtek vendor commands (ROM version 0xFC6D, security key 0xFC61,
download 0xFC20) and checks that the patch it receives is the one btrtl's
rules take from the firmware file (given with --firmware).

Two devices are near it:
- "Tastiera Emu", a BR/EDR keyboard (KeyboardOnly): Secure Simple Pairing
  with passkey entry (the controller asks the host to show the passkey,
  the keyboard "types" it), SDP with its HID descriptor (in two answers,
  with a continuation state), the HID control and interrupt channels.
  Once paired it types --text and Enter.
- "Mouse Emu", an LE mouse (no input, no output; random static address):
  SMP Secure Connections, Just Works, done here with the real
  cryptography (P-256, AES-CMAC f4/f5/f6); LE encryption only with the
  key both sides derived; a GATT server with the HID service (the report
  map takes a Read Blob). Once subscribed it moves 10 times by (10, 5)
  and keeps the left button down.

Everything it sees and does is printed with a "btemu:" prefix; the test
looks for those lines.

Usage: scripts/btemu.py --socket PATH --firmware DIR [--text TEXT]
"""
import argparse
import hashlib
import os
import socket
import struct
import sys
import threading
import time

from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers import algorithms
from cryptography.hazmat.primitives.cmac import CMAC


def log(msg):
    print(f"btemu: {msg}", flush=True)


# ---------------------------------------------------------------------------
# Bluetooth cryptography, written from the spec's definitions (values most
# significant byte first), independently of the kernel's code.

def cmac(key, msg):
    c = CMAC(algorithms.AES(key))
    c.update(msg)
    return c.finalize()


def f4(u, v, x, z):
    return cmac(x, u + v + bytes([z]))


def f5(w, n1, n2, a1, a2):
    salt = bytes.fromhex("6C888391AAF5A53860370BDB5A6083BE")
    t = cmac(salt, w)
    def part(counter):
        return cmac(t, bytes([counter]) + b"btle" + n1 + n2 + a1 + a2 + b"\x01\x00")
    return part(0), part(1)


def f6(w, n1, n2, r, io_cap, a1, a2):
    return cmac(w, n1 + n2 + r + io_cap + a1 + a2)


def rev(b):
    return bytes(reversed(b))


# ---------------------------------------------------------------------------
# Realtek firmware: what btrtl sends for a ROM version.

def rtl_patch(fw, rom, key_id=0):
    """The patch from an RTBTCore (v2) file for ROM `rom`."""
    assert fw[:8] == b"RTBTCore", "only the v2 format is emulated"
    end = len(fw) - 7
    n = struct.unpack_from("<I", fw, 16)[0]
    p = 20
    pieces = []
    for _ in range(n):
        if p + 8 > end:
            break
        op, ln = struct.unpack_from("<II", fw, p)
        p += 8
        if p + ln > end:
            break
        sec = fw[p:p + ln]
        p += ln
        if op not in (1, 2, 3) or (op == 3 and key_id == 0):
            continue
        num = struct.unpack_from("<H", sec, 0)[0]
        q = 4
        for _ in range(num):
            if q + 8 > len(sec):
                break
            eco, prio, key, _r, sl = struct.unpack_from("<BBBBI", sec, q)
            q += 8
            data = sec[q:q + sl]
            q += sl
            if eco != rom + 1 or (op == 3 and key != key_id):
                continue
            i = next((k for k, (pr, _) in enumerate(pieces) if pr >= prio), len(pieces))
            pieces.insert(i, (prio, data))
    return b"".join(d for _, d in pieces)


# ---------------------------------------------------------------------------
# usbredir

HELLO, CONNECT, DISCONNECT, RESET, IFACE_INFO, EP_INFO = 0, 1, 2, 3, 4, 5
SET_CONFIG, GET_CONFIG, CONFIG_STATUS, SET_ALT, GET_ALT, ALT_STATUS = 6, 7, 8, 9, 10, 11
START_INT, STOP_INT, INT_STATUS, CANCEL = 15, 16, 17, 21
CONTROL, BULK, INTERRUPT = 100, 101, 103
CAP_CONNECT_DEVICE_VERSION, CAP_EP_INFO_MAX_PACKET_SIZE = 1, 4
# QEMU's xHCI wants these two as well: 64-bit packet ids (the header grows
# to 16 bytes once both sides said so) and 32-bit bulk lengths.
CAP_64BITS_IDS, CAP_32BITS_BULK_LENGTH = 5, 6
ST_SUCCESS, ST_CANCELLED, ST_STALL = 0, 1, 4

VID, PID = 0x13D3, 0x3571
DEVICE_DESC = struct.pack("<BBHBBBBHHHBBBB", 18, 1, 0x0200, 0xE0, 0x01, 0x01, 64, VID, PID, 0x0000, 1, 2, 3, 1)


def config_desc():
    ifaces = b""
    # Interface 0: HCI commands, events, ACL.
    ifaces += struct.pack("<BBBBBBBBB", 9, 4, 0, 0, 3, 0xE0, 0x01, 0x01, 0)
    ifaces += struct.pack("<BBBBHB", 7, 5, 0x81, 3, 16, 1)
    ifaces += struct.pack("<BBBBHB", 7, 5, 0x02, 2, 64, 0)
    ifaces += struct.pack("<BBBBHB", 7, 5, 0x82, 2, 64, 0)
    # Interface 1: SCO audio, isochronous (alternate 0 and 1).
    for alt, size in ((0, 0), (1, 9)):
        ifaces += struct.pack("<BBBBBBBBB", 9, 4, 1, alt, 2, 0xE0, 0x01, 0x01, 0)
        ifaces += struct.pack("<BBBBHB", 7, 5, 0x03, 1, size, 1)
        ifaces += struct.pack("<BBBBHB", 7, 5, 0x83, 1, size, 1)
    total = 9 + len(ifaces)
    return struct.pack("<BBHBBBBB", 9, 2, total, 2, 1, 0, 0xE0, 250) + ifaces


STRINGS = {1: "Realtek", 2: "Bluetooth Radio", 3: "00e04c885201"}


def string_desc(i):
    if i == 0:
        return bytes([4, 3, 0x09, 0x04])
    s = STRINGS.get(i, "").encode("utf-16-le")
    return bytes([2 + len(s), 3]) + s


class Redir:
    """The device side of a usbredir connection."""

    def __init__(self, sock, ctrl):
        self.sock = sock
        self.ctrl = ctrl
        self.out = threading.Lock()
        self.int_on = False
        self.int_queue = []
        self.bulk_in = []      # pending IN requests: (id, length)
        self.acl_in = []       # pieces of ACL data for them
        self.lock = threading.RLock()
        self.next_id = 1
        self.ids64 = False
        ctrl.redir = self

    def send(self, kind, ident, payload):
        with self.out:
            fmt = "<IIQ" if self.ids64 else "<III"
            self.sock.sendall(struct.pack(fmt, kind, len(payload), ident) + payload)

    def recv_exact(self, n):
        b = b""
        while len(b) < n:
            c = self.sock.recv(n - len(b))
            if not c:
                raise EOFError
            b += c
        return b

    def start(self):
        version = b"btemu 1.0".ljust(64, b"\0")
        caps = (1 << CAP_CONNECT_DEVICE_VERSION) | (1 << CAP_EP_INFO_MAX_PACKET_SIZE) | (1 << CAP_64BITS_IDS) | (1 << CAP_32BITS_BULK_LENGTH)
        self.send(HELLO, 0, version + struct.pack("<I", caps))

    def announce(self):
        info = struct.pack("<I", 2)
        info += bytes([0, 1] + [0] * 30) + bytes([0xE0, 0xE0] + [0] * 30)
        info += bytes([1, 1] + [0] * 30) + bytes([1, 1] + [0] * 30)
        self.send(IFACE_INFO, 0, info)
        types = [255] * 32
        interval = [0] * 32
        iface = [0] * 32
        mps = [0] * 32
        for ep, t, ivl, ifc, size in ((0x00, 0, 0, 0, 64), (0x80, 0, 0, 0, 64), (0x81, 3, 1, 0, 16), (0x02, 2, 0, 0, 64),
                                      (0x82, 2, 0, 0, 64), (0x03, 1, 1, 1, 0), (0x83, 1, 1, 1, 0)):
            i = ((ep & 0x80) >> 3) | (ep & 0x0F)
            types[i], interval[i], iface[i], mps[i] = t, ivl, ifc, size
        self.send(EP_INFO, 0, bytes(types) + bytes(interval) + bytes(iface) + struct.pack("<32H", *mps))
        self.send(CONNECT, 0, struct.pack("<BBBBHHH", 1, 0xE0, 0x01, 0x01, VID, PID, 0x0000))
        log("device connected: 13d3:3571 full speed")

    # From the controller --------------------------------------------------

    def event(self, ev):
        with self.lock:
            for i in range(0, len(ev), 16):
                self.int_queue.append(ev[i:i + 16])
            self.flush()

    def acl(self, data):
        with self.lock:
            for i in range(0, len(data), 64):
                self.acl_in.append(data[i:i + 64])
            self.flush()

    def flush(self):
        if self.int_on:
            for piece in self.int_queue:
                self.send(INTERRUPT, self.next_id, struct.pack("<BBH", 0x81, 0, len(piece)) + piece)
                self.next_id += 1
            self.int_queue = []
        while self.bulk_in and self.acl_in:
            ident, length = self.bulk_in.pop(0)
            piece = self.acl_in.pop(0)
            if len(piece) > length:
                self.acl_in.insert(0, piece[length:])
                piece = piece[:length]
            self.send(BULK, ident, struct.pack("<BBHIH", 0x82, 0, len(piece), 0, 0) + piece)

    # From QEMU ------------------------------------------------------------

    def run(self):
        self.start()
        while True:
            if self.ids64:
                kind, length, ident = struct.unpack("<IIQ", self.recv_exact(16))
            else:
                kind, length, ident = struct.unpack("<III", self.recv_exact(12))
            data = self.recv_exact(length) if length else b""
            with self.lock:
                self.packet(kind, ident, data)

    def packet(self, kind, ident, d):
        if kind == HELLO:
            caps = struct.unpack_from("<I", d, 64)[0] if len(d) >= 68 else 0
            self.ids64 = bool(caps & (1 << CAP_64BITS_IDS))
            self.announce()
        elif kind == RESET:
            log("usb reset")
        elif kind == SET_CONFIG:
            self.send(CONFIG_STATUS, ident, bytes([ST_SUCCESS, d[0]]))
        elif kind == GET_CONFIG:
            self.send(CONFIG_STATUS, ident, bytes([ST_SUCCESS, 1]))
        elif kind == SET_ALT:
            self.send(ALT_STATUS, ident, bytes([ST_SUCCESS, d[0], d[1]]))
        elif kind == GET_ALT:
            self.send(ALT_STATUS, ident, bytes([ST_SUCCESS, d[0], 0]))
        elif kind == START_INT:
            self.int_on = True
            self.send(INT_STATUS, ident, bytes([ST_SUCCESS, d[0]]))
            self.flush()
        elif kind == STOP_INT:
            self.int_on = False
            self.send(INT_STATUS, ident, bytes([ST_SUCCESS, d[0]]))
        elif kind == CANCEL:
            for i, (pid, _) in enumerate(self.bulk_in):
                if pid == ident:
                    del self.bulk_in[i]
                    self.send(BULK, ident, struct.pack("<BBHIH", 0x82, ST_CANCELLED, 0, 0, 0))
                    break
        elif kind == CONTROL:
            self.control(ident, d)
        elif kind == BULK:
            ep, _st, length, _stream, high = struct.unpack_from("<BBHIH", d)
            length |= high << 16
            if ep & 0x80:
                self.bulk_in.append((ident, length))
                self.flush()
            else:
                payload = d[10:]
                self.send(BULK, ident, struct.pack("<BBHIH", ep, ST_SUCCESS, len(payload), 0, 0))
                self.ctrl.host_acl(payload)

    def control(self, ident, d):
        ep, req, rtype, _st, value, index, length = struct.unpack_from("<BBBBHHH", d)
        data = d[10:]
        reply = None
        status = ST_SUCCESS
        if rtype & 0x80:
            if rtype == 0x80 and req == 6:
                kind, i = value >> 8, value & 0xFF
                reply = {1: DEVICE_DESC, 2: config_desc(), 3: string_desc(i)}.get(kind)
            elif rtype == 0x80 and req == 0:
                reply = b"\0\0"
            elif rtype == 0x80 and req == 8:
                reply = b"\x01"
            elif rtype == 0x81 and req == 10:
                reply = b"\x00"
            if reply is None:
                status, reply = ST_STALL, b""
            reply = reply[:length]
            self.send(CONTROL, ident, struct.pack("<BBBBHHH", ep, req, rtype, status, value, index, len(reply)) + reply)
            return
        self.send(CONTROL, ident, struct.pack("<BBBBHHH", ep, req, rtype, status, value, index, len(data)))
        if rtype == 0x20 and req == 0 and len(data) >= 3:
            self.ctrl.host_command(data)


# ---------------------------------------------------------------------------
# The controller and the two devices

def addr(text):
    """An address as HCI carries it (least significant byte first)."""
    return bytes(reversed(bytes.fromhex(text.replace(":", ""))))


OWN = addr("00:E0:4C:88:52:01")
KBD = addr("0A:1B:2C:3D:4E:01")
MOUSE = addr("C0:FF:EE:00:00:02")
KBD_HANDLE, MOUSE_HANDLE = 0x0001, 0x0040
LE_ACL = 27

KBD_MAP = bytes.fromhex(
    "05010906a101850105071ae0002ae700150025017501950881029501750881019506750815002565"
    "0507190029658100c0")
MOUSE_MAP = bytes.fromhex(
    "05010902a10185020901a1000509190129031500250195037501810295017505810105010930"
    "093109381581257f750895038106c0c0")

# HID usages (US layout) for what the keyboard types.
US = {c: (0x04 + i, False) for i, c in enumerate("abcdefghijklmnopqrstuvwxyz")}
US.update({c: (0x1E + i, False) for i, c in enumerate("1234567890")})
US.update({" ": (0x2C, False), ".": (0x37, False), "/": (0x38, False), ":": (0x33, True), "-": (0x2D, False), "\n": (0x28, False)})


class Controller:
    def __init__(self, fw, config, text):
        self.redir = None
        self.lock = threading.RLock()
        self.fw = fw
        self.config = config
        self.text = text
        self.rom = 1
        self.patched = False
        self.download = b""
        self.next_index = 0
        self.frags = 0
        self.inquiry = None
        self.le_scan = False
        self.acl_rx = {}
        # Keyboard
        self.kbd_conn = False
        self.kbd_enc = False
        self.kbd_key = None
        self.kbd_host_io = None
        self.chans = {}          # our cid -> dict(psm, remote, conf_in, conf_out)
        self.next_cid = 0x0040
        self.sig_id = 0x40
        self.typed = False
        # Mouse
        self.le_conn = False
        self.le_enc = False
        self.smp = {}
        self.ltk = None
        self.cccd = False
        self.moved = False

    # Sending ----------------------------------------------------------------

    def event(self, code, params):
        self.redir.event(bytes([code, len(params)]) + params)

    def complete(self, op, params=b"\x00"):
        self.event(0x0E, bytes([1]) + struct.pack("<H", op) + params)

    def status(self, op, st=0):
        self.event(0x0F, bytes([st, 1]) + struct.pack("<H", op))

    def later(self, secs, fn, *args):
        def run():
            with self.lock:
                fn(*args)
        threading.Timer(secs, run).start()

    def l2cap(self, handle, cid, payload):
        pdu = struct.pack("<HH", len(payload), cid) + payload
        size = LE_ACL if handle == MOUSE_HANDLE else 1021
        for i in range(0, len(pdu), size):
            pb = 0x2 if i == 0 else 0x1
            chunk = pdu[i:i + size]
            self.redir.acl(struct.pack("<HH", handle | pb << 12, len(chunk)) + chunk)

    # HCI commands -------------------------------------------------------------

    def host_command(self, data):
        with self.lock:
            op, plen = struct.unpack_from("<HB", data)
            self.command(op, data[3:3 + plen])

    def command(self, op, p):
        if op == 0x0C03:
            log("HCI_Reset")
            self.inquiry = None
            self.le_scan = False
            self.complete(op)
        elif op == 0x1001:
            if self.patched:
                self.complete(op, struct.pack("<BBHBHH", 0, 0x0B, 0x000B, 0x0B, 0x005D, 0x2D4B))
            else:
                self.complete(op, struct.pack("<BBHBHH", 0, 0x0B, 0x000B, 0x0B, 0x005D, 0x8852))
        elif op == 0xFC6D:
            log(f"Realtek: ROM version asked ({self.rom})")
            self.complete(op, bytes([0, self.rom]))
        elif op == 0xFC61:
            log(f"Realtek: security key asked ({p.hex()})")
            self.complete(op, bytes([0, 0, 0]))
        elif op == 0xFC20:
            self.rtl_download(op, p)
        elif op == 0x1009:
            self.complete(op, b"\x00" + OWN)
        elif op == 0x1005:
            self.complete(op, struct.pack("<BHBHH", 0, 1021, 255, 8, 8))
        elif op == 0x2002:
            self.complete(op, struct.pack("<BHB", 0, LE_ACL, 8))
        elif op in (0x0C01, 0x2001, 0x0C56, 0x0C45, 0x0C6D, 0x0C7A, 0x0C24, 0x0C13, 0x0C1A, 0x200B):
            self.complete(op)
        elif op == 0x0C12:
            self.complete(op, b"\x00\x00\x00")
        elif op == 0x200C:
            self.le_scan = p[0] == 1
            log(f"LE scan {'on' if self.le_scan else 'off'}")
            self.complete(op)
            if self.le_scan:
                self.later(0.3, self.advertise)
        elif op == 0x0401:
            self.status(op)
            self.inquiry = time.time()
            log("inquiry")
            self.later(0.5, self.inquiry_result)
            self.later(min(p[3] * 1.28, 3.0), self.inquiry_done, self.inquiry)
        elif op == 0x0402:
            self.inquiry = None
            self.complete(op)
        elif op == 0x0419:
            self.status(op)
            name = b"Tastiera Emu" if p[:6] == KBD else b""
            self.event(0x07, bytes([0 if name else 0x04]) + p[:6] + name.ljust(248, b"\0"))
        elif op == 0x0405:
            self.status(op)
            if p[:6] == KBD:
                self.later(0.2, self.kbd_connected)
            else:
                self.later(1.0, lambda: self.event(0x03, bytes([0x04, 0, 0]) + p[:6] + b"\x01\x00"))
        elif op == 0x0411:
            self.status(op)
            log("keyboard: authentication requested")
            self.event(0x17, KBD)
        elif op == 0x040B:
            self.complete(op, b"\x00" + p[:6])
            ok = self.kbd_key == p[6:22]
            log(f"keyboard: link key from the host {'matches' if ok else 'is WRONG'}")
            self.event(0x06, bytes([0 if ok else 0x06]) + struct.pack("<H", KBD_HANDLE))
        elif op == 0x040C:
            self.complete(op, b"\x00" + p[:6])
            self.event(0x31, KBD)
        elif op == 0x042B:
            self.complete(op, b"\x00" + p[:6])
            self.kbd_host_io = p[6]
            log(f"keyboard: host IO capability {p[6]}, auth {p[8]:#x}")
            self.event(0x32, KBD + bytes([2, 0, 0x05]))  # KeyboardOnly, MITM
            if self.kbd_host_io in (0, 1, 4):
                self.passkey = int.from_bytes(os.urandom(4), "little") % 1000000
                self.event(0x3B, KBD + struct.pack("<I", self.passkey))
                log(f"keyboard: the host shows passkey {self.passkey:06d}; typing it")
                self.later(1.5, self.kbd_paired)
            else:
                log("keyboard: the host cannot show a passkey: pairing fails")
                self.event(0x36, bytes([0x05]) + KBD)
                self.event(0x06, bytes([0x05]) + struct.pack("<H", KBD_HANDLE))
        elif op in (0x042C, 0x042D, 0x042E, 0x042F):
            self.complete(op, b"\x00" + p[:6])
        elif op == 0x0413:
            self.status(op)
            self.kbd_enc = p[2] == 1 and self.kbd_key is not None
            log(f"keyboard: link {'encrypted' if self.kbd_enc else 'NOT encrypted'}")
            self.event(0x08, bytes([0 if self.kbd_enc else 0x06]) + struct.pack("<HB", KBD_HANDLE, int(self.kbd_enc)))
        elif op == 0x0406:
            self.status(op)
            handle = struct.unpack_from("<H", p)[0]
            log(f"disconnect {handle:#x}")
            if handle == KBD_HANDLE:
                self.kbd_conn = self.kbd_enc = False
                self.chans = {}
            if handle == MOUSE_HANDLE:
                self.le_conn = self.le_enc = False
            self.event(0x05, bytes([0]) + struct.pack("<HB", handle, 0x16))
        elif op == 0x200D:
            self.status(op)
            if p[6:12] == MOUSE and p[5] == 1:
                self.later(0.2, self.mouse_connected)
            else:
                log(f"LE create connection to an unknown device {p[5]} {rev(p[6:12]).hex()}")
        elif op == 0x200E:
            self.complete(op)
        elif op == 0x2013:
            self.status(op)
        elif op == 0x2019:
            self.status(op)
            ok = self.ltk is not None and p[12:28] == self.ltk and p[2:12] == bytes(10)
            log(f"mouse: LE encryption {'with the right key' if ok else 'with a WRONG key'}")
            self.le_enc = ok
            self.event(0x08, bytes([0 if ok else 0x06]) + struct.pack("<HB", MOUSE_HANDLE, int(ok)))
        else:
            log(f"unknown command {op:04x}")
            self.complete(op, b"\x01")

    def rtl_download(self, op, p):
        index = p[0]
        if self.patched:
            log("Realtek: download after the patch")
        want = self.next_index
        if index & 0x7F != want & 0x7F or len(p) - 1 > 252:
            log(f"Realtek: download fragment {index:#x}, expected {want:#x} ({len(p) - 1} bytes): REJECTED")
            self.complete(op, bytes([0x12, index]))
            return
        self.download += p[1:]
        self.frags += 1
        self.next_index = 1 if want == 0x7F else want + 1
        self.complete(op, bytes([0, index]))
        if index & 0x80:
            expected = rtl_patch(self.fw, self.rom) + self.config
            got = hashlib.sha256(self.download).hexdigest()[:16]
            if self.download == expected:
                log(f"Realtek: patch accepted: {len(self.download)} bytes in {self.frags} fragments, sha256 {got}")
                self.patched = True
            else:
                log(f"Realtek: patch WRONG: {len(self.download)} bytes (expected {len(expected)}), sha256 {got}")
            self.download = b""
            self.next_index = 0
            self.frags = 0

    # Discovery --------------------------------------------------------------

    def inquiry_result(self):
        if self.inquiry is None:
            return
        name = b"Tastiera Emu"
        eir = bytes([len(name) + 1, 0x09]) + name
        eir += bytes([3, 0x03, 0x24, 0x11])  # HID service
        cod = 0x002540  # limited discoverable, peripheral, keyboard
        self.event(0x2F, bytes([1]) + KBD + bytes([1, 0]) + cod.to_bytes(3, "little") + struct.pack("<Hb", 0x1234, -52) + eir.ljust(240, b"\0"))

    def inquiry_done(self, started):
        if self.inquiry == started:
            self.inquiry = None
            self.event(0x01, b"\x00")

    def advertise(self):
        if not self.le_scan:
            return
        name = b"Mouse Emu"
        ad = bytes([2, 0x01, 0x05]) + bytes([3, 0x19, 0xC2, 0x03]) + bytes([3, 0x03, 0x12, 0x18]) + bytes([len(name) + 1, 0x09]) + name
        self.event(0x3E, bytes([0x02, 1, 0x00, 1]) + MOUSE + bytes([len(ad)]) + ad + struct.pack("<b", -60))
        self.later(0.5, self.advertise)

    # The keyboard: BR/EDR -----------------------------------------------------

    def kbd_connected(self):
        self.kbd_conn = True
        log("keyboard: connected")
        self.event(0x03, bytes([0]) + struct.pack("<H", KBD_HANDLE) + KBD + b"\x01\x00")

    def kbd_paired(self):
        self.kbd_key = os.urandom(16)
        self.event(0x36, bytes([0]) + KBD)
        self.event(0x18, KBD + self.kbd_key + bytes([0x05]))
        self.event(0x06, bytes([0]) + struct.pack("<H", KBD_HANDLE))
        log("keyboard: paired (passkey entry)")

    def host_acl(self, data):
        with self.lock:
            hf, ln = struct.unpack_from("<HH", data)
            handle, pb = hf & 0x0FFF, (hf >> 12) & 3
            self.event(0x13, bytes([1]) + struct.pack("<HH", handle, 1))
            buf = self.acl_rx.get(handle, b"")
            buf = buf + data[4:4 + ln] if pb == 1 else data[4:4 + ln]
            if len(buf) < 4 or len(buf) < 4 + struct.unpack_from("<H", buf)[0]:
                self.acl_rx[handle] = buf
                return
            self.acl_rx[handle] = b""
            plen, cid = struct.unpack_from("<HH", buf)
            payload = buf[4:4 + plen]
            if handle == KBD_HANDLE:
                self.kbd_l2cap(cid, payload)
            elif handle == MOUSE_HANDLE:
                self.mouse_l2cap(cid, payload)

    def sig(self, code, ident, data):
        self.l2cap(KBD_HANDLE, 1, bytes([code, ident]) + struct.pack("<H", len(data)) + data)

    def kbd_l2cap(self, cid, p):
        if cid == 1:
            while len(p) >= 4:
                code, ident, ln = struct.unpack_from("<BBH", p)
                self.kbd_signal(code, ident, p[4:4 + ln])
                p = p[4 + ln:]
            return
        ch = self.chans.get(cid)
        if ch is None:
            log(f"keyboard: data on unknown channel {cid:#x}")
        elif ch["psm"] == 0x0001:
            self.sdp(cid, p)
        elif ch["psm"] == 0x0011:
            log(f"keyboard: HID control {p.hex()}")
            self.l2cap(KBD_HANDLE, ch["remote"], b"\x00")  # handshake: successful

    def kbd_signal(self, code, ident, d):
        if code == 0x02:
            psm, scid = struct.unpack_from("<HH", d)
            if psm in (0x11, 0x13) and not self.kbd_enc:
                log(f"keyboard: channel {psm:#x} asked on an unencrypted link: refused")
                self.sig(0x03, ident, struct.pack("<HHHH", 0, scid, 3, 0))
                return
            local = self.next_cid
            self.next_cid += 1
            self.chans[local] = {"psm": psm, "remote": scid, "conf_in": False, "conf_out": False}
            if psm == 0x11:
                self.sig(0x03, ident, struct.pack("<HHHH", local, scid, 1, 2))  # pending: authorisation
            self.sig(0x03, ident, struct.pack("<HHHH", local, scid, 0, 0))
            self.sig_id += 1
            # Our configuration: MTU 672.
            self.sig(0x04, self.sig_id, struct.pack("<HH", scid, 0) + bytes([0x01, 2]) + struct.pack("<H", 672))
            log(f"keyboard: channel {psm:#x} opened by the host")
        elif code == 0x04:
            dcid = struct.unpack_from("<H", d)[0]
            ch = self.chans.get(dcid)
            if ch:
                ch["conf_in"] = True
                self.sig(0x05, ident, struct.pack("<HHH", ch["remote"], 0, 0))
                self.kbd_check(dcid)
        elif code == 0x05:
            scid, _flags, result = struct.unpack_from("<HHH", d)
            for local, ch in self.chans.items():
                if ch["remote"] == scid or local == scid:
                    ch["conf_out"] = result == 0
                    self.kbd_check(local)
        elif code == 0x06:
            dcid, scid = struct.unpack_from("<HH", d)
            self.chans.pop(dcid, None)
            self.sig(0x07, ident, struct.pack("<HH", dcid, scid))
        elif code == 0x0A:
            self.sig(0x0B, ident, d[:2] + b"\x01\x00")
        elif code in (0x03, 0x07, 0x0B, 0x01):
            pass
        else:
            log(f"keyboard: signalling code {code:#x}")

    def kbd_check(self, local):
        ch = self.chans[local]
        if ch["psm"] == 0x13 and ch["conf_in"] and ch["conf_out"] and not self.typed:
            self.typed = True
            log("keyboard: HID channels open")
            self.later(4.0, self.type_text)

    def sdp(self, cid, p):
        ch = self.chans[cid]
        if p[0] != 0x06:
            log(f"keyboard: SDP request {p[0]:#x} not emulated")
            return
        tid = p[1:3]
        # The record: attribute 0x0206 (HID descriptor list).
        desc = bytes([0x08, 0x22, 0x25, len(KBD_MAP)]) + KBD_MAP
        inner = bytes([0x35, len(desc)]) + desc
        lst = bytes([0x35, len(inner)]) + inner
        rec = bytes([0x09, 0x02, 0x06]) + lst
        record = bytes([0x35, len(rec)]) + rec
        lists = bytes([0x35, len(record)]) + record
        cont = p[-1]
        half = len(lists) // 2
        if cont == 0:
            part, state = lists[:half], bytes([1, 0xAA])
        else:
            part, state = lists[half:], b"\x00"
        body = struct.pack(">H", len(part)) + part + state
        log(f"keyboard: SDP answer, part {1 if cont == 0 else 2} of 2")
        self.l2cap(KBD_HANDLE, ch["remote"], b"\x07" + tid + struct.pack(">H", len(body)) + body)

    def type_text(self):
        intr = next((c for c in self.chans.values() if c["psm"] == 0x13), None)
        if intr is None or not self.kbd_conn:
            log("keyboard: nothing to type on")
            return
        log(f"keyboard: typing {self.text!r}")
        delay = 0.0
        for ch in self.text + "\n":
            usage, shift = US[ch]
            for report in (bytes([0x02 if shift else 0, 0, usage, 0, 0, 0, 0, 0]), bytes(8)):
                self.later(delay, self.l2cap, KBD_HANDLE, intr["remote"], b"\xA1\x01" + report)
                delay += 0.04

    # The mouse: LE --------------------------------------------------------------

    def mouse_connected(self):
        self.le_conn = True
        self.le_enc = False
        self.cccd = False
        log("mouse: connected")
        self.event(0x3E, bytes([0x01, 0]) + struct.pack("<HBB", MOUSE_HANDLE, 0, 1) + MOUSE + struct.pack("<HHHB", 0x18, 0, 0x1F4, 0))
        # Connection parameters a mouse likes: 7.5-15 ms.
        self.later(0.3, self.l2cap, MOUSE_HANDLE, 5, bytes([0x12, 1]) + struct.pack("<HHHHH", 8, 6, 12, 0, 300))

    def mouse_l2cap(self, cid, p):
        if cid == 4:
            self.att(p)
        elif cid == 5:
            if p[0] == 0x13:
                log(f"mouse: connection parameters {'accepted' if p[4:6] == b'\0\0' else 'refused'}")
        elif cid == 6:
            self.smp_rx(p)

    def smp_send(self, data):
        self.l2cap(MOUSE_HANDLE, 6, data)

    def smp_rx(self, p):
        s = self.smp
        code = p[0]
        if code == 0x01:
            s.clear()
            s["preq"] = p[:7]
            s["pres"] = bytes([0x02, 0x03, 0x00, 0x09, 16, 0x00, 0x01])  # no IO, bonding + SC
            log(f"mouse: pairing request {p[:7].hex()}")
            self.smp_send(s["pres"])
        elif code == 0x0C:
            x, y = int.from_bytes(p[1:33], "little"), int.from_bytes(p[33:65], "little")
            try:
                host = ec.EllipticCurvePublicNumbers(x, y, ec.SECP256R1()).public_key()
            except ValueError:
                log("mouse: the host's public key is not on the curve")
                self.smp_send(b"\x05\x0b")
                return
            priv = ec.generate_private_key(ec.SECP256R1())
            n = priv.public_key().public_numbers()
            s["pka"], s["pkb"] = x.to_bytes(32, "big"), n.x.to_bytes(32, "big")
            s["dh"] = priv.exchange(ec.ECDH(), host)
            self.smp_send(b"\x0c" + n.x.to_bytes(32, "little") + n.y.to_bytes(32, "little"))
            s["nb"] = os.urandom(16)
            cb = f4(s["pkb"], s["pka"], s["nb"], 0)
            self.smp_send(b"\x03" + rev(cb))
        elif code == 0x04:
            s["na"] = rev(p[1:17])
            self.smp_send(b"\x04" + rev(s["nb"]))
        elif code == 0x0D:
            a1 = bytes([0]) + rev(OWN)       # the host: public address
            a2 = bytes([1]) + rev(MOUSE)     # us: random static
            mackey, ltk = f5(s["dh"], s["na"], s["nb"], a1, a2)
            preq, pres = s["preq"], s["pres"]
            ea = f6(mackey, s["na"], s["nb"], bytes(16), bytes([preq[3], preq[2], preq[1]]), a1, a2)
            if rev(p[1:17]) != ea:
                log("mouse: DHKey check WRONG")
                self.smp_send(b"\x05\x0b")
                return
            eb = f6(mackey, s["nb"], s["na"], bytes(16), bytes([pres[3], pres[2], pres[1]]), a2, a1)
            self.smp_send(b"\x0d" + rev(eb))
            self.ltk = rev(ltk)
            log("mouse: Secure Connections pairing (Just Works), DHKey check ok")
        elif code == 0x05:
            log(f"mouse: the host stopped pairing (reason {p[1]:#x})")
        else:
            log(f"mouse: SMP code {code:#x}")

    def att_db(self):
        """handle -> (type, value, needs encryption)"""
        def chr_decl(props, handle, uuid):
            return bytes([props]) + struct.pack("<HH", handle, uuid)
        return {
            0x0001: (0x2800, struct.pack("<H", 0x1800), False),
            0x0002: (0x2803, chr_decl(0x02, 0x0003, 0x2A00), False),
            0x0003: (0x2A00, b"Mouse Emu", False),
            0x0010: (0x2800, struct.pack("<H", 0x1812), False),
            0x0011: (0x2803, chr_decl(0x02, 0x0012, 0x2A4A), False),
            0x0012: (0x2A4A, bytes([0x11, 0x01, 0x00, 0x02]), True),
            0x0013: (0x2803, chr_decl(0x02, 0x0014, 0x2A4B), False),
            0x0014: (0x2A4B, MOUSE_MAP, True),
            0x0015: (0x2803, chr_decl(0x12, 0x0016, 0x2A4D), False),
            0x0016: (0x2A4D, bytes(4), True),
            0x0017: (0x2902, b"\x00\x00", True),
            0x0018: (0x2908, bytes([2, 1]), True),
            0x0019: (0x2803, chr_decl(0x0A, 0x001A, 0x2A4D), False),
            0x001A: (0x2A4D, bytes(1), True),
            0x001B: (0x2908, bytes([3, 3]), True),
            0x001C: (0x2803, chr_decl(0x04, 0x001D, 0x2A4C), False),
            0x001D: (0x2A4C, bytes(1), True),
        }

    def att_send(self, data):
        self.l2cap(MOUSE_HANDLE, 4, data)

    def att_error(self, op, handle, code):
        self.att_send(struct.pack("<BBHB", 0x01, op, handle, code))

    def att(self, p):
        db = self.att_db()
        op = p[0]
        if op == 0x02:
            self.att_send(b"\x03\x17\x00")
        elif op == 0x06:  # find by type value
            start, end, typ = struct.unpack_from("<HHH", p, 1)
            value = p[7:]
            found = [h for h in sorted(db) if start <= h <= end and db[h][0] == typ and db[h][1] == value]
            if not found:
                return self.att_error(op, start, 0x0A)
            out = b"\x07"
            services = sorted(k for k in db if db[k][0] == 0x2800)
            for h in found:
                later = [k for k in services if k > h]
                out += struct.pack("<HH", h, later[0] - 1 if later else max(db))
            self.att_send(out[:23])
        elif op == 0x08:  # read by type
            start, end, typ = struct.unpack_from("<HHH", p, 1)
            found = [h for h in sorted(db) if start <= h <= end and db[h][0] == typ]
            if not found:
                return self.att_error(op, start, 0x0A)
            size = len(db[found[0]][1])
            out = bytes([0x09, 2 + size])
            for h in found:
                if len(db[h][1]) != size or len(out) + 2 + size > 23:
                    break
                out += struct.pack("<H", h) + db[h][1]
            self.att_send(out)
        elif op == 0x04:  # find information
            start, end = struct.unpack_from("<HH", p, 1)
            found = [h for h in sorted(db) if start <= h <= end]
            if not found:
                return self.att_error(op, start, 0x0A)
            out = b"\x05\x01"
            for h in found[:5]:
                out += struct.pack("<HH", h, db[h][0])
            self.att_send(out)
        elif op in (0x0A, 0x0C):  # read, read blob
            h = struct.unpack_from("<H", p, 1)[0]
            off = struct.unpack_from("<H", p, 3)[0] if op == 0x0C else 0
            if h not in db:
                return self.att_error(op, h, 0x01)
            if db[h][2] and not self.le_enc:
                log(f"mouse: read of {h:#x} on an unencrypted link: refused")
                return self.att_error(op, h, 0x05)
            v = db[h][1]
            if off > len(v):
                return self.att_error(op, h, 0x07)
            if h == 0x0014:
                log(f"mouse: report map read at {off}")
            self.att_send(bytes([op + 1]) + v[off:off + 22])
        elif op == 0x12:  # write
            h = struct.unpack_from("<H", p, 1)[0]
            if h != 0x0017:
                return self.att_error(op, h, 0x03)
            if not self.le_enc:
                return self.att_error(op, h, 0x05)
            self.cccd = p[3:5] == b"\x01\x00"
            self.att_send(b"\x13")
            log(f"mouse: notifications {'on' if self.cccd else 'off'}")
            if self.cccd and not self.moved:
                self.moved = True
                self.later(3.0, self.move)
        elif op & 0x01 or op in (0x1E,):
            pass
        else:
            self.att_error(op, 0, 0x06)

    def move(self):
        if not (self.le_conn and self.le_enc and self.cccd):
            log("mouse: not subscribed, no moves")
            return
        log("mouse: moving 10 times by (10, 5), then the left button down")
        for i in range(10):
            self.later(0.05 * i, self.att_send, b"\x1b" + struct.pack("<H", 0x0016) + bytes([0, 10, 5, 0]))
        self.later(0.6, self.att_send, b"\x1b" + struct.pack("<H", 0x0016) + bytes([1, 0, 0, 0]))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--socket", required=True)
    ap.add_argument("--firmware", required=True, help="the folder with rtl8852bu_fw.bin and rtl8852bu_config.bin")
    ap.add_argument("--text", default="write sys:/home/bt.txt ciao dal bluetooth")
    a = ap.parse_args()
    fw = open(os.path.join(a.firmware, "rtl8852bu_fw.bin"), "rb").read()
    config = open(os.path.join(a.firmware, "rtl8852bu_config.bin"), "rb").read()
    expected = rtl_patch(fw, 1) + config
    log(f"firmware: the patch for ROM version 1 is {len(expected)} bytes")
    if os.path.exists(a.socket):
        os.remove(a.socket)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(a.socket)
    srv.listen(1)
    log(f"waiting for QEMU on {a.socket}")
    conn, _ = srv.accept()
    ctrl = Controller(fw, config, a.text)
    try:
        Redir(conn, ctrl).run()
    except (EOFError, ConnectionResetError, BrokenPipeError):
        log("QEMU went away")
    os._exit(0)


if __name__ == "__main__":
    main()
