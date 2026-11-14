//! Frame pacing: how often the screen may be drawn (the "Fluidità"
//! setting), and how the last frames went (the shell's `fps`).
//!
//! The interface's loop ([`super::ui::Looper`]) draws only what changed,
//! and never sooner than a frame period after the frame before: while
//! something moves (an animation, the pointer) it draws at the cap, and
//! between moves it rests until input wakes it. The system tick stays at
//! 1 kHz: a period is waited for on whole milliseconds, with the deadlines
//! kept in nanoseconds so that frames come at the cap on average (at
//! 240 Hz, every 4 or 5 ms).

use crate::sync::IrqLock;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// The choices of "Fluidità", in Hz; 0 is automatic.
pub const CHOICES: [u32; 5] = [0, 60, 120, 144, 240];

/// The rate when nothing better is known.
const DEFAULT_HZ: u32 = 60;

static CHOICE: AtomicU32 = AtomicU32::new(0);

/// The display's refresh rate, when its driver reports one. None does yet:
/// the display ABI carries no refresh rate and no driver reads the EDID,
/// so "Automatica" is 60 Hz.
pub fn display_hz() -> Option<u32> {
    None
}

/// The rate chosen (0: automatic).
pub fn choice() -> u32 {
    CHOICE.load(Ordering::Relaxed)
}

/// Sets the rate chosen (anything but one of [`CHOICES`] is automatic).
pub fn set_choice(hz: u32) {
    CHOICE.store(if CHOICES.contains(&hz) { hz } else { 0 }, Ordering::Relaxed);
}

/// The most frames a second the screen is drawn now.
pub fn cap() -> u32 {
    match choice() {
        0 => display_hz().unwrap_or(DEFAULT_HZ).clamp(30, 240),
        hz => hz,
    }
}

/// The time between two frames at the cap.
pub fn period_ns() -> u64 {
    1_000_000_000 / cap() as u64
}

/// When the frame after one that started at `start` may start, given when
/// that one was due (`due`, 0 for none): a period after it was due while
/// the loop keeps time, so that frames come at the cap on average; a
/// period after it started after a pause or a slow frame.
pub fn next_due(due: u64, start: u64) -> u64 {
    let period = period_ns();
    if due != 0 && start < due + period {
        due + period
    } else {
        start + period
    }
}

/// Sleeps until `ns` of uptime (at once if it passed). The scheduler wakes
/// sleepers on whole milliseconds: this sleeps to the first tick at or
/// after `ns`.
pub fn sleep_until(ns: u64) {
    let now = crate::time::uptime_ns();
    if ns <= now {
        return;
    }
    let target = ns.div_ceil(1_000_000);
    let now_ms = crate::time::uptime_ms();
    if target > now_ms {
        crate::task::sleep_ms(target - now_ms);
    }
}

// ----------------------------------------------------------------------------
// What the last frames were

/// The frames remembered: more than a second's worth at 240 Hz.
const KEPT: usize = 512;

struct Frames {
    /// When each frame was on the screen (uptime, ns) and how long it took
    /// (ns), in a ring; `next` is where the next one goes.
    at: [u64; KEPT],
    took: [u32; KEPT],
    next: usize,
}

static FRAMES: IrqLock<Frames> = IrqLock::new(Frames { at: [0; KEPT], took: [0; KEPT], next: 0 });

/// The second the loop's wake-ups are being counted for, how many so far,
/// and how many in the second before.
static WAKE_SEC: AtomicU64 = AtomicU64::new(0);
static WAKES: AtomicU32 = AtomicU32::new(0);
static WAKES_BEFORE: AtomicU32 = AtomicU32::new(0);

/// A frame was drawn (or only the pointer moved) from `start` to `end`.
pub fn frame(start: u64, end: u64) {
    let mut f = FRAMES.lock();
    let i = f.next;
    f.at[i] = end;
    f.took[i] = end.saturating_sub(start).min(u32::MAX as u64) as u32;
    f.next = (i + 1) % KEPT;
}

/// The loop woke up (to draw or only to look).
pub fn woke() {
    let sec = crate::time::uptime_ms() / 1000;
    let was = WAKE_SEC.swap(sec, Ordering::Relaxed);
    if was != sec {
        let n = WAKES.swap(0, Ordering::Relaxed);
        WAKES_BEFORE.store(if was + 1 == sec { n } else { 0 }, Ordering::Relaxed);
    }
    WAKES.fetch_add(1, Ordering::Relaxed);
}

/// The last second of frames.
pub struct Stats {
    pub frames: u32,
    /// How long a frame took to draw and show, on average and at most (µs).
    pub avg_us: u32,
    pub max_us: u32,
    /// The longest time between two frames (µs), the time since the first
    /// of them counting.
    pub max_gap_us: u32,
    /// The loop's wake-ups in the last whole second.
    pub wakes: u32,
}

pub fn stats() -> Stats {
    let now = crate::time::uptime_ns();
    let from = now.saturating_sub(1_000_000_000);
    let f = FRAMES.lock();
    let mut times: alloc::vec::Vec<(u64, u32)> = (0..KEPT).filter(|&i| f.at[i] != 0 && f.at[i] >= from).map(|i| (f.at[i], f.took[i])).collect();
    drop(f);
    times.sort_unstable_by_key(|t| t.0);
    let n = times.len() as u64;
    let total: u64 = times.iter().map(|t| t.1 as u64).sum();
    let max = times.iter().map(|t| t.1).max().unwrap_or(0);
    let gap = times.windows(2).map(|w| w[1].0 - w[0].0).max().unwrap_or(0);
    // A second ago sooner than this frame: a wake-up count from then is stale.
    let sec = crate::time::uptime_ms() / 1000;
    let wakes = if WAKE_SEC.load(Ordering::Relaxed) == sec { WAKES_BEFORE.load(Ordering::Relaxed) } else { 0 };
    Stats {
        frames: n as u32,
        avg_us: if n == 0 { 0 } else { (total / n / 1000) as u32 },
        max_us: max / 1000,
        max_gap_us: (gap / 1000).min(u32::MAX as u64) as u32,
        wakes,
    }
}
