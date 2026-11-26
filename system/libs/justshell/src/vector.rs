//! Line icons drawn from vector paths (a subset of SVG path data on a 24 x 24
//! grid, stroked with round caps and joins), rendered into alpha masks of
//! any size; and the outline of a pill, for the battery ring that follows
//! the status pill's edge.
//!
//! The paths are Lucide icons (ISC licence, see the kernel's
//! assets/LICENSES.md), as the design's reference uses them.

use alloc::vec;
use alloc::vec::Vec;

/// How an element is painted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paint {
    Stroke,
    Fill,
    Both,
}

/// One element of an icon, on the 24 x 24 grid.
#[derive(Clone, Copy, Debug)]
pub enum El {
    Path(&'static str, Paint),
    Circle(f32, f32, f32, Paint),
    /// x, y, w, h, corner radius.
    Rect(f32, f32, f32, f32, f32, Paint),
}

use El::{Circle as C, Path as P, Rect as Rr};
use Paint::{Both as B, Fill as F, Stroke as S};

/// The icons by name.
pub const ICONS: &[(&str, &[El])] = &[
    (
        "wifi",
        &[P("M5 12.55a11 11 0 0 1 14.08 0", S), P("M1.42 9a16 16 0 0 1 21.16 0", S), P("M8.53 16.11a6 6 0 0 1 6.95 0", S), C(12.0, 20.0, 1.0, B)],
    ),
    ("wifi-low", &[P("M8.53 16.11a6 6 0 0 1 6.95 0", S), C(12.0, 20.0, 1.0, B)]),
    ("wifi-mid", &[P("M5 12.55a11 11 0 0 1 14.08 0", S), P("M8.53 16.11a6 6 0 0 1 6.95 0", S), C(12.0, 20.0, 1.0, B)]),
    ("wifi-off", &[P("M2 2l20 20", S), P("M8.53 16.11a6 6 0 0 1 6.95 0", S), P("M5 12.55a11 11 0 0 1 5.17-2.39", S), P("M1.42 9a16 16 0 0 1 4.7-2.88", S), C(12.0, 20.0, 1.0, B)]),
    ("bt", &[P("m7 7 10 10-5 5V2l5 5L7 17", S)]),
    ("moon", &[P("M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z", S)]),
    ("vol", &[P("M11 5 6 9H2v6h4l5 4V5z", S), P("M15.54 8.46a5 5 0 0 1 0 7.07", S), P("M19.07 4.93a10 10 0 0 1 0 14.14", S)]),
    ("vol-low", &[P("M11 5 6 9H2v6h4l5 4V5z", S), P("M15.54 8.46a5 5 0 0 1 0 7.07", S)]),
    ("vol-mute", &[P("M11 5 6 9H2v6h4l5 4V5z", S), P("M22 9l-6 6M16 9l6 6", S)]),
    ("sun", &[C(12.0, 12.0, 4.0, S), P("M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41", S)]),
    ("power", &[P("M12 2v10", S), P("M18.4 6.6a9 9 0 1 1-12.77.04", S)]),
    ("lock", &[Rr(3.0, 11.0, 18.0, 11.0, 2.0, S), P("M7 11V7a5 5 0 0 1 10 0v4", S)]),
    ("restart", &[P("M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", S), P("M3 3v5h5", S)]),
    (
        "gear",
        &[
            P("M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z", S),
            C(12.0, 12.0, 3.0, S),
        ],
    ),
    ("play", &[P("M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z", F)]),
    ("pause", &[Rr(6.0, 4.0, 4.0, 16.0, 1.2, F), Rr(14.0, 4.0, 4.0, 16.0, 1.2, F)]),
    ("prev", &[P("M19 20 9 12l10-8z", B), P("M5 19V5", S)]),
    ("next", &[P("m5 4 10 8-10 8z", B), P("M19 5v14", S)]),
    ("bolt", &[P("M13 2 3 14h9l-1 8 10-12h-9l1-8z", B)]),
    ("chev-r", &[P("m9 18 6-6-6-6", S)]),
    ("chev-l", &[P("m15 18-6-6 6-6", S)]),
    ("chev-d", &[P("m6 9 6 6 6-6", S)]),
    ("check", &[P("M20 6 9 17l-5-5", S)]),
    ("heart", &[P("M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z", S)]),
    ("heart-on", &[P("M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z", B)]),
    (
        "shuffle",
        &[
            P("m18 14 4 4-4 4", S),
            P("m18 2 4 4-4 4", S),
            P("M2 18h1.973a4 4 0 0 0 3.3-1.7l5.454-7.6a4 4 0 0 1 3.3-1.7H22", S),
            P("M2 6h1.972a4 4 0 0 1 3.6 2.2", S),
            P("M22 18h-6.041a4 4 0 0 1-3.3-1.8l-.359-.45", S),
        ],
    ),
    ("repeat", &[P("m17 2 4 4-4 4", S), P("M3 11v-1a4 4 0 0 1 4-4h14", S), P("m7 22-4-4 4-4", S), P("M21 13v1a4 4 0 0 1-4 4H3", S)]),
    ("headphones", &[P("M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3", S)]),
    ("speaker", &[Rr(4.0, 2.0, 16.0, 20.0, 2.0, S), C(12.0, 14.0, 4.0, S), P("M12 6h.01", S)]),
    ("keyboard", &[Rr(2.0, 4.0, 20.0, 16.0, 2.0, S), P("M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10", S)]),
    ("mouse", &[Rr(5.0, 2.0, 14.0, 20.0, 7.0, S), P("M12 6v4", S)]),
    ("search", &[C(11.0, 11.0, 8.0, S), P("m21 21-4.3-4.3", S)]),
    ("note", &[P("M9 18V5l12-2v13", S), C(6.0, 18.0, 3.0, S), C(18.0, 16.0, 3.0, S)]),
    ("max", &[P("M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7", S)]),
    ("unmax", &[P("M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7", S)]),
    ("x", &[P("M18 6 6 18M6 6l12 12", S)]),
    ("back", &[P("M21 4H8l-7 8 7 8h13a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2z", S), P("M18 9l-6 6M12 9l6 6", S)]),
    ("globe", &[C(12.0, 12.0, 10.0, S), P("M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z", S)]),
    ("msg", &[P("M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z", S)]),
    ("bell", &[P("M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9", S), P("M10.3 21a1.94 1.94 0 0 0 3.4 0", S)]),
    ("phone", &[Rr(5.0, 2.0, 14.0, 20.0, 2.0, S), P("M12 18h.01", S)]),
    ("laptop", &[Rr(4.0, 4.0, 16.0, 12.0, 2.0, S), P("M2 20h20", S)]),
    ("lock-key", &[Rr(3.0, 11.0, 18.0, 11.0, 2.0, S), P("M7 11V7a5 5 0 0 1 10 0v4", S)]),
];

/// The elements of icon `name`.
pub fn find(name: &str) -> Option<&'static [El]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, e)| *e)
}

