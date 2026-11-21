//! Sound outputs registered by drivers, and the mixer that plays on them.
//!
//! Everything that makes sound goes through one mixer thread, which adds
//! the sources up and writes the sum to the first output:
//!
//! * the system voice: the desktop's sounds and the music player, one at a
//!   time (a new sound stops the one playing);
//! * streams that programs feed ([`Stream`]): the PulseAudio server of the
//!   Linux layer keeps one for each stream of a Linux program, and they all
//!   play at once.
//!
//! Each source is converted to the output's rate as it plays (linear
//! interpolation); the system volume applies to the sum. The mixer keeps
//! only a little queued in the device ([`AHEAD_MS`]), so that stopping,
//! volume changes and new sounds are heard at once, and a program can be
//! told when what it writes will be heard ([`Stream::timing`]). With no
//! output, streams are still consumed in real time: a program that waits
//! for its sound to be played goes on. When nothing plays, the device plays
//! what it has queued and stops.

use crate::sync::{IrqLock, Mutex};
use crate::task::WaitQueue;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use justdk::{AudioDesc, AudioQueued, Status, E_INVAL, OK};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// How far ahead of the device the mixer writes, and in steps of how much.
pub const AHEAD_MS: u64 = 50;
pub const STEP_MS: u64 = 10;
/// A stream that ran out keeps the output going this long, for more to
/// come.
const LINGER_MS: u64 = 1000;
/// The rate streams are consumed at when there is no output.
const NO_OUTPUT_RATE: u32 = 48_000;

pub struct Output {
    pub device: u32,
    pub name: String,
    pub rate: u32,
    pub channels: u32,
    desc: *const AudioDesc,
    /// How many frames the device still has to play, if the driver says.
    queued: Option<AudioQueued>,
}

// SAFETY: the descriptor belongs to the driver; only the mixer calls it.
unsafe impl Send for Output {}

static OUTPUTS: IrqLock<Vec<Option<Output>>> = IrqLock::new(Vec::new());
static VOLUME: AtomicU32 = AtomicU32::new(80);
/// The sound [`play_track`] started: a number given to each new one, the
/// frame being played, how many it has, and whether it is still going.
static TRACK: AtomicU64 = AtomicU64::new(0);
static POSITION: AtomicU64 = AtomicU64::new(0);
static LENGTH: AtomicU64 = AtomicU64::new(0);
static PLAYING: AtomicBool = AtomicBool::new(false);

pub fn register(device: u32, desc: *const AudioDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    if d.rate < 8000 || d.rate > 192_000 || d.channels == 0 || d.channels > 8 {
        return E_INVAL;
    }
    let len = d.name.iter().position(|&b| b == 0).unwrap_or(d.name.len());
    let name = String::from(core::str::from_utf8(&d.name[..len]).unwrap_or("sound output"));
    let mut outs = OUTPUTS.lock();
    let n = outs.len();
    crate::info!("audio", "audio{n}: {name}, {} Hz, {} channels", d.rate, d.channels);
    outs.push(Some(Output { device, name, rate: d.rate, channels: d.channels, desc, queued: None }));
    n as Status
}

/// The driver of output `output` says how to ask it what it has queued.
pub fn set_queued(output: u32, queued: AudioQueued) -> Status {
    match OUTPUTS.lock().get_mut(output as usize) {
        Some(Some(o)) => {
            o.queued = Some(queued);
            OK
        }
        _ => E_INVAL,
    }
}

pub fn remove_device(device: u32) {
    for slot in OUTPUTS.lock().iter_mut() {
        if slot.as_ref().is_some_and(|o| o.device == device) {
            *slot = None;
        }
    }
}

pub struct OutputInfo {
    pub index: usize,
    pub name: String,
    pub rate: u32,
    pub channels: u32,
}

pub fn list() -> Vec<OutputInfo> {
    OUTPUTS
        .lock()
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let o = o.as_ref()?;
            Some(OutputInfo { index: i, name: o.name.clone(), rate: o.rate, channels: o.channels })
        })
        .collect()
}

