//! Sound inputs registered by drivers (microphones, line ins), and the
//! recordings programs make.
//!
//! A recording ([`Recording`]) takes its sound from one of two sources:
//!
//! * the microphone: the first input. One thread, `capture`, starts it while
//!   something records from it, reads it and hands each recording what came,
//!   with the input volume applied. Without an input, recordings get silence
//!   by the clock, at 48 kHz: a program that records goes on as with a silent
//!   microphone, and does not hang;
//! * the monitor: what the mixer plays ([`monitor`]), before the system
//!   volume. The mixer keeps running while someone listens to it.
//!
//! Each recording converts what it gets to its own rate as it comes (linear
//! interpolation, as the mixer does on the way out) and keeps it until the
//! program takes it, a few seconds at most.

use crate::sync::{IrqLock, Mutex};
use crate::task::WaitQueue;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use justdk::{CaptureDesc, Status, E_INVAL};

/// How much the capture thread reads at a time.
pub const STEP_MS: u64 = 10;
/// The rate of the silence recordings get without an input.
const NO_INPUT_RATE: u32 = 48_000;
/// A recording nobody takes from keeps this much, the newest.
const MAX_QUEUED_S: usize = 4;

/// Where a recording takes its sound from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Mic,
    Monitor,
}

struct Input {
    device: u32,
    name: String,
    rate: u32,
    channels: u32,
    desc: *const CaptureDesc,
}

// SAFETY: the descriptor belongs to the driver; only the capture thread
// calls it.
unsafe impl Send for Input {}

static INPUTS: IrqLock<Vec<Option<Input>>> = IrqLock::new(Vec::new());
/// The input volume, in 1/65536, and whether it is muted.
static GAIN: AtomicU32 = AtomicU32::new(65536);
static MUTED: AtomicBool = AtomicBool::new(false);

pub fn register(device: u32, desc: *const CaptureDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    if d.rate < 8000 || d.rate > 192_000 || d.channels == 0 || d.channels > 8 {
        return E_INVAL;
    }
    let len = d.name.iter().position(|&b| b == 0).unwrap_or(d.name.len());
    let name = String::from(core::str::from_utf8(&d.name[..len]).unwrap_or("sound input"));
    let mut ins = INPUTS.lock();
    let n = ins.len();
    crate::info!("audio", "mic{n}: {name}, {} Hz, {} channels", d.rate, d.channels);
    ins.push(Some(Input { device, name, rate: d.rate, channels: d.channels, desc }));
    WORK.notify_all();
    n as Status
}

pub fn remove_device(device: u32) {
    for slot in INPUTS.lock().iter_mut() {
        if slot.as_ref().is_some_and(|i| i.device == device) {
            *slot = None;
        }
    }
}

pub struct InputInfo {
    pub index: usize,
    pub name: String,
    pub rate: u32,
    pub channels: u32,
}

pub fn list() -> Vec<InputInfo> {
    INPUTS
        .lock()
        .iter()
        .enumerate()
        .filter_map(|(index, i)| {
            let i = i.as_ref()?;
            Some(InputInfo { index, name: i.name.clone(), rate: i.rate, channels: i.channels })
        })
        .collect()
}

/// The rate the microphone records at (the first input's).
pub fn rate() -> u32 {
    first_input().map_or(NO_INPUT_RATE, |i| i.rate)
}

/// The input volume, in 1/65536 (on top of each recording's own).
pub fn set_gain(gain: u32) {
    GAIN.store(gain, Ordering::Relaxed);
}

pub fn set_muted(muted: bool) {
    MUTED.store(muted, Ordering::Relaxed);
}

pub fn muted() -> bool {
    MUTED.load(Ordering::Relaxed)
}

/// `a` to `b` by `frac` (in 1/65536).
fn lerp(a: i16, b: i16, frac: u64) -> i64 {
    let (a, b) = (a as i64, b as i64);
    a + (b - a) * frac as i64 / 65536
}

fn scale(v: i64, gain: u32) -> i16 {
    (v * gain as i64 / 65536).clamp(-32768, 32767) as i16
}

// ----------------------------------------------------------------------------
// Recordings

/// A recording a program makes: the capture thread or the mixer adds
/// stereo frames at the recording's rate, the program takes them. The
/// PulseAudio server of the Linux layer keeps one for each record stream
/// of its clients.
pub struct Recording {
    st: Mutex<RecState>,
    /// Frames added so far: the recording's write index.
    captured: AtomicU64,
    /// Told when frames were added.
    pub wq: WaitQueue,
}

struct RecState {
    source: Source,
    rate: u32,
    corked: bool,
    /// Volume, in 1/65536.
    gain: u32,
    /// Frames waiting to be taken.
    queue: VecDeque<[i16; 2]>,
    /// The conversion: frames of the source not used up yet (the last one
    /// used, and those after it), and where the next frame of the recording
    /// falls among them, in 1/2^32.
    from: Vec<[i16; 2]>,
    pos: u64,
    /// Frames dropped because nobody took them.
    dropped: u64,
}