/// A polyline, closed or open.
#[derive(Clone, Debug, PartialEq)]
pub struct Sub {
    pub pts: Vec<(f32, f32)>,
    pub closed: bool,
}

/// Reads SVG path data (M L H V C S Q A Z, absolute and relative) into
/// polylines; curves become short segments.
pub fn flatten(d: &str) -> Vec<Sub> {
    let mut out: Vec<Sub> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // The last control point, for S.
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut p = Parser { s: d.as_bytes(), i: 0 };
    let mut cmd = b'M';
    loop {
        p.skip();
        if p.i >= p.s.len() {
            break;
        }
        let c = p.s[p.i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            p.i += 1;
            if cmd == b'Z' || cmd == b'z' {
                if !cur.is_empty() {
                    out.push(Sub { pts: core::mem::take(&mut cur), closed: true });
                }
                x = sx;
                y = sy;
                last_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(a), Some(b)) = (p.num(), p.num()) else { break };
                if cur.len() > 1 {
                    out.push(Sub { pts: core::mem::take(&mut cur), closed: false });
                }
                cur.clear();
                x = ox + a;
                y = oy + b;
                sx = x;
                sy = y;
                cur.push((x, y));
                // Further pairs are lines.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                let (Some(a), Some(b)) = (p.num(), p.num()) else { break };
                if cur.is_empty() {
                    cur.push((x, y));
                }
                x = ox + a;
                y = oy + b;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'H' => {
                let Some(a) = p.num() else { break };
                if cur.is_empty() {
                    cur.push((x, y));
                }
                x = if rel { x + a } else { a };
                cur.push((x, y));
                last_ctrl = None;
            }
            b'V' => {
                let Some(a) = p.num() else { break };
                if cur.is_empty() {
                    cur.push((x, y));
                }
                y = if rel { y + a } else { a };
                cur.push((x, y));
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let smooth = cmd.eq_ignore_ascii_case(&b'S');
                let (c1x, c1y) = if smooth {
                    last_ctrl.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy))
                } else {
                    let (Some(a), Some(b)) = (p.num(), p.num()) else { break };
                    (ox + a, oy + b)
                };
                let (Some(a), Some(b), Some(e), Some(f)) = (p.num(), p.num(), p.num(), p.num()) else { break };
                let (c2x, c2y, ex, ey) = (ox + a, oy + b, ox + e, oy + f);
                if cur.is_empty() {
                    cur.push((x, y));
                }
                for i in 1..=12 {
                    let t = i as f32 / 12.0;
                    let u = 1.0 - t;
                    let px = u * u * u * x + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * ex;
                    let py = u * u * u * y + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * ey;
                    cur.push((px, py));
                }
                last_ctrl = Some((c2x, c2y));
                x = ex;
                y = ey;
            }
            b'Q' => {
                let (Some(a), Some(b), Some(e), Some(f)) = (p.num(), p.num(), p.num(), p.num()) else { break };
                let (cx, cy, ex, ey) = (ox + a, oy + b, ox + e, oy + f);
                if cur.is_empty() {
                    cur.push((x, y));
                }
                for i in 1..=10 {
                    let t = i as f32 / 10.0;
                    let u = 1.0 - t;
                    cur.push((u * u * x + 2.0 * u * t * cx + t * t * ex, u * u * y + 2.0 * u * t * cy + t * t * ey));
                }
                last_ctrl = None;
                x = ex;
                y = ey;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (p.num(), p.num(), p.num()) else { break };
                let (Some(large), Some(sweep)) = (p.flag(), p.flag()) else { break };
                let (Some(a), Some(b)) = (p.num(), p.num()) else { break };
                let (ex, ey) = (ox + a, oy + b);
                if cur.is_empty() {
                    cur.push((x, y));
                }
                arc(&mut cur, (x, y), rx, ry, rot, large, sweep, (ex, ey));
                last_ctrl = None;
                x = ex;
                y = ey;
            }
            _ => break,
        }
    }
    if cur.len() > 1 {
        out.push(Sub { pts: cur, closed: false });
    } else if cur.len() == 1 {
        // A lone point ("M12 6h.01" style dots are handled by H): keep it so
        // that it paints a round dot.
        out.push(Sub { pts: cur, closed: false });
    }
    out
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] == b' ' || self.s[self.i] == b',' || self.s[self.i] == b'\n' || self.s[self.i] == b'\t') {
            self.i += 1;
        }
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }

    fn num(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let s = self.s;
        if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
            self.i += 1;
        }
        let mut dot = false;
        let mut digits = false;
        while self.i < s.len() {
            let c = s[self.i];
            if c.is_ascii_digit() {
                digits = true;
            } else if c == b'.' && !dot {
                dot = true;
            } else {
                break;
            }
            self.i += 1;
        }
        if !digits {
            self.i = start;
            return None;
        }
        // Parse by hand (no std).
        let text = &s[start..self.i];
        let mut neg = false;
        let mut v = 0.0f64;
        let mut scale = 0.0f64;
        for &c in text {
            match c {
                b'-' => neg = true,
                b'+' => {}
                b'.' => scale = 1.0,
                d => {
                    let d = (d - b'0') as f64;
                    if scale > 0.0 {
                        scale /= 10.0;
                        v += d * scale;
                    } else {
                        v = v * 10.0 + d;
                    }
                }
            }
        }
        Some(if neg { -v as f32 } else { v as f32 })
    }
}