pub fn available() -> bool {
    OUTPUTS.lock().iter().any(|o| o.is_some())
}

/// Volume, 0 to 100.
pub fn volume() -> u32 {
    VOLUME.load(Ordering::Relaxed)
}

pub fn set_volume(v: u32) {
    VOLUME.store(v.min(100), Ordering::Relaxed);
}

/// A sound in memory: interleaved signed 16-bit samples.
pub struct Sound {
    pub rate: u32,
    pub channels: u32,
    pub samples: Vec<i16>,
}

impl Sound {
    pub fn seconds_x10(&self) -> u64 {
        let frames = self.samples.len() as u64 / self.channels.max(1) as u64;
        frames * 10 / self.rate.max(1) as u64
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads a WAV file: integer PCM of 8, 16, 24 or 32 bits, any rate, one or
/// more channels.
pub fn parse_wav(b: &[u8]) -> Result<Sound, &'static str> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file");
    }
    let mut at = 12;
    let mut fmt: Option<(u16, u32, u16, u16)> = None;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = le32(b, at + 4) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            let mut format = le16(body, 0);
            if format == 0xFFFE && body.len() >= 26 {
                format = le16(body, 24); // WAVE_FORMAT_EXTENSIBLE: the sub-format
            }
            fmt = Some((format, le32(body, 4), le16(body, 2), le16(body, 14)));
        } else if id == b"data" {
            let Some((format, rate, channels, bits)) = fmt else { return Err("WAV data before its format") };
            if format != 1 {
                return Err("compressed WAV files are not supported");
            }
            if channels == 0 || !(1000..=192_000).contains(&rate) {
                return Err("unsupported WAV format");
            }
            let width = match bits {
                8 | 16 | 24 | 32 => bits as usize / 8,
                _ => return Err("unsupported sample size"),
            };
            let samples = body
                .chunks_exact(width)
                .map(|s| match width {
                    1 => ((s[0] as i16) - 128) << 8,
                    2 => i16::from_le_bytes([s[0], s[1]]),
                    3 => i16::from_le_bytes([s[1], s[2]]),
                    _ => i16::from_le_bytes([s[2], s[3]]),
                })
                .collect();
            return Ok(Sound { rate, channels: channels as u32, samples });
        }
        at += 8 + len + (len & 1);
    }
    Err("no sound in the WAV file")
}

/// A sine-ish tone (a triangle wave, which needs no floating point).
pub fn tone(hz: u32, ms: u32) -> Sound {
    let rate = 48_000u32;
    let frames = (rate as u64 * ms as u64 / 1000) as usize;
    let period = (rate / hz.clamp(20, 20_000)).max(2) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let p = (i % period) as i32 * 4 * 12000 / period as i32;
        let mut v = if p < 2 * 12000 { p - 12000 } else { 3 * 12000 - p };
        // 5 ms fade in and out, against clicks.
        let edge = (rate as usize / 200).max(1);
        let fade = i.min(frames - 1 - i).min(edge) as i32;
        v = v * fade / edge as i32;
        samples.push(v as i16);
        samples.push(v as i16);
    }
    Sound { rate, channels: 2, samples }
}

/// Reads only the header of a WAV file: (rate, channels, frames).
pub fn wav_info(b: &[u8]) -> Option<(u32, u32, u64)> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut fmt = None;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = le32(b, at + 4) as usize;
        if id == b"fmt " && at + 8 + 16 <= b.len() {
            fmt = Some((le32(b, at + 12), le16(b, at + 10) as u32, le16(b, at + 22) as u32));
        } else if id == b"data" {
            let (rate, ch, bits) = fmt?;
            let frame = (ch * bits / 8).max(1) as u64;
            return Some((rate, ch, len as u64 / frame));
        }
        at += 8 + len + (len & 1);
    }
    None
}


// ----------------------------------------------------------------------------
// The system voice

/// The sound the system plays: one at a time.
struct Voice {
    sound: Arc<Sound>,
    /// Where it is, in 1/2^32 frames of the sound.
    pos: u64,
    /// The number [`play_track`] gave it (0 for other sounds).
    track: u64,
}

