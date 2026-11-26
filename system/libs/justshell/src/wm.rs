//! Window geometry: where a new window opens, the eight resize handles,
//! maximised and full-screen modes, the genie towards the dock, and the
//! bar's peek over a full-screen window (the bottom edge, or the logo key
//! pressed alone).

use crate::bar::{BOTTOM, IDLE_H};
use crate::round;

/// The smallest window.
pub const MIN_W: i32 = 460;
pub const MIN_H: i32 = 320;
/// The title row.
pub const TITLE_H: i32 = 60;
/// Resize bands: 10 wide, centred on the edge, leaving 18 at the ends; the
/// corners 20 x 20 (5 outside the frame).
pub const EDGE_BAND: i32 = 10;
pub const EDGE_INSET: i32 = 18;
pub const CORNER: i32 = 20;
/// Windows open in a cascade of this step.
pub const CASCADE: i32 = 28;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl R {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> R {
        R { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
}

/// The height windows may use above the bar.
pub fn work_h(sh: i32, k: f32) -> i32 {
    sh - (BOTTOM as i32 + round(IDLE_H * k) + 18)
}

/// A new window's size when the app has no size of its own.
pub fn default_size(sw: i32, sh: i32) -> (i32, i32) {
    (760.min(sw - 80), 520.min(sh - 60))
}

/// Where the `n`th window of `w` x `h` opens: centred in the work area,
/// stepped by the cascade, never above 20.
pub fn open_rect(w: i32, h: i32, n: usize, sw: i32, sh: i32, k: f32) -> R {
    let area = work_h(sh, k);
    let (w, h) = (w.min(sw), h.min(area.max(MIN_H)));
    let c = (n % 5) as i32 * CASCADE;
    let x = ((sw - w) / 2 + c).clamp(0, (sw - w).max(0));
    let y = ((area - h) / 2 + c).max(20);
    R::new(x, y, w, h)
}

/// A maximised window: the bar stays visible.
pub fn maximized(sw: i32, sh: i32, k: f32) -> R {
    R::new(14, 14, sw - 28, work_h(sh, k) - 14)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    /// Moves the left edge.
    fn west(self) -> bool {
        matches!(self, Handle::W | Handle::NW | Handle::SW)
    }

    fn east(self) -> bool {
        matches!(self, Handle::E | Handle::NE | Handle::SE)
    }

    fn north(self) -> bool {
        matches!(self, Handle::N | Handle::NE | Handle::NW)
    }

    fn south(self) -> bool {
        matches!(self, Handle::S | Handle::SE | Handle::SW)
    }

    pub const ALL: [Handle; 8] = [Handle::N, Handle::S, Handle::E, Handle::W, Handle::NE, Handle::NW, Handle::SE, Handle::SW];
}

/// The resize handle of window `r` under (`x`, `y`), if any.
pub fn handle_at(r: R, x: i32, y: i32) -> Option<Handle> {
    let half = EDGE_BAND / 2;
    let (l, t, rt, b) = (r.x, r.y, r.right(), r.bottom());
    // Corners: 20 x 20 reaching 5 outside.
    let corner = |cx: i32, cy: i32| x >= cx && x < cx + CORNER && y >= cy && y < cy + CORNER;
    if corner(l - half, t - half) {
        return Some(Handle::NW);
    }
    if corner(rt + half - CORNER, t - half) {
        return Some(Handle::NE);
    }
    if corner(l - half, b + half - CORNER) {
        return Some(Handle::SW);
    }
    if corner(rt + half - CORNER, b + half - CORNER) {
        return Some(Handle::SE);
    }
    let along_x = x >= l + EDGE_INSET && x < rt - EDGE_INSET;
    let along_y = y >= t + EDGE_INSET && y < b - EDGE_INSET;
    let near = |edge: i32, v: i32| v >= edge - half && v < edge + half;
    if along_x && near(t, y) {
        Some(Handle::N)
    } else if along_x && near(b, y) {
        Some(Handle::S)
    } else if along_y && near(l, x) {
        Some(Handle::W)
    } else if along_y && near(rt, x) {
        Some(Handle::E)
    } else {
        None
    }
}

/// Window `start` resized from handle `h` by (`dx`, `dy`), never smaller
/// than `min`; the opposite edge stays where it is.
pub fn resize(start: R, h: Handle, dx: i32, dy: i32, min: (i32, i32)) -> R {
    let mut r = start;
    if h.east() {
        r.w = (start.w + dx).max(min.0);
    }
    if h.west() {
        r.w = (start.w - dx).max(min.0);
        r.x = start.right() - r.w;
    }
    if h.south() {
        r.h = (start.h + dy).max(min.1);
    }
    if h.north() {
        r.h = (start.h - dy).max(min.1);
        r.y = start.bottom() - r.h;
    }
    r
}

/// The genie: how far the window's centre moves to reach `target`'s
/// centre, and the scale it shrinks to.
pub fn genie(win: R, target: R) -> (i32, i32, f32) {
    let dx = (target.x * 2 + target.w - win.x * 2 - win.w) / 2;
    let dy = (target.y * 2 + target.h - win.y * 2 - win.h) / 2;
    (dx, dy, (target.w as f32 / win.w.max(1) as f32).max(0.06))
}

/// `r` scaled by `s` about its centre and moved by `t` (0..1 of the way).
pub fn genie_at(win: R, target: R, t: f32) -> R {
    let (dx, dy, s) = genie(win, target);
    let k = 1.0 + (s - 1.0) * t;
    let (w, h) = (round(win.w as f32 * k).max(1), round(win.h as f32 * k).max(1));
    let cx = win.x * 2 + win.w + round(2.0 * dx as f32 * t);
    let cy = win.y * 2 + win.h + round(2.0 * dy as f32 * t);
    R::new((cx - w) / 2, (cy - h) / 2, w, h)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Normal,
    Maximized {
        restore: R,
    },
    /// `max_restore`: it was maximised (with that rect to go back to).
    Fullscreen {
        restore: R,
        max_restore: Option<R>,
    },
}

impl Mode {
    /// Double click on the title: maximised and back (nothing in full
    /// screen). The new mode and rect.
    pub fn toggle_max(self, rect: R, max: R) -> (Mode, R) {
        match self {
            Mode::Normal => (Mode::Maximized { restore: rect }, max),
            Mode::Maximized { restore } => (Mode::Normal, restore),
            Mode::Fullscreen { .. } => (self, rect),
        }
    }

    /// The full-screen button: the whole screen and back.
    pub fn toggle_full(self, rect: R, screen: R) -> (Mode, R) {
        match self {
            Mode::Normal => (Mode::Fullscreen { restore: rect, max_restore: None }, screen),
            Mode::Maximized { restore } => (Mode::Fullscreen { restore: rect, max_restore: Some(restore) }, screen),
            Mode::Fullscreen { restore, max_restore } => (max_restore.map_or(Mode::Normal, |r| Mode::Maximized { restore: r }), restore),
        }
    }

    pub fn is_full(self) -> bool {
        matches!(self, Mode::Fullscreen { .. })
    }

    pub fn is_max(self) -> bool {
        matches!(self, Mode::Maximized { .. })
    }

    /// Handles and dragging work only in the normal mode.
    pub fn free(self) -> bool {
        self == Mode::Normal
    }
}

/// How the bar was brought up over a full-screen window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeekBy {
    Mouse,
    Key,
}

/// The activation band at the bottom edge.
pub const EDGE_PX: i32 = 6;
pub const HIDE_DELAY_MS: u64 = 450;

/// The full-screen window and the bar's peek over it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Fullscreen {
    pub window: Option<u32>,
    pub peek: Option<PeekBy>,
    hide_at: Option<u64>,
}