/// An SVG elliptical arc from `p0` to `p1`, appended as segments.
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<(f32, f32)>, p0: (f32, f32), rx: f32, ry: f32, rot: f32, large: bool, sweep: bool, p1: (f32, f32)) {
    use justmath::{atan2, cos, sin, sqrt};
    let (mut rx, mut ry) = (fabs64(rx as f64), fabs64(ry as f64));
    if rx == 0.0 || ry == 0.0 || p0 == p1 {
        out.push(p1);
        return;
    }
    let phi = rot as f64 * core::f64::consts::PI / 180.0;
    let (cp, sp) = (cos(phi), sin(phi));
    let (dx, dy) = ((p0.0 - p1.0) as f64 / 2.0, (p0.1 - p1.1) as f64 / 2.0);
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lambda = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lambda > 1.0 {
        let s = sqrt(lambda);
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den == 0.0 { 0.0 } else { sqrt((num / den).max(0.0)) };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cp * cxp - sp * cyp + (p0.0 + p1.0) as f64 / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.1 + p1.1) as f64 / 2.0;
    let t1 = atan2((y1 - cyp) / ry, (x1 - cxp) / rx);
    let t2 = atan2((-y1 - cyp) / ry, (-x1 - cxp) / rx);
    let mut dt = t2 - t1;
    let two_pi = 2.0 * core::f64::consts::PI;
    if sweep && dt < 0.0 {
        dt += two_pi;
    } else if !sweep && dt > 0.0 {
        dt -= two_pi;
    }
    let n = ((fabs64(dt) * rx.max(ry) / 0.6) as usize).clamp(4, 64);
    for i in 1..=n {
        let t = t1 + dt * i as f64 / n as f64;
        let (ct, st) = (cos(t), sin(t));
        let x = cp * rx * ct - sp * ry * st + cx;
        let y = sp * rx * ct + cp * ry * st + cy;
        out.push((x as f32, y as f32));
    }
    // Land exactly on the end.
    if let Some(l) = out.last_mut() {
        *l = p1;
    }
}