static VOICE: Mutex<Option<Voice>> = Mutex::new(None);

/// Puts a sound on the system voice, in place of the one playing.
fn start_voice(v: Option<Voice>) {
    let old = core::mem::replace(&mut *VOICE.lock(), v);
    if let Some(old) = old {
        track_ended(old.track, None);
    }
    start_mixer();
    WORK.notify_all();
}

/// Track `track` stopped (at its end, `length` frames, if given).
fn track_ended(track: u64, length: Option<u64>) {
    if track != 0 && TRACK.load(Ordering::Acquire) == track {
        if let Some(n) = length {
            POSITION.store(n, Ordering::Relaxed);
        }
        PLAYING.store(false, Ordering::Release);
    }
}

/// Plays a sound in the background from frame `from`, keeping track of
/// where it is ([`progress`]). Returns the number it is known by.
pub fn play_track(s: Arc<Sound>, from: usize) -> Result<u64, &'static str> {
    if !available() {
        return Err("no sound output");
    }
    if s.channels == 0 {
        return Err("unsupported sound");
    }
    let track = TRACK.fetch_add(1, Ordering::AcqRel) + 1;
    POSITION.store(from as u64, Ordering::Relaxed);
    LENGTH.store((s.samples.len() / s.channels as usize) as u64, Ordering::Relaxed);
    PLAYING.store(true, Ordering::Release);
    start_voice(Some(Voice { sound: s, pos: (from as u64) << 32, track }));
    Ok(track)
}

/// The sound started by [`play_track`]: its number, the frame being played,
/// its length in frames and whether it is still playing.
pub fn progress() -> (u64, u64, u64, bool) {
    (TRACK.load(Ordering::Acquire), POSITION.load(Ordering::Relaxed), LENGTH.load(Ordering::Relaxed), PLAYING.load(Ordering::Acquire))
}

/// Plays a sound in the background.
pub fn play(s: Sound) -> Result<(), &'static str> {
    if !available() {
        return Err("no sound output");
    }
    let voice = (!s.samples.is_empty() && s.channels != 0).then(|| Voice { sound: Arc::new(s), pos: 0, track: 0 });
    start_voice(voice);
    Ok(())
}

/// Stops the sound being played.
pub fn stop() {
    start_voice(None);
    PLAYING.store(false, Ordering::Release);
}

/// `a` to `b` by `frac` (in 1/65536).
fn lerp(a: i16, b: i16, frac: u64) -> i64 {
    let (a, b) = (a as i64, b as i64);
    a + (b - a) * frac as i64 / 65536
}

/// Adds the system voice to `mix` (stereo frames at `rate`).
fn mix_voice(mix: &mut [[i32; 2]], rate: u32) {
    let mut slot = VOICE.lock();
    let Some(v) = slot.as_mut() else { return };
    let s = &*v.sound;
    let ch = s.channels as usize;
    let frames = s.samples.len() / ch.max(1);
    let step = ((s.rate as u64) << 32) / rate as u64;
    for out in mix.iter_mut() {
        let j = (v.pos >> 32) as usize;
        if j >= frames {
            break;
        }
        let k = (j + 1).min(frames - 1);
        let frac = (v.pos >> 16) & 0xFFFF;
        for (c, o) in out.iter_mut().enumerate() {
            // Mono goes to both sides; channels past the second are left out.
            let src = c.min(ch - 1);
            *o += lerp(s.samples[j * ch + src], s.samples[k * ch + src], frac) as i32;
        }
        v.pos += step;
    }
    let at = ((v.pos >> 32) as usize).min(frames) as u64;
    if v.track != 0 && TRACK.load(Ordering::Acquire) == v.track {
        POSITION.store(at, Ordering::Relaxed);
    }
    if at as usize >= frames {
        track_ended(v.track, Some(frames as u64));
        *slot = None;
    }
}

// ----------------------------------------------------------------------------
// Streams

