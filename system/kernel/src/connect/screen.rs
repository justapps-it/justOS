//! The screen for phones that watch it. The interface's loop offers every
//! picture it drew ([`offer`]); while somebody watches, it is scaled down
//! and kept here, and the sessions encode it for the phone at their own
//! pace (a new frame only once the phone has shown the last one).

use crate::sync::IrqLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use justgfx::Canvas;

/// A picture of the screen, scaled down.
pub struct Frame {
    pub seq: u64,
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    /// The size of the screen in units (what taps on the phone map to).
    pub sw: i32,
    pub sh: i32,
    /// The pointer, in the frame's pixels.
    pub cx: i32,
    pub cy: i32,
}

/// Sessions that watch the screen.
static VIEWERS: AtomicU32 = AtomicU32::new(0);
/// The widest picture a viewer asked for, in pixels.
static MAX_W: AtomicU32 = AtomicU32::new(1280);
/// When the last picture was taken (uptime, ms).
static LAST: AtomicU64 = AtomicU64::new(0);
/// Something changed while it was too early for a picture.
static STALE: AtomicBool = AtomicBool::new(false);
/// Take a picture at the next look, changed or not.
static FORCE: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static FRAME: IrqLock<Option<Arc<Frame>>> = IrqLock::new(None);

/// The pointer and the screen, in units, as the interface last had them.
static CUR_X: AtomicI32 = AtomicI32::new(0);
static CUR_Y: AtomicI32 = AtomicI32::new(0);
static SCREEN_W: AtomicI32 = AtomicI32::new(0);
static SCREEN_H: AtomicI32 = AtomicI32::new(0);

/// At most this often (ms): about 15 pictures a second.
const PERIOD: u64 = 66;

/// From the interface's loop after each look: the scene, where the pointer
/// is (units) and whether anything changed on the screen.
pub fn offer(scene: &Canvas, cx: i32, cy: i32, changed: bool) {
    CUR_X.store(cx, Ordering::Relaxed);
    CUR_Y.store(cy, Ordering::Relaxed);
    SCREEN_W.store(scene.w, Ordering::Relaxed);
    SCREEN_H.store(scene.h, Ordering::Relaxed);
    if VIEWERS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let force = FORCE.swap(false, Ordering::AcqRel);
    if !changed && !force {
        return;
    }
    let now = crate::time::uptime_ms();
    if !force && now.saturating_sub(LAST.load(Ordering::Relaxed)) < PERIOD {
        STALE.store(true, Ordering::Release);
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    STALE.store(false, Ordering::Release);

    let (pw, ph) = (scene.pixel_width().max(1) as usize, scene.pixel_height().max(1) as usize);
    if scene.px.len() < pw * ph {
        return;
    }
    let max_w = MAX_W.load(Ordering::Relaxed).clamp(160, 4096) as usize;
    let f = pw.div_ceil(max_w).max(1);
    let (w, h) = (pw / f, ph / f);
    let mut px = Vec::with_capacity(w * h);
    if f == 1 {
        px.extend_from_slice(&scene.px[..w * h]);
    } else {
        let n = (f * f) as u32;
        for y in 0..h {
            for x in 0..w {
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for dy in 0..f {
                    let row = &scene.px[(y * f + dy) * pw + x * f..][..f];
                    for &p in row {
                        r += (p >> 16) & 0xFF;
                        g += (p >> 8) & 0xFF;
                        b += p & 0xFF;
                    }
                }
                px.push(((r / n) << 16) | ((g / n) << 8) | (b / n));
            }
        }
    }
    let k = scene.scale().max(1) as usize;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let frame = Frame {
        seq,
        w,
        h,
        px,
        sw: scene.w,
        sh: scene.h,
        cx: (cx.max(0) as usize * k / f) as i32,
        cy: (cy.max(0) as usize * k / f) as i32,
    };
    *FRAME.lock() = Some(Arc::new(frame));
}

/// A session starts watching, with pictures at most `max_w` pixels wide.
pub fn watch(max_w: u32) {
    MAX_W.store(max_w, Ordering::Relaxed);
    VIEWERS.fetch_add(1, Ordering::AcqRel);
    refresh();
}

pub fn unwatch() {
    let _ = VIEWERS.try_update(Ordering::AcqRel, Ordering::Relaxed, |v| v.checked_sub(1));
    if VIEWERS.load(Ordering::Relaxed) == 0 {
        *FRAME.lock() = None;
    }
}

/// Asks the interface for a picture now.
pub fn refresh() {
    FORCE.store(true, Ordering::Release);
    crate::dev::input::wake_gui();
}

/// The newest picture, if it is newer than `seen`. A change held back for
/// the pace is asked for here once its time has come.
pub fn newer(seen: u64) -> Option<Arc<Frame>> {
    let f = FRAME.lock().clone();
    if let Some(f) = f.filter(|f| f.seq > seen) {
        return Some(f);
    }
    if STALE.load(Ordering::Acquire) && crate::time::uptime_ms().saturating_sub(LAST.load(Ordering::Relaxed)) >= PERIOD {
        STALE.store(false, Ordering::Release);
        refresh();
    }
    None
}

/// The pointer and the screen size in units: (x, y, w, h).
pub fn geometry() -> (i32, i32, i32, i32) {
    (CUR_X.load(Ordering::Relaxed), CUR_Y.load(Ordering::Relaxed), SCREEN_W.load(Ordering::Relaxed), SCREEN_H.load(Ordering::Relaxed))
}

/// The pointer was moved by a session: where it will be.
pub fn moved_to(x: i32, y: i32) {
    CUR_X.store(x, Ordering::Relaxed);
    CUR_Y.store(y, Ordering::Relaxed);
}

/// The frame as a JPEG, with the pointer drawn on it.
pub fn encode(f: &Frame, quality: u32) -> Vec<u8> {
    let mut px = f.px.clone();
    // A small arrow-like mark: white with a dark edge.
    for dy in 0..14i32 {
        let wide = dy / 2 + 1;
        for dx in 0..=wide {
            let (x, y) = (f.cx + dx, f.cy + dy);
            if x < 0 || y < 0 || x as usize >= f.w || y as usize >= f.h {
                continue;
            }
            let edge = dx == 0 || dx == wide || dy == 13;
            px[y as usize * f.w + x as usize] = if edge { 0x101010 } else { 0xFFFFFF };
        }
    }
    justimage::encode_jpeg(&px, f.w, f.h, f.w, quality)
}