fn fabs64(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// An element as polylines on the grid, and how it is painted.
fn polylines(el: &El) -> (Vec<Sub>, Paint) {
    match *el {
        El::Path(d, paint) => (flatten(d), paint),
        El::Circle(cx, cy, r, paint) => {
            let n = 48;
            let pts = (0..n)
                .map(|i| {
                    let t = i as f64 * 2.0 * core::f64::consts::PI / n as f64;
                    (cx + r * justmath::cos(t) as f32, cy + r * justmath::sin(t) as f32)
                })
                .collect();
            (vec![Sub { pts, closed: true }], paint)
        }
        El::Rect(x, y, w, h, rad, paint) => (vec![Sub { pts: round_rect(x, y, w, h, rad), closed: true }], paint),
    }
}

/// The outline of a rounded rectangle, clockwise from the top left corner's
/// end.
fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pts = Vec::new();
    let corners = [(x + w - r, y + r, -90.0f64), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0)];
    for (cx, cy, a0) in corners {
        for i in 0..=8 {
            let a = (a0 + 90.0 * i as f64 / 8.0) * core::f64::consts::PI / 180.0;
            pts.push((cx + r * justmath::cos(a) as f32, cy + r * justmath::sin(a) as f32));
        }
    }
    pts
}

/// Distance from (`px`, `py`) to the segment `a`-`b`.
fn seg_dist(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let (wx, wy) = (px - a.0, py - a.1);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (wx - t * vx, wy - t * vy);
    justmath::sqrt((dx * dx + dy * dy) as f64) as f32
}

