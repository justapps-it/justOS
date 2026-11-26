//! Line symbols (Lucide's, see `justshell::vector`) drawn at the screen's
//! pixels: the dynamic bar's and the windows' symbols. Each is rasterised
//! once per size and stroke and kept.

use super::gfx::Canvas;
use crate::sync::IrqLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use justshell::vector;

type Key = (&'static str, i32, i32);

static CACHE: IrqLock<BTreeMap<Key, Option<Vec<u8>>>> = IrqLock::new(BTreeMap::new());

/// Draws symbol `name`, `size` units square, its top left corner at
/// (`x`, `y`) units, in `color`, with the reference's stroke (2 of 24).
pub fn draw(c: &mut Canvas, name: &'static str, size: i32, x: i32, y: i32, color: u32) {
    draw_stroke(c, name, size, x, y, 200, color);
}

/// [`draw`] centred on (`cx`, `cy`).
pub fn centered(c: &mut Canvas, name: &'static str, size: i32, cx: i32, cy: i32, color: u32) {
    draw(c, name, size, cx - size / 2, cy - size / 2, color);
}

/// [`draw`] with a stroke of `stroke` hundredths of a grid unit.
pub fn draw_stroke(c: &mut Canvas, name: &'static str, size: i32, x: i32, y: i32, stroke: i32, color: u32) {
    let k = c.scale();
    let px = size * k;
    if px <= 0 {
        return;
    }
    let key = (name, px, stroke);
    let have = CACHE.lock().contains_key(&key);
    if !have {
        let mask = justshell::vector::icon(name, px, stroke as f32 / 100.0);
        CACHE.lock().insert(key, mask);
    }
    let cache = CACHE.lock();
    if let Some(Some(mask)) = cache.get(&key) {
        c.draw_mask_px(mask, px, px, x * k, y * k, color);
    }
}

/// Rings drawn lately: (width, height, thousandths, stroke) in pixels.
static RINGS: IrqLock<BTreeMap<(i32, i32, i32, i32), Vec<u8>>> = IrqLock::new(BTreeMap::new());

/// The outline of a pill `w` x `h` units at (`x`, `y`), clockwise from the
/// middle of its top edge, `permille` of the way round, `stroke` units wide
/// (inside the pill): the battery ring round the bar's status.
#[allow(clippy::too_many_arguments)]
pub fn pill_ring(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, permille: i32, stroke: i32, color: u32) {
    let k = c.scale();
    let (pw, ph, sw) = (w * k, h * k, stroke * k);
    if pw <= 0 || ph <= 0 || permille <= 0 {
        return;
    }
    let key = (pw, ph, permille.min(1000), sw);
    if !RINGS.lock().contains_key(&key) {
        let pts = vector::pill_outline(pw as f32, ph as f32, ph as f32 / 2.0, sw as f32 / 2.0);
        let part = vector::partial(&pts, key.2 as f32 / 1000.0);
        let mut mask = alloc::vec![0u8; (pw * ph) as usize];
        vector::paint(&mut mask, pw, ph, &[vector::Sub { pts: part, closed: false }], vector::Paint::Stroke, sw as f32);
        let mut rings = RINGS.lock();
        if rings.len() >= 16 {
            rings.clear();
        }
        rings.insert(key, mask);
    }
    let rings = RINGS.lock();
    if let Some(mask) = rings.get(&key) {
        c.draw_mask_px(mask, pw, ph, x * k, y * k, color);
    }
}

/// A spinner: a quarter of a ring `d` units across at (`x`, `y`), turned
/// `turn` thousandths of a turn, `stroke` units wide.
pub fn spinner(c: &mut Canvas, x: i32, y: i32, d: i32, turn: u32, stroke: i32, color: u32) {
    let k = c.scale();
    let (pd, sw) = (d * k, stroke * k);
    let mut pts = vector::pill_outline(pd as f32, pd as f32, pd as f32 / 2.0, sw as f32 / 2.0);
    // The loop starts and ends at the top: drop the repeated end and
    // start it further round.
    pts.pop();
    let n = pts.len();
    if n == 0 {
        return;
    }
    let shift = (turn as usize % 1000) * n / 1000;
    pts.rotate_left(shift);
    let first = pts[0];
    pts.push(first);
    let part = vector::partial(&pts, 0.25);
    let mut mask = alloc::vec![0u8; (pd * pd) as usize];
    vector::paint(&mut mask, pd, pd, &[vector::Sub { pts: part, closed: false }], vector::Paint::Stroke, sw as f32);
    c.draw_mask_px(&mask, pd, pd, x * k, y * k, color);
}