static RECORDINGS: Mutex<Vec<Arc<Recording>>> = Mutex::new(Vec::new());
/// Told when a recording starts or the inputs change.
static WORK: WaitQueue = WaitQueue::new();
static CAPTURE: AtomicBool = AtomicBool::new(false);

/// A new recording from `source`, of stereo frames at `rate`; a `corked`
/// one takes nothing until uncorked.
pub fn open_recording(source: Source, rate: u32, corked: bool) -> Arc<Recording> {
    let st = RecState {
        source,
        rate: rate.clamp(1000, 384_000),
        corked,
        gain: 65536,
        queue: VecDeque::new(),
        from: Vec::new(),
        pos: 0,
        dropped: 0,
    };
    let r = Arc::new(Recording { st: Mutex::new(st), captured: AtomicU64::new(0), wq: WaitQueue::new() });
    RECORDINGS.lock().push(r.clone());
    started(source);
    r
}

/// Someone records from `source` now: its feeder runs.
fn started(source: Source) {
    match source {
        Source::Mic => {
            start_capture();
            WORK.notify_all();
        }
        Source::Monitor => super::audio::wake_mixer(),
    }
}

impl Recording {
    /// Takes up to `max` frames, the oldest first.
    pub fn take(&self, max: usize) -> Vec<[i16; 2]> {
        let mut st = self.st.lock();
        let n = st.queue.len().min(max);
        st.queue.drain(..n).collect()
    }

    /// Frames waiting to be taken.
    pub fn queued(&self) -> usize {
        self.st.lock().queue.len()
    }

    /// Frames added so far.
    pub fn captured(&self) -> u64 {
        self.captured.load(Ordering::Relaxed)
    }

    /// Frames dropped so far because nobody took them.
    pub fn dropped(&self) -> u64 {
        self.st.lock().dropped
    }

    pub fn cork(&self, corked: bool) {
        let source = {
            let mut st = self.st.lock();
            st.corked = corked;
            st.source
        };
        if !corked {
            started(source);
        }
    }

    /// Drops what is waiting; returns how many frames.
    pub fn flush(&self) -> usize {
        let mut st = self.st.lock();
        let n = st.queue.len();
        st.queue.clear();
        n
    }

    /// Volume, in 1/65536 (on top of the input volume).
    pub fn set_gain(&self, gain: u32) {
        self.st.lock().gain = gain;
    }

    pub fn set_rate(&self, rate: u32) {
        self.st.lock().rate = rate.clamp(1000, 384_000);
    }

    pub fn source(&self) -> Source {
        self.st.lock().source
    }

    /// Records from `source` from now on.
    pub fn set_source(&self, source: Source) {
        let corked = {
            let mut st = self.st.lock();
            st.source = source;
            st.from.clear();
            st.pos = 0;
            st.corked
        };
        if !corked {
            started(source);
        }
    }

    /// The recording ends: nothing more is added.
    pub fn close(self: &Arc<Self>) {
        RECORDINGS.lock().retain(|r| !Arc::ptr_eq(r, self));
    }
}

impl RecState {
    /// Adds `frames` (stereo, at `rate`) at the recording's rate. Returns
    /// how many frames it added.
    fn feed(&mut self, frames: &[[i16; 2]], rate: u32) -> u64 {
        let step = ((rate as u64) << 32) / self.rate as u64;
        self.from.extend_from_slice(frames);
        let mut made = 0;
        while ((self.pos >> 32) as usize) + 1 < self.from.len() {
            let j = (self.pos >> 32) as usize;
            let frac = (self.pos >> 16) & 0xFFFF;
            let (a, b) = (self.from[j], self.from[j + 1]);
            self.queue.push_back([scale(lerp(a[0], b[0], frac), self.gain), scale(lerp(a[1], b[1], frac), self.gain)]);
            self.pos += step;
            made += 1;
        }
        // The frame the next one starts from stays, with what follows it.
        let used = ((self.pos >> 32) as usize).min(self.from.len().saturating_sub(1));
        self.from.drain(..used);
        self.pos -= (used as u64) << 32;
        let max = self.rate as usize * MAX_QUEUED_S;
        if self.queue.len() > max {
            let extra = self.queue.len() - max;
            self.queue.drain(..extra);
            self.dropped += extra as u64;
        }
        made
    }
}

/// Hands `frames` (stereo, at `rate`) to the recordings from `source`.
fn deliver(source: Source, frames: &[[i16; 2]], rate: u32) {
    let recordings = RECORDINGS.lock().clone();
    for r in recordings {
        let made = {
            let mut st = r.st.lock();
            if st.source != source || st.corked {
                continue;
            }
            st.feed(frames, rate)
        };
        if made > 0 {
            r.captured.fetch_add(made, Ordering::Relaxed);
            r.wq.notify_all();
        }
    }
}