/// A stream of sound a program plays: the program queues frames, the
/// mixer takes them as the output plays. The PulseAudio server of the
/// Linux layer keeps one for each playback stream of its clients.
pub struct Stream {
    st: Mutex<StreamState>,
    /// Frames the mixer took and wrote to the output: the stream's read
    /// index. It moves together with [`END_NS`] (under [`TIMING`]).
    taken: AtomicU64,
    /// Told when the mixer took frames, when the stream ran out and when it
    /// started playing.
    pub wq: WaitQueue,
}

struct StreamState {
    rate: u32,
    /// Stereo frames waiting to be played.
    queue: VecDeque<[i16; 2]>,
    /// Between the first queued frame and the next, in 1/2^32.
    frac: u64,
    /// Volume, in 1/65536.
    gain: u32,
    corked: bool,
    /// The stream waits for this many frames before it plays, at the start
    /// and each time it ran out (PulseAudio's prebuffering): it is
    /// `filling` meanwhile, unless `triggered` lets the next frames play
    /// however few they are.
    prebuf: usize,
    filling: bool,
    triggered: bool,
    /// Frames played since it last started, and frames of silence since it
    /// last ran out.
    playing_for: u64,
    underrun_for: u64,
    /// How many times it ran out while playing, and how many times it
    /// started.
    underruns: u64,
    starts: u64,
    /// When it last ran out (uptime, ms; 0: never).
    ran_out: u64,
}

/// A stream's place in time ([`Stream::timing`]).
pub struct Timing {
    /// Frames taken from the stream so far.
    pub taken: u64,
    /// How long the last of them waits before it is heard (µs).
    pub latency_us: u64,
    /// It is playing: not corked, not filling, started.
    pub playing: bool,
    /// Frames played since it last started, and of silence since it last
    /// ran out.
    pub playing_for: u64,
    pub underrun_for: u64,
}

static STREAMS: Mutex<Vec<Arc<Stream>>> = Mutex::new(Vec::new());

/// A new stream of frames at `rate` (frames per second), which plays once
/// `prebuf` frames are queued (and it is not `corked`).
pub fn open_stream(rate: u32, prebuf: usize, corked: bool) -> Arc<Stream> {
    let st = StreamState {
        rate: rate.clamp(1000, 384_000),
        queue: VecDeque::new(),
        frac: 0,
        gain: 65536,
        corked,
        prebuf,
        filling: true,
        triggered: false,
        playing_for: 0,
        underrun_for: 0,
        underruns: 0,
        starts: 0,
        ran_out: 0,
    };
    let s = Arc::new(Stream { st: Mutex::new(st), taken: AtomicU64::new(0), wq: WaitQueue::new() });
    STREAMS.lock().push(s.clone());
    start_mixer();
    s
}

impl Stream {
    /// Queues frames.
    pub fn push(&self, frames: &[[i16; 2]]) {
        let mut st = self.st.lock();
        st.queue.extend(frames.iter().copied());
        st.check_filled();
    }

    /// Frames queued and not taken yet.
    pub fn queued(&self) -> usize {
        self.st.lock().queue.len()
    }

    /// Frames the mixer took from the stream so far.
    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// It waits to have enough queued before playing.
    pub fn filling(&self) -> bool {
        self.st.lock().filling
    }

    /// Drops what is queued; the stream fills again before it plays.
    /// Returns how many frames it dropped.
    pub fn flush(&self) -> usize {
        let mut st = self.st.lock();
        let n = st.queue.len();
        st.queue.clear();
        st.frac = 0;
        if st.prebuf > 0 {
            st.filling = true;
        }
        st.triggered = false;
        n
    }

    /// Drops the last `n` frames queued, as many as are still there (a
    /// program that writes over what it wrote). Returns how many.
    pub fn rewind(&self, n: usize) -> usize {
        let mut st = self.st.lock();
        let n = n.min(st.queue.len());
        let keep = st.queue.len() - n;
        st.queue.truncate(keep);
        n
    }

    /// How many frames it waits for before playing.
    pub fn set_prebuf(&self, frames: usize) {
        let mut st = self.st.lock();
        st.prebuf = frames;
        st.check_filled();
    }

