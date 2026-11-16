#!/bin/bash
# Screenshots of the installer (make shots-install). Usage:
#   scripts/shots-install.sh <image> <live CD> <out folder>
#
# justOS runs from a copy of the image as a USB stick; the disks it is
# installed on are files under build/tmp (scripts/fake-disk.py), never a disk
# of this machine. In <out folder>:
#   dark-*    every screen, the dark theme: welcome, the disks, the
#             confirmation (before and after the box is ticked), the
#             installation at three moments, done; the disk pulled out
#             halfway (failed, then no disk left)
#   light-*   the same in the light theme, but the failure
#   after-*   the disk installed on, started alone: the first-start setup
#   cd-*      the welcome from the live CD
set -eu
IMAGE=${1:?usage: shots-install.sh <image> <live CD> <out folder>}
ISO=${2:?usage: shots-install.sh <image> <live CD> <out folder>}
OUT=${3:?usage: shots-install.sh <image> <live CD> <out folder>}
JUSTTOOL=${JUSTTOOL:-tools/target/release/justtool}
mkdir -p build/tmp "$OUT"
T=$(mktemp -d build/tmp/shots-install.XXXXXX)
trap 'rm -rf "$T"' EXIT
export MEMORY=${MEMORY:-1G}

# The disks: a SATA one with Windows (MBR), an NVMe one with Windows (GPT),
# one too small.
disks() {
    python3 scripts/fake-disk.py "$T/sata.img" 8G mbr
    python3 scripts/fake-disk.py "$T/nvme.img" 16G gpt
    python3 scripts/fake-disk.py "$T/small.img" 256M empty
    DISKS=(-device ahci,id=ahci -drive "file=$T/sata.img,format=raw,if=none,id=a" -device ide-hd,drive=a,bus=ahci.0
        -drive "file=$T/nvme.img,format=raw,if=none,id=n" -device nvme,drive=n,serial=shots0001
        -drive "file=$T/small.img,format=raw,if=none,id=s" -device virtio-blk-pci,drive=s)
}

# keep <session> <prefix>: its pictures into OUT, and its journal
# (<session>-serial.log).
keep() {
    for f in "$T/$1"/*.png; do
        [ -e "$f" ] && cp "$f" "$OUT/$2-$(basename "$f")"
    done
    cp "$T/$1/serial.log" "$OUT/$1-serial.log"
}

echo "== dark"
disks
USB=1 scripts/gui-shots.py "$IMAGE" scripts/gui/install.txt "$T/dark" "${DISKS[@]}"
keep dark dark
echo "== the disk installed on, alone"
scripts/gui-shots.py "$T/sata.img" scripts/gui/install-after.txt "$T/after"
keep after after

echo "== dark: the disk pulled out"
python3 scripts/fake-disk.py "$T/usb.img" 8G empty
USB=1 scripts/gui-shots.py "$IMAGE" scripts/gui/install-fail.txt "$T/fail" \
    -drive "file=$T/usb.img,format=raw,if=none,id=u" -device usb-storage,bus=xhci.0,drive=u,id=t
keep fail dark

echo "== light"
cp --sparse=always "$IMAGE" "$T/light.img"
printf 'theme=light\n' > "$T/settings"
"$JUSTTOOL" fs "$T/light.img" put "$T/settings" /system/settings > /dev/null
disks
USB=1 scripts/gui-shots.py "$T/light.img" scripts/gui/install.txt "$T/light" "${DISKS[@]}"
keep light light
rm -f "$T/light.img"

echo "== the live CD"
disks
CDROM=1 scripts/gui-shots.py "$ISO" scripts/gui/install-cd.txt "$T/cd" "${DISKS[@]}"
keep cd cd
ls "$OUT"
