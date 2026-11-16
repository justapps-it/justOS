//! The installer's screens: justOS from the medium it runs from (a USB
//! stick, a live CD) onto a disk of the computer, which is erased. Shown
//! when the interface starts on such a medium, before the first-start
//! setup ([`crate::install::offer_at_start`]); what is written, and how, is
//! in [`crate::install`].
//!
//! 1. Welcome: install, or try justOS without installing (no disk is
//!    written then).
//! 2. The disks: every disk of the computer but the ones justOS runs from,
//!    with its size, bus and what is on it; those it does not fit on are
//!    listed greyed out and cannot be chosen. The list follows disks
//!    plugged in or out.
//! 3. The confirmation, for the chosen disk: its name, size, bus, what is on
//!    it and its partitions, and in plain words that all of it will be
//!    erased. "Cancella e installa" is not the default (the keyboard starts
//!    on "Indietro") and does nothing until the box "Ho capito, cancella
//!    questo disco" is ticked, with a click or the space bar. Going back,
//!    or the disk changing meanwhile (its erase code is made from its first
//!    sectors and is checked again when the installation starts), unticks
//!    it.
//! 4. The installation: the bar, the step, the time left, a few true things
//!    about justOS.
//! 5. Done (restart now), or failed: what happened and what state the disk
//!    is in (try again, or turn the computer off).

use super::gfx::{Canvas, Rect};
use super::res;
use super::settings;
use super::splash::draw_logo;
use super::text::{self, Font};
use super::ui::{self, Anim, Event, Looper, View};
use super::Display;
use crate::dev::input::{Key, KeyEvent};
use crate::fs;
use crate::install::{self, Failure, Medium, Progress, State, Target};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The disks in soft pixels, by bus: 'o' are lights, 'm' metal. A drive
/// (SATA, IDE, virtio, anything else), a chip (NVMe), a stick (USB, a
/// memory card).
const DRIVE: [&str; 7] = [".##########.", "############", "############", "............", "############", "#######o#o##", ".##########."];
const CHIP: [&str; 9] = [
    "..#.#.#.#..",
    "...........",
    "#.#######.#",
    "..#######..",
    "#.###o###.#",
    "..#######..",
    "#.#######.#",
    "...........",
    "..#.#.#.#..",
];
const STICK: [&str; 7] = [".#######....", "#########mmm", "#########m.m", "##o######mmm", "#########m.m", "#########mmm", ".#######...."];
/// A warning sign: 'o' is the mark in it.
const WARNING: [&str; 10] = [
    ".....#.....",
    "....###....",
    "....###....",
    "...##o##...",
    "...##o##...",
    "..###o###..",
    "..#######..",
    ".####o####.",
    ".#########.",
    "###########",
];
/// A tick.
const TICK: [&str; 5] = ["......#", ".....#.", "#...#..", ".#.#...", "..#...."];

/// How often the disks are looked at again (ms).
const LOOK_MS: u64 = 2000;
/// How long a tip stays, and how long it fades in and out (ms).
const TIP_MS: u64 = 8000;
const FADE_MS: u64 = 400;
/// While it installs, the screen moves at most this often (ms), and never
/// takes more than a quarter of the processor: the copy needs it more on a
/// slow computer.
const WORK_FRAME_MS: u64 = 40;
/// The disk rows: their height and the gap between them.
const ROW: i32 = 76;
const GAP: i32 = 10;
/// The dots along the bottom: welcome, disks, confirmation, installation.
const STEPS: usize = 4;

const UNDERSTOOD: &str = "Ho capito, cancella questo disco";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Welcome,
    Disks,
    Confirm,
    Working,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Try,
    Install,
    Back,
    Next,
    Disk(usize),
    Understood,
    Erase,
    Retry,
    Continue,
    Restart,
    PowerOff,
}

/// The words for what justOS runs from.
struct Words {
    /// "Ora justOS funziona dalla chiavetta USB."
    from: &'static str,
    /// "Togli la chiavetta."
    item: &'static str,
    /// "Se riparte dalla chiavetta..."
    again: &'static str,
}

fn words(m: Medium) -> Words {
    match m {
        Medium::Usb => Words { from: "dalla chiavetta USB", item: "la chiavetta", again: "dalla chiavetta" },
        Medium::Cd => Words { from: "dal CD", item: "il CD", again: "dal CD" },
        Medium::Card => Words { from: "dalla scheda di memoria", item: "la scheda", again: "dalla scheda" },
        Medium::Disk => Words { from: "da un disco esterno", item: "il disco esterno", again: "dal disco esterno" },
    }
}

/// Where things are on the current screen.
struct Lay {
    card: Rect,
    controls: Vec<(Control, Rect)>,
    /// The disk rows' area.
    list: Rect,
}

struct Installer {
    screen: Screen,
    /// When the screen came: its content fades in.
    since: Anim,
    w: i32,
    h: i32,
    blur: Canvas,
    mouse: (i32, i32),
    /// The keyboard is in use: the focused control shows a ring.
    keyboard: bool,
    focus: Control,
    medium: Medium,
    /// The space the system needs.
    need: u64,
    disks: Vec<Target>,
    chosen: Option<usize>,
    /// The first disk row shown.
    scroll: usize,
    /// When the disks were last looked at (ms), and whether they have been.
    looked: u64,
    looked_once: bool,
    /// A line on the disks screen: what changed.
    notice: Option<String>,
    /// The disk confirmed, then installed on.
    target: Option<Target>,
    understood: bool,
    /// "Cancella e installa" was pressed before the box was ticked: the box
    /// glows for a moment.
    nudge: Option<Anim>,
    progress: Progress,
    /// The bar as shown, 0..=1000: it glides towards the progress.
    bar: i32,
    bar_at: u64,
    /// When the installation screen was last drawn (ms), and how long that
    /// took (µs).
    drawn_at: u64,
    draw_us: u64,
    started: u64,
    /// The time left, smoothed (ms).
    eta: Option<u64>,
    /// The installation finished (when, and the version): the bar fills up
    /// before the last screen comes.
    finished: Option<(u64, String)>,
    version: String,
    failure: Option<Failure>,
    tips: Vec<&'static str>,
    leave: bool,
}

