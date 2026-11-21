#!/bin/sh
# Boots a justOS image once per sound card QEMU emulates, plays a 440 Hz
# tone and a WAV file from the shell and checks what QEMU recorded: the
# tone must be there, at the right pitch and for its whole length.
#
# Two more cases play from a Linux program: paplay (libpulse, through the
# kernel's PulseAudio server) plays a 660 Hz WAV file while the shell's
# 440 Hz tone plays. With a card ("linux") both must be in the recording,
# mixed, the 660 Hz one for its whole 3 s; without one ("linux-nocard")
# the stream must still take its 3 s, played by the clock.
#
# Usage: scripts/test-audio.sh <image> [case...]
# Cases: hda (ICH9 + line out), hda-duplex (ICH6), hda-speaker, ac97,
# linux, linux-nocard.
# SLEEP=1 puts the machine to sleep (S3) for a few seconds first: the
# drivers must set the card up again on waking.
set -u
IMAGE=${1:?usage: test-audio.sh <image> [case...]}
shift
HERE=$(dirname "$0")
JUSTTOOL=${JUSTTOOL:-$HERE/../tools/target/release/justtool}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
CASES=${*:-hda hda-duplex hda-speaker ac97 linux linux-nocard}

SLEEP_CMDS=
[ "${SLEEP:-0}" = 1 ] && SLEEP_CMDS='suspend 3
@sleep 9'
cat > "$WORK/cmds.txt" <<EOT
@sleep 3
$SLEEP_CMDS
display
tone 440 2000
@sleep 3
play sys:/system/sounds/avvio.wav
@sleep 3
shutdown
EOT

# The Linux cases: pactl first (libpulse is loaded once, and the server
# answers), then the tone and paplay at once.
cat > "$WORK/linux.txt" <<EOT
@sleep 3
display
cd sys:/home/lt
pactl info
@sleep 8
tone 440 10000
paplay a660.wav
@sleep 12
volume
shutdown
EOT

