//! The dynamic bar: one surface anchored at the bottom centre of the screen,
//! one state at a time. Each state has a shape (width, height, radius) and
//! the bar morphs between them; transient states (the volume, a
//! notification...) go back to the dock by themselves.

use crate::anim::{Tween, EASE, LINEAR, MORPH};

/// Distance of the bar from the bottom of the screen.
pub const BOTTOM: f32 = 28.0;
/// The bar's height at rest (the dock).
pub const IDLE_H: f32 = 68.0;

pub const MORPH_MS: u32 = 520;
pub const SCALE_MS: u32 = 500;
pub const BOTTOM_MS: u32 = 600;
pub const BG_MS: u32 = 500;
/// The view going out: opacity, then scale.
pub const OUT_FADE_MS: u32 = 200;
pub const VIEW_SCALE_MS: u32 = 450;
/// The view coming in waits so that the bar has started to change shape.
pub const IN_DELAY_MS: u32 = 80;

pub const HUD_MS: u32 = 1600;
pub const NOTIFY_MS: u32 = 4600;
pub const SILENT_MS: u32 = 1600;
pub const CHARGE_MS: u32 = 2600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarState {
    Idle,
    Launch,
    Lock,
    Control,
    CcWifi,
    CcBt,
    Hud,
    Notify,
    Silent,
    Music,
    Power,
    Spin,
    Boot,
    Off,
    Charge,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shape {
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

impl BarState {
    /// Width, height and radius before the scale `K`.
    pub fn shape(self, music_active: bool) -> Shape {
        use BarState::*;
        let (w, h, r) = match self {
            Idle => (if music_active { 760.0 } else { 690.0 }, IDLE_H, 20.0),
            Launch => (540.0, 60.0, 30.0),
            Lock => (340.0, 540.0, 32.0),
            Control => (712.0, 316.0, 28.0),
            CcWifi | CcBt => (440.0, 384.0, 28.0),
            Hud => (400.0, 68.0, 34.0),
            Notify => (490.0, 92.0, 24.0),
            Silent => (300.0, 56.0, 28.0),
            Music => (620.0, 196.0, 28.0),
            Power => (420.0, 180.0, 28.0),
            Spin => (72.0, 72.0, 36.0),
            Boot => (150.0, 96.0, 48.0),
            Off => (14.0, 14.0, 7.0),
            Charge => (330.0, 68.0, 34.0),
        };
        Shape { w, h, r }
    }

    pub fn is_cc(self) -> bool {
        matches!(self, Self::Control | Self::CcWifi | Self::CcBt)
    }

    /// A solid dark bar instead of glass.
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Spin | Self::Boot | Self::Off)
    }

    /// The desktop behind is dimmed.
    pub fn dims_desktop(self) -> bool {
        self.is_cc() || matches!(self, Self::Music | Self::Power)
    }

    /// The windows fade out and take no input.
    pub fn hides_windows(self) -> bool {
        matches!(self, Self::Lock | Self::Launch | Self::Off | Self::Boot)
    }

    /// How long a transient state stays.
    pub fn transient_ms(self) -> Option<u32> {
        match self {
            Self::Hud => Some(HUD_MS),
            Self::Notify => Some(NOTIFY_MS),
            Self::Silent => Some(SILENT_MS),
            Self::Charge => Some(CHARGE_MS),
            _ => None,
        }
    }

    /// One level back (Esc): a subpage to the control centre, the rest to
    /// the dock. None where Esc does nothing.
    pub fn back(self) -> Option<BarState> {
        match self {
            Self::CcWifi | Self::CcBt => Some(Self::Control),
            Self::Control | Self::Music | Self::Power | Self::Hud | Self::Notify | Self::Silent | Self::Charge => Some(Self::Idle),
            _ => None,
        }
    }
}

/// The global scale: everything shrinks on small screens, never grows.
pub fn scale(sw: f32, sh: f32) -> f32 {
    1.0f32.min((sw - 32.0) / 880.0).min((sh - 60.0) / 640.0).max(0.2)
}

/// Where the lock screen's big clock and its date end.
pub fn clock_bottom(sh: f32) -> f32 {
    sh * 0.07 + 120.0f32.min(sh * 0.14) + 56.0
}