fn now_ms() -> u64 {
    crate::time::uptime_ms()
}

fn exists(path: &str) -> bool {
    fs::abs(path).is_ok_and(|p| fs::exists(&p))
}

/// Things about justOS that are true on this system, shown while it
/// installs.
fn tips() -> Vec<&'static str> {
    let mut v = Vec::new();
    if exists("sys:/usr/bin/firefox-esr") {
        v.push("Firefox è già installato: cercalo con Ctrl+Spazio, come ogni app, file o impostazione.");
    } else {
        v.push("Ctrl+Spazio cerca app, file e impostazioni, e fa anche i conti.");
    }
    if exists("sys:/usr/lib/wine/wine64") {
        v.push("Le app di Windows e di Linux si aprono con un doppio clic: Wine è già installato.");
    } else {
        v.push("I programmi per Linux si aprono con un doppio clic.");
    }
    v.push("Gli aggiornamenti di justOS vanno nel secondo slot del sistema: se la nuova versione non parte, il computer torna da solo a quella di prima.");
    v.push("Nel Negozio ci sono giochi come 2048 e Sudoku, e strumenti come BusyBox e Lua.");
    v.push("Il tema chiaro o scuro si sceglie nelle Impostazioni.");
    v.push("F3 mostra tutte le finestre e i desktop virtuali.");
    v.push("Logo+L blocca lo schermo; Stamp salva una schermata in Immagini.");
    v
}

/// "1 partizione", "3 partizioni".
fn parts_count(n: usize) -> String {
    match n {
        0 => String::from("nessuna partizione"),
        1 => String::from("1 partizione"),
        n => format!("{n} partizioni"),
    }
}