    /// Plays what is queued, and what comes next, however little it is.
    pub fn trigger(&self) {
        let mut st = self.st.lock();
        st.triggered = true;
        st.check_filled();
    }

    /// Waits to fill up again before playing on.
    pub fn prebuf_force(&self) {
        let mut st = self.st.lock();
        if st.prebuf > 0 {
            st.filling = true;
            st.triggered = false;
        }
    }

    pub fn cork(&self, corked: bool) {
        self.st.lock().corked = corked;
        if !corked {
            WORK.notify_all();
        }
    }

    /// Volume, in 1/65536 (on top of the system volume).
    pub fn set_gain(&self, gain: u32) {
        self.st.lock().gain = gain;
    }

    pub fn set_rate(&self, rate: u32) {
        self.st.lock().rate = rate.clamp(1000, 384_000);
    }

    /// How many times it ran out while playing, and how many times it
    /// started (counts that only grow: the caller tells what changed).
    pub fn events(&self) -> (u64, u64) {
        let st = self.st.lock();
        (st.underruns, st.starts)
    }

    /// Where the stream is: what the mixer took from it and when the last
    /// of that is heard, as of now.
    pub fn timing(&self) -> Timing {
        let (playing, playing_for, underrun_for) = {
            let st = self.st.lock();
            (!st.corked && !st.filling && st.playing_for > 0, st.playing_for, st.underrun_for)
        };
        let (taken, end) = {
            let _t = TIMING.lock();
            (self.taken.load(Ordering::Relaxed), END_NS.load(Ordering::Relaxed))
        };
        let latency_us = end.saturating_sub(crate::time::uptime_ns()) / 1000;
        Timing { taken, latency_us, playing, playing_for, underrun_for }
    }

    /// The stream ends: the mixer lets it go (with what it had queued).
    pub fn close(self: &Arc<Self>) {
        STREAMS.lock().retain(|s| !Arc::ptr_eq(s, self));
    }
}

impl StreamState {
    /// Ends filling once there is enough to play.
    fn check_filled(&mut self) {
        if self.filling && !self.queue.is_empty() && (self.triggered || self.queue.len() >= self.prebuf) {
            self.filling = false;
            self.triggered = false;
            WORK.notify_all();
        }
    }

    /// Keeps the output going: it plays, or ran out a moment ago (more may
    /// come: a program that is late, or between two sounds).
    fn active(&self, now: u64) -> bool {
        !self.corked && (!self.filling || (self.ran_out != 0 && now.saturating_sub(self.ran_out) < LINGER_MS))
    }

    /// Adds the stream to `mix` (stereo frames at `rate`). Returns how many
    /// of its frames it took, and whether it started or ran out.
    fn mix_into(&mut self, mix: &mut [[i32; 2]], rate: u32, now: u64) -> (u64, bool) {
        let n = mix.len();
        if self.corked {
            return (0, false);
        }
        if self.filling {
            if self.playing_for == 0 {
                self.underrun_for += n as u64 * self.rate as u64 / rate as u64;
            }
            return (0, false);
        }
        let step = ((self.rate as u64) << 32) / rate as u64;
        let gain = self.gain as i64;
        let len = self.queue.len();
        let mut made = 0;
        for out in mix.iter_mut() {
            let j = (self.frac >> 32) as usize;
            if j >= len {
                break;
            }
            let a = self.queue[j];
            let b = if j + 1 < len { self.queue[j + 1] } else { a };
            let frac = (self.frac >> 16) & 0xFFFF;
            out[0] += (lerp(a[0], b[0], frac) * gain / 65536) as i32;
            out[1] += (lerp(a[1], b[1], frac) * gain / 65536) as i32;
            self.frac += step;
            made += 1;
        }
        let used = ((self.frac >> 32) as usize).min(len);
        self.queue.drain(..used);
        self.frac -= (used as u64) << 32;
        if self.queue.is_empty() {
            self.frac &= 0xFFFF_FFFF;
        }
        let mut changed = false;
        if made > 0 && self.playing_for == 0 {
            self.starts += 1;
            self.underrun_for = 0;
            changed = true;
        }
        let played = self.playing_for > 0 || made > 0;
        self.playing_for += used as u64;
        if made < n {
            // Ran out: silence for the rest, and it fills up again.
            if played {
                self.underruns += 1;
            }
            self.playing_for = 0;
            self.underrun_for = (n - made) as u64 * self.rate as u64 / rate as u64;
            self.filling = true;
            self.triggered = false;
            self.ran_out = now.max(1);
            changed = true;
        }
        (used as u64, changed)
    }
}

