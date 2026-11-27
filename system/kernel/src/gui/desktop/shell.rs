//! The dynamic bar (docs/design/dynamic-bar): one surface at the bottom of
//! the primary monitor. At rest it is the dock (the justOS mark, the apps,
//! the song playing, the status with the battery round it); in place it
//! becomes the launchpad's search, the lock screen's keypad, the control
//! centre with its Wi-Fi and Bluetooth pages, the volume and the
//! brightness, a notification, the music player, the power questions and
//! the charge. Its states, shapes and timers are `justshell::bar`'s; this
//! draws them and routes the input to them.
//!
//! Each state has a view of its own size, anchored at the bar's bottom
//! middle and clipped by the bar while it changes shape; the view going out
//! fades, the one coming in fades in and grows from 97% (drawn into a layer
//! then, at rest straight onto the screen).

use super::*;
use crate::gui::icons;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU32, Ordering};
use justshell::anim::{Tween, EASE, LINEAR};
use justshell::bar::{self as jbar, Bar, BarState, Geometry, ViewFade, CHARGE_MS, HUD_MS, NOTIFY_MS, SILENT_MS};
use justshell::launchpad;
use justshell::round;
use justshell::wm::Fullscreen;
use justwifi::frame::Security;
use justwifi::sta::State as WifiState;

// The design's colours: text at full, .62 and .42 white; fills of .07 and
// .11 white; the brand violet, green, red.
const T2: u32 = 0x9EFF_FFFF;
const T3: u32 = 0x6BFF_FFFF;
const FILL: u32 = 0x12FF_FFFF;
const FILL_H: u32 = 0x1CFF_FFFF;
const LINE: u32 = 0x14FF_FFFF;
const BRAND: u32 = 0xFF8C_52FF;
const GREEN: u32 = 0xFF34_C759;
const RED: u32 = 0xFFFF_453A;
const WHITE: u32 = 0xFFFF_FFFF;
const INK: u32 = 0xFF15_151A;
/// rgba(18,18,22,.62) over the blurred wallpaper.
const GLASS: u32 = 0x9E12_1216;
/// Over a window (which the blurred wallpaper does not hold): the opaque
/// fallback, rgba(18,18,22,.85).
const GLASS_SOLID: u32 = 0xD912_1216;
/// The music player: the accent at 20% over rgba(16,16,20,.8).
const GLASS_MUSIC: u32 = 0xD629_1D43;
const DARK: u32 = 0xFF0B_0B0F;
/// The desktop under the control centre, the player, the power question.
const DIM: u32 = 0x3800_0000;
/// The launchpad's veil over the blurred wallpaper.
const LP_VEIL: u32 = 0x4D0C_0C10;
const VOLUME_STEP: u32 = 6;
/// The justOS mark on the bar's first button (the "J" and the orange
/// cursor of the wordmark, 9 x 7 cells of 3 units) and the wordmark at
/// boot: justOS keeps its own logo where the design has another one.
const MARK_H: i32 = 21;
const MARK_W: i32 = 27;
const WORDMARK_H: i32 = 21;
/// The volume before muting, to unmute to.
static UNMUTE: AtomicU32 = AtomicU32::new(50);

const PIN_OK_MS: u64 = 600;
const PIN_BAD_MS: u64 = 550;
const POWER_OFF_MS: u64 = 1600;
const RESTART_MS: u64 = 1500;
/// How long the dark screen stays before the machine goes.
const LAST_MS: u64 = 800;
/// The pairs of colours of a song's picture (chosen by its title).
const ARTS: [(u32, u32); 6] = [
    (0xFFFF_7A59, 0xFF8C_52FF),
    (0xFF34_C7A9, 0xFF2B_59C3),
    (0xFFFF_C247, 0xFFFF_5A1F),
    (0xFFC7_A8FF, 0xFF3E_1C9C),
    (0xFF5A_C8FA, 0xFF34_C759),
    (0xFFFF_6B9A, 0xFF6D_3FE0),
];

fn ms() -> u64 {
    time::uptime_ms()
}

fn at(o: (i32, i32), r: Rect) -> Rect {
    r.offset(o.0, o.1)
}

/// `c` with its alpha times `a`/255.
fn fade(c: u32, a: u32) -> u32 {
    ((((c >> 24) * a.min(255)) / 255) << 24) | (c & 0xFF_FFFF)
}

fn mmss(s: u32) -> String {
    format!("{}:{:02}", s / 60, s % 60)
}

/// "AR" for "Alex Rossi".
fn initials(name: &str) -> String {
    let mut s: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    if s.is_empty() {
        s.push('?');
    }
    s.to_uppercase()
}

fn art_of(title: &str) -> (u32, u32) {
    let h = title.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    ARTS[h as usize % ARTS.len()]
}

fn vol_icon(v: u32) -> &'static str {
    match v {
        0 => "vol-mute",
        1..=49 => "vol-low",
        _ => "vol",
    }
}

/// The Wi-Fi symbol for the connection, and whether there is one.
fn wifi_icon() -> (&'static str, bool) {
    match crate::net::wifi::status() {
        Some(s) if s.state == WifiState::Connected => {
            let icon = if s.rssi > -60 {
                "wifi"
            } else if s.rssi > -72 {
                "wifi-mid"
            } else {
                "wifi-low"
            };
            (icon, true)
        }
        _ => ("wifi", false),
    }
}

/// Which slider a drag moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Level {
    Volume,
    Brightness,
}

/// What can be pressed on the bar (in its view's units) and round it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Part {
    Logo,
    App(usize),
    NowPlaying,
    Status,
    Key(u8),
    KeyBack,
    Field,
    Wifi,
    WifiMore,
    Bt,
    BtMore,
    Quiet,
    Volume,
    Brightness,
    Art,
    Play,
    Next,
    Lock,
    Restart,
    PowerOff,
    Battery,
    Account,
    Gear,
    Back,
    Switch,
    Row(usize),
    More,
    Hud,
    Note,
    Shuffle,
    Prev,
    MPlay,
    MNext,
    Repeat,
    Seek,
    Cancel,
    Confirm,
    Cell(usize),
    Found(usize),
    Dot(usize),
}

pub(super) struct Note {
    title: String,
    body: String,
    icon: String,
    /// The app it is from ("justOS" when it is not an app's).
    app: String,
}

/// The song loaded in a window (see [`wm::Live::Music`]).
#[derive(Clone, PartialEq)]
pub(super) struct Song {
    id: u32,
    title: String,
    album: String,
    pos: u32,
    len: u32,
    playing: bool,
    shuffle: bool,
    repeat: bool,
}

/// The dock at rest: where its parts are, in the view's units.
pub(super) struct IdleLayout {
    logo: Rect,
    apps: Vec<Rect>,
    np: Option<Rect>,
    status: Rect,
    icons: Vec<(&'static str, i32, u32)>,
    /// What the content needs (the bar is never narrower than its shape).
    need: i32,
}

pub(super) struct Shell {
    pub bar: Bar,
    /// The dim over the desktop, the launchpad's veil, the dark screen of
    /// the power flow (0..1).
    dim: Tween,
    lp: Tween,
    black: Tween,
    /// Where the bar (and its head) was painted since its area was last
    /// damaged.
    painted: Cell<Rect>,
    /// Something moved on the last frame: one more, to paint where it
    /// stopped (a slow frame may have skipped the end).
    busy: bool,
    queue: VecDeque<Note>,
    note: Option<Note>,
    level: Level,
    /// The volume and the brightness last seen.
    pub levels: (u32, u32),
    pub battery: Option<(u8, bool)>,
    battery_at: u64,
    /// The computer is locked: the bar is the keypad, the windows hide.
    pub locked: bool,
    /// The digits of the PIN (0: a password), the account's name.
    pin_len: u32,
    name: String,
    typed: String,
    wrong_at: u64,
    ok_at: u64,
    // The launchpad: the query, the apps, the page, the files found.
    query: String,
    entries: Vec<apps::Entry>,
    page: usize,
    found: Vec<apps::search::Found>,
    index: apps::search::Index,
    wheel_at: u64,
    pub fs: Fullscreen,
    hover: Option<Part>,
    song: Option<Song>,
    /// A seek being dragged (thousandths).
    seek: Option<u32>,
    nets: Vec<crate::net::wifi::Network>,
    devs: Vec<crate::bt::DeviceView>,
    lists_at: u64,
    /// The network or device being joined, and since when.
    joining: Option<(String, u64)>,
    /// Powering off or restarting, since when.
    pub ending: Option<(Ending, u64)>,
    /// Wine's preparation: put away by the user, last seen failed, and
    /// its bar as drawn (gliding to the target) with when.
    prep_hidden: bool,
    prep_failed: bool,
    prep_shown: Cell<(u32, u64)>,
}

impl Shell {
    pub fn new(locked: bool, w: i32, h: i32) -> Shell {
        let state = if locked { BarState::Lock } else { BarState::Idle };
        let s = settings::get();
        Shell {
            bar: Bar::new(state, w as f32, h as f32),
            dim: Tween::at(0.0, 300, EASE),
            lp: Tween::at(0.0, 280, EASE),
            black: Tween::at(0.0, 500, LINEAR),
            painted: Cell::new(Rect::EMPTY),
            busy: false,
            queue: VecDeque::new(),
            note: None,
            level: Level::Volume,
            levels: (crate::dev::audio::volume(), crate::gui::brightness()),
            battery: crate::acpi::power::battery(),
            battery_at: 0,
            locked,
            pin_len: s.pin,
            name: s.name.clone(),
            typed: String::new(),
            wrong_at: 0,
            ok_at: 0,
            query: String::new(),
            entries: Vec::new(),
            page: 0,
            found: Vec::new(),
            index: Default::default(),
            wheel_at: 0,
            fs: Fullscreen::default(),
            hover: None,
            song: None,
            seek: None,
            nets: Vec::new(),
            devs: Vec::new(),
            lists_at: 0,
            joining: None,
            prep_hidden: false,
            prep_failed: false,
            prep_shown: Cell::new((0, 0)),
            ending: None,
        }
    }
}

impl Desktop {
    // ------------------------------------------------------------------
    // Geometry

    /// The bar's scale at rest on the primary monitor.
    fn bar_scale(&self) -> f32 {
        let m = self.main_mon();
        jbar::scale(m.w as f32, m.h as f32)
    }

    /// The bar at geometry `g`, on the screen.
    fn geom_rect(&self, g: Geometry) -> Rect {
        let m = self.main_mon();
        let (x, y) = (round(g.x), round(g.y));
        Rect::new(m.x + x, m.y + y, (round(g.x + g.w) - x).max(1), (round(g.y + g.h) - y).max(1))
    }

    /// The bar where it is going.
    pub(super) fn bar_rest(&self) -> Rect {
        self.geom_rect(self.sh.bar.final_geometry())
    }

    fn view_size(&self, s: BarState) -> (i32, i32) {
        let shp = self.sh.bar.shape_of(s);
        (round(shp.w), round(shp.h))
    }

    /// Where state `s`'s view goes when the bar is at `g`, grown by
    /// `scale`: its top left corner and its scale.
    fn view_place(&self, s: BarState, g: Geometry, scale: f32) -> (f32, f32, f32) {
        let m = self.main_mon();
        let (vw, vh) = self.view_size(s);
        let k = g.k * scale;
        let cx = m.x as f32 + g.x + g.w / 2.0;
        let bottom = m.y as f32 + g.y + g.h;
        (cx - vw as f32 * k / 2.0, bottom - vh as f32 * k, k)
    }

    /// The head over the control centre (the account, the settings): its
    /// top left corner, its size, its scale.
    fn head_place(&self, s: BarState, g: Geometry) -> (f32, f32, i32, f32) {
        let m = self.main_mon();
        let (vw, _) = self.view_size(s);
        let cx = m.x as f32 + g.x + g.w / 2.0;
        (cx - vw as f32 * g.k / 2.0, m.y as f32 + g.y - 70.0 * g.k, vw, g.k)
    }

    /// The items of the dock at rest (the mark opens the launchpad: the
    /// launcher's own icon is not shown).
    pub(super) fn bar_items(&self) -> Vec<DockItem> {
        self.dock_items().into_iter().filter(|d| d.id != "launcher").collect()
    }

    fn status_icons(&self) -> Vec<(&'static str, i32, u32)> {
        let mut v = Vec::new();
        if self.sh.battery.is_some_and(|b| b.1) {
            v.push(("bolt", 14, GREEN));
        }
        if self.quiet {
            v.push(("moon", 17, WHITE));
        }
        let (wi, on) = wifi_icon();
        v.push((wi, 19, if on { WHITE } else { fade(WHITE, 77) }));
        v.push((vol_icon(self.sh.levels.0), 19, WHITE));
        v
    }