impl Fullscreen {
    /// Window `id` goes full screen (another one leaves it first: its id).
    pub fn enter(&mut self, id: u32) -> Option<u32> {
        let old = self.window.filter(|&w| w != id);
        self.window = Some(id);
        self.reset_peek();
        old
    }

    pub fn exit(&mut self) {
        self.window = None;
        self.reset_peek();
    }

    /// Window `id` takes the focus: a full-screen other window leaves full
    /// screen first (its id).
    pub fn focus(&mut self, id: u32) -> Option<u32> {
        match self.window {
            Some(w) if w != id => {
                self.exit();
                Some(w)
            }
            _ => None,
        }
    }

    /// Window `id` closed or minimised.
    pub fn gone(&mut self, id: u32) {
        if self.window == Some(id) {
            self.exit();
        }
    }

    pub fn reset_peek(&mut self) {
        self.peek = None;
        self.hide_at = None;
    }

    /// The bar is hidden: full screen, the dock at rest, no peek.
    pub fn bar_hidden(&self, idle: bool) -> bool {
        self.window.is_some() && idle && self.peek.is_none()
    }

    /// Esc leaves full screen only with the dock at rest and no peek.
    pub fn esc_exits(&self, idle: bool) -> bool {
        self.window.is_some() && idle && self.peek.is_none()
    }

