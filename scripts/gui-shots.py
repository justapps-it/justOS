#!/usr/bin/env python3
"""Drives justOS's graphical interface in QEMU and takes screenshots.

QEMU is controlled through QMP: a USB tablet gives absolute pointer
positions, keys are sent as key codes, and `screendump` saves the screen.
A script is a list of steps, one per line:

    wait <seconds>
    until <seconds> <text>      wait until the serial log has that text (colours
                                left out), at most that long
    shot <name>                 save <out>/<name>.png (with HEADS=n, each
                                head in <out>/<name>-h<i>.png too, and all
                                of them side by side in <name>.png)
    click <x> <y>               left click at a screen position
    double <x> <y>              double click
    right <x> <y>               right click
    move <x> <y>
    drag <x0> <y0> <x1> <y1>
    press <x> <y>               left button down (a drag that goes on...)
    glide <x> <y>               move there in small steps (button held or not)
    release <x> <y>             ...and up again, there
    type <text>                 letters, digits, space and simple punctuation
    wheel up|down [n]           turn the mouse wheel n notches (up: away from
                                the user)
    key <name>                  a QEMU key name: ret, esc, spc, tab, ctrl-spc...
    hold <name> / unhold <name> keep a key down (ctrl, shift, alt) and let go
    serial <text>               a command line typed on the serial console
                                (the shell there), e.g. "serial suspend 3"
    find <rrggbb>               look for the first pixel of that colour on the
                                screen (top to bottom, left to right); after
                                it, "@n" in a position is n pixels from there
                                ("click @20 @40"). Stops the script if the
                                colour is nowhere.
    qmp <command> [json]        a QMP command, e.g. qmp device_del {"id": "t"}
    plug <head> [<w>x<h>]       a monitor attached at that head (HEADS=n)
    unplug <head>               ...and taken away

Usage: scripts/gui-shots.py <image> <script> <out folder> [qemu options...]
Environment: QEMU (default qemu-system-x86_64), UEFI=1 for OVMF, CDROM=1 to
boot the image as a CD, USB=1 as a USB stick (xHCI), KEEP=<file> to keep the
disk as the session left it, SCREEN=<w>x<h> when the screen is not 1024x768,
MEMORY (default 512M). HEADS=<n> for a display with several heads (the
monitors of "-device virtio-gpu-pci,max_outputs=2,id=gpu"): screenshots
take every head of the device named by GPU (default gpu), and SCREEN is
then the whole desktop the pointer moves over (two 1280x800 monitors side
by side: 2560x800). Without a window of its own QEMU shows only the first
head as a monitor, so each other one gets a VNC server and a small client
that tells QEMU its size (MONITOR, default 1280x800), as a window would.
"""
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

# Screen size the pointer positions refer to (SCREEN=1920x1080 for a
# loader that picked another mode).
W, H = map(int, os.environ.get("SCREEN", "1024x768").split("x"))


class Qmp:
    def __init__(self, path):
        for _ in range(100):
            try:
                self.s = socket.socket(socket.AF_UNIX)
                self.s.connect(path)
                break
            except OSError:
                time.sleep(0.1)
        self.f = self.s.makefile("rw")
        self.read()
        self.cmd("qmp_capabilities")

    def read(self):
        while True:
            msg = json.loads(self.f.readline())
            if "event" not in msg:
                return msg

    def cmd(self, name, **args):
        self.f.write(json.dumps({"execute": name, "arguments": args}) + "\n")
        self.f.flush()
        r = self.read()
        if "error" in r:
            raise RuntimeError(f"{name}: {r['error']}")
        return r.get("return")

    def pointer(self, x, y, down=None, button="left"):
        events = [
            {"type": "abs", "data": {"axis": "x", "value": int(x * 32767 / (W - 1))}},
            {"type": "abs", "data": {"axis": "y", "value": int(y * 32767 / (H - 1))}},
        ]
        if down is not None:
            events.append({"type": "btn", "data": {"button": button, "down": down}})
        self.cmd("input-send-event", events=events)

    def key(self, name):
        keys = name.split("-")
        self.cmd("send-key", keys=[{"type": "qcode", "data": k} for k in keys])

    def key_state(self, name, down):
        self.cmd("input-send-event", events=[{"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": name}}}])


