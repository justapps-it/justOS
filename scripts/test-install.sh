#!/bin/bash
# Installing justOS on a disk, end to end. Usage:
#   scripts/test-install.sh <image> [<live CD>]
#
# Every disk is a file in a folder under build/tmp: a copy of the image (the
# installation medium), disks to install on (empty, sparse, some with a
# made-up Windows on them) and one too small. No disk of this machine is
# opened. Each machine is driven on its serial console (install-system,
# the shell's command), then the installed disk is started alone:
#
# 1. BIOS: the medium on IDE (set up, with a file of the user's); a disk
#    with Windows (MBR) on AHCI and one too small on virtio. The list (the
#    medium never in it, what each disk holds), refusals (a wrong code, the
#    small disk, the medium: nothing is written), then the installation on
#    the AHCI disk, and what it wrote read from its file here.
# 2. BIOS: that disk alone. It starts, its system volume fills the disk,
#    `check sys` finds no problems, the files are there, the setup has not
#    run, both slots hold this version, the installer is not offered.
# 3. UEFI: the medium as a USB stick (xHCI): the interface offers the
#    installer; a disk with Windows (GPT: EFI and NTFS) on NVMe, installed
#    from the shell.
# 4. UEFI, then BIOS: that NVMe disk alone, as in 2.
# 5. With a live CD: BIOS, the CD and an empty virtio disk; installed from
#    what the CD carries (sys:/system/install).
# 6. BIOS: that disk alone.
#
# Exits non-zero if a check failed; KEEP=<folder> keeps the sessions' logs.
set -u
IMAGE=${1:?usage: test-install.sh <image> [<live CD>]}
ISO=${2:-}
QEMU=${QEMU:-qemu-system-x86_64}
JUSTTOOL=${JUSTTOOL:-tools/target/release/justtool}
OVMF_CODE=${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}
OVMF_VARS=${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' system/Cargo.toml | head -1)
# The disks to install on: bigger than the medium.
TARGET_SIZE=${TARGET_SIZE:-3G}
mkdir -p build/tmp
T=$(mktemp -d build/tmp/test-install.XXXXXX)
PID=
cleanup() {
    [ -n "$PID" ] && kill "$PID" 2>/dev/null
    rm -rf "$T"
}
trap cleanup EXIT
FAILED=0
CUR=

# Disks made up for the test: <file> <size> empty|mbr|gpt (see
# scripts/fake-disk.py: a made-up Windows on MBR or GPT).
disk() {
    python3 scripts/fake-disk.py "$@"
}

# start <name> <qemu options...>: a machine in the background, its serial
# console fed from a pipe and written to <name>.raw.
start() {
    CUR=$1
    shift
    rm -f "$T/$CUR.in"
    mkfifo "$T/$CUR.in"
    timeout 1200 "$QEMU" -m 1G -display none -monitor none -no-reboot -serial stdio "$@" < "$T/$CUR.in" > "$T/$CUR.raw" 2>&1 &
    PID=$!
    exec 3> "$T/$CUR.in"
    STARTED=$(date +%s)
    FROM=0
    SINCE=0
    BAD=0
}

# The session so far, without colours.
log() {
    sed 's/\x1b\[[0-9;]*[A-Za-z]//g; s/\r//g' "$T/$CUR.raw"
}

# say <line>: types it on the console.
say() {
    printf '%s\r' "$1" >&3
}

# upto <seconds> <pattern>: waits until the session shows it (after line
# FROM: the answer to the last command, not an earlier one).
upto() {
    local n=$(($1 * 2))
    while [ $n -gt 0 ]; do
        log | tail -n +"$((FROM + 1))" | grep -qaE -- "$2" && return 0
        kill -0 "$PID" 2>/dev/null || break
        sleep 0.5
        n=$((n - 1))
    done
    return 1
}

# run <line> <pattern> [seconds]: types the line and waits for its answer.
run() {
    local mark
    mark=$(log | wc -l)
    FROM=$mark
    say "$1"
    if ! upto "${3:-20}" "$2"; then
        echo "      (no \"$2\" after \"$1\")"
    fi
    sleep 0.5
    SINCE=$mark
}

# The lines since the last command.
answer() {
    log | tail -n +"$((SINCE + 1))"
}

# stop: turns the machine off, keeps its log (VERBOSE=1: prints it if a
# check failed).
stop() {
    say shutdown
    for _ in $(seq 60); do
        kill -0 "$PID" 2>/dev/null || break
        sleep 0.5
    done
    kill "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null
    PID=
    exec 3>&-
    log > "$T/$CUR.log"
    echo "      ($CUR: $(($(date +%s) - STARTED)) s)"
    if [ -n "${KEEP:-}" ]; then
        mkdir -p "$KEEP" && cp "$T/$CUR.log" "$KEEP/"
    fi
    if [ "${VERBOSE:-}" = 1 ] && [ "$BAD" = 1 ]; then
        echo "---- $CUR"
        cat "$T/$CUR.log"
        echo "----"
    fi
}

ok() { echo "ok    $CUR: $1"; }
fail() {
    echo "FAIL  $CUR: $1"
    FAILED=1
    BAD=1
}
# expect <pattern> <what>; refuse <pattern> <what>: in the whole session.
expect() { if log | grep -qaE -- "$1"; then ok "$2"; else fail "$2 (no \"$1\")"; fi; }
refuse() { if log | grep -qaE -- "$1"; then fail "$2 (\"$1\")"; else ok "$2"; fi; }
# booted <seconds>: the shell's prompt is there.
booted() {
    if upto "$1" "^sys:/ ›"; then ok "started"; else fail "did not start"; fi
}

# The list's line for a bus: "  disk2    code 1a2b3c · 3,2 GB · SATA · ...".
line_for() { log | tail -n +"$((SINCE + 1))" | grep -aE "^  disk[0-9]+ +code [0-9a-f]{6} · [^·]+ · $1 ·" | tail -1; }
name_of() { echo "$1" | awk '{print $1}'; }
code_of() { echo "$1" | awk '{print $3}'; }

# install <bus>: installs on the listed disk of that bus.
install_on() {
    run "install-system list" "disks justOS can be installed on|no disk to install"
    local l
    l=$(line_for "$1")
    TARGET=$(name_of "$l")
    CODE=$(code_of "$l")
    if [ -z "$TARGET" ]; then
        fail "no $1 disk listed"
        return 1
    fi
    local t0
    t0=$(date +%s)
    run "install-system $TARGET --yes-erase $CODE" "is installed on $TARGET|install-system: " 900
    if answer | grep -qa "justOS $VERSION is installed on $TARGET"; then
        ok "installed on $TARGET ($1) in $(($(date +%s) - t0)) s"
    else
        fail "installing on $TARGET ($1): $(answer | grep -a 'install-system:' | head -1)"
    fi
    for phase in "Preparo il disco" "Copio il sistema" "Copio le applicazioni" "Verifico la copia" "Ultimi ritocchi"; do
        if answer | grep -qaE "^install: [0-9]+% $phase"; then ok "phase: $phase"; else fail "phase: $phase"; fi
    done
    if answer | grep -qaE "^install: 99% "; then ok "progress up to 99% before the end"; else fail "progress"; fi
}

# The checks on an installed disk started alone.
installed_checks() {
    run "vol" "^  sys: "
    echo "      $(answer | grep -aE '^  sys: ' | head -1)"
    run "sysupdate" "slot B:"
    expect "slot A: $VERSION · works · running" "slot A holds this version and runs"
    expect "slot B: $VERSION · works" "slot B holds it too"
    expect "system slot A$" "the loader started slot A"
    run "check sys" "sys: (no problems found|[0-9]+ problems)" 120
    expect "sys: no problems found" "check sys finds no problems"
    run "cat sys:/system/installed" "^justOS .* installed |not found|error"
    expect "^justOS $VERSION installed [0-9]+ from " "the installer's mark"
    run "cat sys:/system/settings" "^[a-z-]+=|not found|error" 5
    refuse "^setup=done" "the first-start setup has not run"
    run "ls sys:/system/install" "stage1|not found|error|empty" 5
    if answer | grep -qa "stage1.bin"; then fail "the installer's payload was copied"; else ok "no installer payload"; fi
    run "ls sys:/apps" "\.app" 5
    expect "\.app" "the apps are there"
    run "sha256 sys:/system/gui/icons.qoi" "[0-9a-f]{64}" 10
    expect "[0-9a-f]{64}" "the interface's pictures are there"
    run "install-system list" "disks justOS can be installed on|no disk to install"
    expect "no disk to install justOS on" "its own disk is not a target"
    refuse "installer shown" "the installer is not offered"
    refuse "justOS stopped|kernel panic" "no panic"
}

# What the installer wrote, read on this machine from the disk's file.
host_checks() {
    python3 - "$1" "$2" > "$T/host.txt" 2>&1 <<'EOF'
import struct, sys
path, medium = sys.argv[1:3]
d = open(path, 'rb')
mbr = d.read(512)
def check(cond, what):
    print(('ok ' if cond else 'FAIL ') + what)
check(mbr[510:512] == b'\x55\xaa' and mbr[0x190:0x194] == b'ARCB', 'MBR with the BIOS loader and its boot table')
parts = [struct.unpack_from('<BxxxBxxxII', mbr, 0x1BE + i * 16) for i in range(4)]
check(parts[0][0] == 0x80 and parts[0][1] == 0xAC, 'partition 1: JustFS, active')
check(parts[1][1] == 0xEF and parts[1][2] == 2048, 'partition 2: EFI system partition at 1 MiB')
d.seek(0, 2)
size = d.tell() // 512
end = parts[0][2] + parts[0][3]
check(size - 8 <= end <= size, f'the system volume reaches the end of the disk ({end} of {size} sectors)')
sig = struct.unpack_from('<I', mbr, 0x1B8)[0]
m = open(medium, 'rb').read(512)
check(sig != 0 and sig != struct.unpack_from('<I', m, 0x1B8)[0], f'a disk signature of its own ({sig:08x})')
ctl = struct.unpack_from('<I', mbr, 0x18C)[0]
d.seek(ctl * 512)
c = d.read(512)
check(c[:8] == b'ARCCTL01' and sum(struct.unpack('<128I', c)) & 0xFFFFFFFF == 0, 'BOOT.CTL sealed')
check(struct.unpack_from('<I', c, 0x1C)[0] == sig, 'BOOT.CTL names the disk signature')
for slot in struct.unpack_from('<II', c, 8):
    d.seek(slot * 512)
    h = d.read(512)
    check(h[:8] == b'ARCSLOT1' and sum(struct.unpack('<128I', h)) & 0xFFFFFFFF == 0, f'slot at {slot} sealed')
EOF
    while read -r l; do
        case "$l" in
            "ok "*) ok "${l#ok }" ;;
            "FAIL "*) fail "${l#FAIL }" ;;
            *) fail "reading the disk: $l" ;;
        esac
    done < "$T/host.txt"
    if "$JUSTTOOL" fs "$1" check > "$T/check.txt" 2>&1; then ok "justtool: $(cat "$T/check.txt")"; else fail "justtool fs check: $(cat "$T/check.txt")"; fi
    "$JUSTTOOL" fs "$1" ls /system > "$T/ls.txt" 2>&1
    if grep -qE ' settings$' "$T/ls.txt"; then fail "the medium's settings were copied"; else ok "no settings copied"; fi
    if grep -qE ' install/$' "$T/ls.txt"; then fail "the installer's payload was copied"; else ok "no installer payload copied"; fi
}