    pub(super) fn idle_layout(&self) -> IdleLayout {
        let items = self.bar_items();
        let logo_w = 16 + MARK_W + 16;
        let icons = self.status_icons();
        let status_w = 42 + icons.iter().map(|i| i.1).sum::<i32>() + 12 * (icons.len() as i32 - 1);
        let music = self.sh.song.is_some();
        let np_w = 5 + 32 + 10 + 18 + 14;
        let fixed = 12 + logo_w + 16 + if music { 16 + np_w } else { 0 } + 10 + status_w + 12;
        let m = self.main_mon();
        let room = ((m.w - 32) as f32 / self.bar_scale()) as i32;
        let n = items.len().min(((room - fixed + 8) / 50).max(0) as usize);
        let apps_w = if n > 0 { n as i32 * 50 - 8 } else { 0 };
        let need = fixed + apps_w;
        let w = need.max(round(BarState::Idle.shape(music).w));
        let logo = Rect::new(12, 13, logo_w, 42);
        let x0 = logo.right() + 16;
        let apps = (0..n).map(|i| Rect::new(x0 + i as i32 * 50, 13, 42, 42)).collect();
        let np = music.then(|| Rect::new(x0 + apps_w + 16, 13, np_w, 42));
        let status = Rect::new(w - 12 - status_w, 11, status_w, 46);
        IdleLayout { logo, apps, np, status, icons, need }
    }

    /// The dock icon of window kind `kind` at rest, on the screen (the
    /// middle of the dock for one it does not show): where a window
    /// shrinks to.
    pub(super) fn dock_icon_of(&self, kind: &str) -> Rect {
        let m = self.main_mon();
        let k = self.bar_scale();
        let lay = self.idle_layout();
        let w = lay.need.max(round(BarState::Idle.shape(self.sh.song.is_some()).w)) as f32 * k;
        let (x, y) = (m.x as f32 + (m.w as f32 - w) / 2.0, m.y as f32 + m.h as f32 - jbar::BOTTOM - jbar::IDLE_H * k);
        let items = self.bar_items();
        let r = items.iter().position(|d| d.kind == kind).and_then(|i| lay.apps.get(i).copied()).unwrap_or(Rect::new(round(w / k) / 2 - 21, 13, 42, 42));
        Rect::new(round(x + r.x as f32 * k), round(y + r.y as f32 * k), round(r.w as f32 * k), round(r.h as f32 * k))
    }

    /// Dock icon `i` at rest, on the screen.
    pub(super) fn bar_app_rect(&self, i: usize) -> Rect {
        let items = self.bar_items();
        items.get(i).map_or(self.bar_rest(), |d| self.dock_icon_of(&d.kind.clone()))
    }

    /// What the bar may paint now and where it goes: its damage.
    pub(super) fn dock_zone(&self) -> Rect {
        let b = &self.sh.bar;
        let cur = self.geom_rect(b.geometry(ms()));
        let r = cur.union(&self.bar_rest()).union(&self.sh.painted.get());
        Rect::new(r.x - 48, r.y - 110, r.w + 96, r.h + 180)
    }

    pub(super) fn damage_bar(&self, l: &mut Looper) {
        l.damage(self.dock_zone());
        self.sh.painted.set(Rect::EMPTY);
    }

    // ------------------------------------------------------------------
    // The parts of each view

    fn parts(&self, s: BarState) -> Vec<(Part, Rect)> {
        let mut v = Vec::new();
        match s {
            BarState::Idle => {
                let lay = self.idle_layout();
                v.push((Part::Logo, lay.logo));
                for (i, r) in lay.apps.iter().enumerate() {
                    v.push((Part::App(i), r.grow(4)));
                }
                if let Some(np) = lay.np {
                    v.push((Part::NowPlaying, np));
                }
                v.push((Part::Status, lay.status));
            }
            BarState::Lock => {
                if self.sh.pin_len > 0 {
                    for (i, key) in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 11].iter().enumerate() {
                        let r = Rect::new(55 + (i % 3) as i32 * 84, 234 + (i / 3) as i32 * 74, 62, 62);
                        match key {
                            0..=9 => v.push((Part::Key(*key), r)),
                            11 => v.push((Part::KeyBack, r)),
                            _ => {}
                        }
                    }
                } else {
                    v.push((Part::Field, Rect::new(40, 262, 260, 46)));
                }
            }
            BarState::Control => {
                v.push((Part::WifiMore, Rect::new(12 + 236 - 10 - 30, 12 + 31, 30, 30)));
                v.push((Part::Wifi, Rect::new(12, 12, 236, 92)));
                v.push((Part::BtMore, Rect::new(12 + 236 - 10 - 30, 112 + 31, 30, 30)));
                v.push((Part::Bt, Rect::new(12, 112, 236, 92)));
                v.push((Part::Quiet, Rect::new(12, 212, 236, 92)));
                v.push((Part::Volume, Rect::new(256, 12, 96, 292)));
                v.push((Part::Brightness, Rect::new(360, 12, 96, 292)));
                v.push((Part::Play, Rect::new(464 + 236 - 12 - 32 - 6 - 32, 42, 32, 32)));
                v.push((Part::Next, Rect::new(464 + 236 - 12 - 32, 42, 32, 32)));
                v.push((Part::Art, Rect::new(464, 12, 236, 92)));
                for (i, p) in [Part::Lock, Part::Restart, Part::PowerOff].iter().enumerate() {
                    v.push((*p, Rect::new(464 + i as i32 * 81, 112, if i == 2 { 74 } else { 73 }, 92)));
                }
                v.push((Part::Battery, Rect::new(464, 212, 236, 92)));
            }
            BarState::CcWifi | BarState::CcBt => {
                v.push((Part::Back, Rect::new(12, 16, 36, 36)));
                v.push((Part::Switch, Rect::new(440 - 12 - 46, 20, 46, 28)));
                let n = if s == BarState::CcWifi { self.sh.nets.len() } else { self.sh.devs.len() };
                for i in 0..n.min(4) {
                    v.push((Part::Row(i), Rect::new(12, 84 + i as i32 * 56, 416, 54)));
                }
                v.push((Part::More, Rect::new(12, 384 - 12 - 48, 416, 48)));
            }
            BarState::Hud => v.push((Part::Hud, Rect::new(52, 10, 278, 48))),
            BarState::Notify => v.push((Part::Note, Rect::new(0, 0, 490, 92))),
            BarState::Music => {
                v.push((Part::Seek, Rect::new(238, 100, 324, 22)));
                v.push((Part::Shuffle, Rect::new(267, 134, 34, 34)));
                v.push((Part::Prev, Rect::new(315, 131, 40, 40)));
                v.push((Part::MPlay, Rect::new(369, 128, 62, 46)));
                v.push((Part::MNext, Rect::new(445, 131, 40, 40)));
                v.push((Part::Repeat, Rect::new(499, 134, 34, 34)));
            }
            BarState::Power => {
                v.push((Part::Cancel, Rect::new(24, 110, 181, 50)));
                v.push((Part::Confirm, Rect::new(215, 110, 181, 50)));
            }
            BarState::Prep => {
                for (p, r) in Self::prep_buttons(crate::gui::apps::winprep::view().is_some_and(|v| v.failed)) {
                    v.push((p, r));
                }
            }
            _ => {}
        }
        v
    }

    /// The head's parts (in its own units: the chip and the gear).
    fn head_parts(&self, w: i32) -> [(Part, Rect); 2] {
        let name = self.account_name();
        let tw = text::width(Font::Strong16, &name).max(text::width(Font::Small, "Account di questo computer"));
        [(Part::Account, Rect::new(0, 0, 6 + 44 + 12 + tw + 20, 56)), (Part::Gear, Rect::new(w - 56, 0, 56, 56))]
    }

    fn account_name(&self) -> String {
        let n = String::from(self.sh.name.trim());
        if n.is_empty() {
            String::from("Utente")
        } else {
            n
        }
    }

    /// A part's rectangle on the screen, at rest.
    fn part_rect(&self, p: Part) -> Option<Rect> {
        let s = self.sh.bar.state;
        let g = self.sh.bar.final_geometry();
        let (ox, oy, k) = self.view_place(s, g, 1.0);
        let r = self.parts(s).into_iter().find(|(q, _)| *q == p)?.1;
        Some(Rect::new(round(ox + r.x as f32 * k), round(oy + r.y as f32 * k), round(r.w as f32 * k), round(r.h as f32 * k)))
    }

    /// The part of the bar (or of the launchpad, or of the head) under
    /// (`x`, `y`).
    pub(super) fn part_at(&self, x: i32, y: i32) -> Option<Part> {
        let b = &self.sh.bar;
        let s = b.state;
        let g = b.final_geometry();
        if self.bar_hidden() {
            return None;
        }
        let bar = self.geom_rect(g);
        if s == BarState::Launch && !bar.contains(x, y) {
            return self.launch_part(x, y);
        }
        if s.is_cc() {
            let (hx, hy, hw, k) = self.head_place(s, g);
            let (lx, ly) = (((x as f32 - hx) / k) as i32, ((y as f32 - hy) / k) as i32);
            if let Some((p, _)) = self.head_parts(hw).into_iter().find(|(_, r)| r.contains(lx, ly)) {
                return Some(p);
            }
        }
        if !bar.contains(x, y) {
            return None;
        }
        let (ox, oy, k) = self.view_place(s, g, 1.0);
        let (lx, ly) = (((x as f32 - ox) / k) as i32, ((y as f32 - oy) / k) as i32);
        self.parts(s).into_iter().find(|(_, r)| r.contains(lx, ly)).map(|(p, _)| p)
    }

    /// The bar is down below the screen (over a full-screen window).
    pub(super) fn bar_hidden(&self) -> bool {
        self.sh.bar.hidden()
    }

    // ------------------------------------------------------------------
    // States

    pub(super) fn go(&mut self, l: &mut Looper, s: BarState) {
        let now = ms();
        self.damage_bar(l);
        let was = self.sh.bar.state;
        self.sh.bar.go(s, now);
        self.after_go(l, was, now);
    }

    /// A transient state (see [`Bar::transient`]); whether it showed.
    fn transient(&mut self, l: &mut Looper, s: BarState, ms_: u32) -> bool {
        let now = ms();
        let was = self.sh.bar.state;
        self.damage_bar(l);
        let shown = self.sh.bar.transient(s, ms_, now);
        if shown {
            self.after_go(l, was, now);
        }
        shown
    }

    fn after_go(&mut self, l: &mut Looper, was: BarState, now: u64) {
        let s = self.sh.bar.state;
        if self.sh.dim.set(now, if s.dims_desktop() { 1.0 } else { 0.0 }) {
            l.damage(self.screen());
        }
        if self.sh.lp.set(now, if s == BarState::Launch { 1.0 } else { 0.0 }) {
            l.damage(self.screen());
        }
        if s.hides_windows() != was.hides_windows() || s.is_cc() != was.is_cc() {
            l.damage(self.screen());
        }
        self.sh.hover = None;
        if s == BarState::Idle {
            self.next_note(l);
        }
        self.sync_hidden(l);
        self.damage_bar(l);
    }

    /// The bar goes down over a full-screen window (and comes back for a
    /// peek, or when it has something to show).
    pub(super) fn sync_hidden(&mut self, l: &mut Looper) {
        let id = if self.full_screen() { self.focused() } else { None };
        if id != self.sh.fs.window {
            match id {
                Some(id) => {
                    self.sh.fs.enter(id);
                }
                None => self.sh.fs.exit(),
            }
        }
        let idle = self.sh.bar.state == BarState::Idle;
        let hidden = !self.sh.locked && self.sh.fs.bar_hidden(idle);
        if self.sh.bar.set_hidden(hidden, ms()) {
            self.damage_bar(l);
        }
    }

    /// The dock at its width at once (at the start).
    pub(super) fn sync_idle_now(&mut self) {
        let need = self.idle_layout().need as f32;
        self.sh.bar.set_idle_width(need, ms());
        self.sh.bar.settle();
    }

    /// A right click: true if the bar took it (an app's icon has its menu,
    /// the mark the system's).
    pub(super) fn shell_menu(&mut self, l: &mut Looper, x: i32, y: i32) -> bool {
        if self.sh.locked || self.sh.ending.is_some() {
            return true;
        }
        match self.part_at(x, y) {
            Some(Part::App(i)) => {
                let items = self.bar_items();
                if i < items.len() {
                    self.dock_menu(l, &items, i);
                }
                true
            }
            Some(Part::Logo) => {
                self.system_menu(l);
                true
            }
            Some(_) => true,
            None => self.sh.bar.state == BarState::Launch || self.bar_rest().contains(x, y),
        }
    }

    /// The system's menu, over the mark.
    fn system_menu(&mut self, l: &mut Looper) {
        let mut items = alloc::vec![
            ("Info su justOS", Some(MenuAct::About)),
            ("", None),
            ("Impostazioni di sistema…", Some(MenuAct::Settings)),
            ("Negozio…", Some(MenuAct::Store)),
            ("Monitor attività", Some(MenuAct::Activity)),
            ("Desktop e finestre (F3)", Some(MenuAct::Overview)),
            ("Schermata", Some(MenuAct::Screenshot)),
            ("", None),
            ("Blocca schermo", Some(MenuAct::Lock)),
        ];
        if crate::sleep::available().is_ok() {
            items.push(("Sospendi", Some(MenuAct::Sleep)));
        }
        items.push(("Riavvia…", Some(MenuAct::Restart)));
        items.push(("Spegni…", Some(MenuAct::PowerOff)));
        let logo = self.part_rect(Part::Logo).unwrap_or(self.bar_rest());
        let p = Popup::menu(logo.x, 0, items, self.main_mon()).above(self.bar_rest().y, 12);
        self.show_popup(l, p);
    }