# Pitch of the first 1.9 s of sound, and the length of all of it. QEMU
# records only while the card plays, so the tone (2 s) and the WAV file
# (1.9 s) come one right after the other.
cat > "$WORK/look.py" <<'EOT'
import struct, sys
d = open(sys.argv[1], "rb").read()[44:]
s = struct.unpack("<%dh" % (len(d) // 2), d[:len(d) // 2 * 2])[::2]
rate = 44100
loud = [i for i, v in enumerate(s) if abs(v) > 300]
if not loud:
    print("0 0")
    sys.exit()
a = loud[0]
seg = s[a:a + rate * 19 // 10]
crossings = sum(1 for i in range(1, len(seg)) if seg[i - 1] < 0 <= seg[i])
print(round((loud[-1] - a) / rate * 1000), round(crossings / (len(seg) / rate)))
EOT

# The Linux case's recording, in windows of 100 ms: in how many the 660 Hz
# sound is (well above what lies between it and the tone, at 560 Hz), and
# in how many both are.
cat > "$WORK/look2.py" <<'EOT'
import math, struct, sys
d = open(sys.argv[1], "rb").read()[44:]
s = struct.unpack("<%dh" % (len(d) // 2), d[:len(d) // 2 * 2])[::2]
rate = 44100
n = rate // 10
def level(seg, f):
    k = 2 * math.cos(2 * math.pi * f / rate)
    a = b = 0.0
    for x in seg:
        a, b = x + k * a - b, a
    return math.sqrt(max(a * a + b * b - k * a * b, 0.0)) * 2 / len(seg)
high = both = 0
for i in range(0, len(s) - n + 1, n):
    seg = s[i:i + n]
    t660 = level(seg, 660)
    if t660 > 1000 and t660 > 4 * level(seg, 560):
        high += 1
        both += level(seg, 440) > 1000
print(high, both)
EOT

# A 3 s, 660 Hz WAV file (48 kHz, stereo) for paplay, in a copy of the image.
cat > "$WORK/a660.py" <<'EOT'
import math, struct, sys
rate, n = 48000, 3 * 48000
pcm = b"".join(struct.pack("<hh", v, v) for v in (round(8000 * math.sin(2 * math.pi * 660 * i / rate)) for i in range(n)))
head = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 2, rate, rate * 4, 4, 16)
open(sys.argv[1], "wb").write(head + b"data" + struct.pack("<I", len(pcm)) + pcm)
EOT
linux_image() {
    [ -f "$WORK/linux.img" ] && return 0
    python3 "$WORK/a660.py" "$WORK/a660.wav" &&
        cp --sparse=always "$IMAGE" "$WORK/linux.img" &&
        { "$JUSTTOOL" fs "$WORK/linux.img" mkdir /home/lt >/dev/null 2>&1; true; } &&
        "$JUSTTOOL" fs "$WORK/linux.img" put "$WORK/a660.wav" /home/lt/a660.wav >/dev/null
}

FAILED=0
for c in $CASES; do
    IMG=$IMAGE
    CMDS=$WORK/cmds.txt
    case $c in
        hda) ARGS="-device ich9-intel-hda -device hda-output,audiodev=snd" ;;
        hda-duplex) ARGS="-device intel-hda -device hda-duplex,audiodev=snd" ;;
        hda-speaker) ARGS="-device ich9-intel-hda -device hda-micro,audiodev=snd" ;;
        ac97) ARGS="-device AC97,audiodev=snd" ;;
        linux) ARGS="-device ich9-intel-hda -device hda-output,audiodev=snd" ;;
        linux-nocard) ARGS= ;;
        *) echo "unknown case: $c" >&2; exit 2 ;;
    esac
    case $c in
        linux*)
            linux_image || { printf '  FAIL  %s: [cannot prepare the image]\n' "$c"; FAILED=1; continue; }
            IMG=$WORK/linux.img
            CMDS=$WORK/linux.txt ;;
    esac
    rm -f "$WORK/out.wav"
    "$HERE/test.sh" "$IMG" "$CMDS" -audiodev "wav,id=snd,path=$WORK/out.wav" $ARGS > "$WORK/out.txt" 2>&1
    STATUS=$?
    problems=
    [ $STATUS -ne 0 ] && problems="$problems [kernel stopped]"
    case $c in
        linux*)
            grep -q "Default Sink: justos" "$WORK/out.txt" || problems="$problems [pactl found no server]"
            # The stream lives as long as its sound: 3 s, and a little more
            # (the mixer's lead, the drain).
            ms=$(sed -n 's/.*pulse: client [0-9]*: stream 0 closed after \([0-9]*\) ms.*/\1/p' "$WORK/out.txt" | tail -n 1)
            { [ -n "$ms" ] && [ "$ms" -ge 2900 ] && [ "$ms" -le 4500 ]; } || problems="$problems [the stream lasted ${ms:-?} ms, not about 3 s]"
            grep -q "^volume [0-9]" "$WORK/out.txt" || problems="$problems [paplay did not end]"
            ;;
        *)
            grep -q "^playing sys:/system/sounds/avvio.wav" "$WORK/out.txt" || problems="$problems [the WAV file did not play]" ;;
    esac
    [ "$c" = linux-nocard ] || grep -q "^sound: audio0" "$WORK/out.txt" || problems="$problems [no sound output]"
    if [ "${SLEEP:-0}" = 1 ] && [ "$CMDS" = "$WORK/cmds.txt" ]; then
        grep -q "awake again after" "$WORK/out.txt" || problems="$problems [did not wake]"
    fi
    if [ "$c" = linux-nocard ]; then
        :
    elif [ ! -f "$WORK/out.wav" ]; then
        problems="$problems [nothing recorded]"
    elif [ "$c" = linux ]; then
        set -- $(python3 "$WORK/look2.py" "$WORK/out.wav")
        [ "${VERBOSE:-0}" = 1 ] && echo "  linux: 660 Hz for $1/10 s, with the tone for $2/10 s; stream of $ms ms"
        { [ "$1" -ge 27 ] && [ "$1" -le 33 ]; } || problems="$problems [660 Hz for $1/10 s, not 3 s]"
        [ "$2" -ge 10 ] || problems="$problems [660 and 440 Hz together for $2/10 s only]"
    else
        set -- $(python3 "$WORK/look.py" "$WORK/out.wav")
        { [ "$2" -ge 435 ] && [ "$2" -le 445 ]; } || problems="$problems [tone at $2 Hz]"
        { [ "$1" -ge 3500 ] && [ "$1" -le 4200 ]; } || problems="$problems [$1 ms of sound, not about 3.9 s]"
    fi
    if [ -z "$problems" ]; then
        printf '  ok    %s\n' "$c"
    else
        printf '  FAIL  %s:%s\n' "$c" "$problems"
        [ "${VERBOSE:-0}" = 1 ] && cat "$WORK/out.txt"
        FAILED=1
    fi
done
exit $FAILED
