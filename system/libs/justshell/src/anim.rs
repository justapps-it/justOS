//! Cubic-bezier curves (as in CSS) and tweens that can be interrupted: a new
//! target starts from the value shown now, never from the old target.

use crate::fabs;

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    ax: f32,
    bx: f32,
    cx: f32,
    ay: f32,
    by: f32,
    cy: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let ax = 1.0 - cx - bx;
        let cy = 3.0 * y1;
        let by = 3.0 * (y2 - y1) - cy;
        let ay = 1.0 - cy - by;
        Self { ax, bx, cx, ay, by, cy }
    }

    fn x(&self, t: f32) -> f32 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn y(&self, t: f32) -> f32 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn dx(&self, t: f32) -> f32 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    /// Time progress 0..1 to value 0..1 (above 1 for a curve that
    /// overshoots).
    pub fn eval(&self, x: f32) -> f32 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        // Newton first, bisection if it does not settle.
        let mut t = x;
        for _ in 0..8 {
            let e = self.x(t) - x;
            if fabs(e) < 1e-5 {
                return self.y(t);
            }
            let d = self.dx(t);
            if fabs(d) < 1e-6 {
                break;
            }
            t -= e / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        for _ in 0..24 {
            let v = self.x(t);
            if fabs(v - x) < 1e-5 {
                break;
            }
            if v < x {
                lo = t;
            } else {
                hi = t;
            }
            t = 0.5 * (lo + hi);
        }
        self.y(t)
    }
}

/// General movement, windows.
pub const EASE: CubicBezier = CubicBezier::new(0.25, 1.0, 0.5, 1.0);
/// The bar changing shape (a slight overshoot).
pub const MORPH: CubicBezier = CubicBezier::new(0.3, 1.04, 0.42, 1.0);
pub const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

/// A value moving from where it was towards a target along a curve.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: u64,
    ms: u32,
    curve: CubicBezier,
}

impl Tween {
    /// Resting at `v`; later moves take `ms` along `curve`.
    pub const fn at(v: f32, ms: u32, curve: CubicBezier) -> Tween {
        Tween { from: v, to: v, start: 0, ms, curve }
    }

    /// The value at time `now`.
    pub fn value(&self, now: u64) -> f32 {
        if self.ms == 0 || now >= self.start + self.ms as u64 {
            return self.to;
        }
        if now <= self.start {
            return self.from;
        }
        let t = (now - self.start) as f32 / self.ms as f32;
        self.from + (self.to - self.from) * self.curve.eval(t)
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Moves towards `to` from the value at `now`; false if that was the
    /// target already.
    pub fn set(&mut self, now: u64, to: f32) -> bool {
        self.set_after(now, 0, to)
    }

    /// Like [`set`](Self::set), starting `delay` ms from now.
    pub fn set_after(&mut self, now: u64, delay: u32, to: f32) -> bool {
        if to == self.to {
            return false;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now + delay as u64;
        true
    }

    /// There at once.
    pub fn jump(&mut self, v: f32) {
        self.from = v;
        self.to = v;
    }

    pub fn moving(&self, now: u64) -> bool {
        self.from != self.to && now < self.start + self.ms as u64
    }

    /// When it arrives.
    pub fn end(&self) -> u64 {
        self.start + self.ms as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_are_fixed() {
        for c in [EASE, MORPH, LINEAR] {
            assert_eq!(c.eval(0.0), 0.0);
            assert_eq!(c.eval(1.0), 1.0);
            assert_eq!(c.eval(-1.0), 0.0);
            assert_eq!(c.eval(2.0), 1.0);
        }
    }

    #[test]
    fn ease_is_monotonic_and_fast_at_first() {
        let mut last = 0.0;
        for i in 1..=200 {
            let v = EASE.eval(i as f32 / 200.0);
            assert!(v >= last - 1e-4, "EASE goes back at {i}");
            assert!(v <= 1.0 + 1e-4);
            last = v;
        }
        assert!(EASE.eval(0.25) > 0.6);
    }

    #[test]
    fn morph_overshoots() {
        let peak = (1..200).map(|i| MORPH.eval(i as f32 / 200.0)).fold(0.0f32, f32::max);
        assert!(peak > 1.0, "MORPH peak {peak}");
        assert!(peak < 1.02);
    }

    #[test]
    fn linear_is_identity() {
        for i in 0..=10 {
            let x = i as f32 / 10.0;
            assert!(fabs(LINEAR.eval(x) - x) < 1e-3);
        }
    }

    #[test]
    fn tween_interrupted_starts_from_current_value() {
        let mut t = Tween::at(0.0, 500, EASE);
        t.set(1000, 100.0);
        let mid = t.value(1200);
        assert!(mid > 0.0 && mid < 100.0);
        t.set(1200, -50.0);
        assert!(fabs(t.value(1200) - mid) < 1e-3, "no jump when interrupted");
        assert_eq!(t.value(1700), -50.0);
        assert!(!t.moving(1700));
        assert!(t.moving(1300));
    }

    #[test]
    fn tween_delay() {
        let mut t = Tween::at(1.0, 200, LINEAR);
        t.set_after(0, 80, 0.0);
        assert_eq!(t.value(50), 1.0);
        assert!(fabs(t.value(180) - 0.5) < 1e-3);
        assert_eq!(t.value(280), 0.0);
    }

    #[test]
    fn same_target_is_not_a_move() {
        let mut t = Tween::at(3.0, 100, EASE);
        assert!(!t.set(10, 3.0));
        assert!(!t.moving(20));
    }
}