    /// The dock icon under carried files (lit up), if any.
    pub(super) fn carry_hover(&mut self, l: &mut Looper, x: i32, y: i32) -> Option<usize> {
        let p = if self.sh.bar.state == BarState::Idle { self.part_at(x, y) } else { None };
        if p != self.sh.hover {
            self.sh.hover = p;
            self.damage_bar(l);
        }
        match p {
            Some(Part::App(i)) => Some(i),
            _ => None,
        }
    }

    /// The dock's width follows its content.
    pub(super) fn sync_idle(&mut self, l: &mut Looper) {
        let need = self.idle_layout().need as f32;
        let before = self.sh.bar.final_geometry();
        self.sh.bar.set_idle_width(need, ms());
        if self.sh.bar.final_geometry() != before {
            self.damage_bar(l);
        }
    }

    /// Opens the launchpad, or closes it.
    pub(super) fn toggle_launcher(&mut self, l: &mut Looper) {
        if self.sh.locked || self.sh.ending.is_some() {
            return;
        }
        if self.sh.bar.state == BarState::Launch {
            self.go(l, BarState::Idle);
            return;
        }
        self.close_popup(l);
        self.sh.query.clear();
        self.sh.entries = apps::catalog();
        self.sh.page = 0;
        self.sh.found.clear();
        self.sh.index = Default::default();
        self.go(l, BarState::Launch);
    }

    /// Locks the computer: the bar becomes the keypad over the blurred
    /// wallpaper.
    pub(super) fn lock_screen(&mut self, l: &mut Looper) {
        if self.sh.locked {
            return;
        }
        self.close_popup(l);
        self.drag = None;
        let s = settings::get();
        self.sh.pin_len = if s.password.is_empty() { 0 } else { s.pin };
        self.sh.name = s.name.clone();
        self.sh.typed.clear();
        self.sh.ok_at = 0;
        self.sh.wrong_at = 0;
        self.sh.locked = true;
        self.go(l, BarState::Lock);
        l.damage(self.screen());
    }

    fn unlock(&mut self, l: &mut Looper) {
        self.sh.locked = false;
        self.sh.typed.clear();
        self.sh.ok_at = 0;
        self.sh.wrong_at = 0;
        self.last_input = ms();
        self.minute = u64::MAX;
        self.go(l, BarState::Idle);
        l.damage(self.screen());
    }

    fn try_unlock(&mut self, l: &mut Looper) {
        if settings::check_password(&self.sh.typed) {
            self.sh.ok_at = ms();
        } else {
            self.sh.wrong_at = ms();
        }
        self.damage_bar(l);
    }

    fn pin_digit(&mut self, l: &mut Looper, d: Option<u8>) {
        if self.sh.ok_at != 0 || self.sh.wrong_at != 0 {
            return;
        }
        match d {
            Some(d) if (self.sh.typed.len() as u32) < self.sh.pin_len => self.sh.typed.push((b'0' + d) as char),
            None => {
                self.sh.typed.pop();
            }
            _ => {}
        }
        if self.sh.pin_len > 0 && self.sh.typed.len() as u32 == self.sh.pin_len {
            self.try_unlock(l);
        }
        self.damage_bar(l);
    }

    fn lock_key(&mut self, l: &mut Looper, k: KeyEvent) {
        if self.sh.pin_len > 0 {
            match k.key {
                Key::Char(c) if c.is_ascii_digit() => self.pin_digit(l, Some(c as u8 - b'0')),
                Key::Backspace => self.pin_digit(l, None),
                Key::Escape => {
                    self.sh.typed.clear();
                    self.damage_bar(l);
                }
                _ => {}
            }
            return;
        }
        if self.sh.ok_at != 0 || self.sh.wrong_at != 0 {
            return;
        }
        match k.key {
            Key::Enter => self.try_unlock(l),
            Key::Escape => self.sh.typed.clear(),
            _ => {
                ui::edit_text(&mut self.sh.typed, &k, 64);
            }
        }
        self.damage_bar(l);
    }

    /// Powers off or restarts after the bar's animation (the apps close
    /// at once).
    pub(super) fn start_ending(&mut self, l: &mut Looper, how: Ending) {
        if self.sh.ending.is_some() {
            return;
        }
        crate::fs::sync_all();
        for w in self.windows.iter_mut() {
            w.app.close();
        }
        self.close_popup(l);
        self.drag = None;
        self.sh.ending = Some((how, ms()));
        self.go(l, BarState::Spin);
        self.sh.black.set(ms(), 0.78);
        l.damage(self.screen());
    }

    fn ending_step(&mut self, l: &mut Looper) {
        let Some((how, since)) = self.sh.ending else { return };
        let t = ms().saturating_sub(since);
        let (step, last) = match how {
            Ending::PowerOff => (POWER_OFF_MS, BarState::Off),
            Ending::Restart => (RESTART_MS, BarState::Boot),
        };
        if t >= step && self.sh.bar.state == BarState::Spin {
            self.go(l, last);
            self.sh.black.set(ms(), 1.0);
            l.damage(self.screen());
        }
        if t >= step + LAST_MS {
            match how {
                Ending::Restart => crate::acpi::reboot(),
                Ending::PowerOff => crate::acpi::power_off(),
            }
        }
    }

    // ------------------------------------------------------------------
    // Notifications, levels, charge

    /// A notification: it comes out of the bar (or waits for it), or with
    /// "Non disturbare" the bar only says that one came.
    pub(super) fn shell_notify(&mut self, l: &mut Looper, title: String, body: String, icon: String) {
        if self.quiet {
            self.transient(l, BarState::Silent, SILENT_MS);
            return;
        }
        let app = apps::BUILT_IN.iter().find(|(_, _, i)| *i == icon).map_or("justOS", |(_, n, _)| n);
        if self.sh.queue.len() >= 5 {
            self.sh.queue.pop_front();
        }
        self.sh.queue.push_back(Note { title, body, icon, app: String::from(app) });
        self.next_note(l);
    }

    fn next_note(&mut self, l: &mut Looper) {
        if self.sh.queue.is_empty() || self.sh.bar.state != BarState::Idle {
            return;
        }
        let note = self.sh.queue.pop_front();
        self.sh.note = note;
        if !self.transient(l, BarState::Notify, NOTIFY_MS) {
            if let Some(n) = self.sh.note.take() {
                self.sh.queue.push_front(n);
            }
        }
    }

    pub(super) fn volume_key(&mut self, l: &mut Looper, key: Key) {
        let v = crate::dev::audio::volume();
        let next = match key {
            Key::VolumeUp => (v + VOLUME_STEP).min(100),
            Key::VolumeDown => v.saturating_sub(VOLUME_STEP),
            _ if v > 0 => {
                UNMUTE.store(v, Ordering::Relaxed);
                0
            }
            _ => UNMUTE.load(Ordering::Relaxed).max(VOLUME_STEP),
        };
        crate::gui::change_volume(next);
        self.sh.levels = (crate::dev::audio::volume(), crate::gui::brightness());
        self.show_hud(l, Level::Volume);
    }

    fn show_hud(&mut self, l: &mut Looper, lv: Level) {
        self.sh.level = lv;
        if !self.transient(l, BarState::Hud, HUD_MS) {
            self.damage_bar(l);
        }
    }

    fn set_level(&mut self, l: &mut Looper, lv: Level, v: u32) {
        match lv {
            Level::Volume => crate::gui::change_volume(v.min(100)),
            Level::Brightness => {
                crate::gui::set_brightness(v.clamp(20, 100));
                l.damage(self.screen());
            }
        }
        self.sh.levels = (crate::dev::audio::volume(), crate::gui::brightness());
        self.sh.level = lv;
        self.damage_bar(l);
    }

    /// The pointer at (`x`, `y`) while dragging a slider of the bar.
    pub(super) fn slide(&mut self, l: &mut Looper, x: i32, y: i32) {
        match self.drag {
            Some(Drag::Volume) | Some(Drag::Brightness) => {
                let (p, lv) = if matches!(self.drag, Some(Drag::Volume)) { (Part::Volume, Level::Volume) } else { (Part::Brightness, Level::Brightness) };
                if let Some(r) = self.part_rect(p) {
                    let v = ((r.bottom() - y) * 100 / r.h.max(1)).clamp(0, 100) as u32;
                    self.set_level(l, lv, v);
                }
            }
            Some(Drag::Hud) => {
                if let Some(r) = self.part_rect(Part::Hud) {
                    let k = self.sh.bar.final_geometry().k;
                    let (x0, w) = (r.x + round(6.0 * k), round(266.0 * k).max(1));
                    let v = ((x - x0) * 100 / w).clamp(0, 100) as u32;
                    let lv = self.sh.level;
                    self.set_level(l, lv, v);
                    // Still being moved: it stays.
                    self.transient(l, BarState::Hud, HUD_MS);
                }
            }
            Some(Drag::Seek) => {
                if let Some(r) = self.part_rect(Part::Seek) {
                    self.sh.seek = Some(((x - r.x) * 1000 / r.w.max(1)).clamp(0, 1000) as u32);
                    self.damage_bar(l);
                }
            }
            _ => {}
        }
    }

    /// The button went up: true if it was a drag of the bar's.
    pub(super) fn shell_release(&mut self, l: &mut Looper) -> bool {
        match self.drag {
            Some(Drag::Volume) | Some(Drag::Brightness) | Some(Drag::Hud) => {
                self.drag = None;
                true
            }
            Some(Drag::Seek) => {
                self.drag = None;
                if let Some(v) = self.sh.seek.take() {
                    self.media(l, wm::Media::Seek(v));
                }
                true
            }
            _ => false,
        }
    }

    fn media(&mut self, l: &mut Looper, m: wm::Media) {
        let Some(id) = self.sh.song.as_ref().map(|s| s.id) else { return };
        if let Some(w) = self.window_mut(id) {
            if w.app.media(m) {
                l.damage(w.bounds());
            }
        }
        self.refresh_song(l);
    }

    fn refresh_song(&mut self, l: &mut Looper) {
        let mut song = None;
        for w in &self.windows {
            if let Some(wm::Live::Music { title, album, pos, len, playing, shuffle, repeat }) = w.app.activity() {
                song = Some(Song { id: w.id, title, album, pos, len, playing, shuffle, repeat });
                break;
            }
        }
        if song != self.sh.song {
            let shape = song.is_some() != self.sh.song.is_some();
            self.sh.song = song;
            self.sh.bar.set_music(self.sh.song.is_some(), ms());
            if shape {
                self.sync_idle(l);
            }
            self.damage_bar(l);
        }
    }

    fn refresh_lists(&mut self) {
        self.sh.nets = crate::net::wifi::networks();
        self.sh.nets.sort_by_key(|n| (!n.connected, !n.saved, -(n.rssi as i32)));
        self.sh.devs = crate::bt::devices();
        self.sh.lists_at = ms();
    }

    fn open_sub(&mut self, l: &mut Looper, s: BarState) {
        if s == BarState::CcWifi {
            crate::net::wifi::scan();
        }
        self.refresh_lists();
        self.go(l, s);
    }

    fn row_press(&mut self, l: &mut Looper, i: usize) {
        if self.sh.bar.state == BarState::CcWifi {
            let Some(n) = self.sh.nets.get(i).cloned() else { return };
            if n.connected {
                return;
            }
            if n.saved || n.security == Security::Open {
                if crate::net::wifi::connect(&n.name, None).is_ok() {
                    self.sh.joining = Some((n.name.clone(), ms()));
                }
            } else {
                // A password is needed: Settings asks for it.
                self.go(l, BarState::Idle);
                apps::settings_at_wifi();
                self.open_id(l, "settings");
            }
        } else {
            let Some(d) = self.sh.devs.get(i).cloned() else { return };
            let addr = d.addr.clone();
            let what = (d.paired, d.connected);
            self.sh.joining = Some((d.name.clone(), ms()));
            crate::task::spawn("bar-bt", move || {
                let _ = match what {
                    (false, _) => crate::bt::pair(&addr),
                    (true, false) => crate::bt::parse_addr(&addr).map_or(Ok(()), |a| crate::bt::connect(&a)),
                    (true, true) => {
                        if let Some(a) = crate::bt::parse_addr(&addr) {
                            crate::bt::disconnect(&a);
                        }
                        Ok(())
                    }
                };
                crate::dev::input::wake_gui_soon();
            });
        }
        self.damage_bar(l);
    }

    fn wifi_connected() -> bool {
        crate::net::wifi::status().is_some_and(|s| s.state == WifiState::Connected)
    }

    // ------------------------------------------------------------------
    // Input

    /// A press: true if the bar (or what it opened) took it.
    pub(super) fn shell_press(&mut self, l: &mut Looper, x: i32, y: i32) -> bool {
        if self.sh.ending.is_some() {
            return true;
        }
        let s = self.sh.bar.state;
        if let Some(p) = self.part_at(x, y) {
            self.press_part(l, p, x, y);
            return true;
        }
        if self.bar_hidden() {
            return false;
        }
        if self.bar_rest().contains(x, y) {
            return true;
        }
        match s {
            BarState::Launch => {
                self.go(l, BarState::Idle);
                true
            }
            s if s.dims_desktop() => {
                self.go(l, BarState::Idle);
                true
            }
            BarState::Prep => {
                // Put away: the click goes on to what is under it.
                self.sh.prep_hidden = true;
                self.go(l, BarState::Idle);
                false
            }
            BarState::Lock | BarState::Spin | BarState::Boot | BarState::Off => true,
            _ => false,
        }
    }

