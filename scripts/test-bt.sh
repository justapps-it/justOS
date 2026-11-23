#!/bin/sh
# Bluetooth without Bluetooth hardware: boots the image with the emulated
# Realtek RTL8852BU of scripts/btemu.py on QEMU's xHCI (usb-redir over a
# Unix socket; QEMU has no Bluetooth of its own and no host device is
# used). Checks that btusb loads the firmware patch the way btrtl does,
# pairs the emulated BR/EDR keyboard (passkey) and LE mouse (Secure
# Connections), that what the keyboard types reaches the shell and that
# the mouse moves the pointer and presses its button.
#
# This proves the stack against an emulation written from the same
# specifications: the real chip can only be confirmed on real hardware.
#
# Usage: scripts/test-bt.sh <image>
# VERBOSE=1 prints the session and the emulator's log.
set -u
IMAGE=${1:?usage: test-bt.sh <image>}
HERE=$(dirname "$0")
WORK=$(mktemp -d)
EMU=
trap '[ -n "$EMU" ] && kill $EMU 2>/dev/null; rm -rf "$WORK"' EXIT
SOCK=$WORK/bt.sock

python3 "$HERE/btemu.py" --socket "$SOCK" --firmware "$HERE/../rootfs/system/firmware/rtl_bt" > "$WORK/emu.log" 2>&1 &
EMU=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -S "$SOCK" ] && break
    sleep 0.5
done
[ -S "$SOCK" ] || { echo "the emulator did not start:"; cat "$WORK/emu.log"; exit 2; }

# No screen (-vga none): no desktop, so the keyboard's keys go to the
# shell. The keyboard types with the US layout.
cat > "$WORK/bt.txt" <<'EOT'
@sleep 4
kbd us
bt
bt scan 4
@sleep 6
bt pair Tastiera Emu
@sleep 14
cat sys:/home/bt.txt
bt pair Mouse Emu
@sleep 10
kbd
bt
shutdown
EOT
QEMU_TIMEOUT=${QEMU_TIMEOUT:-150} "$HERE/test.sh" "$IMAGE" "$WORK/bt.txt" -vga none \
    -device qemu-xhci,id=u -chardev socket,id=btc,path="$SOCK" -device usb-redir,chardev=btc,bus=u.0 > "$WORK/out.txt" 2>&1
STATUS=$?
cat "$WORK/emu.log" >> "$WORK/out.txt"

missing=
check() {
    grep -q -- "$1" "$WORK/out.txt" || missing="$missing
  [no \"$1\"]"
}
check "btusb 1.0.0 installed"
check "RTL8852BU: ROM version 1, loading rtl_bt/rtl8852bu_fw.bin (62539 bytes)"
check "btemu: Realtek: patch accepted: 62539 bytes in 249 fragments"
check "RTL8852BU: firmware running"
check "hci0 ready: address 00:E0:4C:88:52:01"
check "0A:1B:2C:3D:4E:01  Tastiera Emu"
check "C0:FF:EE:00:00:02  Mouse Emu"
check "btemu: keyboard: link encrypted"
check "btemu: keyboard: SDP answer, part 2 of 2"
check "btemu: keyboard: HID channels open"
check "ciao dal bluetooth"
check "btemu: mouse: Secure Connections pairing (Just Works), DHKey check ok"
check "btemu: mouse: LE encryption with the right key"
check "btemu: mouse: report map read at 44"
check "btemu: mouse: connection parameters accepted"
check "HID over GATT: report map of 54 bytes, 1 input reports"
check "mouse:  moved 100, 50  wheel 0  buttons 001"
[ "$(grep -c 'paired and connected' "$WORK/out.txt")" -ge 2 ] || missing="$missing
  [not both paired]"
grep -q "WRONG\|REJECTED" "$WORK/out.txt" && missing="$missing
  [the emulator saw something wrong]"
[ $STATUS -ne 0 ] && missing="$missing
  [kernel stopped]"
if [ -z "$missing" ]; then
    echo "  ok    bluetooth (emulated RTL8852BU: firmware, keyboard, mouse)"
    [ "${VERBOSE:-0}" = 1 ] && cat "$WORK/out.txt"
    exit 0
fi
echo "  FAIL  bluetooth:$missing"
[ "${VERBOSE:-0}" = 1 ] && cat "$WORK/out.txt"
exit 1
