#!/bin/sh
# Boots a justOS image once per display controller QEMU emulates, switches
# the resolution from the shell and checks the screen QEMU shows: it must
# have the new size and show the desktop (not a black or stale picture).
#
# Usage: scripts/test-display.sh <image> [case...]
# Cases: std (QEMU VGA), bochs (bochs-display), qxl, vmware, virtio, cirrus
# (800x600 at most in 32-bit colour), ati (no driver of its own: the generic
# driver keeps the 1024x768 the loader set up).
# virtio2 is a virtio GPU with two monitors: they show the same until
# "display mirror off" puts them side by side; each must then have the new
# size and a picture of its own half. QEMU without a window enables only
# its first output, so the second gets a VNC server and a small client that
# tells QEMU its size, as a window would.
# std-sleep, vmware-sleep, virtio-sleep and cirrus-sleep put the computer to
# sleep instead (QEMU's firmware leaves the card in text mode on waking) and
# check the picture comes back in the mode it had.
set -u
IMAGE=${1:?usage: test-display.sh <image> [case...]}
shift
HERE=$(dirname "$0")
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
CASES=${*:-std bochs qxl vmware virtio virtio2 cirrus ati std-sleep vmware-sleep virtio-sleep cirrus-sleep}
DEFAULT_MODE=${MODE:-1280x720}

commands() {
    if [ -n "$HEADS" ]; then
        cat > "$WORK/cmds.txt" <<EOT
@sleep 4
display $MODE
@sleep 6
display
display mirror off
@sleep 5
display
@sleep 2
shutdown
EOT
    elif [ -n "$SLEEP" ]; then
        cat > "$WORK/cmds.txt" <<EOT
@sleep 4
suspend 3
@sleep 12
display
@sleep 1
shutdown
EOT
    else
        cat > "$WORK/cmds.txt" <<EOT
@sleep 4
display
display $MODE
@sleep 6
display
@sleep 2
shutdown
EOT
    fi
}

# Takes screenshots through QMP into $2, $3... seconds after the start; the
# last one stays. Without a window QEMU draws the screen only for a
# screenshot, and a VGA that never drew before sleep shows QEMU's own
# "not initialized" picture after waking up, whatever the guest did: the
# sleep cases look once before sleeping too, as a real screen would.
# With HEADS=n, every head of the GPU named gpu: the first in $2, the others
# in $2.1, $2.2...
cat > "$WORK/shot.py" <<'EOT'
import json, os, socket, sys, time
start = time.time()
sock, out, delays = sys.argv[1], sys.argv[2], [float(d) for d in sys.argv[3:]]
heads = int(os.environ.get("HEADS") or 1)
time.sleep(delays[0])
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
f = s.makefile("rw")
f.readline()
def cmd(c):
    f.write(json.dumps(c) + "\n")
    f.flush()
    while "event" in json.loads(f.readline()):
        pass
cmd({"execute": "qmp_capabilities"})
for d in delays:
    time.sleep(max(0, d - (time.time() - start)))
    if heads == 1:
        cmd({"execute": "screendump", "arguments": {"filename": out}})
    for i in range(heads if heads > 1 else 0):
        name = out if i == 0 else "%s.%d" % (out, i)
        cmd({"execute": "screendump", "arguments": {"filename": name, "device": "gpu", "head": i}})
EOT

# A VNC client on the server of one head ($1): asks for a monitor of
# $2 x $3 (RFB SetDesktopSize, as a window resized), which makes QEMU
# enable that output, and stays connected for $4 seconds.
cat > "$WORK/head.py" <<'EOT'
import socket, struct, sys, threading, time
path, w, h, keep = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
start = time.time()
while True:
    try:
        s = socket.socket(socket.AF_UNIX)
        s.connect(path)
        break
    except OSError:
        if time.time() - start > 20:
            sys.exit("no VNC server")
        time.sleep(0.1)
def read(n):
    b = b""
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c:
            sys.exit("VNC server went away")
        b += c
    return b
read(12)
s.sendall(b"RFB 003.008\n")
n = read(1)[0]
read(n)
s.sendall(b"\x01")
read(4)
s.sendall(b"\x01")
read(20)
read(struct.unpack(">I", read(4))[0])
# Raw pixels, and the extended desktop size pseudo-encoding (-308).
s.sendall(struct.pack(">BxHii", 2, 2, 0, -308))
s.sendall(struct.pack(">BxHHBx", 251, w, h, 1) + struct.pack(">IHHHHI", 0, 0, 0, w, h, 0))
def drain():
    try:
        while s.recv(65536):
            pass
    except OSError:
        pass