    fn press_part(&mut self, l: &mut Looper, p: Part, x: i32, y: i32) {
        let lists = matches!(self.sh.bar.state, BarState::CcWifi | BarState::CcBt);
        match p {
            Part::Logo => self.toggle_launcher(l),
            Part::App(i) => {
                if let Some(item) = self.bar_items().get(i).cloned() {
                    self.bounce_icon(&item.kind);
                    self.dock_press(l, &item);
                }
            }
            Part::NowPlaying => self.go(l, BarState::Music),
            Part::Status => {
                self.refresh_lists();
                self.go(l, BarState::Control);
            }
            Part::Key(d) => self.pin_digit(l, Some(d)),
            Part::KeyBack => self.pin_digit(l, None),
            Part::Field => {}
            Part::Wifi => {
                if Self::wifi_connected() {
                    crate::net::wifi::disconnect();
                } else {
                    self.open_sub(l, BarState::CcWifi);
                }
            }
            Part::WifiMore => self.open_sub(l, BarState::CcWifi),
            Part::Bt => {
                if crate::bt::present() {
                    let on = crate::bt::powered();
                    crate::task::spawn("bar-bt", move || {
                        crate::bt::set_powered(!on);
                        crate::dev::input::wake_gui_soon();
                    });
                }
            }
            Part::BtMore => self.open_sub(l, BarState::CcBt),
            Part::Quiet => self.quiet = !self.quiet,
            Part::Volume => {
                self.drag = Some(Drag::Volume);
                self.slide(l, x, y);
            }
            Part::Brightness => {
                self.drag = Some(Drag::Brightness);
                self.slide(l, x, y);
            }
            Part::Art => {
                if self.sh.song.is_some() {
                    self.go(l, BarState::Music);
                } else {
                    self.go(l, BarState::Idle);
                    self.open_id(l, "music");
                }
            }
            Part::Play => {
                if self.sh.song.is_some() {
                    self.media(l, wm::Media::Toggle);
                } else {
                    self.go(l, BarState::Idle);
                    self.open_id(l, "music");
                }
            }
            Part::Next => self.media(l, wm::Media::Next),
            Part::Lock => self.lock_screen(l),
            Part::Restart => self.start_ending(l, Ending::Restart),
            Part::PowerOff => self.go(l, BarState::Power),
            Part::Battery => {}
            Part::Account => {
                self.go(l, BarState::Idle);
                self.open_id(l, "settings");
            }
            Part::Gear => {
                self.go(l, BarState::Idle);
                self.open_id(l, "settings");
            }
            Part::Back => self.go(l, BarState::Control),
            Part::Switch => {
                if self.sh.bar.state == BarState::CcWifi {
                    if Self::wifi_connected() {
                        crate::net::wifi::disconnect();
                    } else {
                        crate::net::wifi::scan();
                    }
                } else if let Some((_, _, on)) = crate::bt::adapter() {
                    crate::task::spawn("bar-bt", move || {
                        crate::bt::set_powered(!on);
                        crate::dev::input::wake_gui_soon();
                    });
                }
            }
            Part::Row(i) => self.row_press(l, i),
            Part::More => {
                let wifi = self.sh.bar.state == BarState::CcWifi;
                self.go(l, BarState::Idle);
                if wifi {
                    apps::settings_at_wifi();
                } else {
                    apps::settings_at_bluetooth();
                }
                self.open_id(l, "settings");
            }
            Part::Hud => {
                self.drag = Some(Drag::Hud);
                self.slide(l, x, y);
            }
            Part::Note => self.go(l, BarState::Idle),
            Part::Shuffle => self.media(l, wm::Media::Shuffle),
            Part::Prev => self.media(l, wm::Media::Prev),
            Part::MPlay => self.media(l, wm::Media::Toggle),
            Part::MNext => self.media(l, wm::Media::Next),
            Part::Repeat => self.media(l, wm::Media::Repeat),
            Part::Seek => {
                self.drag = Some(Drag::Seek);
                self.slide(l, x, y);
            }
            Part::Cancel if self.sh.bar.state == BarState::Prep => crate::gui::apps::winprep::stop(),
            Part::Confirm if self.sh.bar.state == BarState::Prep => crate::gui::apps::winprep::again(),
            Part::Cancel => self.go(l, BarState::Control),
            Part::Confirm => self.start_ending(l, Ending::PowerOff),
            Part::Cell(i) => self.lp_open(l, i),
            Part::Found(i) => self.choose_found(l, i),
            Part::Dot(p) => {
                self.sh.page = p;
                l.damage(self.screen());
            }
        }
        if lists {
            self.refresh_lists();
        }
        self.damage_bar(l);
    }

    /// A key: true if the bar took it.
    pub(super) fn shell_key(&mut self, l: &mut Looper, k: KeyEvent) -> bool {
        if self.sh.ending.is_some() {
            return true;
        }
        if self.sh.locked {
            self.lock_key(l, k);
            return true;
        }
        let s = self.sh.bar.state;
        if k.key == Key::Super {
            if self.sh.fs.super_alone() {
                self.sync_hidden(l);
            }
            return true;
        }
        if s == BarState::Launch {
            self.launch_key(l, k);
            return true;
        }
        if k.key == Key::Escape {
            if s != BarState::Idle && s != BarState::Lock {
                if s == BarState::Prep {
                    self.sh.prep_hidden = true;
                }
                let back = s.back().unwrap_or(BarState::Idle);
                self.go(l, back);
                return true;
            }
            if self.sh.fs.esc_exits(true) {
                if let Some(id) = self.sh.fs.window {
                    self.full(l, id);
                }
                return true;
            }
        }
        false
    }

    fn launch_key(&mut self, l: &mut Looper, k: KeyEvent) {
        match k.key {
            Key::Escape => return self.go(l, BarState::Idle),
            Key::Enter => {
                let list = self.lp_list();
                if let Some(&i) = list.first() {
                    self.lp_open(l, i);
                } else if !self.sh.found.is_empty() {
                    self.choose_found(l, 0);
                }
                return;
            }
            Key::Left | Key::PageUp if self.sh.query.is_empty() => self.sh.page = self.sh.page.saturating_sub(1),
            Key::Right | Key::PageDown if self.sh.query.is_empty() => self.sh.page += 1,
            _ => {
                if ui::edit_text(&mut self.sh.query, &k, 40) {
                    self.sh.page = 0;
                    self.sh.found = if self.sh.query.trim().is_empty() { Vec::new() } else { apps::search::find(&self.sh.query, &mut self.sh.index) };
                }
            }
        }
        self.clamp_page();
        l.damage(self.screen());
    }

    fn clamp_page(&mut self) {
        let m = self.main_mon();
        let lay = launchpad::layout(m.w, m.h);
        let pages = lay.pages(self.lp_list().len());
        self.sh.page = self.sh.page.min(pages - 1);
    }

    /// The wheel: true if the bar took it.
    pub(super) fn shell_wheel(&mut self, l: &mut Looper, lines: i32) -> bool {
        let (x, y) = self.mouse;
        match self.sh.bar.state {
            BarState::Launch => {
                let now = ms();
                if now.saturating_sub(self.sh.wheel_at) >= 450 && self.sh.query.is_empty() {
                    self.sh.wheel_at = now;
                    self.sh.page = if lines > 0 { self.sh.page + 1 } else { self.sh.page.saturating_sub(1) };
                    self.clamp_page();
                    l.damage(self.screen());
                }
                true
            }
            BarState::Idle if self.part_at(x, y) == Some(Part::Status) => {
                let v = self.sh.levels.0 as i32 - lines * 5;
                self.set_level(l, Level::Volume, v.clamp(0, 100) as u32);
                self.show_hud(l, Level::Volume);
                true
            }
            BarState::Control => match self.part_at(x, y) {
                Some(Part::Volume) => {
                    let v = self.sh.levels.0 as i32 - lines * 5;
                    self.set_level(l, Level::Volume, v.clamp(0, 100) as u32);
                    true
                }
                Some(Part::Brightness) => {
                    let v = self.sh.levels.1 as i32 - lines * 5;
                    self.set_level(l, Level::Brightness, v.clamp(0, 100) as u32);
                    true
                }
                _ => self.bar_rest().contains(x, y),
            },
            _ => self.bar_rest().contains(x, y),
        }
    }

    /// The pointer moved (no button held, or not on a slider): the parts
    /// light up, the bar peeks over a full-screen window.
    pub(super) fn shell_move(&mut self, l: &mut Looper, _x: i32, y: i32) {
        let (x, _) = self.mouse;
        let m = self.main_mon();
        let k = self.bar_scale();
        if self.sh.fs.pointer(y - m.y, m.h, k, ms()) {
            self.sync_hidden(l);
        }
        let p = self.part_at(x, y);
        if p != self.sh.hover {
            self.sh.hover = p;
            self.damage_bar(l);
            if self.sh.bar.state == BarState::Launch {
                l.damage(self.screen());
            }
        }
    }

    /// A click in a window closes a peek the logo key opened.
    pub(super) fn shell_window_click(&mut self, l: &mut Looper) {
        if self.sh.fs.click_in_window() {
            self.sync_hidden(l);
        }
    }

    // ------------------------------------------------------------------
    // The launchpad

    /// The apps the query matches (indices into the entries).
    fn lp_list(&self) -> Vec<usize> {
        let q = self.sh.query.trim();
        (0..self.sh.entries.len()).filter(|&i| q.is_empty() || launchpad::matches(&self.sh.entries[i].name, q)).collect()
    }

    fn lp_open(&mut self, l: &mut Looper, i: usize) {
        let Some(e) = self.sh.entries.get(i).map(|e| e.launch.clone()) else { return };
        self.go(l, BarState::Idle);
        self.launch(l, e);
    }

    /// Does what a search result is for: copies a result, opens the rest.
    fn choose_found(&mut self, l: &mut Looper, i: usize) {
        let Some(f) = self.sh.found.get(i) else { return };
        let (copy, launch) = (f.copy.clone(), f.launch.clone());
        self.go(l, BarState::Idle);
        if let Some(v) = copy {
            ui::copy(&v);
            self.notify(l, String::from("Calcolo"), format!("{v} copiato negli appunti."), String::from("calculator"));
            return;
        }
        if let Some(e) = launch {
            self.launch(l, e);
        }
    }

    /// The cells of the page shown: (entry, rectangle on the screen).
    fn lp_cells(&self) -> Vec<(usize, Rect)> {
        let m = self.main_mon();
        let lay = launchpad::layout(m.w, m.h);
        let list = self.lp_list();
        let per = lay.per_page();
        let page = self.sh.page.min(lay.pages(list.len()) - 1);
        list.iter()
            .skip(page * per)
            .take(per)
            .enumerate()
            .map(|(j, &i)| {
                let (x, y, cw) = lay.cell(j);
                (i, Rect::new(m.x + x, m.y + y, cw, launchpad::ROW_H))
            })
            .collect()
    }

    /// The rows of files and settings found, under the apps found.
    fn lp_found_rows(&self) -> Vec<Rect> {
        if self.sh.query.trim().is_empty() || self.sh.found.is_empty() {
            return Vec::new();
        }
        let m = self.main_mon();
        let lay = launchpad::layout(m.w, m.h);
        let n = self.lp_list().len().min(lay.per_page());
        let rows = n.div_ceil(lay.cols) as i32;
        let w = 560.min(m.w - 32);
        let mut y = m.y + lay.y + rows * (launchpad::ROW_H + launchpad::ROW_GAP);
        let bottom = m.bottom() - 140;
        let mut v = Vec::new();
        for _ in 0..self.sh.found.len().min(6) {
            if y + 44 > bottom {
                break;
            }
            v.push(Rect::new(m.x + (m.w - w) / 2, y, w, 44));
            y += 50;
        }
        v
    }

    fn lp_dots(&self) -> Vec<Rect> {
        let m = self.main_mon();
        let lay = launchpad::layout(m.w, m.h);
        let pages = lay.pages(self.lp_list().len());
        if pages < 2 {
            return Vec::new();
        }
        let w = pages as i32 * 8 + (pages as i32 - 1) * 10;
        let x0 = m.x + (m.w - w) / 2;
        (0..pages).map(|i| Rect::new(x0 + i as i32 * 18, m.y + lay.dots_y(), 8, 8)).collect()
    }