OVMF() {
    cp "$OVMF_VARS" "$T/vars-$1.fd"
    echo "-drive if=pflash,format=raw,readonly=on,file=$OVMF_CODE -drive if=pflash,format=raw,file=$T/vars-$1.fd"
}

echo "== 1. BIOS: medium on IDE, Windows on AHCI, a small disk on virtio"
cp --sparse=always "$IMAGE" "$T/medium.img"
# The medium was set up (its settings stay on it) and has a file of the
# user's (that goes along).
printf 'setup=done\ntheme=light\n' > "$T/settings"
echo "a file of the user's, $(date +%s)" > "$T/note.txt"
"$JUSTTOOL" fs "$T/medium.img" put "$T/settings" /system/settings > /dev/null
"$JUSTTOOL" fs "$T/medium.img" put "$T/note.txt" /home/nota-install.txt > /dev/null
disk "$T/ahci.img" "$TARGET_SIZE" mbr
disk "$T/small.img" 256M empty
start bios-install \
    -drive file="$T/medium.img",format=raw,if=none,id=m -device ide-hd,drive=m,bus=ide.0,bootindex=0 \
    -device ahci,id=ahci -drive file="$T/ahci.img",format=raw,if=none,id=a -device ide-hd,drive=a,bus=ahci.0 \
    -drive file="$T/small.img",format=raw,if=none,id=s -device virtio-blk-pci,drive=s
