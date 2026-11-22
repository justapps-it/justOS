#!/bin/sh
# Checks Wi-Fi without a Wi-Fi card. QEMU has none, so justOS's test radio
# carries 802.11 frames inside Ethernet frames (type 0x88B5) over a virtio
# network card, whose other end (-netdev dgram, UDP on this machine) is
# scripts/wifi/apsim.py: five simulated access points (open, WPA2, WPA2/WPA3,
# WPA3, WPA3 with hash-to-element only) with DHCP and a gateway that pings.
#
#   1. the station alone against the simulator (cargo test, on this machine)
#   2. justOS: scan, a wrong password, WPA2 (with a new group key), WPA3,
#      WPA3 hash-to-element, an open network, DHCP and ping on each, the
#      known networks (their passwords encrypted), forgetting one
#   3. justOS again on the disk the first session left: it joins a known
#      network by itself, and another with the password it saved
#
# What it cannot check: the Realtek card's own driver (rtw8852be), which
# needs the real card.
#
# Usage: scripts/test-wifi.sh <image>    (VERBOSE=1 prints the sessions)
set -u
IMAGE=$(realpath "${1:?usage: test-wifi.sh <image>}")
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
AP=
cleanup() { [ -n "$AP" ] && kill "$AP" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT
FAILED=0

echo "  ...   the station against the simulator (host)"
if ! (cd "$HERE/../system" && cargo test -q --target x86_64-unknown-linux-gnu -p justwifi -- --ignored > "$WORK/host.txt" 2>&1); then
    tail -30 "$WORK/host.txt"
    echo "  missing: the station passes against the simulator"
    FAILED=1
fi

cp --sparse=always "$IMAGE" "$WORK/justos.img" || exit 2
cd "$WORK"
port() { python3 -c 'import socket; s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
P_AP=$(port); P_VM=$(port)
NET="-nic none -netdev dgram,id=w,local.type=inet,local.host=127.0.0.1,local.port=$P_VM,remote.type=inet,remote.host=127.0.0.1,remote.port=$P_AP -device virtio-net-pci,netdev=w"

start_ap() {
    # The simulator's stdin stays open (a fifo) so that it keeps running.
    rm -f ap.in; mkfifo ap.in
    python3 "$HERE/wifi/apsim.py" --listen "$P_AP" --peer "$P_VM" --rekey 4 < ap.in >> ap.txt 2>&1 & AP=$!
    exec 3> ap.in
    sleep 1
}
stop_ap() { exec 3>&-; kill "$AP" 2>/dev/null; wait "$AP" 2>/dev/null; AP=; }

cat > s1.txt <<'EOF'
@sleep 2
wifi
wifi sim net0
wifi scan
@sleep 6
wifi connect justOS-wpa2 sbagliata1
@sleep 14
wifi connect justOS-wpa2 password-wpa2
@sleep 8
ping 192.168.11.1 3
@sleep 6
wifi status
wifi connect justOS-wpa3 password-wpa3
@sleep 8
ping 192.168.13.1 2
@sleep 3
wifi connect justOS-h2e password-h2e
@sleep 8
ping 192.168.14.1 2
@sleep 3
wifi connect justOS-aperta
@sleep 6
ping 192.168.10.1 2
@sleep 3
wifi saved
cat sys:/system/wifi.json
wifi forget justOS-wpa3
wifi saved
net
shutdown
EOF

start_ap
STEP_WAIT=2 QEMU_TIMEOUT=200 KEEP_DISK="$WORK/session1.img" "$HERE/test.sh" justos.img s1.txt $NET > out1.txt 2>&1
S1=$?
stop_ap
# The second session boots the disk as the first one left it.

cat > s2.txt <<'EOF'
@sleep 2
wifi sim net0
@sleep 12
wifi status
wifi connect justOS-wpa2
@sleep 8
wifi disconnect
wifi status
shutdown
EOF
start_ap
STEP_WAIT=2 QEMU_TIMEOUT=120 KEEP_DISK= "$HERE/test.sh" "$WORK/session1.img" s2.txt $NET > out2.txt 2>&1 || true
stop_ap

check() { # file pattern
    if ! grep -Fq -- "$2" "$1"; then
        printf '  missing: %s\n' "$2"
        FAILED=1
    fi
}
check out1.txt 'wifi0: test radio over net0'
check out1.txt 'justOS-aperta'
check out1.txt 'justOS-h2e'
check out1.txt '"justOS-wpa2": wrong password'
check ap.txt 'message 2 from 52:54:00'
check out1.txt 'connected to "justOS-wpa2", address 192.168.11.100'
check out1.txt '3 of 3 replies'
check ap.txt 'group key handshake done'
check ap.txt 'the group key works'
check out1.txt 'connected to "justOS-wpa3", address 192.168.13.100'
check ap.txt 'SAE authenticated'
check ap.txt 'associated 52:54:00:12:34:d6 PMF'
check out1.txt 'connected to "justOS-h2e", address 192.168.14.100'
check ap.txt '(hash-to-element)'
check out1.txt 'connected to "justOS-aperta", address 192.168.10.100'
check out1.txt 'forgot "justOS-wpa3"'
# The passwords are kept encrypted, never in clear.
check out1.txt '"sealed"'
if grep -q '"password"' out1.txt; then
    echo "  missing: passwords encrypted on disk"; FAILED=1
fi
[ $S1 -ne 0 ] && { echo "  the kernel stopped"; FAILED=1; }
check out2.txt 'connected to "justOS-aperta"'
# Its saved password, decrypted after a reboot.
check out2.txt 'connected to "justOS-wpa2", address 192.168.11.100'
check out2.txt 'not connected'
grep -q "justOS stopped" out2.txt && { echo "  the kernel stopped (second session)"; FAILED=1; }

if [ "${VERBOSE:-0}" = 1 ] || [ $FAILED != 0 ]; then
    echo "----- session 1"; cat out1.txt
    echo "----- session 2"; cat out2.txt
    echo "----- access points"; cat ap.txt
fi
[ $FAILED = 0 ] && echo "  ok    wifi (host station, scan, WPA2, WPA3 H&P and H2E, open, PMF, rekey, DHCP, ping, known networks)"
exit $FAILED