/// The scale and the distance from the bottom for state `s` of height `h`
/// (before scaling) on a screen of `sw` x `sh`.
pub fn placement(s: BarState, h: f32, sw: f32, sh: f32) -> (f32, f32) {
    let k = scale(sw, sh);
    match s {
        BarState::Lock => {
            let avail = sh - clock_bottom(sh) - 24.0;
            let k = k.min(avail / h).max(0.2);
            (k, 24.0f32.max((avail - h * k) / 2.0 + 24.0))
        }
        BarState::Off | BarState::Boot | BarState::Spin => (k, BOTTOM.max((sh - h * k) / 2.0)),
        _ => (k, BOTTOM),
    }
}

/// Where the bar is at one moment, on the screen (already scaled).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Geometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
    /// The scale (the content is drawn at it).
    pub k: f32,
}

/// One state's content as it fades: opacity 0..1 and scale.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewFade {
    pub state: BarState,
    pub alpha: f32,
    pub scale: f32,
}

pub struct Bar {
    pub state: BarState,
    /// The state before (its view fades out).
    pub prev: BarState,
    /// When the state last changed.
    pub changed_at: u64,
    deadline: Option<u64>,
    w: Tween,
    h: Tween,
    r: Tween,
    k: Tween,
    bottom: Tween,
    /// 0 shown, 1 hidden below the screen (full screen).
    hide: Tween,
    /// 0 glass, 1 dark (Spin, Boot, Off).
    dark: Tween,
    idle_w: f32,
    music: bool,
    sw: f32,
    sh: f32,
}

impl Bar {
    pub fn new(state: BarState, sw: f32, sh: f32) -> Bar {
        let mut b = Bar {
            state,
            prev: state,
            changed_at: 0,
            deadline: None,
            w: Tween::at(0.0, MORPH_MS, MORPH),
            h: Tween::at(0.0, MORPH_MS, MORPH),
            r: Tween::at(0.0, MORPH_MS, MORPH),
            k: Tween::at(1.0, SCALE_MS, EASE),
            bottom: Tween::at(BOTTOM, BOTTOM_MS, EASE),
            hide: Tween::at(0.0, SCALE_MS, EASE),
            dark: Tween::at(if state.is_dark() { 1.0 } else { 0.0 }, BG_MS, LINEAR),
            idle_w: 0.0,
            music: false,
            sw,
            sh,
        };
        b.retarget(0);
        b.settle();
        b
    }

    /// Everything where it is going, at once.
    pub fn settle(&mut self) {
        for t in [&mut self.w, &mut self.h, &mut self.r, &mut self.k, &mut self.bottom, &mut self.hide, &mut self.dark] {
            let v = t.target();
            t.jump(v);
        }
        self.changed_at = 0;
        self.prev = self.state;
    }

    /// State `s`'s shape here: the dock is as wide as its content needs.
    pub fn shape_of(&self, s: BarState) -> Shape {
        let mut sh = s.shape(self.music);
        if s == BarState::Idle {
            sh.w = sh.w.max(self.idle_w);
        }
        sh
    }

    fn retarget(&mut self, now: u64) {
        let s = self.shape_of(self.state);
        let (k, bottom) = placement(self.state, s.h, self.sw, self.sh);
        self.w.set(now, s.w);
        self.h.set(now, s.h);
        self.r.set(now, s.r.min(s.w / 2.0).min(s.h / 2.0));
        self.k.set(now, k);
        self.bottom.set(now, bottom);
        self.dark.set(now, if self.state.is_dark() { 1.0 } else { 0.0 });
    }

    /// Into state `s` (cancelling a transient's timer).
    pub fn go(&mut self, s: BarState, now: u64) {
        self.deadline = None;
        if s != self.state {
            self.prev = self.state;
            self.state = s;
            self.changed_at = now;
        }
        self.retarget(now);
    }