booted 90
run "disks" "→ sys:"
MEDIUM=$(answer | grep -a '→ sys:' | awk '{print $1}' | sed 's/p[0-9]*$//' | head -1)
echo "      the medium is $MEDIUM"
run "install-system list" "disks justOS can be installed on|no disk to install"
SATA=$(line_for SATA)
SMALL=$(line_for virtio)
echo "      $SATA"
echo "      $SMALL"
if echo "$SATA" | grep -q "contiene Windows (NTFS)"; then ok "the AHCI disk: contiene Windows (NTFS)"; else fail "the AHCI disk's contents"; fi
if answer | grep -qa "NTFS 3,2 GB"; then ok "its partition: NTFS 3,2 GB"; else fail "its partition"; fi
if echo "$SMALL" | grep -q "è vuoto"; then ok "the small disk: è vuoto"; else fail "the small disk's contents"; fi
if answer | grep -qa "cannot be used: troppo piccolo: servono almeno"; then ok "the small disk is too small"; else fail "the small disk is not refused"; fi
if [ -n "$MEDIUM" ] && ! answer | grep -qaE "^  $MEDIUM "; then ok "the medium ($MEDIUM) is not listed"; else fail "the medium is listed"; fi
A=$(name_of "$SATA")
ACODE=$(code_of "$SATA")
run "install-system $A" "to go ahead"
expect "installing justOS on $A erases everything on it: contiene Windows \(NTFS\)" "it says what is erased"
expect "to go ahead: install-system $A --yes-erase $ACODE" "it names the code"
run "install-system $A --yes-erase 000000" "install-system: "
expect "il codice di conferma per $A non è 000000" "a wrong code is refused"
run "install-system $A" "to go ahead"
run "install-system $(name_of "$SMALL") --yes-erase $(code_of "$SMALL")" "install-system: "
expect "non si può usare: troppo piccolo" "the small disk is refused"
run "install-system $MEDIUM --yes-erase $ACODE" "install-system: "
expect "$MEDIUM non è un disco su cui installare justOS" "the medium is refused"
run "install-system list" "disks justOS can be installed on"
if [ "$(code_of "$(line_for SATA)")" = "$ACODE" ]; then ok "nothing was written: the code is the same"; else fail "the AHCI disk changed"; fi
install_on SATA
run "install-system list" "disks justOS can be installed on"
if line_for SATA | grep -q "contiene justOS"; then ok "the disk now holds justOS"; else fail "the disk's new contents: $(line_for SATA)"; fi
refuse "installer shown" "no installer offered from a disk that is not removable"
refuse "justOS stopped|kernel panic" "no panic"
stop
host_checks "$T/ahci.img" "$T/medium.img"
"$JUSTTOOL" fs "$T/ahci.img" cat /home/nota-install.txt > "$T/note-copy.txt" 2>&1
if cmp -s "$T/note.txt" "$T/note-copy.txt"; then ok "the user's file went along"; else fail "the user's file: $(cat "$T/note-copy.txt")"; fi