    /// The pointer at `y` on a screen `sh` high; true if the peek changed.
    pub fn pointer(&mut self, y: i32, sh: i32, k: f32, now: u64) -> bool {
        if self.window.is_none() {
            return false;
        }
        let edge = sh - 1 - y;
        if edge < EDGE_PX {
            self.hide_at = None;
            let changed = self.peek != Some(PeekBy::Mouse);
            self.peek = Some(PeekBy::Mouse);
            return changed;
        }
        if self.peek != Some(PeekBy::Mouse) {
            return false;
        }
        let zone = BOTTOM as i32 + round(IDLE_H * k) + 40;
        if edge > zone {
            if self.hide_at.is_none() {
                self.hide_at = Some(now + HIDE_DELAY_MS);
            }
        } else {
            self.hide_at = None;
        }
        false
    }

    /// When the pending hide is due (to wake up then).
    pub fn hide_due(&self) -> Option<u64> {
        self.hide_at
    }

    /// Time passes: the pointer's peek ends after the delay, if the bar is
    /// still at rest (an open control centre stays). True if it changed.
    pub fn tick(&mut self, now: u64, idle: bool) -> bool {
        match self.hide_at {
            Some(t) if now >= t => {
                self.hide_at = None;
                if idle && self.peek == Some(PeekBy::Mouse) {
                    self.peek = None;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// The logo key pressed and released alone: the bar comes or goes.
    pub fn super_alone(&mut self) -> bool {
        if self.window.is_none() {
            return false;
        }
        self.hide_at = None;
        self.peek = if self.peek.is_some() { None } else { Some(PeekBy::Key) };
        true
    }

    /// A click inside a window closes a peek opened by the key.
    pub fn click_in_window(&mut self) -> bool {
        if self.peek == Some(PeekBy::Key) {
            self.peek = None;
            return true;
        }
        false
    }
}

/// Tells a press and release of the logo key alone from the logo key used
/// with another key.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SuperKey {
    down: bool,
    used: bool,
}

impl SuperKey {
    /// A key went down or up; true when the logo key was released with no
    /// other key in between.
    pub fn key(&mut self, is_super: bool, pressed: bool) -> bool {
        if is_super {
            if pressed {
                if !self.down {
                    self.down = true;
                    self.used = false;
                }
                false
            } else {
                let alone = self.down && !self.used;
                self.down = false;
                self.used = false;
                alone
            }
        } else {
            if pressed && self.down {
                self.used = true;
            }
            false
        }
    }

    /// A mouse button with the key held also counts as a combination.
    pub fn other(&mut self) {
        if self.down {
            self.used = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: R = R::new(300, 200, 700, 500);

    #[test]
    fn minimum_size_from_every_handle() {
        for h in Handle::ALL {
            let r = resize(START, h, 5000, 5000, (MIN_W, MIN_H));
            let r2 = resize(START, h, -5000, -5000, (MIN_W, MIN_H));
            for x in [r, r2] {
                assert!(x.w >= MIN_W && x.h >= MIN_H, "{h:?} {x:?}");
            }
        }
    }

    #[test]
    fn opposite_edge_stays_for_west_and_north() {
        for h in [Handle::W, Handle::NW, Handle::SW] {
            for dx in [-200, 30, 1000] {
                let r = resize(START, h, dx, 0, (MIN_W, MIN_H));
                assert_eq!(r.right(), START.right(), "{h:?} {dx}");
            }
        }
        for h in [Handle::N, Handle::NE, Handle::NW] {
            for dy in [-200, 30, 1000] {
                let r = resize(START, h, 0, dy, (MIN_W, MIN_H));
                assert_eq!(r.bottom(), START.bottom(), "{h:?} {dy}");
            }
        }
        let r = resize(START, Handle::SE, 40, 50, (MIN_W, MIN_H));
        assert_eq!(r, R::new(300, 200, 740, 550));
        let r = resize(START, Handle::E, 0, 99, (MIN_W, MIN_H));
        assert_eq!(r.h, START.h);
    }

    #[test]
    fn handles_where_the_design_puts_them() {
        let r = START;
        assert_eq!(handle_at(r, r.x - 4, r.y - 4), Some(Handle::NW));
        assert_eq!(handle_at(r, r.right() + 4, r.y + 10), Some(Handle::NE));
        assert_eq!(handle_at(r, r.x + 2, r.bottom() - 2), Some(Handle::SW));
        assert_eq!(handle_at(r, r.right() - 1, r.bottom() - 1), Some(Handle::SE));
        assert_eq!(handle_at(r, r.x + 300, r.y - 3), Some(Handle::N));
        assert_eq!(handle_at(r, r.x + 300, r.bottom() + 4), Some(Handle::S));
        assert_eq!(handle_at(r, r.x - 5, r.y + 200), Some(Handle::W));
        assert_eq!(handle_at(r, r.right() + 2, r.y + 200), Some(Handle::E));
        assert_eq!(handle_at(r, r.x + 300, r.y + 6), None, "inside the title");
        assert_eq!(handle_at(r, r.x + 300, r.y - 6), None, "too far out");
    }

    #[test]
    fn double_click_maximises_and_restores_exactly() {
        let max = maximized(1440, 1024, 1.0);
        let (m, r) = Mode::Normal.toggle_max(START, max);
        assert_eq!(r, max);
        assert!(m.is_max());
        let (m, r) = m.toggle_max(r, max);
        assert_eq!((m, r), (Mode::Normal, START));
        let full = Mode::Fullscreen { restore: START, max_restore: None };
        assert_eq!(full.toggle_max(R::new(0, 0, 1440, 1024), max), (full, R::new(0, 0, 1440, 1024)));
    }

    #[test]
    fn maximised_leaves_the_bar_visible() {
        let max = maximized(1440, 1024, 1.0);
        assert_eq!(max, R::new(14, 14, 1412, 1024 - (28 + 68 + 18) - 14));
        assert!(max.bottom() <= 1024 - 28 - 68);
    }

    #[test]
    fn full_screen_and_back() {
        let screen = R::new(0, 0, 1440, 1024);
        let (m, r) = Mode::Normal.toggle_full(START, screen);
        assert_eq!(r, screen);
        assert!(m.is_full() && !m.free());
        assert_eq!(m.toggle_full(r, screen), (Mode::Normal, START));
        // From maximised: back to maximised.
        let max = maximized(1440, 1024, 1.0);
        let (m, r) = Mode::Normal.toggle_max(START, max);
        let (m, r) = m.toggle_full(r, screen);
        let (m, r) = m.toggle_full(r, screen);
        assert_eq!((m, r), (Mode::Maximized { restore: START }, max));
        let mut fs = Fullscreen::default();
        fs.enter(7);
        assert_eq!(fs.window, Some(7));
        assert!(fs.bar_hidden(true));
        assert!(!fs.bar_hidden(false), "transients show over full screen");
    }

    #[test]
    fn focus_elsewhere_leaves_full_screen() {
        let mut fs = Fullscreen::default();
        fs.enter(1);
        assert_eq!(fs.focus(1), None);
        assert_eq!(fs.focus(2), Some(1));
        assert_eq!(fs.window, None);
        assert!(!fs.bar_hidden(true));
    }

    #[test]
    fn closing_the_full_screen_window_shows_the_bar() {
        let mut fs = Fullscreen::default();
        fs.enter(3);
        fs.super_alone();
        fs.gone(4);
        assert_eq!(fs.window, Some(3));
        fs.gone(3);
        assert_eq!(fs.window, None);
        assert_eq!(fs.peek, None);
        assert!(!fs.bar_hidden(true));
    }

    #[test]
    fn peek_by_the_bottom_edge() {
        let mut fs = Fullscreen::default();
        assert!(!fs.pointer(1023, 1024, 1.0, 0), "no full screen, no peek");
        fs.enter(1);
        assert!(!fs.pointer(1000, 1024, 1.0, 0));
        assert!(fs.pointer(1024 - 6, 1024, 1.0, 0));
        assert_eq!(fs.peek, Some(PeekBy::Mouse));
        // Still in the bar's zone: stays.
        fs.pointer(1024 - 100, 1024, 1.0, 100);
        assert_eq!(fs.hide_due(), None);
        // Out of it: hidden 450 ms later, if the bar is at rest.
        fs.pointer(500, 1024, 1.0, 200);
        assert_eq!(fs.hide_due(), Some(200 + HIDE_DELAY_MS));
        assert!(!fs.tick(600, true));
        assert!(fs.tick(650, true));
        assert_eq!(fs.peek, None);
    }

    #[test]
    fn peek_stays_while_the_bar_is_busy() {
        let mut fs = Fullscreen::default();
        fs.enter(1);
        fs.pointer(1023, 1024, 1.0, 0);
        fs.pointer(100, 1024, 1.0, 10);
        assert!(!fs.tick(1000, false));
        assert_eq!(fs.peek, Some(PeekBy::Mouse));
        // Back into the zone before the delay: no hide.
        fs.pointer(100, 1024, 1.0, 2000);
        fs.pointer(1024 - 50, 1024, 1.0, 2100);
        assert!(!fs.tick(5000, true));
        assert_eq!(fs.peek, Some(PeekBy::Mouse));
    }

    #[test]
    fn super_alone_toggles_and_combinations_do_nothing() {
        let mut k = SuperKey::default();
        assert!(!k.key(true, true));
        assert!(k.key(true, false), "alone");
        assert!(!k.key(true, true));
        assert!(!k.key(false, true));
        assert!(!k.key(false, false));
        assert!(!k.key(true, false), "with another key");
        k.key(true, true);
        k.other();
        assert!(!k.key(true, false), "with a click");
        assert!(!k.key(true, false), "a release without press");

        let mut fs = Fullscreen::default();
        assert!(!fs.super_alone());
        fs.enter(1);
        assert!(fs.super_alone());
        assert_eq!(fs.peek, Some(PeekBy::Key));
        assert!(!fs.esc_exits(true));
        assert!(fs.click_in_window());
        assert_eq!(fs.peek, None);
        assert!(fs.esc_exits(true));
        fs.super_alone();
        assert!(fs.super_alone());
        assert_eq!(fs.peek, None);
    }

    #[test]
    fn opening_in_a_cascade() {
        let (w, h) = default_size(1440, 1024);
        assert_eq!((w, h), (760, 520));
        let a = open_rect(w, h, 0, 1440, 1024, 1.0);
        let b = open_rect(w, h, 1, 1440, 1024, 1.0);
        let f = open_rect(w, h, 5, 1440, 1024, 1.0);
        assert_eq!(a.x, (1440 - 760) / 2);
        assert_eq!((b.x - a.x, b.y - a.y), (28, 28));
        assert_eq!(f, a);
        assert!(open_rect(w, 900, 0, 1440, 1024, 1.0).y >= 20);
        assert_eq!(default_size(800, 500), (720, 440));
    }

    #[test]
    fn genie_reaches_the_icon() {
        let win = R::new(100, 100, 800, 600);
        let icon = R::new(600, 950, 42, 42);
        let (dx, dy, s) = genie(win, icon);
        assert_eq!((dx, dy), (621 - 500, 971 - 400));
        assert_eq!(s, 0.06, "42 / 800 is under the floor");
        assert!((genie(win, R::new(0, 0, 200, 50)).2 - 0.25).abs() < 1e-6);
        assert_eq!(genie(win, R::new(0, 0, 10, 10)).2, 0.06);
        let end = genie_at(win, icon, 1.0);
        assert!((end.x + end.w / 2 - 621).abs() <= 1 && (end.y + end.h / 2 - 971).abs() <= 1);
        assert_eq!(genie_at(win, icon, 0.0), win);
    }
}