/// Adds the streams that play to `mix`; returns the streams it took from
/// (or that started or ran out), and how many frames.
fn mix_streams(mix: &mut [[i32; 2]], rate: u32, now: u64) -> Vec<(Arc<Stream>, u64)> {
    let streams = STREAMS.lock().clone();
    let mut took = Vec::new();
    for s in streams {
        let (n, changed) = s.st.lock().mix_into(mix, rate, now);
        if n > 0 || changed {
            took.push((s, n));
        }
    }
    took
}

// ----------------------------------------------------------------------------
// The mixer

/// Told when there is something new to play.
static WORK: WaitQueue = WaitQueue::new();
static MIXER: AtomicBool = AtomicBool::new(false);
/// When the output will have played everything written to it so far
/// (uptime, ns; 0 while it is stopped).
static END_NS: AtomicU64 = AtomicU64::new(0);
/// Held while the mixer moves [`END_NS`] and the streams' read indexes,
/// so that a stream's timing never mixes one step's values with another's.
static TIMING: IrqLock<()> = IrqLock::new(());

fn start_mixer() {
    if !MIXER.swap(true, Ordering::AcqRel) && crate::task::spawn("mixer", mixer).is_none() {
        MIXER.store(false, Ordering::Release);
        crate::warn!("audio", "no thread for the mixer");
    }
}

/// The rate the mix plays at (the first output's).
pub fn rate() -> u32 {
    first_sink().map_or(NO_OUTPUT_RATE, |s| s.rate)
}

/// How long a frame mixed now waits before it is heard (µs).
pub fn latency_us() -> u64 {
    END_NS.load(Ordering::Relaxed).saturating_sub(crate::time::uptime_ns()) / 1000
}

/// Something to play: the system voice or a stream that plays.
fn busy(now: u64) -> bool {
    VOICE.lock().is_some() || STREAMS.lock().iter().any(|s| s.st.lock().active(now))
}

/// Where the mix goes: an output, by its number.
#[derive(Clone, Copy)]
struct Sink {
    index: usize,
    desc: *const AudioDesc,
    rate: u32,
    channels: u32,
    queued: Option<AudioQueued>,
}

fn first_sink() -> Option<Sink> {
    OUTPUTS.lock().iter().enumerate().find_map(|(index, o)| {
        o.as_ref().map(|o| Sink { index, desc: o.desc, rate: o.rate, channels: o.channels, queued: o.queued })
    })
}

/// The output being played to (or none: streams are then consumed by the
/// clock), and how far.
struct Playing {
    sink: Option<Sink>,
    rate: u32,
    /// Frames written since it started, and when that was (uptime, ns).
    written: u64,
    since: u64,
}

impl Playing {
    fn new(sink: Option<Sink>) -> Playing {
        Playing { rate: sink.map_or(NO_OUTPUT_RATE, |s| s.rate), sink, written: 0, since: 0 }
    }

    /// Still the output `sink` (none, or the same one, which says how much
    /// it has queued if it did before).
    fn same(&self, sink: Option<Sink>) -> bool {
        match (self.sink, sink) {
            (None, None) => true,
            (Some(a), Some(b)) => a.index == b.index && a.desc == b.desc && a.queued.is_some() == b.queued.is_some(),
            _ => false,
        }
    }

    fn channels(&self) -> u32 {
        self.sink.map_or(2, |s| s.channels)
    }