/// The segments of a polyline (closed ones back to the start).
fn segments(s: &Sub) -> impl Iterator<Item = ((f32, f32), (f32, f32))> + '_ {
    let n = s.pts.len();
    let extra = if s.closed && n > 2 { 1 } else { 0 };
    let single = if n == 1 { 1 } else { 0 };
    (0..(n.saturating_sub(1) + extra + single)).map(move |i| if n == 1 { (s.pts[0], s.pts[0]) } else { (s.pts[i], s.pts[(i + 1) % n]) })
}

/// Winding number of the closed polylines around a point.
fn winding(subs: &[Sub], px: f32, py: f32) -> i32 {
    let mut wn = 0;
    for s in subs {
        let n = s.pts.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (s.pts[i], s.pts[(i + 1) % n]);
            let cross = (b.0 - a.0) * (py - a.1) - (px - a.0) * (b.1 - a.1);
            if a.1 <= py {
                if b.1 > py && cross > 0.0 {
                    wn += 1;
                }
            } else if b.1 <= py && cross < 0.0 {
                wn -= 1;
            }
        }
    }
    wn
}

/// Paints polylines (in pixels) into `mask` (`w` x `h`): strokes `width`
/// wide with round ends, fills by the non-zero rule, both anti-aliased.
pub fn paint(mask: &mut [u8], w: i32, h: i32, subs: &[Sub], paint: Paint, width: f32) {
    let half = width / 2.0;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in subs {
        for &(x, y) in &s.pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 {
        return;
    }
    let m = half + 1.5;
    let (bx0, by0) = (((x0 - m) as i32).max(0), ((y0 - m) as i32).max(0));
    let (bx1, by1) = (((x1 + m) as i32 + 1).min(w), ((y1 + m) as i32 + 1).min(h));
    let stroke = matches!(paint, Paint::Stroke | Paint::Both);
    let fill = matches!(paint, Paint::Fill | Paint::Both);
    for py in by0..by1 {
        for px in bx0..bx1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let mut d = f32::MAX;
            for s in subs {
                for (a, b) in segments(s) {
                    d = d.min(seg_dist(cx, cy, a, b));
                }
            }
            let mut cov = 0.0f32;
            if stroke {
                cov = cov.max((half + 0.5 - d).clamp(0.0, 1.0));
            }
            if fill {
                let inside = winding(subs, cx, cy) != 0;
                cov = cov.max(if inside { (0.5 + d).min(1.0) } else { (0.5 - d).max(0.0) });
            }
            if cov > 0.0 {
                let i = (py * w + px) as usize;
                let v = (cov * 255.0 + 0.5) as u8;
                if v > mask[i] {
                    mask[i] = v;
                }
            }
        }
    }
}

/// Icon `name` as a `px` x `px` alpha mask, strokes `stroke` grid units
/// wide (2 is the reference's weight).
pub fn icon(name: &str, px: i32, stroke: f32) -> Option<Vec<u8>> {
    let els = find(name)?;
    let px = px.max(1);
    let mut mask = vec![0u8; (px * px) as usize];
    let k = px as f32 / 24.0;
    for el in els {
        let (subs, p) = polylines(el);
        let scaled: Vec<Sub> = subs.into_iter().map(|s| Sub { pts: s.pts.iter().map(|&(x, y)| (x * k, y * k)).collect(), closed: s.closed }).collect();
        paint(&mut mask, px, px, &scaled, p, stroke * k);
    }
    Some(mask)
}

/// The outline of a pill `w` x `h` with radius `r` (inset by `inset`),
/// clockwise from the middle of the top edge, as the battery ring follows
/// it.
pub fn pill_outline(w: f32, h: f32, r: f32, inset: f32) -> Vec<(f32, f32)> {
    let (x, y, w, h) = (inset, inset, w - 2.0 * inset, h - 2.0 * inset);
    let r = (r - inset).min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pts = alloc::vec![(x + w / 2.0, y)];
    let corners = [(x + w - r, y + r, -90.0f64), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0)];
    for (cx, cy, a0) in corners {
        for i in 0..=16 {
            let a = (a0 + 90.0 * i as f64 / 16.0) * core::f64::consts::PI / 180.0;
            pts.push((cx + r * justmath::cos(a) as f32, cy + r * justmath::sin(a) as f32));
        }
    }
    pts.push((x + w / 2.0, y));
    pts
}