class Vnc:
    """A VNC client of one head that only says how big its window is: QEMU
    then shows the guest a monitor attached there (none for 0 x 0)."""

    def __init__(self, path):
        for _ in range(100):
            try:
                self.s = socket.socket(socket.AF_UNIX)
                self.s.connect(path)
                break
            except OSError:
                time.sleep(0.1)
        self.recv(12)
        self.s.sendall(b"RFB 003.008\n")
        types = self.recv(self.recv(1)[0])
        if 1 not in types:
            raise SystemExit("vnc: the server wants a password")
        self.s.sendall(b"\x01")
        self.recv(4)
        # Shared; then the server's size, pixel format and name.
        self.s.sendall(b"\x01")
        self.recv(20)
        self.recv(struct.unpack(">I", self.recv(4))[0])
        # Encodings: raw, and the extended desktop size that resizing needs.
        self.s.sendall(struct.pack(">BxHii", 2, 2, 0, -308))
        threading.Thread(target=drain, args=(self.s,), daemon=True).start()

    def recv(self, n):
        data = b""
        while len(data) < n:
            part = self.s.recv(n - len(data))
            if not part:
                raise SystemExit("vnc: the server went away")
            data += part
        return data

    def size(self, w, h):
        """SetDesktopSize with one screen of w x h."""
        self.s.sendall(struct.pack(">BxHHBx", 251, w, h, 1) + struct.pack(">IHHHHI", 0, 0, 0, w, h, 0))


def read_ppm(src):
    """QEMU's binary PPM screendump: (width, height, RGB bytes)."""
    with open(src, "rb") as f:
        data = f.read()
    # Header: "P6", width, height, maxval, each followed by white space.
    fields, pos = [], 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        if data[pos:pos + 1] == b"#":
            pos = data.index(b"\n", pos) + 1
            continue
        end = pos
        while not data[end:end + 1].isspace():
            end += 1
        fields.append(data[pos:end])
        pos = end
    pos += 1
    if fields[0] != b"P6":
        raise SystemExit(f"{src}: not a binary PPM")
    w, h = int(fields[1]), int(fields[2])
    return w, h, data[pos:pos + w * 3 * h]


def ppm_to_png(src, dst):
    """Converts QEMU's binary PPM screendump to PNG with the standard library."""
    write_png(dst, *read_ppm(src))


def side_by_side(shots):
    """Several (width, height, RGB bytes) next to each other, tops aligned;
    black under the shorter ones."""
    w = sum(s[0] for s in shots)
    h = max(s[1] for s in shots)
    rows = []
    for y in range(h):
        for sw, sh, px in shots:
            rows.append(px[y * sw * 3:(y + 1) * sw * 3] if y < sh else b"\0" * (sw * 3))
    return w, h, b"".join(rows)


def write_png(dst, w, h, pixels):
    """Saves RGB bytes as a PNG, with the standard library."""
    stride = w * 3
    raw = b"".join(b"\0" + pixels[y * stride:(y + 1) * stride] for y in range(h))

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    with open(dst, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 6)))
        f.write(chunk(b"IEND", b""))


def drain(conn):
    try:
        while conn.recv(4096):
            pass
    except OSError:
        pass


CHARS = {" ": "spc", ".": "dot", ",": "comma", "-": "minus", "/": "slash", ":": "shift-semicolon", "!": "shift-1", "+": "kp_add", "*": "kp_multiply", "=": "equal"}