/// A message as a sentence: capital first letter, full stop.
fn sentence(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

fn eta_text(ms: Option<u64>) -> String {
    match ms {
        None => String::from("Calcolo il tempo che manca…"),
        Some(ms) if ms < 50_000 => String::from("Manca meno di un minuto"),
        Some(ms) => match (ms + 30_000) / 60_000 {
            0 | 1 => String::from("Manca circa un minuto"),
            m => format!("Mancano circa {m} minuti"),
        },
    }
}

/// `s` in lines of at most `w` units from (`x`, `y`), `line` apart (centred
/// on `x` if `centred`); the y below the last.
#[allow(clippy::too_many_arguments)]
fn paragraph(c: &mut Canvas, font: Font, x: i32, y: i32, w: i32, s: &str, color: u32, line: i32, centred: bool) -> i32 {
    let mut y = y;
    for l in text::wrap(font, s, w) {
        if centred {
            text::draw_centered(c, font, x, y, &l, color);
        } else {
            text::draw(c, font, x, y, &l, color);
        }
        y += line;
    }
    y
}

fn with_alpha(color: u32, a: u32) -> u32 {
    (color & 0xFF_FFFF) | ((((color >> 24) * a.min(255)) / 255) << 24)
}

/// The corner of a button `h` units high (as `ui::button`).
fn button_rad(h: i32) -> i32 {
    14.min(h / 2)
}

/// A button that cannot be pressed yet.
fn button_off(c: &mut Canvas, r: Rect, label: &str) {
    c.fill_round(r, button_rad(r.h), ui::cell());
    let (lh, _) = text::metrics(Font::Strong);
    text::draw_centered(c, Font::Strong, r.x + r.w / 2, r.y + (r.h - lh) / 2, label, ui::text_faint());
}

/// A disk on a tile, its picture by its bus.
fn drive_tile(c: &mut Canvas, r: Rect, bus: &str, bright: bool) {
    c.fill_round(r, r.w * 7 / 26, ui::cell());
    let (body, lights, metal) =
        if bright { (ui::text_dim(), ui::ACCENT, ui::text_faint()) } else { (ui::text_faint(), ui::text_faint(), ui::text_faint()) };
    let color = |k: u8| match k {
        b'o' => lights,
        b'm' => metal,
        _ => body,
    };
    match bus {
        "NVMe" => ui::soft_pixels_keyed(c, &CHIP, r, r.w * 3200 / 52, &color),
        "USB" | "scheda SD" => ui::soft_pixels_keyed(c, &STICK, r, r.w * 3000 / 52, &color),
        _ => ui::soft_pixels_keyed(c, &DRIVE, r, r.w * 3000 / 52, &color),
    }
}

/// A round badge with a white symbol (done, failed).
fn badge(c: &mut Canvas, cx: i32, cy: i32, rad: i32, color: u32, glyph: &[&str], cell: i32) {
    c.shadow(Rect::new(cx - rad, cy - rad, rad * 2, rad * 2), rad, 10, ui::shadow_strength(60));
    c.fill_circle(cx, cy, rad, color);
    ui::soft_pixels_in(c, glyph, Rect::new(cx - rad, cy - rad, rad * 2, rad * 2), cell, ui::WHITE);
}

/// 0..=1000..=0 over `period` ms, eased: a slow breath.
fn breath(t: u64, period: u64) -> u32 {
    let x = (t % period) * 2000 / period;
    ui::ease_in_out(if x < 1000 { x as u32 } else { (2000 - x) as u32 })
}

impl Installer {
    fn card(&self) -> Rect {
        let (cw, ch) = (720.min(self.w - 32), 560.min(self.h - 32));
        Rect::new((self.w - cw) / 2, (self.h - ch) / 2, cw, ch)
    }

    fn rows_shown(list: Rect) -> usize {
        ((list.h + GAP) / (ROW + GAP)).max(1) as usize
    }

    fn layout(&self) -> Lay {
        let card = self.card();
        let cx = card.x + card.w / 2;
        let by = card.bottom() - 92;
        let bw = 240.min((card.w - 96) / 2);
        let left = Rect::new(cx - 8 - bw, by, bw, 50);
        let right = Rect::new(cx + 8, by, bw, 50);
        let list = Rect::new(card.x + 40, card.y + 132, card.w - 80, by - 26 - (card.y + 132));
        let mut controls = Vec::new();
        match self.screen {
            Screen::Welcome => {
                controls.push((Control::Try, left));
                controls.push((Control::Install, right));
            }
            Screen::Disks => {
                let n = Self::rows_shown(list);
                for i in self.scroll..self.disks.len().min(self.scroll + n) {
                    let y = list.y + (i - self.scroll) as i32 * (ROW + GAP);
                    controls.push((Control::Disk(i), Rect::new(list.x, y, list.w, ROW)));
                }
                controls.push((Control::Back, left));
                controls.push((Control::Next, right));
            }
            Screen::Confirm => {
                let label = text::width(Font::Strong, UNDERSTOOD);
                controls.push((Control::Understood, Rect::new(card.x + 40, by - 60, 46 + label, 34)));
                controls.push((Control::Back, left));
                controls.push((Control::Erase, right));
            }
            Screen::Working => {}
            Screen::Done => {
                controls.push((Control::Continue, left));
                controls.push((Control::Restart, right));
            }
            Screen::Failed => {
                controls.push((Control::PowerOff, left));
                controls.push((Control::Retry, right));
            }
        }
        Lay { card, controls, list }
    }

    /// Whether a control can be pressed (and so take the keyboard).
    fn enabled(&self, c: Control) -> bool {
        match c {
            Control::Next => self.chosen.and_then(|i| self.disks.get(i)).is_some_and(Target::usable),
            Control::Erase => self.understood && self.target.is_some(),
            Control::Disk(i) => self.disks.get(i).is_some_and(Target::usable),
            _ => true,
        }
    }

    fn default_focus(&self) -> Control {
        match self.screen {
            Screen::Welcome => Control::Install,
            Screen::Disks if self.enabled(Control::Next) => Control::Next,
            Screen::Disks => self.disks.iter().position(Target::usable).map_or(Control::Back, Control::Disk),
            // Never the erase button.
            Screen::Confirm | Screen::Working => Control::Back,
            Screen::Done => Control::Restart,
            Screen::Failed => Control::Retry,
        }
    }

    fn go(&mut self, s: Screen) {
        self.screen = s;
        self.since = Anim::new(280);
        self.understood = false;
        self.nudge = None;
        if s == Screen::Disks {
            self.look();
        }
        if s == Screen::Confirm {
            self.looked = now_ms();
        }
        self.focus = self.default_focus();
    }

    /// Reads the disks again; keeps the one chosen if it did not change.
    fn look(&mut self) {
        let was = self.chosen.and_then(|i| self.disks.get(i)).map(|t| (t.disk.name.clone(), t.code.clone(), t.name.clone()));
        self.disks = install::targets();
        self.need = install::needed();
        self.looked = now_ms();
        self.chosen = None;
        if let Some((disk, code, name)) = was {
            match self.disks.iter().position(|t| t.disk.name == disk) {
                Some(i) if self.disks[i].code == code && self.disks[i].usable() => self.chosen = Some(i),
                Some(_) => self.notice = Some(format!("Il disco «{name}» è cambiato: controllalo e sceglilo di nuovo.")),
                None => self.notice = Some(format!("Il disco «{name}» non c'è più.")),
            }
        }
        if !self.looked_once {
            // Only one disk it fits on: that one.
            let usable: Vec<usize> = (0..self.disks.len()).filter(|&i| self.disks[i].usable()).collect();
            if usable.len() == 1 {
                self.chosen = Some(usable[0]);
            }
            self.looked_once = true;
        }
        let n = Self::rows_shown(self.layout().list);
        self.scroll = self.scroll.min(self.disks.len().saturating_sub(n));
        if let Some(i) = self.chosen {
            self.show_row(i, n);
        }
        if let Control::Disk(i) = self.focus {
            if !self.enabled(Control::Disk(i)) {
                self.focus = self.default_focus();
            }
        }
    }

    fn show_row(&mut self, i: usize, n: usize) {
        if i < self.scroll {
            self.scroll = i;
        } else if i >= self.scroll + n {
            self.scroll = i + 1 - n;
        }
    }

    /// On the confirmation: the disk must still be there as it was.
    fn recheck(&mut self) {
        self.looked = now_ms();
        let Some(t) = &self.target else { return };
        let same = install::targets().iter().any(|n| n.disk.name == t.disk.name && n.code == t.code && n.usable());
        if !same {
            self.notice = Some(format!("Il disco «{}» è cambiato o non c'è più: controlla e scegli di nuovo.", t.name));
            self.target = None;
            self.chosen = None;
            self.go(Screen::Disks);
        }
    }

    fn activate(&mut self, c: Control) {
        if !self.enabled(c) {
            if c == Control::Erase {
                self.nudge = Some(Anim::new(900));
            }
            return;
        }
        match c {
            Control::Try | Control::Continue => self.leave = true,
            Control::Install | Control::Retry => {
                self.notice = None;
                self.go(Screen::Disks);
            }
            Control::Back => match self.screen {
                Screen::Disks => self.go(Screen::Welcome),
                Screen::Confirm => {
                    self.target = None;
                    self.go(Screen::Disks);
                }
                _ => {}
            },
            Control::Disk(i) => {
                self.chosen = Some(i);
                self.notice = None;
                self.focus = Control::Next;
            }
            Control::Next => {
                if let Some(t) = self.chosen.and_then(|i| self.disks.get(i)).cloned() {
                    crate::info!("install", "confirmation shown for {} ({}, {})", t.disk.name, t.name, install::size_text(t.size));
                    self.target = Some(t);
                    self.go(Screen::Confirm);
                }
            }
            Control::Understood => {
                self.understood = !self.understood;
                self.nudge = None;
            }
            Control::Erase => self.erase(),
            Control::Restart => crate::acpi::reboot(),
            Control::PowerOff => {
                crate::acpi::power_off();
                // A computer that cannot turn itself off: the live system
                // goes on.
                self.leave = true;
            }
        }
    }

    fn erase(&mut self) {
        let Some(t) = self.target.clone() else { return };
        if !self.understood {
            return;
        }
        crate::info!("install", "erasing {} confirmed on screen", t.disk.name);
        match install::start(t.disk.name.clone(), t.code.clone()) {
            Ok(()) => {
                let now = now_ms();
                self.progress = Progress::default();
                self.bar = 0;
                self.bar_at = now;
                self.started = now;
                self.eta = None;
                self.finished = None;
                self.go(Screen::Working);
            }
            Err(e) => {
                self.failure = Some(Failure { message: e, touched: false });
                self.go(Screen::Failed);
            }
        }
    }

    /// Follows the installation: called every frame while it runs.
    fn poll(&mut self) {
        let now = now_ms();
        match install::state() {
            State::Working(p) => self.progress = p,
            State::Done(v) => {
                if self.finished.is_none() {
                    self.progress.permille = 1000;
                    self.finished = Some((now, v));
                }
            }
            State::Failed(f) => {
                self.failure = Some(f);
                self.go(Screen::Failed);
                return;
            }
            State::Idle => {}
        }
        // The bar glides: about a tenth of the way each frame at 60 Hz.
        let dt = now.saturating_sub(self.bar_at).min(100) as i32;
        self.bar_at = now;
        let target = self.progress.permille.min(1000) as i32;
        if self.bar < target {
            self.bar = (self.bar + ((target - self.bar) * dt * 6 / 1000).max(1)).min(target);
        }
        if let Some((at, v)) = &self.finished {
            if self.bar >= 1000 && now >= at + 700 {
                self.version = v.clone();
                self.go(Screen::Done);
            }
        }
    }

    fn update_eta(&mut self) {
        let p = self.progress.permille.min(999) as u64;
        let elapsed = now_ms().saturating_sub(self.started);
        if p < 20 || elapsed < 5000 {
            return;
        }
        let raw = elapsed * (1000 - p) / p;
        self.eta = Some(match self.eta {
            // Counted down meanwhile, then nudged towards the new guess.
            Some(e) => (e.saturating_sub(100) * 9 + raw) / 10,
            None => raw,
        });
    }

    fn hovered(&self, lay: &Lay) -> Option<Control> {
        let (x, y) = self.mouse;
        lay.controls.iter().find(|(_, r)| r.contains(x, y)).map(|c| c.0)
    }

    /// Moves the keyboard to the next control that can be pressed (back with
    /// `back`).
    fn tab(&mut self, lay: &Lay, back: bool) {
        let order: Vec<Control> = lay.controls.iter().map(|c| c.0).filter(|&c| self.enabled(c)).collect();
        if order.is_empty() {
            return;
        }
        let at = order.iter().position(|&c| c == self.focus);
        let n = order.len();
        self.focus = order[match (at, back) {
            (None, _) => 0,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        }];
    }

    /// Up or down the disks: the next one it fits on is chosen.
    fn step_disk(&mut self, lay: &Lay, down: bool) {
        let usable: Vec<usize> = (0..self.disks.len()).filter(|&i| self.disks[i].usable()).collect();
        if usable.is_empty() {
            return;
        }
        let at = self.chosen.and_then(|c| usable.iter().position(|&i| i == c));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(usable.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        let i = usable[next];
        self.chosen = Some(i);
        self.notice = None;
        self.focus = Control::Disk(i);
        self.show_row(i, Self::rows_shown(lay.list));
    }

    fn key(&mut self, k: KeyEvent, lay: &Lay) {
        let buttons = !matches!(self.focus, Control::Disk(_) | Control::Understood);
        match k.key {
            Key::Tab => self.tab(lay, k.shift),
            Key::Left | Key::Right if buttons => {
                // Between the two buttons at the bottom.
                let two: Vec<Control> = lay.controls.iter().rev().take(2).map(|c| c.0).collect();
                let want = if k.key == Key::Left { two.get(1) } else { two.first() };
                if let Some(&c) = want {
                    if self.enabled(c) {
                        self.focus = c;
                    }
                }
            }
            Key::Up | Key::Down if self.screen == Screen::Disks => self.step_disk(lay, k.key == Key::Down),
            Key::Enter | Key::Char(' ') => {
                let f = self.focus;
                self.activate(f);
            }
            Key::Escape => match self.screen {
                Screen::Disks => self.go(Screen::Welcome),
                Screen::Confirm => {
                    self.target = None;
                    self.go(Screen::Disks);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The parts of the installation screen that move.
    fn working_rects(card: Rect) -> [Rect; 3] {
        let cx = card.x + card.w / 2;
        [
            // The logo and its glow.
            Rect::new(cx - 130, card.y + 2, 260, 128).intersect(&card),
            // The bar, its labels and what is copied.
            Rect::new(card.x + 40, card.y + 216, card.w - 80, 100),
            // The tip.
            Rect::new(card.x + 40, card.y + 334, card.w - 80, 100),
        ]
    }

    fn tip(&self) -> (&'static str, u32) {
        if self.tips.is_empty() {
            return ("", 0);
        }
        let t = now_ms().saturating_sub(self.started);
        let i = (t / TIP_MS) as usize % self.tips.len();
        let into = t % TIP_MS;
        let a = if into < FADE_MS {
            into * 255 / FADE_MS
        } else if into > TIP_MS - FADE_MS {
            (TIP_MS - into) * 255 / FADE_MS
        } else {
            255
        };
        (self.tips[i], a as u32)
    }

    // ------------------------------------------------------------------------
    // Drawing

    fn draw_welcome(&self, c: &mut Canvas, card: Rect) {
        let cx = card.x + card.w / 2;
        let w = words(self.medium);
        draw_logo(c, cx, card.y + 96, 35, ui::knob_glyph());
        text::draw_centered(c, Font::Title, cx, card.y + 140, "Installa justOS sul computer", ui::text());
        let intro = format!(
            "Ora justOS funziona {}. Installalo su un disco di questo computer: partirà da lì, più veloce, e {} non servirà più.",
            w.from, w.item
        );
        let y = paragraph(c, Font::Body, cx, card.y + 196, card.w - 150, &intro, ui::text_dim(), 21, true);
        let mut points = alloc::vec![
            format!("Ci vogliono pochi minuti, poi togli {} e riavvii.", w.item),
            format!("Serve un disco da almeno {}. Prima di cancellarlo chiedo conferma.", install::size_text(self.need)),
            String::from("Se scegli di provare soltanto, nessun disco viene toccato."),
        ];
        if self.medium == Medium::Cd && !exists("sys:/usr/bin/firefox-esr") {
            points.push(String::from("Dal CD si installa il sistema del CD, senza Firefox e Wine."));
        }
        let panel = Rect::new(card.x + 70, y + 16, card.w - 140, points.len() as i32 * 32 + 20);
        ui::well_pane(c, panel, 16);
        let (lh, _) = text::metrics(Font::Body);
        for (i, p) in points.iter().enumerate() {
            let py = panel.y + 10 + i as i32 * 32;
            c.fill_round(Rect::new(panel.x + 20, py + 12, 8, 8), 2, ui::ACCENT);
            text::draw(c, Font::Body, panel.x + 42, py + (32 - lh) / 2, &text::fit(Font::Body, p, panel.w - 60), ui::text());
        }
    }

    fn draw_disk_row(&self, c: &mut Canvas, r: Rect, i: usize, hover: bool) {
        let t = &self.disks[i];
        let usable = t.usable();
        let chosen = self.chosen == Some(i);
        let bg = if chosen {
            ui::sel()
        } else if hover && usable {
            ui::hover()
        } else {
            ui::pane()
        };
        c.fill_round(r, 18, bg);
        c.stroke_round(r, 18, if chosen { 2 } else { 1 }, if chosen { ui::ACCENT } else { ui::line() });
        drive_tile(c, Rect::new(r.x + 14, r.y + (r.h - 52) / 2, 52, 52), t.bus, usable);
        let (main, dim) = if usable { (ui::text(), ui::text_dim()) } else { (ui::text_faint(), ui::text_faint()) };
        let x = r.x + 82;
        let right = r.right() - if usable { 58 } else { 22 };
        let size = install::size_text(t.size);
        text::draw_right(c, Font::Heading, right, r.y + 14, &size, main);
        let room = right - text::width(Font::Heading, &size) - 16 - x;
        text::draw(c, Font::Heading, x, r.y + 14, &text::fit(Font::Heading, &t.name, room), main);
        let sub = match &t.problem {
            Some(p) => format!("{} · {p}", t.bus),
            None => format!("{} · {}", t.bus, t.contents),
        };
        text::draw(c, Font::Small, x, r.y + 44, &text::fit(Font::Small, &sub, right - x), dim);
        if usable {
            let (rx, ry) = (r.right() - 30, r.y + r.h / 2);
            if chosen {
                c.fill_circle(rx, ry, 11, ui::ACCENT);
                c.fill_circle(rx, ry, 5, ui::WHITE);
            } else {
                c.fill_circle(rx, ry, 11, ui::line());
                c.fill_circle(rx, ry, 9, bg);
            }
        }
    }

    fn draw_disks(&self, c: &mut Canvas, lay: &Lay) {
        let card = lay.card;
        let cx = card.x + card.w / 2;
        text::draw_centered(c, Font::Title, cx, card.y + 40, "Scegli il disco", ui::text());
        text::draw_centered(c, Font::Body, cx, card.y + 92, "Tutto quello che c'è sul disco che scegli verrà cancellato.", ui::text_dim());
        if self.disks.is_empty() {
            let l = lay.list;
            drive_tile(c, Rect::new(cx - 30, l.y + 30, 60, 60), "", false);
            text::draw_centered(c, Font::Heading, cx, l.y + 110, "Non trovo dischi su cui installare justOS", ui::text());
            let w = words(self.medium);
            let s = format!(
                "Il disco da cui funziona justOS ({}) non compare qui. Collega un altro disco: comparirà da solo.",
                w.from.trim_start_matches("dalla ").trim_start_matches("dal ").trim_start_matches("da ")
            );
            paragraph(c, Font::Body, cx, l.y + 144, l.w - 60, &s, ui::text_dim(), 20, true);
        }
        let by = card.bottom() - 92;
        let shown = Self::rows_shown(lay.list);
        let line = if let Some(n) = &self.notice {
            Some((n.clone(), ui::ACCENT))
        } else if self.disks.len() > shown {
            Some((format!("{} dischi: scorri per vederli tutti", self.disks.len()), ui::text_faint()))
        } else {
            None
        };
        if let Some((s, color)) = line {
            text::draw_centered(c, Font::Small, cx, by - 22, &text::fit(Font::Small, &s, card.w - 80), color);
        }
    }

    fn draw_confirm(&self, c: &mut Canvas, card: Rect) {
        let Some(t) = &self.target else { return };
        let cx = card.x + card.w / 2;
        ui::soft_pixels_keyed(c, &WARNING, Rect::new(cx - 30, card.y + 28, 60, 48), 4400, &|k| if k == b'o' { ui::WHITE } else { ui::ACCENT });
        text::draw_centered(c, Font::Title, cx, card.y + 86, "Cancellare questo disco?", ui::text());
        let parts = t.parts_text();
        let part_lines = if t.parts.is_empty() { Vec::new() } else { text::wrap(Font::Small, &parts, card.w - 120) };
        let extra = (part_lines.len().clamp(1, 2) as i32 - 1) * 16;
        let p = Rect::new(card.x + 40, card.y + 142, card.w - 80, 128 + extra);
        c.fill_round(p, 18, ui::pane());
        c.stroke_round(p, 18, 1, ui::line());
        drive_tile(c, Rect::new(p.x + 16, p.y + 16, 52, 52), t.bus, true);
        let size = install::size_text(t.size);
        text::draw_right(c, Font::Heading, p.right() - 20, p.y + 18, &size, ui::text());
        let room = p.right() - 20 - text::width(Font::Heading, &size) - 16 - (p.x + 84);
        text::draw(c, Font::Heading, p.x + 84, p.y + 18, &text::fit(Font::Heading, &t.name, room), ui::text());
        text::draw(c, Font::Body, p.x + 84, p.y + 46, &format!("{} · {}", t.bus, parts_count(t.parts.len())), ui::text_dim());
        let what = format!("Questo disco {}.", t.contents);
        text::draw(c, Font::Strong, p.x + 20, p.y + 82, &text::fit(Font::Strong, &what, p.w - 40), ui::text());
        for (i, l) in part_lines.iter().take(2).enumerate() {
            let l = if i == 1 && part_lines.len() > 2 { text::fit(Font::Small, &format!("{l} …"), p.w - 40) } else { l.clone() };
            text::draw(c, Font::Small, p.x + 20, p.y + 104 + i as i32 * 16, &l, ui::text_dim());
        }
        let warn = "Se continui, tutto quello che c'è su questo disco verrà cancellato per sempre: file, foto, programmi e altri sistemi operativi. Non si potrà recuperare.";
        let mut y = paragraph(c, Font::Body, card.x + 40, p.bottom() + 16, card.w - 80, warn, ui::text(), 20, false);
        if let Some(n) = &t.note {
            text::draw(c, Font::Small, card.x + 40, y + 2, &text::fit(Font::Small, &sentence(n), card.w - 80), ui::text_dim());
            y += 18;
        }
        let _ = y;
    }

    fn draw_working(&self, c: &mut Canvas, card: Rect) {
        let cx = card.x + card.w / 2;
        let now = now_ms();
        let pulse = breath(now.saturating_sub(self.started), 2400);
        c.glow_round(card, 30, cx, card.y + 66, 120, ui::ACCENT, 16 + 34 * pulse / 1000);
        draw_logo(c, cx, card.y + 66, 35, ui::knob_glyph());
        // Under the mark, a wave runs through a row of soft pixels.
        let t = now.saturating_sub(self.started);
        for i in 0..9i32 {
            let a = breath(t + 1400 - i as u64 * 110, 1400);
            let cell = Rect::new(cx - 52 + i * 12, card.y + 96, 8, 8);
            c.fill_round(cell, 3, ui::cell());
            c.fill_round(cell, 3, with_alpha(ui::ACCENT, 40 + a * 215 / 1000));
        }
        text::draw_centered(c, Font::Title, cx, card.y + 120, "Installo justOS", ui::text());
        if let Some(t) = &self.target {
            let on = format!("su «{}» · {}", t.name, install::size_text(t.size));
            text::draw_centered(c, Font::Body, cx, card.y + 172, &text::fit(Font::Body, &on, card.w - 120), ui::text_dim());
        }
        // The bar and its words.
        let (bx, bw) = (card.x + 60, card.w - 120);
        let p = &self.progress;
        let phase = install::PHASES.get(p.phase).copied().unwrap_or("");
        text::draw(c, Font::Strong, bx, card.y + 222, phase, ui::text());
        text::draw_right(c, Font::Strong, bx + bw, card.y + 222, &format!("{}%", (self.bar + 5) / 10), ui::text());
        let bar = Rect::new(bx, card.y + 248, bw, 14);
        c.fill_round(bar, 7, ui::cell());
        let fw = (bar.w as i64 * self.bar as i64 / 1000) as i32;
        if fw > 0 {
            let f = Rect::new(bar.x, bar.y, fw.max(bar.h), bar.h);
            c.fill_round_grad(f, 7, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
            c.sheen_round(f, 7, 1, ui::TILE_SHEEN);
            // A light that runs along the filled part.
            let span = f.w as i64 + 160;
            let x = f.x - 80 + ((now % 1800) as i64 * span / 1800) as i32;
            c.glow_round(f, 7, x, f.y + f.h / 2, 60, ui::WHITE, 80);
        }
        text::draw(c, Font::Small, bx, card.y + 272, &eta_text(if self.finished.is_some() { Some(0) } else { self.eta }), ui::text_dim());
        text::draw_right(c, Font::Small, bx + bw, card.y + 272, &format!("Passo {} di {}", p.phase + 1, install::PHASES.len()), ui::text_dim());
        if !p.item.is_empty() {
            text::draw(c, Font::Small, bx, card.y + 292, &text::fit(Font::Small, &p.item, bw), ui::text_faint());
        }
        // A tip, fading in and out.
        let tr = Self::working_rects(card)[2];
        let (tip, a) = self.tip();
        c.fill_round(tr, 18, ui::pane());
        c.stroke_round(tr, 18, 1, ui::line());
        ui::caps(c, tr.x + 20, tr.y + 16, "Lo sapevi?", ui::ACCENT);
        paragraph(c, Font::Body, tr.x + 20, tr.y + 38, tr.w - 40, tip, with_alpha(ui::text(), a), 20, false);
        let w = words(self.medium);
        text::draw_centered(c, Font::Small, cx, card.bottom() - 64, &format!("Non spegnere il computer e non togliere {}.", w.item), ui::text_faint());
    }

    fn draw_done(&self, c: &mut Canvas, card: Rect) {
        let cx = card.x + card.w / 2;
        let pop = ui::lerp(24, 36, self.since.out());
        badge(c, cx, card.y + 122, pop, ui::GREEN, &TICK, pop * 1000 * 2 / 13);
        text::draw_centered(c, Font::Title, cx, card.y + 176, "justOS è installato", ui::text());
        let w = words(self.medium);
        let name = self.target.as_ref().map_or(String::from("il disco"), |t| format!("«{}»", t.name));
        let body = format!("Togli {} e riavvia: il computer partirà da {name}.", w.item);
        let y = paragraph(c, Font::Body, cx, card.y + 232, card.w - 140, &body, ui::text(), 21, true);
        let hint = format!(
            "Se riparte {} o non trova justOS, scegli {name} nel menu di avvio del computer: di solito si apre premendo F12, F11 o Esc appena lo accendi.",
            w.again
        );
        let y = paragraph(c, Font::Small, cx, y + 12, card.w - 160, &hint, ui::text_dim(), 18, true);
        if let Some(t) = &self.target {
            let what = format!("justOS {} · {} · {}", self.version, install::size_text(t.size), t.bus);
            text::draw_centered(c, Font::Small, cx, y + 14, &what, ui::text_faint());
        }
    }

    fn draw_failed(&self, c: &mut Canvas, card: Rect) {
        let cx = card.x + card.w / 2;
        let pop = ui::lerp(24, 36, self.since.out());
        badge(c, cx, card.y + 122, pop, ui::RED, &ui::GLYPH_CLOSE, pop * 1000 * 2 / 11);
        text::draw_centered(c, Font::Title, cx, card.y + 176, "L'installazione non è riuscita", ui::text());
        let Some(f) = &self.failure else { return };
        let y = paragraph(c, Font::Body, cx, card.y + 232, card.w - 140, &sentence(&f.message), ui::text(), 21, true);
        let name = self.target.as_ref().map_or(String::from("il disco"), |t| format!("«{}»", t.name));
        let state = if f.touched {
            format!("Il disco {name} è stato scritto in parte: quello che conteneva non c'è più e così com'è non parte. Puoi riprovare.")
        } else {
            String::from("Sul disco non è stato scritto niente: è com'era prima.")
        };
        paragraph(c, Font::Small, cx, y + 12, card.w - 160, &state, ui::text_dim(), 18, true);
    }

    fn draw_controls(&self, c: &mut Canvas, lay: &Lay) {
        let hover = self.hovered(lay);
        for &(ctl, r) in &lay.controls {
            let hot = hover == Some(ctl);
            match ctl {
                Control::Disk(i) => self.draw_disk_row(c, r, i, hot),
                Control::Understood => {
                    let b = Rect::new(r.x + 2, r.y + (r.h - 26) / 2, 26, 26);
                    if let Some(n) = &self.nudge {
                        if !n.done() {
                            let a = breath(n.elapsed(), 450) * 200 / 1000;
                            c.fill_round(b.grow(6), 13, with_alpha(ui::ACCENT, a));
                        }
                    }
                    if self.understood {
                        c.fill_round_grad(b, 8, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
                        c.sheen_round(b, 8, 1, ui::TILE_SHEEN);
                        ui::soft_pixels_in(c, &TICK, b, 2600, ui::WHITE);
                    } else {
                        ui::well_pane(c, b, 8);
                        c.stroke_round(b, 8, 2, if hot { ui::ACCENT } else { ui::text_faint() });
                    }
                    let (lh, _) = text::metrics(Font::Strong);
                    text::draw(c, Font::Strong, b.right() + 14, r.y + (r.h - lh) / 2, UNDERSTOOD, ui::text());
                }
                Control::Erase if self.understood => {
                    let bg = if hot { ui::lighten(ui::RED, 30) } else { ui::RED };
                    ui::button_color(c, r, "Cancella e installa", bg, ui::WHITE);
                }
                Control::Erase => button_off(c, r, "Cancella e installa"),
                Control::Next if !self.enabled(ctl) => button_off(c, r, "Avanti"),
                _ => {
                    let (label, primary) = match ctl {
                        Control::Try => ("Prova senza installare", false),
                        Control::Install => ("Installa justOS", true),
                        Control::Back => ("Indietro", false),
                        Control::Next => ("Avanti", true),
                        Control::Retry => ("Riprova", true),
                        Control::Continue => ("Continua a provare", false),
                        Control::PowerOff => ("Spegni", false),
                        _ => ("Riavvia ora", true),
                    };
                    ui::button(c, r, label, primary, hot);
                }
            }
            if self.keyboard && self.focus == ctl {
                let rad = if matches!(ctl, Control::Disk(_)) { 22 } else { button_rad(r.h) + 4 };
                c.stroke_round(r.grow(4), rad, 2, with_alpha(ui::ACCENT, 200));
            }
        }
    }

    fn draw_dots(&self, c: &mut Canvas, card: Rect) {
        let step = match self.screen {
            Screen::Welcome => 0,
            Screen::Disks => 1,
            Screen::Confirm => 2,
            _ => 3,
        };
        let cx = card.x + card.w / 2;
        let total = STEPS as i32 * 14 + 14;
        let mut dx = cx - total / 2;
        for i in 0..STEPS {
            let wide = i == step;
            let w = if wide { 26 } else { 8 };
            c.fill_round(Rect::new(dx, card.bottom() - 30, w, 8), 4, if wide { ui::ACCENT } else { ui::cell() });
            dx += w + 6;
        }
    }
}

impl View for Installer {
    fn resized(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        let wall = wallpaper();
        self.blur = res::wallpaper(wall, w, h).1;
    }

    fn animating(&self) -> bool {
        self.screen == Screen::Working || !self.since.done() || self.nudge.is_some_and(|n| !n.done())
    }

    fn event(&mut self, l: &mut Looper, ev: Event) -> bool {
        let lay = self.layout();
        let before = (self.screen, self.chosen, self.understood, self.focus, self.keyboard, self.scroll, self.hovered(&lay), self.notice.clone());
        match ev {
            Event::Move { x, y, .. } => {
                self.mouse = (x, y);
                self.keyboard = false;
            }
            Event::Press { x, y, double } => {
                self.mouse = (x, y);
                self.keyboard = false;
                if let Some(ctl) = self.hovered(&lay) {
                    if self.enabled(ctl) {
                        self.focus = ctl;
                    }
                    self.activate(ctl);
                    // A double click on a disk goes on to its confirmation.
                    if double && matches!(ctl, Control::Disk(_)) && self.screen == Screen::Disks {
                        self.activate(Control::Next);
                    }
                }
            }
            Event::Wheel { lines } if self.screen == Screen::Disks => {
                let n = Self::rows_shown(lay.list);
                let max = self.disks.len().saturating_sub(n);
                self.scroll = if lines > 0 { (self.scroll + 1).min(max) } else { self.scroll.saturating_sub(1) };
            }
            Event::Key(k) => {
                self.keyboard = true;
                self.key(k, &lay);
            }
            Event::Tick => {
                let now = now_ms();
                match self.screen {
                    Screen::Disks if now >= self.looked + LOOK_MS => {
                        self.look();
                        l.damage(lay.card);
                    }
                    Screen::Confirm if now >= self.looked + LOOK_MS => self.recheck(),
                    Screen::Working => self.update_eta(),
                    _ => {}
                }
            }
            Event::Frame => {
                if self.screen == Screen::Working {
                    self.poll();
                    let now = now_ms();
                    if self.screen == Screen::Working && now >= self.drawn_at + WORK_FRAME_MS.max(self.draw_us * 4 / 1000) {
                        self.drawn_at = now;
                        for r in Self::working_rects(lay.card) {
                            l.damage(r);
                        }
                    }
                }
                if !self.since.done() || self.nudge.is_some_and(|n| !n.done()) {
                    l.damage(ui::with_shadow(self.card()));
                }
            }
            _ => {}
        }
        let after = (self.screen, self.chosen, self.understood, self.focus, self.keyboard, self.scroll, self.hovered(&self.layout()), self.notice.clone());
        if after != before {
            l.damage(ui::with_shadow(self.card()));
        }
        if self.leave {
            crate::info!("install", "installer closed");
        }
        self.leave
    }

    fn draw(&mut self, c: &mut Canvas, r: Rect) {
        let t0 = crate::time::uptime_ns();
        self.draw_screen(c, r);
        if self.screen == Screen::Working {
            self.draw_us = crate::time::uptime_ns().saturating_sub(t0) / 1000;
        }
    }
}

impl Installer {
    fn draw_screen(&self, c: &mut Canvas, r: Rect) {
        // The wallpaper, softened and darkened, behind the card (as the
        // first-start setup).
        c.blit(&self.blur, r.x, r.y, r);
        c.fill(r, 0x5014_0E1C);
        let lay = self.layout();
        let card = lay.card;
        if !r.overlaps(&ui::with_shadow(card)) {
            return;
        }
        ui::glass(c, &self.blur, card, 30, 0xB023_1A2E);
        match self.screen {
            Screen::Welcome => self.draw_welcome(c, card),
            Screen::Disks => self.draw_disks(c, &lay),
            Screen::Confirm => self.draw_confirm(c, card),
            Screen::Working => self.draw_working(c, card),
            Screen::Done => self.draw_done(c, card),
            Screen::Failed => self.draw_failed(c, card),
        }
        self.draw_controls(c, &lay);
        self.draw_dots(c, card);
        if !self.since.done() {
            // The new screen fades in: the card's own colours over it,
            // thinning out.
            let a = (1000 - self.since.out()) * 255 / 1000;
            c.fill_round_grad(card.grow(-2), 28, with_alpha(ui::panel_top(), a), with_alpha(ui::panel_bottom(), a));
        }
    }
}

/// The wallpaper chosen (the first one before the setup).
fn wallpaper() -> &'static str {
    let s = settings::get();
    res::WALLPAPERS.iter().find(|(n, _)| *n == s.wallpaper).map_or(res::WALLPAPERS[0].0, |w| w.0)
}

pub fn run(d: &mut Display) {
    let (w, h) = (d.width(), d.height());
    let medium = install::medium();
    crate::info!("install", "installer shown (running from {:?})", medium);
    let mut me = Installer {
        screen: Screen::Welcome,
        since: Anim::new(280),
        w,
        h,
        blur: res::wallpaper(wallpaper(), w, h).1,
        mouse: (-1, -1),
        keyboard: false,
        focus: Control::Install,
        medium,
        need: install::needed(),
        disks: Vec::new(),
        chosen: None,
        scroll: 0,
        looked: 0,
        looked_once: false,
        notice: None,
        target: None,
        understood: false,
        nudge: None,
        progress: Progress::default(),
        bar: 0,
        bar_at: 0,
        drawn_at: 0,
        draw_us: 0,
        started: 0,
        eta: None,
        finished: None,
        version: String::new(),
        failure: None,
        tips: tips(),
        leave: false,
    };
    let mut l = Looper::new(d);
    l.damage_all(d);
    l.run(d, &mut me);
}