    fn launch_part(&self, x: i32, y: i32) -> Option<Part> {
        if let Some((i, _)) = self.lp_cells().into_iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Part::Cell(i));
        }
        if let Some(i) = self.lp_found_rows().iter().position(|r| r.contains(x, y)) {
            return Some(Part::Found(i));
        }
        self.lp_dots().iter().position(|r| r.grow(5).contains(x, y)).map(Part::Dot)
    }

    // ------------------------------------------------------------------
    // Time

    pub(super) fn shell_tick(&mut self, l: &mut Looper) {
        let now = ms();
        if self.sh.bar.tick(now) {
            let was = self.sh.bar.prev;
            self.after_go(l, was, now);
        }
        self.next_note(l);
        if now.saturating_sub(self.sh.battery_at) >= 5000 {
            self.sh.battery_at = now;
            let battery = crate::acpi::power::battery();
            if battery != self.sh.battery {
                let plugged = battery.is_some_and(|b| b.1) && !self.sh.battery.is_some_and(|b| b.1);
                self.sh.battery = battery;
                self.sync_idle(l);
                self.damage_bar(l);
                if plugged {
                    self.transient(l, BarState::Charge, CHARGE_MS);
                }
            }
        }
        let levels = (crate::dev::audio::volume(), crate::gui::brightness());
        if levels != self.sh.levels {
            self.sh.levels = levels;
            self.damage_bar(l);
        }
        self.refresh_song(l);
        // The equaliser and the song's progress move while it plays.
        if self.sh.song.as_ref().is_some_and(|s| s.playing)
            && matches!(self.sh.bar.state, BarState::Idle | BarState::Music | BarState::Control)
            && !self.bar_hidden()
        {
            self.damage_bar(l);
        }
        if matches!(self.sh.bar.state, BarState::CcWifi | BarState::CcBt) && now.saturating_sub(self.sh.lists_at) >= 1500 {
            self.refresh_lists();
            self.damage_bar(l);
        }
        if self.sh.joining.as_ref().is_some_and(|j| now.saturating_sub(j.1) > 8000) {
            self.sh.joining = None;
            self.damage_bar(l);
        }
        let idle = self.sh.bar.state == BarState::Idle;
        if self.sh.fs.tick(now, idle) {
            self.sync_hidden(l);
        }
        self.prep_step(l);
        self.pin_timers(l);
        self.ending_step(l);
    }

    fn pin_timers(&mut self, l: &mut Looper) {
        let now = ms();
        if self.sh.ok_at != 0 && now.saturating_sub(self.sh.ok_at) >= PIN_OK_MS {
            self.unlock(l);
        }
        if self.sh.wrong_at != 0 && now.saturating_sub(self.sh.wrong_at) >= PIN_BAD_MS {
            self.sh.wrong_at = 0;
            self.sh.typed.clear();
            self.damage_bar(l);
        }
    }

    pub(super) fn shell_frame(&mut self, l: &mut Looper) {
        let now = ms();
        let busy = self.shell_moving();
        if self.sh.busy && !busy {
            l.damage(self.screen());
        }
        self.sh.busy = busy;
        if self.sh.bar.moving(now) || matches!(self.sh.bar.state, BarState::Spin | BarState::Boot | BarState::Notify | BarState::Prep) {
            self.damage_bar(l);
        }
        if self.sh.ok_at != 0 || self.sh.wrong_at != 0 {
            self.damage_bar(l);
        }
        if self.sh.dim.moving(now) || self.sh.lp.moving(now) || self.sh.black.moving(now) {
            l.damage(self.screen());
        }
        if self.sh.bar.state == BarState::Launch && apps::search::searching(&self.sh.index) {
            let more = apps::search::step(&mut self.sh.index);
            if !more.is_empty() || !apps::search::searching(&self.sh.index) {
                self.sh.found.extend(more);
                l.damage(self.screen());
            }
        }
        self.pin_timers(l);
        self.ending_step(l);
    }

    pub(super) fn shell_animating(&self) -> bool {
        self.sh.busy || self.shell_moving()
    }

    fn shell_moving(&self) -> bool {
        let now = ms();
        let s = self.sh.bar.state;
        self.sh.bar.moving(now)
            || self.sh.dim.moving(now)
            || self.sh.lp.moving(now)
            || self.sh.black.moving(now)
            || matches!(s, BarState::Spin | BarState::Boot | BarState::Notify | BarState::Prep)
            || self.sh.ok_at != 0
            || self.sh.wrong_at != 0
            || self.sh.ending.is_some()
            || (s == BarState::Launch && apps::search::searching(&self.sh.index))
    }

    // ------------------------------------------------------------------
    // Drawing round the bar

    /// The dim over the desktop (under the control centre and the
    /// player).
    pub(super) fn draw_dim(&self, c: &mut Canvas) {
        let a = self.sh.dim.value(ms());
        if a > 0.0 {
            c.fill(self.screen(), fade(DIM, (a * 255.0) as u32));
        }
    }

    /// The launchpad: the blurred wallpaper, its veil, the apps.
    pub(super) fn draw_launchpad(&self, c: &mut Canvas) {
        let a = self.sh.lp.value(ms());
        if a <= 0.0 {
            return;
        }
        let a8 = (a * 255.0) as u32;
        let screen = self.screen();
        c.blit_faded(&self.blur, screen.x, screen.y, screen, a8);
        c.fill(screen, fade(LP_VEIL, a8));
        let hover = self.sh.hover;
        let first = (!self.sh.query.trim().is_empty()).then(|| self.lp_list().first().copied()).flatten();
        let (lh, _) = text::metrics(Font::Label13);
        for (i, r) in self.lp_cells() {
            let e = &self.sh.entries[i];
            let ix = r.x + (r.w - launchpad::ICON) / 2;
            let lift = if hover == Some(Part::Cell(i)) { -3 } else { 0 };
            let icon = Rect::new(ix, r.y + lift, launchpad::ICON, launchpad::ICON);
            if first == Some(i) {
                c.stroke_round(icon.grow(5), 23, 3, fade(BRAND, a8));
            }
            res::draw_icon_faded(c, &e.icon, launchpad::ICON, icon.x, icon.y, a8);
            let name = text::fit(Font::Label13, &e.name, r.w - 8);
            let ty = r.y + launchpad::ICON + 9;
            text::draw_centered(c, Font::Label13, r.x + r.w / 2, ty + 1, &name, fade(0x8C00_0000, a8));
            text::draw_centered(c, Font::Label13, r.x + r.w / 2, ty, &name, fade(WHITE, a8));
            let _ = lh;
        }
        let (bh, _) = text::metrics(Font::Strong);
        let (sh, _) = text::metrics(Font::Small);
        for (i, r) in self.lp_found_rows().into_iter().enumerate() {
            let f = &self.sh.found[i];
            let bg = if hover == Some(Part::Found(i)) { 0x40FF_FFFF } else { 0x26FF_FFFF };
            c.fill_round(r, 14, fade(bg, a8));
            res::draw_icon_faded(c, f.icon, 26, r.x + 12, r.y + 9, a8);
            let x = r.x + 50;
            let name = text::fit(Font::Strong, &f.name, r.w / 2);
            let ny = r.y + (r.h - bh) / 2;
            let nw = text::width(Font::Strong, &name);
            match f.mark {
                // The words found, brighter.
                Some(mark) => draw_marked(c, Font::Strong, (x, ny), &name, mark, fade(T2, a8), fade(WHITE, a8)),
                None => {
                    text::draw(c, Font::Strong, x, ny, &name, fade(WHITE, a8));
                }
            }
            let detail = text::fit(Font::Small, &f.detail, r.right() - 14 - (x + nw + 12) - text::width(Font::Small, f.section) - 12);
            text::draw(c, Font::Small, x + nw + 12, r.y + (r.h - sh) / 2, &detail, fade(T2, a8));
            text::draw_right(c, Font::Small, r.right() - 14, r.y + (r.h - sh) / 2, f.section, fade(T3, a8));
        }
        let page = self.sh.page;
        for (i, d) in self.lp_dots().into_iter().enumerate() {
            c.fill_circle(d.x + 4, d.y + 4, 4, fade(if i == page { WHITE } else { 0x59FF_FFFF }, a8));
        }
        if self.lp_cells().is_empty() && self.lp_found_rows().is_empty() {
            let m = self.main_mon();
            let msg = format!("Nessun risultato per “{}”", self.sh.query.trim());
            text::draw_centered(c, Font::Text17, m.x + m.w / 2, m.y + m.h / 2 - 60, &msg, fade(T2, a8));
        }
    }

    /// The dark screen of the power flow.
    pub(super) fn draw_black(&self, c: &mut Canvas) {
        let a = self.sh.black.value(ms());
        if a > 0.0 {
            c.fill(self.screen(), fade(0xFF00_0000, (a * 255.0) as u32));
        }
    }

    /// The lock screen behind the keypad: the blurred wallpaper and the big
    /// clock.
    pub(super) fn draw_lock_screen(&self, c: &mut Canvas, r: Rect) {
        c.blit(&self.blur, r.x, r.y, r);
        c.fill(r, 0x2600_0000);
        let m = self.main_mon();
        let d = now();
        let clock = format!("{}:{:02}", d.hour, d.min);
        let top = m.y + m.h * 7 / 100;
        text::draw_centered(c, Font::LockClock, m.x + m.w / 2, top, &clock, WHITE);
        // The date right under the digits' baseline.
        let (_, ascent) = text::metrics(Font::LockClock);
        let date = format!("{} {} {}", WEEKDAYS[d.weekday as usize % 7], d.day, MONTHS[(d.month as usize + 11) % 12]);
        text::draw_centered(c, Font::Text20, m.x + m.w / 2, top + ascent + 10, &date, 0xD9FF_FFFF);
    }

    /// The bar, its head over the control centre, and what they show.
    pub(super) fn draw_bar(&self, c: &mut Canvas) {
        let now = ms();
        let b = &self.sh.bar;
        let g = b.geometry(now);
        let r = self.geom_rect(g);
        let rad = round(g.r);
        let mut painted = r.grow(48).offset(0, 24);
        let (inn, out) = b.views(now);
        // The head fades with the control centre's views.
        let head_alpha = match (inn.state.is_cc(), out.map(|o| o.state.is_cc())) {
            (true, Some(true)) | (true, None) => inn.alpha.max(if out.is_some() { 1.0 } else { 0.0 }),
            (true, Some(false)) => inn.alpha,
            (false, Some(true)) => out.map_or(0.0, |o| o.alpha),
            _ => 0.0,
        };
        if head_alpha > 0.0 {
            let s = if inn.state.is_cc() { inn.state } else { out.map_or(inn.state, |o| o.state) };
            painted = painted.union(&self.draw_head(c, s, g, head_alpha));
        }
        c.shadow(r, rad, 40, 110);
        let over_window = !self.sh.locked && self.windows.iter().any(|w| w.shown(self.space) && w.rect.overlaps(&r));
        let tint = if b.state == BarState::Music { GLASS_MUSIC } else { GLASS };
        if over_window {
            c.fill_round(r, rad, if b.state == BarState::Music { GLASS_MUSIC | 0xFF00_0000 } else { GLASS_SOLID });
        } else {
            c.blit_round(&self.blur, r, rad, tint);
        }
        let dark = b.darkness(now);
        if dark > 0.0 {
            c.fill_round(r, rad, fade(DARK, (dark * 255.0) as u32));
        }
        c.stroke_round(r, rad, 1, LINE);
        c.sheen_round(r, rad, 1, 0x12FF_FFFF);
        let saved = c.clip();
        c.set_clip(saved.intersect(&r));
        if let Some(o) = out {
            self.paint_view(c, o, g);
        }
        self.paint_view(c, inn, g);
        c.set_clip(saved);
        self.sh.painted.set(self.sh.painted.get().union(&painted));
    }

    fn paint_view(&self, c: &mut Canvas, f: ViewFade, g: Geometry) {
        let a = (f.alpha * 255.0) as u32;
        if a == 0 {
            return;
        }
        let (ox, oy, k) = self.view_place(f.state, g, f.scale);
        let (vw, vh) = self.view_size(f.state);
        if a >= 255 && (k - 1.0).abs() < 0.002 {
            self.draw_view(c, f.state, (round(ox), round(oy)));
            return;
        }
        let mut layer = Canvas::new_layer_at(vw, vh, c.scale());
        self.draw_view(&mut layer, f.state, (0, 0));
        // A view comes in and goes out of focus (6 px of blur when hidden).
        let blur = ((1.0 - f.alpha) * 6.0) as i32;
        if blur >= 1 {
            layer.blur(blur);
        }
        let dst = Rect::new(round(ox), round(oy), round(vw as f32 * k), round(vh as f32 * k));
        c.draw_scaled(&layer, dst, a);
    }

    fn draw_head(&self, c: &mut Canvas, s: BarState, g: Geometry, alpha: f32) -> Rect {
        let (hx, hy, hw, k) = self.head_place(s, g);
        let dst = Rect::new(round(hx), round(hy), round(hw as f32 * k), round(56.0 * k));
        let a = (alpha * 255.0) as u32;
        let [(_, chip), (_, gear)] = self.head_parts(hw);
        let on_screen = |r: Rect| Rect::new(dst.x + round(r.x as f32 * k), dst.y + round(r.y as f32 * k), round(r.w as f32 * k), round(r.h as f32 * k));
        let (cs, gs) = (on_screen(chip), on_screen(gear));
        // Glass of its own, under the content.
        for (r, rad) in [(cs, cs.h / 2), (gs, gs.h / 2)] {
            c.shadow(r, rad, 18, a * 70 / 255);
            c.blit_round(&self.blur, r, rad, fade(GLASS, a));
            c.stroke_round(r, rad, 1, fade(LINE, a));
        }
        let mut layer = Canvas::new_layer_at(hw, 56, c.scale());
        let name = self.account_name();
        let av = Rect::new(chip.x + 6, 6, 44, 44);
        Self::avatar(&mut layer, av, &name, Font::Strong16);
        layer.fill_circle(av.right() - 5, av.bottom() - 5, 8, 0xFF16_161A);
        layer.fill_circle(av.right() - 5, av.bottom() - 5, 6, GREEN);
        let tx = av.right() + 12;
        text::draw(&mut layer, Font::Strong16, tx, 9, &name, WHITE);
        text::draw(&mut layer, Font::Small, tx, 31, "Account di questo computer", T2);
        let hot = |p: Part| self.sh.hover == Some(p);
        if hot(Part::Gear) {
            layer.fill_circle(gear.x + 28, gear.y + 28, 28, 0x33FF_FFFF);
        }
        icons::centered(&mut layer, "gear", 22, gear.x + 28, gear.y + 28, WHITE);
        if hot(Part::Account) {
            layer.fill_round(chip, 28, 0x14FF_FFFF);
        }
        c.draw_scaled(&layer, dst, a);
        dst.grow(30)
    }

    /// The account's picture: initials on violet.
    fn avatar(c: &mut Canvas, r: Rect, name: &str, font: Font) {
        c.fill_round_grad(r, r.w / 2, 0xFFC7_A8FF, 0xFF3E_1C9C);
        c.glow_round(r, r.w / 2, r.x + r.w * 3 / 10, r.y + r.h / 6, r.w, 0xFFC7_A8FF, 150);
        c.stroke_round(r, r.w / 2, 1, 0x2EFF_FFFF);
        let (h, _) = text::metrics(font);
        text::draw_centered(c, font, r.x + r.w / 2, r.y + (r.h - h) / 2, &initials(name), WHITE);
    }

    fn draw_view(&self, c: &mut Canvas, s: BarState, o: (i32, i32)) {
        match s {
            BarState::Idle => self.draw_idle(c, o),
            BarState::Launch => self.draw_search(c, o),
            BarState::Lock => self.draw_lock(c, o),
            BarState::Control => self.draw_control(c, o),
            BarState::CcWifi | BarState::CcBt => self.draw_sub(c, o, s == BarState::CcWifi),
            BarState::Hud => self.draw_hud(c, o),
            BarState::Notify => self.draw_note(c, o),
            BarState::Silent => {
                let (h, _) = text::metrics(Font::Title15);
                let w = 20 + 10 + text::width(Font::Title15, "Notifica silenziata");
                let x = o.0 + (300 - w) / 2;
                icons::draw(c, "moon", 20, x, o.1 + 18, BRAND);
                text::draw(c, Font::Title15, x + 30, o.1 + (56 - h) / 2, "Notifica silenziata", WHITE);
            }
            BarState::Music => self.draw_music(c, o),
            BarState::Power => self.draw_power(c, o),
            BarState::Spin => {
                let turn = (ms() % 900 * 1000 / 900) as u32;
                icons::spinner(c, o.0 + 21, o.1 + 21, 30, 1000, 3, 0x33FF_FFFF);
                icons::spinner(c, o.0 + 21, o.1 + 21, 30, turn, 3, WHITE);
            }
            BarState::Boot => {
                // The wordmark, breathing.
                let p = (ms() % 1800) as i32;
                let tri = if p < 900 { p } else { 1800 - p };
                let a = (130 + tri * 125 / 900) as u32;
                crate::gui::splash::draw_logo(c, o.0 + 75, o.1 + 48, WORDMARK_H, fade(WHITE, a));
            }
            BarState::Off => {}
            BarState::Charge => self.draw_charge(c, o),
            BarState::Prep => self.draw_prep(c, o),
        }
    }

    fn draw_idle(&self, c: &mut Canvas, o: (i32, i32)) {
        let lay = self.idle_layout();
        let hover = self.sh.hover;
        // The justOS mark.
        let lr = at(o, lay.logo);
        c.fill_round(lr, 21, if hover == Some(Part::Logo) || self.sh.bar.state == BarState::Launch { 0x8C00_0000 } else { 0x6600_0000 });
        c.stroke_round(lr, 21, 1, LINE);
        crate::gui::splash::draw_mark(c, lr.x + lr.w / 2, lr.y + 21, MARK_H, WHITE);
        let items = self.bar_items();
        for (i, r) in lay.apps.iter().enumerate() {
            let Some(item) = items.get(i) else { break };
            let mut dy = if hover == Some(Part::App(i)) { -3 } else { 0 };
            if let Some((kind, a)) = &self.bounce {
                if *kind == item.kind {
                    let t = a.t() as i32;
                    dy = if t < 350 { -7 * t / 350 } else { -7 * (1000 - t) / 650 };
                }
            }
            // A program on its way hops until its first window comes.
            if self.starting.iter().any(|s| s.kind == item.kind) {
                let t = (ms() % 650 * 1000 / 650) as i32;
                dy = if t < 350 { -7 * t / 350 } else { -7 * (1000 - t) / 650 };
            }
            let r = at(o, *r).offset(0, dy);
            res::draw_icon(c, &item.icon, 42, r.x, r.y);
            let open = item.kind != "trash" && self.windows.iter().any(|w| w.app.kind() == item.kind);
            if open {
                c.fill_round(Rect::new(r.x + 19, o.1 + 13 + 45, 4, 4), 2, WHITE);
            }
        }
        if let (Some(np), Some(song)) = (lay.np, &self.sh.song) {
            let np = at(o, np);
            c.fill_round(np, 21, if hover == Some(Part::NowPlaying) { FILL_H } else { FILL });
            let (a, b) = art_of(&song.title);
            c.fill_round_grad(Rect::new(np.x + 5, np.y + 5, 32, 32), 16, a, b);
            self.draw_eq(c, np.x + 47, np.y + 14, song.playing);
        }
        let sr = at(o, lay.status);
        c.fill_round(sr, 23, if hover == Some(Part::Status) || self.sh.bar.state.is_cc() { FILL_H } else { FILL });
        let mut x = sr.x + 21;
        for (name, size, col) in &lay.icons {
            icons::draw(c, name, *size, x, sr.y + (46 - size) / 2, *col);
            x += size + 12;
        }
        if let Some((pct, charging)) = self.sh.battery {
            icons::pill_ring(c, sr.x, sr.y, sr.w, sr.h, 1000, 3, 0x21FF_FFFF);
            let col = if charging {
                GREEN
            } else if pct <= 20 {
                RED
            } else {
                WHITE
            };
            icons::pill_ring(c, sr.x, sr.y, sr.w, sr.h, pct as i32 * 10, 3, col);
        }
    }

    /// Four bars going up and down while the song plays.
    fn draw_eq(&self, c: &mut Canvas, x: i32, y: i32, playing: bool) {
        let now = ms();
        for (i, off) in [0u64, 600, 300, 750].iter().enumerate() {
            let h = if playing {
                let p = ((now + off) % 900) as i32;
                let tri = if p < 450 { p } else { 900 - p };
                4 + 10 * tri / 450
            } else {
                4
            };
            c.fill_round(Rect::new(x + i as i32 * 5, y + (14 - h) / 2, 3, h), 1, BRAND);
        }
    }

    fn draw_search(&self, c: &mut Canvas, o: (i32, i32)) {
        icons::draw(c, "search", 20, o.0 + 20, o.1 + 20, T2);
        let (h, _) = text::metrics(Font::Text17);
        let x = o.0 + 52;
        let y = o.1 + (60 - h) / 2;
        let kw = text::width(Font::Small, "esc") + 16;
        let max = 540 - 52 - 14 - kw - 16;
        let q = &self.sh.query;
        let w = if q.is_empty() {
            text::draw(c, Font::Text17, x + 3, y, "Cerca app", T3);
            0
        } else {
            let shown = text::fit(Font::Text17, q, max);
            text::draw(c, Font::Text17, x, y, &shown, WHITE)
        };
        c.fill(Rect::new(x + w + 1, o.1 + 19, 2, 22), BRAND);
        let kr = Rect::new(o.0 + 540 - 14 - kw, o.1 + 18, kw, 24);
        c.fill_round(kr, 7, FILL_H);
        let (sh, _) = text::metrics(Font::Small);
        text::draw_centered(c, Font::Small, kr.x + kr.w / 2, kr.y + (kr.h - sh) / 2, "esc", T2);
    }

    fn draw_lock(&self, c: &mut Canvas, o: (i32, i32)) {
        let now = ms();
        let pin = self.sh.pin_len > 0;
        let name = self.account_name();
        let top = if pin { 30 } else { 90 };
        let cx = o.0 + 170;
        Self::avatar(c, Rect::new(cx - 42, o.1 + top, 84, 84), &name, Font::Text30);
        text::draw_centered(c, Font::Strong22, cx, o.1 + top + 96, &name, WHITE);
        let (msg, col) = if self.sh.ok_at != 0 {
            ("Bentornato", T2)
        } else if self.sh.wrong_at != 0 {
            (if pin { "PIN errato, riprova" } else { "Password errata, riprova" }, RED)
        } else if !pin && !settings::has_password() {
            ("Premi Invio per sbloccare", T2)
        } else if pin {
            ("Inserisci il PIN", T2)
        } else {
            ("Inserisci la password", T2)
        };
        text::draw_centered(c, Font::Body, cx, o.1 + top + 130, msg, col);
        // The shake of a wrong PIN.
        let shake = if self.sh.wrong_at != 0 {
            let t = now.saturating_sub(self.sh.wrong_at) as i32;
            if t < 400 {
                let p = t % 100;
                let tri = if p < 50 { p } else { 100 - p } - 25;
                tri * 8 * (400 - t) / 400 / 25
            } else {
                0
            }
        } else {
            0
        };
        if pin {
            let n = self.sh.pin_len as i32;
            let w = n * 13 + (n - 1) * 16;
            let x0 = cx - w / 2 + shake;
            for i in 0..n {
                let r = Rect::new(x0 + i * 29, o.1 + 199, 13, 13);
                let filled = (i as usize) < self.sh.typed.len();
                let col = if self.sh.ok_at != 0 {
                    GREEN
                } else if self.sh.wrong_at != 0 {
                    RED
                } else {
                    WHITE
                };
                if filled || self.sh.ok_at != 0 || self.sh.wrong_at != 0 {
                    c.fill_circle(r.x + 6, r.y + 6, 7, col);
                } else {
                    c.stroke_round(r.grow(1), 7, 1, fade(WHITE, 128));
                }
            }
            let (kh, _) = text::metrics(Font::Key26);
            for (p, r) in self.parts(BarState::Lock) {
                let r = at(o, r);
                match p {
                    Part::Key(d) => {
                        let bg = if self.sh.hover == Some(p) { 0x29FF_FFFF } else { FILL_H };
                        c.fill_circle(r.x + 31, r.y + 31, 31, bg);
                        let s = format!("{d}");
                        text::draw_centered(c, Font::Key26, r.x + 31, r.y + (62 - kh) / 2, &s, WHITE);
                    }
                    Part::KeyBack => {
                        if self.sh.hover == Some(p) {
                            c.fill_circle(r.x + 31, r.y + 31, 31, FILL);
                        }
                        icons::centered(c, "back", 24, r.x + 31, r.y + 31, WHITE);
                    }
                    _ => {}
                }
            }
        } else {
            let f = at(o, Rect::new(40, 262, 260, 46)).offset(shake, 0);
            c.fill_round(f, 23, FILL_H);
            c.stroke_round(f, 23, 1, LINE);
            let dots: String = self.sh.typed.chars().map(|_| '•').collect();
            let (h, _) = text::metrics(Font::Text17);
            let shown = text::fit(Font::Text17, &dots, f.w - 40);
            let w = if dots.is_empty() {
                if settings::has_password() {
                    text::draw_centered(c, Font::Body, f.x + f.w / 2, f.y + (f.h - text::metrics(Font::Body).0) / 2, "Password", T3);
                }
                0
            } else {
                text::draw_centered(c, Font::Text17, f.x + f.w / 2, f.y + (f.h - h) / 2, &shown, WHITE);
                text::width(Font::Text17, &shown)
            };
            if !dots.is_empty() || settings::has_password() {
                let cx = if dots.is_empty() { f.x + 20 } else { f.x + f.w / 2 + w / 2 + 2 };
                c.fill(Rect::new(cx, f.y + 12, 2, 22), BRAND);
            }
        }
    }

    /// A box of the control centre: radius 8, 16 at the panel's corners.
    fn cc_box(c: &mut Canvas, r: Rect, corners: [i32; 4], col: u32) {
        c.fill_round_corners(r, corners, col);
    }

    fn draw_control(&self, c: &mut Canvas, o: (i32, i32)) {
        let hover = self.sh.hover;
        let hot = |p: Part| hover == Some(p);
        let (bh, _) = text::metrics(Font::Strong15);
        let (sh, _) = text::metrics(Font::Small);
        // The three switches.
        let wifi = crate::net::wifi::status();
        let wifi_on = wifi.as_ref().is_some_and(|s| s.state == WifiState::Connected);
        let wifi_sub = match &wifi {
            Some(s) if s.state == WifiState::Connected => s.ssid.clone(),
            Some(s) if s.state != WifiState::Idle => s.text(),
            Some(_) => String::from("Non connesso"),
            None => String::from("Nessun adattatore"),
        };
        let bt = crate::bt::adapter();
        let bt_on = bt.as_ref().is_some_and(|b| b.2);
        let bt_sub = match &bt {
            None => String::from("Nessun adattatore"),
            Some((_, _, false)) => String::from("Disattivato"),
            Some(_) => self.sh.devs.iter().find(|d| d.connected).map_or(String::from("Attivo"), |d| d.name.clone()),
        };
        let quiet_sub = String::from(if self.quiet { "Attivo" } else { "Disattivato" });
        let tiles = [
            (Part::Wifi, Rect::new(12, 12, 236, 92), [16, 8, 8, 8], "wifi", "Wi-Fi", wifi_sub, wifi_on, Some(Part::WifiMore)),
            (Part::Bt, Rect::new(12, 112, 236, 92), [8, 8, 8, 8], "bt", "Bluetooth", bt_sub, bt_on, Some(Part::BtMore)),
            (Part::Quiet, Rect::new(12, 212, 236, 92), [8, 8, 8, 16], "moon", "Non disturbare", quiet_sub, self.quiet, None),
        ];
        for (p, r, corners, icon, name, sub, on, more) in tiles {
            let r = at(o, r);
            Self::cc_box(c, r, corners, if hot(p) { FILL_H } else { FILL });
            let (bx, by) = (r.x + 14 + 20, r.y + r.h / 2);
            if on {
                c.glow(bx, by, 34, BRAND, 70);
            }
            c.fill_circle(bx, by, 20, if on { BRAND } else { FILL_H });
            icons::centered(c, icon, 20, bx, by, WHITE);
            let tx = r.x + 14 + 40 + 12;
            let tw = r.right() - tx - if more.is_some() { 48 } else { 12 };
            text::draw(c, Font::Strong15, tx, by - bh + 1, &text::fit(Font::Strong15, name, tw), WHITE);
            text::draw(c, Font::Small, tx, by + 3, &text::fit(Font::Small, &sub, tw), T2);
            if let Some(m) = more {
                let cr = Rect::new(r.right() - 10 - 30, by - 15, 30, 30);
                c.fill_circle(cr.x + 15, cr.y + 15, 15, if hot(m) { 0x29FF_FFFF } else { FILL });
                icons::centered(c, "chev-r", 18, cr.x + 15, cr.y + 15, WHITE);
            }
        }
        // The sliders: white from the bottom, the value at the top, the
        // symbol at the bottom (dark where the white is).
        let (v, b) = self.sh.levels;
        let (ph, _) = text::metrics(Font::Strong13);
        for (r, value, icon) in [(Rect::new(256, 12, 96, 292), v, vol_icon(v)), (Rect::new(360, 12, 96, 292), b, "sun")] {
            let r = at(o, r);
            Self::cc_box(c, r, [8, 8, 8, 8], FILL);
            let fh = r.h * value.min(100) as i32 / 100;
            if fh > 0 {
                let top = if fh > r.h - 8 { 8 } else { 0 };
                c.fill_round_corners(Rect::new(r.x, r.bottom() - fh, r.w, fh), [top, top, 8, 8], WHITE);
            }
            let fill_top = r.bottom() - fh;
            let ink = |y: i32| if y >= fill_top { INK } else { WHITE };
            let s = format!("{value}%");
            text::draw_centered(c, Font::Strong13, r.x + r.w / 2, r.y + 16, &s, ink(r.y + 16 + ph / 2));
            icons::centered(c, icon, 22, r.x + r.w / 2, r.bottom() - 22 - 11, ink(r.bottom() - 33));
        }
        // What plays.
        let np = at(o, Rect::new(464, 12, 236, 92));
        Self::cc_box(c, np, [8, 16, 8, 8], if hot(Part::Art) { FILL_H } else { FILL });
        let art = Rect::new(np.x + 12, np.y + 19, 54, 54);
        let (title, artist, playing) = match &self.sh.song {
            Some(s) => (s.title.clone(), if s.album.is_empty() { String::from("Musica") } else { s.album.clone() }, s.playing),
            None => (String::from("Nessuna musica"), String::from("Apri Musica"), false),
        };
        let (a, bcol) = art_of(&title);
        if self.sh.song.is_some() {
            c.fill_round_grad(art, 11, a, bcol);
        } else {
            c.fill_round(art, 11, FILL_H);
            icons::centered(c, "note", 22, art.x + 27, art.y + 27, T2);
        }
        let tx = art.right() + 12;
        let tw = np.right() - 12 - 70 - 8 - tx;
        let (th, _) = text::metrics(Font::Strong);
        text::draw(c, Font::Strong, tx, np.y + 46 - th, &text::fit(Font::Strong, &title, tw), WHITE);
        text::draw(c, Font::Small, tx, np.y + 49, &text::fit(Font::Small, &artist, tw), T2);
        for (p, icon) in [(Part::Play, if playing { "pause" } else { "play" }), (Part::Next, "next")] {
            let Some(r) = self.parts(BarState::Control).into_iter().find(|(q, _)| *q == p).map(|(_, r)| at(o, r)) else { continue };
            c.fill_circle(r.x + 16, r.y + 16, 16, if hot(p) { 0x29FF_FFFF } else { FILL_H });
            icons::centered(c, icon, 16, r.x + 16, r.y + 16, WHITE);
        }
        // Lock, restart, power off.
        for (i, (p, icon, label)) in [(Part::Lock, "lock", "Blocca"), (Part::Restart, "restart", "Riavvia"), (Part::PowerOff, "power", "Spegni")].iter().enumerate() {
            let r = at(o, Rect::new(464 + i as i32 * 81, 112, if i == 2 { 74 } else { 73 }, 92));
            let bg = match (hot(*p), *p == Part::PowerOff) {
                (true, true) => fade(RED, 220),
                (true, false) => FILL_H,
                _ => FILL,
            };
            Self::cc_box(c, r, [8, 8, 8, 8], bg);
            icons::centered(c, icon, 20, r.x + r.w / 2, r.y + 36, WHITE);
            text::draw_centered(c, Font::Small, r.x + r.w / 2, r.y + 56, label, if hot(*p) { WHITE } else { T2 });
        }
        // The battery (or the mains).
        let br = at(o, Rect::new(464, 212, 236, 92));
        Self::cc_box(c, br, [8, 8, 16, 8], FILL);
        let (x0, x1) = (br.x + 16, br.right() - 16);
        match self.sh.battery {
            Some((pct, charging)) => {
                text::draw(c, Font::Small, x0, br.y + 12, "Batteria", T2);
                text::draw_right(c, Font::Small, x1, br.y + 12, if charging { "Alimentatore" } else { "Batteria" }, T3);
                let mut x = x0;
                if charging {
                    icons::draw(c, "bolt", 16, x, br.y + 35, GREEN);
                    x += 20;
                }
                text::draw(c, Font::Strong22, x, br.y + 30, &format!("{pct} %"), WHITE);
                let sub = if charging {
                    "In carica"
                } else if pct <= 20 {
                    "Carica bassa"
                } else {
                    "In uso"
                };
                text::draw_right(c, Font::Small, x1, br.y + 37, sub, T2);
                let bar = Rect::new(x0, br.bottom() - 12 - 5, x1 - x0, 5);
                c.fill_round(bar, 3, FILL_H);
                let col = if pct <= 20 && !charging { RED } else { GREEN };
                c.fill_round(Rect::new(bar.x, bar.y, bar.w * pct.min(100) as i32 / 100, 5), 3, col);
            }
            None => {
                text::draw(c, Font::Small, x0, br.y + 12, "Alimentazione", T2);
                icons::draw(c, "bolt", 16, x0, br.y + 37, GREEN);
                text::draw(c, Font::Strong18, x0 + 22, br.y + 34, "Corrente", WHITE);
                text::draw(c, Font::Small, x0, br.y + 66, "Nessuna batteria", T3);
            }
        }
        let _ = sh;
    }

    fn draw_sub(&self, c: &mut Canvas, o: (i32, i32), wifi: bool) {
        let hover = self.sh.hover;
        let hot = |p: Part| hover == Some(p);
        let back = at(o, Rect::new(12, 16, 36, 36));
        c.fill_circle(back.x + 18, back.y + 18, 18, if hot(Part::Back) { 0x29FF_FFFF } else { FILL_H });
        icons::centered(c, "chev-l", 18, back.x + 18, back.y + 18, WHITE);
        let (hh, _) = text::metrics(Font::Strong18);
        text::draw(c, Font::Strong18, back.right() + 12, back.y + (36 - hh) / 2, if wifi { "Wi-Fi" } else { "Bluetooth" }, WHITE);
        let on = if wifi { Self::wifi_connected() } else { crate::bt::adapter().is_some_and(|b| b.2) };
        let present = if wifi { crate::net::wifi::present() } else { crate::bt::present() };
        if present {
            let sw = at(o, Rect::new(440 - 12 - 46, 20, 46, 28));
            c.fill_round(sw, 14, if on { BRAND } else { 0x33FF_FFFF });
            let kx = if on { sw.right() - 14 } else { sw.x + 14 };
            c.fill_circle(kx, sw.y + 14, 11, WHITE);
        }
        text::draw(c, Font::Small, o.0 + 16, o.1 + 64, if wifi { "Reti disponibili" } else { "I miei dispositivi" }, T3);
        let (nh, _) = text::metrics(Font::Strong);
        let joining = self.sh.joining.as_ref().map(|j| j.0.clone());
        let rows: Vec<(String, &'static str, bool, &'static str)> = if wifi {
            self.sh
                .nets
                .iter()
                .take(4)
                .map(|n| {
                    let icon = match n.bars {
                        0 | 1 => "wifi-low",
                        2 => "wifi-mid",
                        _ => "wifi",
                    };
                    let trail = if joining.as_deref() == Some(n.name.as_str()) && !n.connected {
                        "spin"
                    } else if n.connected {
                        "check"
                    } else if n.security != Security::Open && !n.saved {
                        "lock"
                    } else {
                        ""
                    };
                    (n.name.clone(), icon, n.connected, trail)
                })
                .collect()
        } else {
            self.sh
                .devs
                .iter()
                .take(4)
                .map(|d| {
                    let icon = match d.kind {
                        crate::bt::Kind::Keyboard => "keyboard",
                        crate::bt::Kind::Mouse => "mouse",
                        crate::bt::Kind::Combo => "keyboard",
                        crate::bt::Kind::Other => "headphones",
                    };
                    let trail = if joining.as_deref() == Some(d.name.as_str()) && crate::bt::scanning() {
                        "spin"
                    } else if d.connected {
                        "check"
                    } else {
                        ""
                    };
                    (if d.name.is_empty() { d.addr.clone() } else { d.name.clone() }, icon, d.connected, trail)
                })
                .collect()
        };
        if rows.is_empty() {
            let msg = if !present {
                if wifi {
                    "Nessun adattatore Wi-Fi"
                } else {
                    "Nessun adattatore Bluetooth"
                }
            } else if wifi {
                "Nessuna rete trovata"
            } else if !on {
                "Il Bluetooth è disattivato"
            } else {
                "Nessun dispositivo"
            };
            let cy = o.1 + 84 + 116;
            icons::centered(c, if wifi { "wifi" } else { "bt" }, 28, o.0 + 220, cy - 22, T3);
            text::draw_centered(c, Font::Body, o.0 + 220, cy + 6, msg, T2);
        }
        for (i, (name, icon, current, trail)) in rows.iter().enumerate() {
            let r = at(o, Rect::new(12, 84 + i as i32 * 56, 416, 54));
            let top = if i == 0 { 14 } else { 6 };
            c.fill_round_corners(r, [top, top, 6, 6], if hot(Part::Row(i)) { FILL_H } else { FILL });
            let (cx, cy) = (r.x + 12 + 15, r.y + 27);
            c.fill_circle(cx, cy, 15, if *current { BRAND } else { FILL_H });
            icons::centered(c, icon, 16, cx, cy, WHITE);
            text::draw(c, Font::Strong, r.x + 12 + 30 + 12, r.y + (54 - nh) / 2, &text::fit(Font::Strong, name, 300), WHITE);
            match *trail {
                "spin" => icons::spinner(c, r.right() - 16 - 18, r.y + 18, 18, (ms() % 900 * 1000 / 900) as u32, 2, WHITE),
                "" => {}
                t => icons::centered(c, t, 18, r.right() - 16 - 9, cy, if t == "check" { WHITE } else { T3 }),
            }
        }
        let more = at(o, Rect::new(12, 384 - 12 - 48, 416, 48));
        c.fill_round_corners(more, [6, 6, 14, 14], if hot(Part::More) { FILL_H } else { FILL });
        let (bh, _) = text::metrics(Font::Body);
        text::draw(c, Font::Body, more.x + 16, more.y + (48 - bh) / 2, if wifi { "Impostazioni Wi-Fi…" } else { "Impostazioni Bluetooth…" }, WHITE);
        icons::centered(c, "chev-r", 16, more.right() - 16 - 8, more.y + 24, T2);
    }

    fn draw_hud(&self, c: &mut Canvas, o: (i32, i32)) {
        let (v, b) = self.sh.levels;
        let (value, icon) = match self.sh.level {
            Level::Volume => (v, vol_icon(v)),
            Level::Brightness => (b, "sun"),
        };
        icons::draw(c, icon, 22, o.0 + 22, o.1 + 23, WHITE);
        let track = Rect::new(o.0 + 58, o.1 + 30, 266, 8);
        c.fill_round(track, 4, 0x29FF_FFFF);
        let fw = track.w * value.min(100) as i32 / 100;
        c.fill_round(Rect::new(track.x, track.y, fw.max(8), 8), 4, WHITE);
        let kx = (track.x + fw).clamp(track.x + 10, track.right() - 10);
        c.shadow(Rect::new(kx - 10, track.y - 6, 20, 20), 10, 6, 70);
        c.fill_circle(kx, track.y + 4, 10, WHITE);
        let (h, _) = text::metrics(Font::Strong15);
        text::draw_right(c, Font::Strong15, o.0 + 400 - 22, o.1 + (68 - h) / 2, &format!("{value}"), WHITE);
    }

    fn draw_note(&self, c: &mut Canvas, o: (i32, i32)) {
        let Some(n) = &self.sh.note else { return };
        res::draw_icon(c, &n.icon, 54, o.0 + 18, o.1 + 19);
        let x = o.0 + 86;
        let w = 490 - 86 - 18;
        text::draw(c, Font::Small, x, o.1 + 14, &text::fit(Font::Small, &n.app, w - 40), T3);
        text::draw_right(c, Font::Small, o.0 + 490 - 18, o.1 + 14, "ora", T3);
        text::draw(c, Font::Strong16, x, o.1 + 31, &text::fit(Font::Strong16, &n.title, w), WHITE);
        text::draw(c, Font::Body, x, o.1 + 53, &text::fit(Font::Body, &n.body, w), T2);
        let t = ms().saturating_sub(self.sh.bar.changed_at) as i32;
        let left = (4500 - t).clamp(0, 4500);
        let full = 490 - 48;
        c.fill_round(Rect::new(o.0 + 24, o.1 + 92 - 7 - 3, full * left / 4500, 3), 1, BRAND);
    }

    fn draw_music(&self, c: &mut Canvas, o: (i32, i32)) {
        let hover = self.sh.hover;
        let hot = |p: Part| hover == Some(p);
        let Some(s) = &self.sh.song else {
            text::draw_centered(c, Font::Body, o.0 + 310, o.1 + 90, "Nessuna musica", T2);
            return;
        };
        let (a, b) = art_of(&s.title);
        let art = Rect::new(o.0 + 18, o.1 + 18, 160, 160);
        c.fill_round_grad(art, 16, a, b);
        c.glow_round(art, 16, art.x + 40, art.y + 30, 160, 0xFFFF_FFFF, 60);
        icons::centered(c, "note", 44, art.x + 80, art.y + 80, 0xB3FF_FFFF);
        let x = o.0 + 198;
        let right = o.0 + 620 - 18;
        // "Musica" where the reference has its app's wordmark.
        text::draw(c, Font::Strong15, x, o.1 + 18, "Musica", WHITE);
        self.draw_eq(c, right - 18, o.1 + 20, s.playing);
        text::draw(c, Font::Strong20, x, o.1 + 44, &text::fit(Font::Strong20, &s.title, right - x), WHITE);
        let album = if s.album.is_empty() { "—" } else { s.album.as_str() };
        text::draw(c, Font::Body, x, o.1 + 72, &text::fit(Font::Body, album, right - x), T2);
        let pos = match self.sh.seek {
            Some(v) => s.len * v / 1000,
            None => s.pos,
        };
        let (sh, _) = text::metrics(Font::Small);
        text::draw(c, Font::Small, x, o.1 + 111 - sh / 2, &mmss(pos), T2);
        text::draw_right(c, Font::Small, right, o.1 + 111 - sh / 2, &mmss(s.len), T2);
        let track = Rect::new(x + 40, o.1 + 109, right - x - 80, 5);
        c.fill_round(track, 3, 0x29FF_FFFF);
        let fw = if s.len > 0 { track.w * pos.min(s.len) as i32 / s.len as i32 } else { 0 };
        c.fill_round(Rect::new(track.x, track.y, fw.max(5), 5), 3, WHITE);
        if hot(Part::Seek) || self.sh.seek.is_some() {
            c.fill_circle(track.x + fw, track.y + 2, 7, WHITE);
        }
        let btn = |c: &mut Canvas, p: Part, r: Rect, icon: &'static str, size: i32, on: bool| {
            let r = at(o, r);
            if hot(p) {
                c.fill_circle(r.x + r.w / 2, r.y + r.h / 2, r.w / 2, FILL_H);
            }
            icons::centered(c, icon, size, r.x + r.w / 2, r.y + r.h / 2, if on { BRAND } else { WHITE });
        };
        btn(c, Part::Shuffle, Rect::new(267, 134, 34, 34), "shuffle", 17, s.shuffle);
        btn(c, Part::Prev, Rect::new(315, 131, 40, 40), "prev", 20, false);
        btn(c, Part::MNext, Rect::new(445, 131, 40, 40), "next", 20, false);
        btn(c, Part::Repeat, Rect::new(499, 134, 34, 34), "repeat", 17, s.repeat);
        let pr = at(o, Rect::new(369, 128, 62, 46));
        let col = if hot(Part::MPlay) { ui::lighten(BRAND, 30) } else { BRAND };
        c.fill_round(pr, if s.playing { 14 } else { 23 }, col);
        icons::centered(c, if s.playing { "pause" } else { "play" }, 22, pr.x + 31, pr.y + 23, WHITE);
    }

    fn draw_power(&self, c: &mut Canvas, o: (i32, i32)) {
        let hover = self.sh.hover;
        let (h, _) = text::metrics(Font::Strong20);
        icons::draw(c, "power", 20, o.0 + 24, o.1 + 24 + (h - 20) / 2, RED);
        text::draw(c, Font::Strong20, o.0 + 24 + 30, o.1 + 24, "Spegnere il computer?", WHITE);
        text::draw(c, Font::Body, o.0 + 24, o.1 + 62, "Le app aperte verranno chiuse.", T2);
        let (bh, _) = text::metrics(Font::Strong15);
        for (p, r, corners, label) in [
            (Part::Cancel, Rect::new(24, 110, 181, 50), [10, 10, 10, 22], "Annulla"),
            (Part::Confirm, Rect::new(215, 110, 181, 50), [10, 10, 22, 10], "Spegni"),
        ] {
            let r = at(o, r);
            let bg = match (p, hover == Some(p)) {
                (Part::Confirm, false) => RED,
                (Part::Confirm, true) => ui::lighten(RED, 25),
                (_, false) => FILL_H,
                (_, true) => 0x29FF_FFFF,
            };
            c.fill_round_corners(r, corners, bg);
            text::draw_centered(c, Font::Strong15, r.x + r.w / 2, r.y + (50 - bh) / 2, label, WHITE);
        }
    }

    /// A Windows program was opened and Wine must be prepared first: the
    /// bar turns into the preparation.
    pub(super) fn show_prep(&mut self, l: &mut Looper) {
        self.sh.prep_hidden = false;
        if self.sh.bar.state != BarState::Prep {
            self.sh.prep_shown.set((0, ms()));
        }
        if !self.sh.locked && self.sh.ending.is_none() {
            self.go(l, BarState::Prep);
        }
    }

    /// The bar follows the preparation: back to the dock when it ends,
    /// shown again when it fails.
    fn prep_step(&mut self, l: &mut Looper) {
        let v = crate::gui::apps::winprep::view();
        let failed = v.as_ref().is_some_and(|v| v.failed);
        if failed && !self.sh.prep_failed {
            self.sh.prep_hidden = false;
        }
        self.sh.prep_failed = failed;
        let s = self.sh.bar.state;
        match v {
            None if s == BarState::Prep => {
                self.sh.prep_hidden = false;
                self.go(l, BarState::Idle);
            }
            Some(_) if s == BarState::Idle && !self.sh.prep_hidden && !self.sh.locked => self.go(l, BarState::Prep),
            _ => {}
        }
    }

    /// The preparation's buttons: Annulla; after a failure Chiudi and
    /// Riprova.
    fn prep_buttons(failed: bool) -> Vec<(Part, Rect)> {
        if failed {
            alloc::vec![(Part::Cancel, Rect::new(520 - 22 - 212, 60, 100, 30)), (Part::Confirm, Rect::new(520 - 22 - 100, 60, 100, 30))]
        } else {
            alloc::vec![(Part::Cancel, Rect::new(520 - 22 - 100, 20, 100, 32))]
        }
    }

    fn draw_prep(&self, c: &mut Canvas, o: (i32, i32)) {
        let Some(v) = crate::gui::apps::winprep::view() else { return };
        // The Windows logo in an orange tile.
        let tile = Rect::new(o.0 + 22, o.1 + 22, 60, 60);
        let (top, bottom) = if v.failed { (0xFF55_5260, 0xFF3A_3844) } else { ui::ORANGE_GRAD };
        c.fill_round_grad(tile, 18, top, bottom);
        c.sheen_round(tile, 18, 1, ui::TILE_SHEEN);
        for (i, j) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            c.fill_round(Rect::new(tile.x + 17 + i * 15, tile.y + 17 + j * 15, 12, 12), 3, WHITE);
        }
        let x = o.0 + 98;
        let buttons = Self::prep_buttons(v.failed);
        let room = if v.failed { 520 - 98 - 22 } else { 520 - 98 - 22 - 112 };
        let heading = match (v.failed, v.update) {
            (true, _) => "Windows non è pronto",
            (false, true) => "Aggiorno i programmi Windows",
            (false, false) => "Preparo i programmi Windows",
        };
        text::draw(c, Font::Strong16, x, o.1 + 20, &text::fit(Font::Strong16, heading, room), WHITE);
        let who = if v.more > 0 { format!("«{}» e altri {}", v.name, v.more) } else { format!("«{}»", v.name) };
        if v.failed {
            text::draw(c, Font::Small, x, o.1 + 42, &text::fit(Font::Small, &format!("Puoi riprovare ({})", v.why), 520 - 98 - 22), T2);
        } else {
            text::draw(c, Font::Small, x, o.1 + 42, &text::fit(Font::Small, &format!("{who} si apre da solo"), room), T2);
            // The bar glides to the target, never backwards.
            let now = ms();
            let (mut shown, last) = self.sh.prep_shown.get();
            if v.target < shown && v.target < 50 {
                shown = v.target;
            }
            let dt = now.saturating_sub(last).min(500);
            if v.target > shown {
                let step = ((v.target - shown) as u64 * dt / (dt + 450)).max(u64::from(dt > 0)) as u32;
                shown = (shown + step).min(v.target);
            }
            self.sh.prep_shown.set((shown, now));
            let track = Rect::new(x, o.1 + 72, 520 - 98 - 22, 6);
            c.fill_round(track, 3, 0x29FF_FFFF);
            let fw = (track.w as i64 * shown.min(1000) as i64 / 1000) as i32;
            if fw > 0 {
                c.fill_round_grad(Rect::new(track.x, track.y, fw.max(6), 6), 3, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
            }
            let elapsed = now.saturating_sub(v.began) / 1000;
            let note = if v.cancelling {
                String::from("Annullo…")
            } else if v.settling {
                String::from("Ultimi ritocchi…")
            } else if elapsed >= 12 && shown >= 40 {
                let left = elapsed * (1000 - u64::from(shown.min(999))) / u64::from(shown);
                format!("{} rimasti", crate::gui::apps::winprep::how_long(left))
            } else {
                String::from("Calcolo il tempo che manca…")
            };
            text::draw(c, Font::Small, x, o.1 + 82, &format!("{}%", shown / 10), T3);
            text::draw_right(c, Font::Small, track.right(), o.1 + 82, &note, T3);
        }
        if v.cancelling {
            return;
        }
        let (bh, _) = text::metrics(Font::Strong15);
        for (p, r) in buttons {
            let r = at(o, r);
            let hot = self.sh.hover == Some(p);
            let (bg, label) = match p {
                Part::Confirm => (if hot { ui::lighten(ui::ORANGE, 25) } else { ui::ORANGE }, "Riprova"),
                _ => (if hot { 0x29FF_FFFF } else { FILL_H }, if v.failed { "Chiudi" } else { "Annulla" }),
            };
            c.fill_round(r, r.h / 2, bg);
            text::draw_centered(c, Font::Strong15, r.x + r.w / 2, r.y + (r.h - bh) / 2, label, WHITE);
        }
    }

    fn draw_charge(&self, c: &mut Canvas, o: (i32, i32)) {
        let pct = self.sh.battery.map_or(100, |b| b.0) as i32;
        let (h, _) = text::metrics(Font::Strong15);
        icons::draw(c, "bolt", 22, o.0 + 22, o.1 + 23, GREEN);
        text::draw(c, Font::Strong15, o.0 + 22 + 22 + 12, o.1 + (68 - h) / 2, "In carica", WHITE);
        let s = format!("{pct} %");
        let sw = text::width(Font::Strong15, &s);
        text::draw_right(c, Font::Strong15, o.0 + 330 - 22, o.1 + (68 - h) / 2, &s, WHITE);
        let cell = Rect::new(o.0 + 330 - 22 - sw - 14 - 46, o.1 + 22, 46, 24);
        c.stroke_round(cell, 7, 2, 0x80FF_FFFF);
        c.fill_round(Rect::new(cell.right() + 1, cell.y + 8, 3, 8), 1, 0x80FF_FFFF);
        let inner = cell.grow(-4);
        c.fill_round(Rect::new(inner.x, inner.y, inner.w * pct.clamp(0, 100) / 100, inner.h), 3, GREEN);
    }
}