def main():
    image, script, out = sys.argv[1:4]
    extra = sys.argv[4:]
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp()
    disk = os.path.join(work, "disk.img")
    # The image is mostly empty space (the one with Wine is 5 GiB): the copy
    # keeps the holes.
    subprocess.run(["cp", "--sparse=always", image, disk], check=True)
    sock = os.path.join(work, "qmp.sock")
    ser = os.path.join(work, "serial.sock")
    qemu = os.environ.get("QEMU", "qemu-system-x86_64")
    # The serial console goes to serial.log, and to a socket the "serial"
    # step types on.
    args = [qemu, "-m", os.environ.get("MEMORY", "512M"), "-display", "none", "-no-reboot",
            "-chardev", f"socket,id=ser,path={ser},server=on,wait=off,logfile={out}/serial.log", "-serial", "chardev:ser",
            "-qmp", f"unix:{sock},server,nowait", "-usb", "-device", "usb-tablet"]
    # The image starts first, before the disks given as QEMU options.
    if os.environ.get("CDROM") == "1":
        args += ["-drive", f"file={disk},format=raw,if=none,id=cd,media=cdrom",
                 "-device", "ide-cd,drive=cd,bus=ide.1,bootindex=0"]
    elif os.environ.get("USB") == "1":
        args += ["-device", "qemu-xhci,id=xhci", "-drive", f"file={disk},format=raw,if=none,id=stick",
                 "-device", "usb-storage,bus=xhci.0,drive=stick,bootindex=0"]
    else:
        args += ["-drive", f"file={disk},format=raw,if=ide"]
    if os.environ.get("UEFI") == "1":
        vars_ = os.path.join(work, "vars.fd")
        shutil.copy("/usr/share/OVMF/OVMF_VARS_4M.fd", vars_)
        args += ["-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd",
                 "-drive", f"if=pflash,format=raw,file={vars_}"]
    heads = int(os.environ.get("HEADS", "1"))
    gpu = os.environ.get("GPU", "gpu")
    monitor = tuple(map(int, os.environ.get("MONITOR", "1280x800").split("x")))
    for head in range(1, heads):
        args += ["-vnc", f"unix:{work}/vnc{head}.sock,id=vnc{head},display={gpu},head={head}"]
    proc = subprocess.Popen(args + extra)
    try:
        q = Qmp(sock)
        # The other monitors are there from the start, as the first one.
        vnc = {head: Vnc(f"{work}/vnc{head}.sock") for head in range(1, heads)}
        for v in vnc.values():
            v.size(*monitor)
        at = (W // 2, H // 2)
        console = None
        origin = (0, 0)
        for line in open(script):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            op, _, rest = line.partition(" ")
            a = rest.split()
            if op in ("click", "double", "right", "move", "drag", "press", "glide", "release"):
                # "@n": from where "find" found its colour.
                a = [str(origin[i % 2] + int(v[1:])) if v.startswith("@") else v for i, v in enumerate(a)]
            if op == "wait":
                time.sleep(float(a[0]))
            elif op == "until":
                text = rest.partition(" ")[2]
                start = time.time()
                while True:
                    try:
                        log = open(os.path.join(out, "serial.log"), "rb").read().decode("utf-8", "replace")
                    except OSError:
                        log = ""
                    if text in re.sub(r"\x1b\[[0-9;]*m", "", log):
                        print("until", text, "after", round(time.time() - start, 1), flush=True)
                        break
                    if time.time() - start > float(a[0]):
                        print("until", text, "not seen", flush=True)
                        break
                    time.sleep(0.5)
            elif op == "shot":
                ppm = os.path.join(work, "shot.ppm")
                if heads == 1:
                    q.cmd("screendump", filename=ppm)
                    time.sleep(0.3)
                    ppm_to_png(ppm, os.path.join(out, a[0] + ".png"))
                else:
                    shots = []
                    for head in range(heads):
                        q.cmd("screendump", filename=ppm, device=gpu, head=head)
                        time.sleep(0.3)
                        shots.append(read_ppm(ppm))
                        write_png(os.path.join(out, f"{a[0]}-h{head}.png"), *shots[-1])
                    write_png(os.path.join(out, a[0] + ".png"), *side_by_side(shots))
                print("shot", a[0], flush=True)
            elif op in ("click", "double", "right"):
                x, y = int(a[0]), int(a[1])
                button = "right" if op == "right" else "left"
                q.pointer(x, y)
                time.sleep(0.15)
                for _ in range(2 if op == "double" else 1):
                    q.pointer(x, y, True, button)
                    time.sleep(0.08)
                    q.pointer(x, y, False, button)
                    time.sleep(0.12)
            elif op == "move":
                q.pointer(int(a[0]), int(a[1]))
            elif op == "drag":
                x0, y0, x1, y1 = map(int, a)
                q.pointer(x0, y0)
                time.sleep(0.15)
                q.pointer(x0, y0, True)
                for i in range(1, 9):
                    q.pointer(x0 + (x1 - x0) * i // 8, y0 + (y1 - y0) * i // 8)
                    time.sleep(0.1)
                q.pointer(x1, y1, False)
            elif op == "press":
                x, y = int(a[0]), int(a[1])
                q.pointer(x, y)
                time.sleep(0.15)
                q.pointer(x, y, True)
                time.sleep(0.1)
            elif op == "glide":
                x1, y1 = int(a[0]), int(a[1])
                x0, y0 = at
                for i in range(1, 9):
                    q.pointer(x0 + (x1 - x0) * i // 8, y0 + (y1 - y0) * i // 8)
                    time.sleep(0.1)
            elif op == "release":
                q.pointer(int(a[0]), int(a[1]), False)
                time.sleep(0.1)
            elif op == "wheel":
                # One event per notch, as a mouse sends them.
                for _ in range(int(a[1]) if len(a) > 1 else 1):
                    for down in (True, False):
                        q.cmd("input-send-event", events=[{"type": "btn", "data": {"button": "wheel-" + a[0], "down": down}}])
                    time.sleep(0.05)
            elif op in ("hold", "unhold"):
                q.key_state(a[0], op == "hold")
                time.sleep(0.05)
            elif op == "type":
                for ch in rest:
                    if ch.isupper():
                        q.key("shift-" + ch.lower())
                    elif ch in CHARS:
                        q.key(CHARS[ch])
                    else:
                        q.key(ch)
                    time.sleep(0.08)
            elif op == "key":
                q.key(a[0])
            elif op == "find":
                want = bytes.fromhex(a[0])
                ppm = os.path.join(work, "shot.ppm")
                q.cmd("screendump", filename=ppm)
                time.sleep(0.3)
                sw, _, pixels = read_ppm(ppm)
                i = pixels.find(want)
                while i > 0 and i % 3:
                    i = pixels.find(want, i + 1)
                if i < 0:
                    print("find", a[0], "nowhere", flush=True)
                    break
                origin = ((i // 3) % sw, (i // 3) // sw)
                print("find", a[0], "at", *origin, flush=True)
            elif op == "serial":
                if console is None:
                    console = socket.socket(socket.AF_UNIX)
                    console.connect(ser)
                    # What the system writes back is in the log already;
                    # reading it keeps the console from stalling.
                    threading.Thread(target=drain, args=(console,), daemon=True).start()
                console.sendall((rest + "\r").encode())
            elif op == "qmp":
                name, _, body = rest.partition(" ")
                print("qmp", name, q.cmd(name, **(json.loads(body) if body.strip() else {})), flush=True)
            elif op in ("plug", "unplug"):
                size = tuple(map(int, a[1].split("x"))) if len(a) > 1 else monitor
                vnc[int(a[0])].size(*(size if op == "plug" else (0, 0)))
                print(op, a[0], flush=True)
            else:
                raise SystemExit(f"unknown step: {line}")
            if op in ("click", "double", "right", "move", "press", "glide", "release"):
                at = (int(a[0]), int(a[1]))
            elif op == "drag":
                at = (int(a[2]), int(a[3]))
    finally:
        proc.kill()
        proc.wait()
        if os.environ.get("KEEP"):
            subprocess.run(["cp", "--sparse=always", disk, os.environ["KEEP"]], check=True)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