threading.Thread(target=drain, daemon=True).start()
time.sleep(keep)
EOT

# Width, height and number of distinct colours of a PPM screenshot.
cat > "$WORK/look.py" <<'EOT'
import sys
data = open(sys.argv[1], "rb").read()
parts = data.split(maxsplit=4)
w, h = int(parts[1]), int(parts[2])
px = parts[4]
colours = len({px[i:i + 3] for i in range(0, len(px), 3 * 97)})
print(w, h, colours)
EOT

FAILED=0
for c in $CASES; do
    MODE=$DEFAULT_MODE
    SLEEP=
    HEADS=
    DELAY=16
    case $c in
        *-sleep)
            # The mode the system started in: the loader's, or the one the
            # driver chose (cirrus), kept across sleep.
            SLEEP=1 DELAY="6 19" MODE=1024x768
            c=${c%-sleep}
            [ $c = cirrus ] && MODE=800x600 ;;
    esac
    case $c in
        std) ARGS="-vga std" ;;
        bochs) ARGS="-vga none -device bochs-display" ;;
        qxl) ARGS="-vga qxl" ;;
        vmware) ARGS="-vga vmware" ;;
        virtio) ARGS="-vga virtio" ;;
        virtio2)
            HEADS=2 DELAY=24
            ARGS="-vga none -device virtio-gpu-pci,max_outputs=2,id=gpu -vnc unix:$WORK/vnc1.sock,id=vnc1,display=gpu,head=1" ;;
        cirrus) ARGS="-vga cirrus"; MODE=800x600 ;;
        ati) ARGS="-vga none -device ati-vga"; MODE=1024x768 ;;
        *) echo "unknown case: $c" >&2; exit 2 ;;
    esac
    commands
    rm -f "$WORK/shot.ppm" "$WORK/shot.ppm.1" "$WORK/vnc1.sock"
    HEADS=$HEADS python3 "$WORK/shot.py" "$WORK/qmp.sock" "$WORK/shot.ppm" $DELAY &
    SHOT=$!
    HEAD=
    if [ -n "$HEADS" ]; then
        python3 "$WORK/head.py" "$WORK/vnc1.sock" 1280 800 40 &
        HEAD=$!
    fi
    "$HERE/test.sh" "$IMAGE" "$WORK/cmds.txt" $ARGS -qmp "unix:$WORK/qmp.sock,server,nowait" > "$WORK/out.txt" 2>&1
    STATUS=$?
    wait $SHOT
    [ -n "$HEAD" ] && kill $HEAD 2>/dev/null
    [ -n "$SLEEP" ] && c=$c-sleep
    problems=
    [ $STATUS -ne 0 ] && problems="$problems [kernel stopped]"
    grep -q "^screen $MODE" "$WORK/out.txt" || problems="$problems [no \"screen $MODE\"]"
    if [ -n "$SLEEP" ]; then
        grep -q "awake again" "$WORK/out.txt" || problems="$problems [did not wake]"
    fi
    SHOTS_OF="$WORK/shot.ppm"
    if [ -n "$HEADS" ]; then
        SHOTS_OF="$WORK/shot.ppm $WORK/shot.ppm.1"
        grep -q "^monitors: 2 mirrored" "$WORK/out.txt" || problems="$problems [not mirrored at first]"
        grep -q "^monitor 0: output 0 $MODE at 0,0 (primary)" "$WORK/out.txt" || problems="$problems [no monitor 0 at the left]"
        grep -q "^monitor 1: output 1 $MODE at ${MODE%x*},0" "$WORK/out.txt" || problems="$problems [no monitor 1 at the right]"
    fi
    for shot in $SHOTS_OF; do
        if [ -f "$shot" ]; then
            set -- $(python3 "$WORK/look.py" "$shot")
            [ "${1}x${2}" = "$MODE" ] || problems="$problems [${shot##*/} is ${1}x${2}]"
            [ "$3" -gt 50 ] || problems="$problems [${shot##*/} blank: $3 colours]"
        else
            problems="$problems [no ${shot##*/}]"
        fi
    done
    if [ -z "$problems" ]; then
        printf '  ok    %s\n' "$c"
    else
        printf '  FAIL  %s:%s\n' "$c" "$problems"
        [ "${VERBOSE:-0}" = 1 ] && cat "$WORK/out.txt"
        FAILED=1
    fi
    [ -n "${SHOTS:-}" ] && cp "$WORK/shot.ppm" "$SHOTS/$c.ppm" 2>/dev/null
done
exit $FAILED