    /// Frames written and not played yet: what the driver says, or else
    /// what the clock says.
    fn queued(&mut self) -> u64 {
        if let Some(Sink { desc, queued: Some(queued), .. }) = self.sink {
            // SAFETY: the output exists while its driver does; only the
            // mixer calls it.
            return unsafe { queued((*desc).ctx) } as u64;
        }
        if self.written == 0 {
            return 0;
        }
        let now = crate::time::uptime_ns();
        let played = ((now - self.since) as u128 * self.rate as u128 / 1_000_000_000) as u64;
        if played > self.written {
            // The mixer was late: count from now, as if it had not been.
            self.since = now - (self.written as u128 * 1_000_000_000 / self.rate as u128) as u64;
            return 0;
        }
        self.written - played
    }

    fn write(&mut self, samples: &[i16], frames: usize) {
        if self.written == 0 {
            self.since = crate::time::uptime_ns();
        }
        if let Some(s) = self.sink {
            // The sound pauses while the machine sleeps.
            super::pm::wait_awake();
            // SAFETY: as in `queued`; the driver copies the samples before
            // returning.
            unsafe { ((*s.desc).write)((*s.desc).ctx, samples.as_ptr(), frames) };
        }
        self.written += frames as u64;
    }

    /// Nothing more to play: the output plays what it has and stops.
    fn finish(self) {
        if let Some(s) = self.sink {
            super::pm::wait_awake();
            // SAFETY: as in `queued`.
            unsafe { ((*s.desc).drain)((*s.desc).ctx) };
        }
    }
}

/// The mix as the output's samples, with the system volume applied.
fn render(mix: &[[i32; 2]], channels: u32) -> Vec<i16> {
    // Volume on a curve that sounds even: (v/100)^2, in 1/65536 units.
    let v = volume() as i64;
    let gain = v * v * 65536 / 10_000;
    let level = |x: i64| (x * gain / 65536).clamp(-32768, 32767) as i16;
    let ch = channels as usize;
    let mut out = Vec::with_capacity(mix.len() * ch);
    for f in mix {
        if ch == 1 {
            out.push(level((f[0] as i64 + f[1] as i64) / 2));
        } else {
            out.push(level(f[0] as i64));
            out.push(level(f[1] as i64));
            out.extend(core::iter::repeat_n(0, ch - 2));
        }
    }
    out
}

/// The mixer thread: while something plays, it keeps [`AHEAD_MS`] queued
/// in the output, [`STEP_MS`] at a time.
fn mixer() {
    let mut out: Option<Playing> = None;
    loop {
        let now = crate::time::uptime_ms();
        let sink = first_sink();
        if out.as_ref().is_some_and(|p| !p.same(sink)) {
            // The output went away, or came: start again on what there is.
            out = None;
            END_NS.store(0, Ordering::Relaxed);
        }
        let seen = WORK.notified();
        if !busy(now) {
            if let Some(p) = out.take() {
                p.finish();
            }
            END_NS.store(0, Ordering::Relaxed);
            crate::task::wait_any(&[&WORK], None, || (WORK.notified() != seen).then_some(()));
            continue;
        }
        let p = out.get_or_insert_with(|| Playing::new(sink));
        let ahead = p.queued();
        let target = p.rate as u64 * AHEAD_MS / 1000;
        if ahead >= target {
            crate::task::sleep_ms(((ahead - target) * 1000 / p.rate as u64).clamp(1, STEP_MS));
            continue;
        }
        let frames = (p.rate as u64 * STEP_MS / 1000) as usize;
        let mut mix = alloc::vec![[0i32; 2]; frames];
        mix_voice(&mut mix, p.rate);
        let took = mix_streams(&mut mix, p.rate, now);
        p.write(&render(&mix, p.channels()), frames);
        // What was just written is heard once everything queued before it is.
        let end = crate::time::uptime_ns() + p.queued() * 1_000_000_000 / p.rate as u64;
        {
            let _t = TIMING.lock();
            END_NS.store(end, Ordering::Relaxed);
            for (s, n) in &took {
                s.taken.fetch_add(*n, Ordering::Relaxed);
            }
        }
        for (s, _) in &took {
            s.wq.notify_all();
        }
    }
}
