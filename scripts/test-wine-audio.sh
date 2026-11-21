#!/bin/sh
# By hand, not in the suites (it needs the 5 GiB image with Wine and takes
# about 6 minutes, most of them Wine preparing its C: drive): Windows
# programs make sound and record through Wine's winepulse.drv and the
# kernel's PulseAudio server, on an emulated card with an input.
#
# A Windows program (scripts/compat/sndtest.c) plays 2 s of 660 Hz through
# WASAPI (IAudioClient, in Wine's mix format, 32-bit float), then 2 s of
# 880 Hz through the old waveOut (16 bits), then records 3 s through
# WASAPI's capture client from the microphone, into which QEMU feeds a
# 1000 Hz tone (SDL's "disk" sound driver, which also writes what the card
# plays to a file). Checks: each tone is in what the card played for about
# 2 s, the program's clocks agree, the recording has the 1000 Hz tone and
# about 3 s of frames, and the server saw a float stream and a 16-bit one.
#
# Usage: scripts/test-wine-audio.sh [image]     (default build/justos.img;
# it runs on a copy, the image is left unchanged). VERBOSE=1 prints the
# session.
set -u
cd "$(dirname "$0")/.."
IMG=${1:-build/justos.img}
J=tools/target/release/justtool
mkdir -p build/tmp
export TMPDIR=$PWD/build/tmp
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

x86_64-w64-mingw32-gcc -O2 -o "$WORK/sndtest.exe" scripts/compat/sndtest.c -lole32 -lwinmm -luuid -lm || exit 1
cp --sparse=always "$IMG" "$WORK/w.img"
$J fs "$WORK/w.img" mkdir /home/wt >/dev/null 2>&1
$J fs "$WORK/w.img" put "$WORK/sndtest.exe" /home/wt/sndtest.exe >/dev/null || exit 1

# Wine prepares C: first (wineboot, without windows), as winprep does.
# The recording goes to Wine's folder, the shell's current one. Wine's
# processes are Linux programs: the microphone permission is theirs.
cat > "$WORK/cmds.txt" <<'EOT'
@sleep 3
export WAYLAND_DISPLAY=
export XDG_RUNTIME_DIR=
export WINEDLLOVERRIDES=mscoree,mshtml=
export WINEDEBUG=-all,err+mmdevapi,err+winmm
perms allow linux.wine64 microphone
perms allow linux.wine64-preloader microphone
perms allow linux.sndtest-exe microphone
cd sys:/usr/lib/wine
./wine64 wineboot --init
@sleep 200
./wine64 /home/wt/sndtest.exe wasapi 660 2000
@sleep 15
./wine64 /home/wt/sndtest.exe waveout 880 2000
@sleep 15
./wine64 /home/wt/sndtest.exe capture 3000 cap.wav
@sleep 15
shutdown
EOT

# The 1000 Hz tone for the input: 40 s at 44.1 kHz, 16 bits, stereo.
python3 - "$WORK/mic.raw" <<'EOT'
import math, struct, sys
rate = 44100
open(sys.argv[1], "wb").write(b"".join(struct.pack("<hh", v, v) for v in (round(6000 * math.sin(2 * math.pi * 1000 * i / rate)) for i in range(40 * rate))))
EOT

# In how many windows of 100 ms each tone is: raw 44.1 kHz output from
# SDL, or a WAV file (its rate from the header).
cat > "$WORK/look.py" <<'EOT'
import math, struct, sys
d = open(sys.argv[1], "rb").read()
rate, ch = 44100, 2
if d[:4] == b"RIFF":
    rate, ch, d = struct.unpack("<I", d[24:28])[0], struct.unpack("<H", d[22:24])[0], d[44:]