/// The length of a polyline.
pub fn length(pts: &[(f32, f32)]) -> f32 {
    pts.windows(2).map(|p| seg_dist(p[0].0, p[0].1, p[1], p[1])).sum()
}

/// The first `frac` (0..1) of a polyline's length.
pub fn partial(pts: &[(f32, f32)], frac: f32) -> Vec<(f32, f32)> {
    let total = length(pts);
    let mut left = total * frac.clamp(0.0, 1.0);
    let mut out = Vec::new();
    if let Some(&p) = pts.first() {
        out.push(p);
    }
    for p in pts.windows(2) {
        let l = seg_dist(p[0].0, p[0].1, p[1], p[1]);
        if l <= left {
            out.push(p[1]);
            left -= l;
        } else {
            if l > 0.0 && left > 0.0 {
                let t = left / l;
                out.push((p[0].0 + (p[1].0 - p[0].0) * t, p[0].1 + (p[1].1 - p[0].1) * t));
            }
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses_and_paints() {
        for (name, _) in ICONS {
            let m = icon(name, 24, 2.0).unwrap();
            let ink: u32 = m.iter().map(|&v| v as u32).sum();
            assert!(ink > 255 * 6, "{name} is almost empty ({ink})");
            assert!(ink < 255 * 24 * 24 * 7 / 10, "{name} is almost full");
        }
        assert!(icon("nope", 20, 2.0).is_none());
    }

    #[test]
    fn numbers_without_separators() {
        let s = flatten("M1.5.5l-.3-2");
        assert_eq!(s.len(), 1);
        let p = &s[0].pts;
        assert_eq!(p[0], (1.5, 0.5));
        assert!((p[1].0 - 1.2).abs() < 1e-5 && (p[1].1 + 1.5).abs() < 1e-5);
    }

    #[test]
    fn relative_lines_and_close() {
        let s = flatten("m5 4 10 8-10 8z");
        assert_eq!(s[0].pts, alloc::vec![(5.0, 4.0), (15.0, 12.0), (5.0, 20.0)]);
        assert!(s[0].closed);
    }

    #[test]
    fn arc_ends_where_asked() {
        let s = flatten("M5 12.55a11 11 0 0 1 14.08 0");
        let last = *s[0].pts.last().unwrap();
        assert!((last.0 - 19.08).abs() < 1e-3 && (last.1 - 12.55).abs() < 1e-3);
        // An upward bulge: the middle of the arc is above the chord.
        let min_y = s[0].pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        assert!(min_y > 9.5 && min_y < 10.5, "{min_y}");
    }

    #[test]
    fn filled_shapes_are_solid_inside() {
        let m = icon("play", 48, 2.0).unwrap();
        assert_eq!(m[24 * 48 + 22], 255);
        assert_eq!(m[2 * 48 + 2], 0);
        let x = icon("x", 24, 2.0).unwrap();
        assert!(x[12 * 24 + 12] > 200, "the cross's middle is inked");
    }

    #[test]
    fn pill_ring_length_and_parts() {
        let pts = pill_outline(100.0, 46.0, 23.0, 0.0);
        let l = length(&pts);
        let expect = 2.0 * (100.0 - 46.0) + core::f32::consts::PI * 46.0;
        assert!((l - expect).abs() < 1.0, "{l} vs {expect}");
        let half = partial(&pts, 0.5);
        assert!((length(&half) - l / 2.0).abs() < 0.5);
        assert_eq!(partial(&pts, 0.0).len(), 1);
        assert!((length(&partial(&pts, 1.0)) - l).abs() < 0.01);
        // It starts at the top middle and goes right first (clockwise).
        assert_eq!(pts[0], (50.0, 0.0));
        assert!(pts[1].0 > 50.0);
    }
}