    /// A transient state for `ms`, then back to the dock; refused while the
    /// bar is busy with something the user is doing.
    pub fn transient(&mut self, s: BarState, ms: u32, now: u64) -> bool {
        use BarState::*;
        if matches!(self.state, Lock | Off | Boot | Spin | Power | Launch) {
            return false;
        }
        if self.state.is_cc() && s == Hud {
            return false; // the control centre has its own sliders
        }
        self.go(s, now);
        self.deadline = Some(now + ms as u64);
        true
    }

    /// When the transient ends (to wake up then).
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    /// Time passes: a transient that is over goes back to the dock. True if
    /// the state changed.
    pub fn tick(&mut self, now: u64) -> bool {
        match self.deadline {
            Some(d) if now >= d => {
                self.go(BarState::Idle, now);
                true
            }
            _ => false,
        }
    }

    pub fn set_screen(&mut self, sw: f32, sh: f32, now: u64) {
        self.sw = sw;
        self.sh = sh;
        self.retarget(now);
    }

    /// The dock's content needs `w` (it is never narrower than its shape).
    pub fn set_idle_width(&mut self, w: f32, now: u64) {
        if w != self.idle_w {
            self.idle_w = w;
            self.retarget(now);
        }
    }

    pub fn set_music(&mut self, on: bool, now: u64) {
        if on != self.music {
            self.music = on;
            self.retarget(now);
        }
    }

    pub fn music(&self) -> bool {
        self.music
    }

    /// Hidden below the screen (the dock over a full-screen window).
    pub fn set_hidden(&mut self, hidden: bool, now: u64) -> bool {
        self.hide.set(now, if hidden { 1.0 } else { 0.0 })
    }

    pub fn hidden(&self) -> bool {
        self.hide.target() == 1.0
    }

    /// 0 glass .. 1 solid dark.
    pub fn darkness(&self, now: u64) -> f32 {
        self.dark.value(now)
    }

    pub fn geometry(&self, now: u64) -> Geometry {
        let k = self.k.value(now);
        let (w, h) = (self.w.value(now).max(1.0) * k, self.h.value(now).max(1.0) * k);
        let r = (self.r.value(now).max(0.0) * k).min(w / 2.0).min(h / 2.0);
        let x = self.sw / 2.0 - w / 2.0;
        let y = self.sh - self.bottom.value(now) - h + self.hide.value(now) * (h + 48.0);
        Geometry { x, y, w, h, r, k }
    }

    /// The shape and place the bar is going to.
    pub fn final_geometry(&self) -> Geometry {
        self.geometry(u64::MAX / 2)
    }

    /// The views: the one coming in, and the one going out while it fades.
    pub fn views(&self, now: u64) -> (ViewFade, Option<ViewFade>) {
        let t = now.saturating_sub(self.changed_at);
        let fade = |ms: u64, d: u32| (ms as f32 / d as f32).min(1.0);
        let out = if self.prev != self.state && t < VIEW_SCALE_MS as u64 {
            let a = 1.0 - fade(t, OUT_FADE_MS);
            let s = 1.0 - 0.03 * EASE.eval(fade(t, VIEW_SCALE_MS));
            Some(ViewFade { state: self.prev, alpha: a, scale: s })
        } else {
            None
        };
        let inn = if self.prev == self.state || self.changed_at == 0 {
            ViewFade { state: self.state, alpha: 1.0, scale: 1.0 }
        } else {
            let ti = t.saturating_sub(IN_DELAY_MS as u64);
            let a = if t < IN_DELAY_MS as u64 { 0.0 } else { fade(ti, OUT_FADE_MS) };
            let s = 0.97 + 0.03 * EASE.eval(fade(ti, VIEW_SCALE_MS));
            ViewFade { state: self.state, alpha: a, scale: s }
        };
        (inn, out)
    }