echo "== 2. BIOS: the AHCI disk alone"
start bios-installed -device ahci,id=ahci -drive file="$T/ahci.img",format=raw,if=none,id=a -device ide-hd,drive=a,bus=ahci.0,bootindex=0
booted 90
expect "loader: BIOS" "started by the BIOS loader"
installed_checks
stop
rm -f "$T/ahci.img" "$T/small.img"

echo "== 3. UEFI: medium on a USB stick, Windows (GPT) on NVMe"
cp --sparse=always "$IMAGE" "$T/medium.img"
disk "$T/nvme.img" "$TARGET_SIZE" gpt
start uefi-install $(OVMF 3) \
    -device qemu-xhci,id=x -drive file="$T/medium.img",format=raw,if=none,id=m -device usb-storage,bus=x.0,drive=m,bootindex=0 \
    -drive file="$T/nvme.img",format=raw,if=none,id=n -device nvme,drive=n,serial=inst0001
booted 150
expect "loader: UEFI" "started by the UEFI loader"
if upto 30 "installer shown"; then ok "the interface offers the installer (running from a USB stick)"; else fail "no installer offered from the USB stick"; fi
expect "installer shown \(running from Usb\)" "it knows it runs from USB"
run "install-system list" "disks justOS can be installed on|no disk to install"
NVME=$(line_for NVMe)
echo "      $NVME"
if echo "$NVME" | grep -q "contiene Windows (NTFS)"; then ok "the NVMe disk: contiene Windows (NTFS)"; else fail "the NVMe disk's contents"; fi
if answer | grep -qaE "EFI [0-9,]+ MB · NTFS [0-9,]+ GB"; then ok "its partitions: EFI and NTFS"; else fail "its partitions"; fi
if answer | grep -qa "· USB ·"; then fail "the USB stick is listed"; else ok "the USB stick is not listed"; fi
install_on NVMe
refuse "justOS stopped|kernel panic" "no panic"
stop
host_checks "$T/nvme.img" "$T/medium.img"
rm -f "$T/medium.img"