s = struct.unpack("<%dh" % (len(d) // 2), d[:len(d) // 2 * 2])[::ch]
n = rate // 10
def level(seg, f):
    k = 2 * math.cos(2 * math.pi * f / rate)
    a = b = 0.0
    for x in seg:
        a, b = x + k * a - b, a
    return math.sqrt(max(a * a + b * b - k * a * b, 0.0)) * 2 / len(seg)
out = []
for f in map(float, sys.argv[2:]):
    hits = 0
    for i in range(0, len(s) - n + 1, n):
        t = level(s[i:i + n], f)
        hits += t > 1000 and t > 4 * level(s[i:i + n], f * 0.85)
    out.append(str(hits))
print(" ".join(out))
EOT

SDL_AUDIODRIVER=disk SDL_DISKAUDIOFILEIN=$WORK/mic.raw SDL_DISKAUDIOFILE=$WORK/out.raw KEEP_DISK=$WORK/after.img \
    QEMU_TIMEOUT=${QEMU_TIMEOUT:-700} BOOT_WAIT=6 STEP_WAIT=1 scripts/test.sh "$WORK/w.img" "$WORK/cmds.txt" \
    -m 2G -enable-kvm -cpu host -smp 2 \
    -audiodev sdl,id=snd -device intel-hda -device hda-duplex,audiodev=snd > "$WORK/out.txt" 2>&1
STATUS=$?
rm -f "$WORK/w.img"
# Wine's console puts cursor codes between the characters.
python3 -c "import re,sys; t=open(sys.argv[1],'rb').read().decode('utf-8','replace'); open(sys.argv[1]+'.clean','w').write(re.sub(r'\x1b\[[0-9;?]*[A-Za-z]','',t))" "$WORK/out.txt"
LOG=$WORK/out.txt.clean
# And it moves the cursor instead of writing spaces: the program's lines
# are matched without any.
tr -d ' ' < "$LOG" > "$WORK/prog.txt"
[ "${VERBOSE:-0}" = 1 ] && cat "$LOG"

problems=
[ $STATUS -ne 0 ] && problems="$problems [kernel stopped]"
grep -a "^sndtest:" "$WORK/prog.txt"
grep -q "^sndtest:wasapiplayed96000framesof660Hz" "$WORK/prog.txt" || problems="$problems [WASAPI did not play]"
grep -q "^sndtest:waveoutplayed96000framesof880Hzin[0-9]*ms$" "$WORK/prog.txt" || problems="$problems [waveOut did not play]"
grep -q "pulse: client [0-9]*: stream 0: format 5 x2 at 48000 Hz" "$LOG" || problems="$problems [no float stream]"
grep -q "pulse: client [0-9]*: stream 0: format 3 x2 at 48000 Hz" "$LOG" || problems="$problems [no 16-bit stream]"
grep -q "pulse: client [0-9]*: record stream [0-9]* from justos-mic" "$LOG" || problems="$problems [no record stream]"
# WASAPI's clock ends near the 2 s it played.
clock=$(sed -n 's/^sndtest:wasapiplayed.*clockat\([0-9]*\)ms.*/\1/p' "$WORK/prog.txt")
{ [ -n "$clock" ] && [ "$clock" -ge 1800 ] && [ "$clock" -le 2300 ]; } || problems="$problems [WASAPI's clock at ${clock:-?} ms]"
if [ -f "$WORK/out.raw" ]; then
    set -- $(python3 "$WORK/look.py" "$WORK/out.raw" 660 880)
    echo "played: 660 Hz for $1/10 s, 880 Hz for $2/10 s"
    { [ "$1" -ge 17 ] && [ "$1" -le 23 ]; } || problems="$problems [660 Hz for $1/10 s, not 2 s]"
    { [ "$2" -ge 17 ] && [ "$2" -le 23 ]; } || problems="$problems [880 Hz for $2/10 s, not 2 s]"
else
    problems="$problems [nothing played]"
fi
frames=$(sed -n 's/^sndtest:capturetook\([0-9]*\)framesin.*/\1/p' "$WORK/prog.txt")
{ [ -n "$frames" ] && [ "$frames" -ge 120000 ] && [ "$frames" -le 165000 ]; } || problems="$problems [captured ${frames:-?} frames, not about 3 s]"
if $J fs "$WORK/after.img" get /usr/lib/wine/cap.wav "$WORK/cap.wav" >/dev/null 2>&1; then
    set -- $(python3 "$WORK/look.py" "$WORK/cap.wav" 1000)
    echo "recorded: 1000 Hz for $1/10 s"
    [ "$1" -ge 25 ] || problems="$problems [1000 Hz in the recording for $1/10 s, not 3 s]"
else
    problems="$problems [no recording]"
fi
if [ -z "$problems" ]; then
    echo "  ok    wine-audio"
else
    echo "  FAIL  wine-audio:$problems"
    grep -a -E "pulse|err:|microphone" "$LOG" | tail -n 40
    exit 1
fi
