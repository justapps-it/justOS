#!/bin/bash
# By hand, not in the suites (it needs the 5 GiB image with Wine and takes
# about 5 minutes under emulation): the first Windows program opened gets
# the dynamic bar's preparation and never Wine's own dialog.
#
# Usage: scripts/test-winprep.sh [image]     (default build/justos.img; the
# image is copied sparsely to build/winprep-test.img and left unchanged)
#
# A fresh image, a double click-equivalent (Ctrl+Space, the file's name) on
# Wine's notepad.exe, then the checks on the serial log: the preparation ran
# and finished, no window named "Wine" was ever created, and the program's
# own window came afterwards. Screenshots land in build/winprep-test/.
set -e
cd "$(dirname "$0")/.."
IMG=${1:-build/justos.img}
W=build/wineroot/usr/lib/x86_64-linux-gnu/wine
J=tools/target/release/justtool
OUT=build/winprep-test
mkdir -p build/tmp "$OUT"
export TMPDIR=$PWD/build/tmp
cp --sparse=always "$IMG" build/winprep-test.img
$J fs build/winprep-test.img put $W/x86_64-windows/notepad.exe /home/provawin.exe >/dev/null
cat > "$OUT/tour.txt" <<'EOF'
wait 12
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
type Arc
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
key ret
wait 1.8
wait 55
key ctrl-spc
wait 1.5
type provawin
wait 6
key ret
wait 5
shot preparing-1
wait 2
shot preparing-1b
wait 38
shot preparing-2
wait 40
shot preparing-3
wait 40
shot program
EOF
python3 scripts/gui-shots.py build/winprep-test.img "$OUT/tour.txt" "$OUT" -m 2G -smp 4 | tail -n 3
rm -f build/winprep-test.img
LOG=$(sed 's/\x1b\[[0-9;]*m//g' "$OUT/serial.log")
fail=0
check() { if ! echo "$LOG" | grep -q "$1"; then echo "FAIL: $2"; fail=1; else echo "ok:   $2"; fi; }
check "winprep: making the prefix" "the preparation started"
check "winprep: ready after" "the preparation finished"
check 'wayland: window ".*\(Notepad\|Blocco Note\)' "the program's window came"
if echo "$LOG" | grep -q 'wayland: window "Wine"'; then echo "FAIL: a window named Wine was created"; fail=1; else echo "ok:   no window named Wine"; fi
exit $fail