echo "== 4. UEFI, then BIOS: the NVMe disk alone"
start uefi-installed $(OVMF 4) -drive file="$T/nvme.img",format=raw,if=none,id=n -device nvme,drive=n,serial=inst0001,bootindex=0
booted 150
expect "loader: UEFI" "started by the UEFI loader (through \\EFI\\BOOT\\BOOTX64.EFI)"
installed_checks
stop
start nvme-bios -drive file="$T/nvme.img",format=raw,if=none,id=n -device nvme,drive=n,serial=inst0001,bootindex=0
booted 90
expect "loader: BIOS" "the same disk starts with the BIOS loader too"
run "check sys" "sys: (no problems found|[0-9]+ problems)" 120
expect "sys: no problems found" "check sys finds no problems"
stop
rm -f "$T/nvme.img"

if [ -n "$ISO" ]; then
    echo "== 5. BIOS: the live CD, an empty virtio disk"
    disk "$T/virtio.img" "$TARGET_SIZE" empty
    start cd-install \
        -drive file="$ISO",format=raw,if=none,id=cd,media=cdrom,readonly=on -device ide-cd,drive=cd,bus=ide.1,bootindex=0 \
        -drive file="$T/virtio.img",format=raw,if=none,id=v -device virtio-blk-pci,drive=v
    booted 120
    if upto 30 "installer shown"; then ok "the interface offers the installer (running from the CD)"; else fail "no installer offered from the CD"; fi
    run "install-system list" "disks justOS can be installed on|no disk to install"
    echo "      $(line_for virtio)"
    if line_for virtio | grep -q "è vuoto"; then ok "the virtio disk: è vuoto"; else fail "the virtio disk's contents"; fi
    if answer | grep -qaE "^  disk[0-9]+ +code .* · IDE · "; then fail "the CD drive is listed"; else ok "the CD drive is not listed"; fi
    install_on virtio
    refuse "justOS stopped|kernel panic" "no panic"
    stop

    echo "== 6. BIOS: the virtio disk alone"
    start cd-installed -drive file="$T/virtio.img",format=raw,if=none,id=v -device virtio-blk-pci,drive=v,bootindex=0
    booted 90
    installed_checks
    expect "^justOS $VERSION installed [0-9]+ from memory" "installed from the CD's system in memory"
    stop
fi

exit $FAILED