    /// Something still moves (to keep drawing frames).
    pub fn moving(&self, now: u64) -> bool {
        let views = self.changed_at != 0 && now < self.changed_at + (IN_DELAY_MS + VIEW_SCALE_MS) as u64;
        views || [&self.w, &self.h, &self.r, &self.k, &self.bottom, &self.hide, &self.dark].iter().any(|t| t.moving(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabs;
    use BarState::*;

    const ALL: [BarState; 15] = [Idle, Launch, Lock, Control, CcWifi, CcBt, Hud, Notify, Silent, Music, Power, Spin, Boot, Off, Charge];

    #[test]
    fn shapes_match_the_table() {
        let t = |s: BarState| {
            let x = s.shape(false);
            (x.w, x.h, x.r)
        };
        assert_eq!(t(Idle), (690.0, 68.0, 20.0));
        assert_eq!(Idle.shape(true).w, 760.0);
        assert_eq!(t(Launch), (540.0, 60.0, 30.0));
        assert_eq!(t(Lock), (340.0, 540.0, 32.0));
        assert_eq!(t(Control), (712.0, 316.0, 28.0));
        assert_eq!(t(CcWifi), (440.0, 384.0, 28.0));
        assert_eq!(t(CcBt), (440.0, 384.0, 28.0));
        assert_eq!(t(Hud), (400.0, 68.0, 34.0));
        assert_eq!(t(Notify), (490.0, 92.0, 24.0));
        assert_eq!(t(Silent), (300.0, 56.0, 28.0));
        assert_eq!(t(Music), (620.0, 196.0, 28.0));
        assert_eq!(t(Power), (420.0, 180.0, 28.0));
        assert_eq!(t(Spin), (72.0, 72.0, 36.0));
        assert_eq!(t(Boot), (150.0, 96.0, 48.0));
        assert_eq!(t(Off), (14.0, 14.0, 7.0));
        assert_eq!(t(Charge), (330.0, 68.0, 34.0));
    }

    #[test]
    fn kinds_of_state() {
        assert!(Control.is_cc() && CcWifi.is_cc() && CcBt.is_cc() && !Idle.is_cc());
        assert!(Spin.is_dark() && Boot.is_dark() && Off.is_dark() && !Lock.is_dark());
        assert!(Music.dims_desktop() && Power.dims_desktop() && Control.dims_desktop() && !Notify.dims_desktop());
        assert_eq!(CcBt.back(), Some(Control));
        assert_eq!(Control.back(), Some(Idle));
        assert_eq!(Lock.back(), None);
    }

    #[test]
    fn transient_refused_while_busy() {
        for s in [Lock, Off, Boot, Spin, Power, Launch] {
            let mut b = Bar::new(s, 1440.0, 1024.0);
            assert!(!b.transient(Notify, NOTIFY_MS, 10));
            assert!(!b.transient(Hud, HUD_MS, 10));
            assert_eq!(b.state, s);
        }
    }

    #[test]
    fn hud_refused_in_control_centre_but_notify_shown() {
        for s in [Control, CcWifi, CcBt] {
            let mut b = Bar::new(s, 1440.0, 1024.0);
            assert!(!b.transient(Hud, HUD_MS, 10));
            assert_eq!(b.state, s);
            assert!(b.transient(Notify, NOTIFY_MS, 10));
            assert_eq!(b.state, Notify);
        }
    }

    #[test]
    fn transient_goes_back_after_its_time() {
        for (s, ms) in [(Hud, HUD_MS), (Notify, NOTIFY_MS), (Silent, SILENT_MS), (Charge, CHARGE_MS)] {
            assert_eq!(s.transient_ms(), Some(ms));
            let mut b = Bar::new(Idle, 1440.0, 1024.0);
            assert!(b.transient(s, ms, 1000));
            assert_eq!(b.deadline(), Some(1000 + ms as u64));
            assert!(!b.tick(1000 + ms as u64 - 1));
            assert_eq!(b.state, s);
            assert!(b.tick(1000 + ms as u64));
            assert_eq!(b.state, Idle);
            assert_eq!(b.deadline(), None);
        }
    }

    #[test]
    fn go_cancels_the_timer() {
        let mut b = Bar::new(Idle, 1440.0, 1024.0);
        b.transient(Notify, NOTIFY_MS, 0);
        b.go(Control, 100);
        assert!(!b.tick(10_000));
        assert_eq!(b.state, Control);
    }

    #[test]
    fn interrupted_morph_starts_from_current_shape() {
        let mut b = Bar::new(Idle, 1440.0, 1024.0);
        b.go(Control, 1000);
        let mid = b.geometry(1200);
        assert!(mid.w > 690.0 && mid.h > 68.0 && mid.h < 316.0);
        b.go(Notify, 1200);
        let again = b.geometry(1200);
        assert!(fabs(again.w - mid.w) < 0.01 && fabs(again.h - mid.h) < 0.01 && fabs(again.r - mid.r) < 0.01);
        let end = b.geometry(1200 + 2000);
        assert_eq!((end.w, end.h, end.r), (490.0, 92.0, 24.0));
    }

    #[test]
    fn placed_at_the_bottom_centre() {
        let b = Bar::new(Idle, 1440.0, 1024.0);
        let g = b.geometry(0);
        assert_eq!((g.x, g.y, g.w, g.h, g.k), (375.0, 1024.0 - 28.0 - 68.0, 690.0, 68.0, 1.0));
    }

    #[test]
    fn scale_shrinks_on_small_screens_only() {
        assert_eq!(scale(1920.0, 1080.0), 1.0);
        assert_eq!(scale(1024.0, 768.0), 1.0);
        assert!(fabs(scale(800.0, 600.0) - (540.0 / 640.0)) < 1e-4);
        assert!(fabs(scale(800.0, 900.0) - (768.0 / 880.0)) < 1e-4);
        assert!(fabs(scale(1400.0, 500.0) - (440.0 / 640.0)) < 1e-4);
    }

    #[test]
    fn lock_sits_under_the_clock() {
        let (sw, sh) = (1440.0, 1024.0);
        let (k, bottom) = placement(Lock, 540.0, sw, sh);
        let cb = clock_bottom(sh);
        let avail = sh - cb - 24.0;
        assert_eq!(k, 1.0f32.min(avail / 540.0));
        assert!(sh - bottom - 540.0 * k >= cb - 0.01, "the keypad does not cover the clock");
        let (k, _) = placement(Lock, 540.0, 1024.0, 600.0);
        assert!(k < 1.0);
    }

    #[test]
    fn dark_states_are_centred() {
        for s in [Off, Boot, Spin] {
            let mut b = Bar::new(Idle, 1440.0, 1024.0);
            b.go(s, 0);
            let g = b.geometry(5000);
            assert!(fabs((g.y + g.h / 2.0) - 512.0) < 0.5, "{s:?} centred");
            assert_eq!(b.darkness(5000), 1.0);
        }
    }

    #[test]
    fn radius_never_beyond_half_the_side() {
        for s in ALL {
            let mut b = Bar::new(Idle, 1440.0, 1024.0);
            b.go(s, 0);
            for t in (0..800).step_by(20) {
                let g = b.geometry(t);
                assert!(g.r <= g.w / 2.0 + 1e-3 && g.r <= g.h / 2.0 + 1e-3);
            }
        }
    }

    #[test]
    fn idle_grows_with_its_content_and_music() {
        let mut b = Bar::new(Idle, 1440.0, 1024.0);
        b.set_idle_width(720.0, 0);
        assert_eq!(b.final_geometry().w, 720.0);
        b.set_music(true, 0);
        assert_eq!(b.final_geometry().w, 760.0);
    }

    #[test]
    fn hidden_slides_below_the_screen() {
        let mut b = Bar::new(Idle, 1440.0, 1024.0);
        b.set_hidden(true, 0);
        let g = b.geometry(5000);
        assert!(g.y >= 1024.0);
        assert!(b.hidden());
    }

    #[test]
    fn views_cross_fade() {
        let mut b = Bar::new(Idle, 1440.0, 1024.0);
        b.go(Control, 1000);
        let (inn, out) = b.views(1000);
        assert_eq!(inn.alpha, 0.0);
        assert_eq!(out.unwrap().alpha, 1.0);
        let (inn, out) = b.views(1000 + 80 + 100);
        assert!(inn.alpha > 0.4 && inn.alpha < 0.6);
        assert!(out.unwrap().alpha < 0.2);
        assert_eq!(b.views(1000 + 200).1.unwrap().alpha, 0.0);
        let (inn, out) = b.views(1000 + 600);
        assert_eq!((inn.alpha, inn.scale), (1.0, 1.0));
        assert!(out.is_none());
        assert!(b.moving(1100) && !b.moving(1000 + 700));
    }
}