/// Something records from `source` (and is not corked).
fn recording(source: Source) -> bool {
    RECORDINGS.lock().iter().any(|r| {
        let st = r.st.lock();
        st.source == source && !st.corked
    })
}

/// Someone listens to what the mixer plays: it keeps running.
pub fn monitoring() -> bool {
    recording(Source::Monitor)
}

/// What the mixer just mixed (stereo frames at `rate`), for the monitor.
pub fn monitor(mix: &[[i32; 2]], rate: u32) {
    if !monitoring() {
        return;
    }
    let frames: Vec<[i16; 2]> =
        mix.iter().map(|f| [f[0].clamp(-32768, 32767) as i16, f[1].clamp(-32768, 32767) as i16]).collect();
    deliver(Source::Monitor, &frames, rate);
}

// ----------------------------------------------------------------------------
// The capture thread

/// Where the microphone's sound comes from: an input, by its number.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Mic {
    index: usize,
    desc: *const CaptureDesc,
    rate: u32,
    channels: u32,
}

fn first_input() -> Option<Mic> {
    INPUTS.lock().iter().enumerate().find_map(|(index, i)| {
        i.as_ref().map(|i| Mic { index, desc: i.desc, rate: i.rate, channels: i.channels })
    })
}

/// Input `mic` is still there (its driver may be called).
fn present(mic: Mic) -> bool {
    INPUTS.lock().get(mic.index).is_some_and(|i| i.as_ref().is_some_and(|i| i.desc == mic.desc))
}

fn start_capture() {
    if !CAPTURE.swap(true, Ordering::AcqRel) && crate::task::spawn("capture", capture).is_none() {
        CAPTURE.store(false, Ordering::Release);
        crate::warn!("audio", "no thread for recording");
    }
}

/// Starts (`on`) or stops input `mic`.
fn switch(mic: Mic, on: bool) {
    super::pm::wait_awake();
    // SAFETY: the input exists while its driver does; only this thread
    // calls it.
    unsafe { ((*mic.desc).capture)((*mic.desc).ctx, on as u32) };
}

/// Reads what input `mic` captured (up to a step of it) as stereo frames,
/// with the input volume applied.
fn read_input(mic: Mic, buf: &mut Vec<i16>) -> Vec<[i16; 2]> {
    let frames = (mic.rate as u64 * STEP_MS / 1000) as usize;
    let ch = mic.channels as usize;
    buf.resize(frames * ch, 0);
    super::pm::wait_awake();
    // SAFETY: as in `switch`; the buffer holds `frames` frames.
    let n = unsafe { ((*mic.desc).read)((*mic.desc).ctx, buf.as_mut_ptr(), frames) };
    let n = (n.max(0) as usize).min(frames);
    let gain = if muted() { 0 } else { GAIN.load(Ordering::Relaxed) };
    buf[..n * ch]
        .chunks_exact(ch)
        .map(|f| {
            // Mono goes to both sides; channels past the second are left out.
            let (l, r) = (f[0], f[1.min(ch - 1)]);
            [scale(l as i64, gain), scale(r as i64, gain)]
        })
        .collect()
}

/// Silence for the recordings without an input: what the clock says is
/// due since `since` (uptime, ns), of which `given` frames were handed out.
fn silence(since: u64, given: &mut u64) -> Vec<[i16; 2]> {
    crate::task::sleep_ms(STEP_MS);
    let due = ((crate::time::uptime_ns() - since) as u128 * NO_INPUT_RATE as u128 / 1_000_000_000) as u64;
    let n = due.saturating_sub(*given);
    *given += n;
    alloc::vec![[0i16; 2]; n as usize]
}

/// The capture thread: while something records from the microphone, it
/// reads the input (or the clock) a step at a time and hands it on.
fn capture() {
    let mut on: Option<Mic> = None;
    let mut clock: Option<(u64, u64)> = None;
    let mut buf = Vec::new();
    loop {
        let seen = WORK.notified();
        let mic = first_input();
        if on.is_some() && (on != mic || !recording(Source::Mic)) {
            // Stopped only if its driver is still there to be told.
            if let Some(old) = on.take().filter(|&m| present(m)) {
                switch(old, false);
            }
        }
        if !recording(Source::Mic) {
            clock = None;
            crate::task::wait_any(&[&WORK], None, || (WORK.notified() != seen).then_some(()));
            continue;
        }
        match mic {
            Some(m) => {
                if on.is_none() {
                    switch(m, true);
                    on = Some(m);
                }
                clock = None;
                let frames = read_input(m, &mut buf);
                if frames.is_empty() {
                    // The input gives nothing (it is waking up, or broken):
                    // no hurry to ask again.
                    crate::task::sleep_ms(STEP_MS);
                }
                deliver(Source::Mic, &frames, m.rate);
            }
            None => {
                let (since, given) = clock.get_or_insert_with(|| (crate::time::uptime_ns(), 0));
                let frames = silence(*since, given);
                deliver(Source::Mic, &frames, NO_INPUT_RATE);
            }
        }
    }
}
