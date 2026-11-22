//! Impostazioni: the settings, in sections, as other systems have them. At
//! the left a search field and the list of sections; at the right the
//! section chosen, as cards of rows: a label (with a line that explains it)
//! on the left, its control on the right. Privacy says what each app may use
//! outside its sandbox ([`crate::sandbox`]): a row per permission, which
//! opens the apps that asked for it ("Fotocamera: le app che la usano"),
//! then a card per app with a switch for each of its permissions. Wi-Fi
//! lists the networks around ([`crate::net::wifi`]): a click joins one,
//! asking for its password in a field under it when it has one and justOS
//! does not know it yet; the known networks can be forgotten.
//!
//! A section is a list of cards, each a list of [`Row`]s
//! ([`Prefs::cards`]); [`Prefs::page`] lays them out, and drawing, clicks,
//! the pointer and the search all work from that layout. A new setting is a
//! row there, plus what its [`Ctl`] draws ([`Prefs::draw_ctl`]), where its
//! parts are ([`Prefs::parts`]) and what a click does ([`Prefs::act`]).

use crate::dev::input::{Key, KeyEvent};
use crate::dev::keymap::Layout;
use crate::gui::gfx::{Canvas, Rect};
use crate::gui::res;
use crate::gui::settings;
use crate::gui::text::{self, Font};
use crate::gui::ui;
use crate::gui::wm::{Action, App, GAP};
use crate::sandbox::Answer;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};
use justdk::DisplayMode;

/// Resolutions offered when the display has them, in this order.
const COMMON_MODES: &[(u32, u32)] = &[
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (3840, 2160),
];

/// The sidebar's width.
const SIDEBAR: i32 = 210;
/// Where the list of sections starts (under the search field), the height
/// of a section's row and the step from one to the next.
const SIDE_TOP: i32 = 62;
const SIDE_ROW: i32 = 38;
const SIDE_STEP: i32 = 40;
/// The margin round the content, and its widest column.
const MARGIN: i32 = 28;
const COLUMN: i32 = 720;
/// The margin inside a card, its corner, and the gap between cards.
const INSET: i32 = 16;
const CARD_RAD: i32 = 18;
const CARD_GAP: i32 = 18;
/// The lowest row.
const ROW_MIN: i32 = 52;
/// The header above a card of search results (the section's tile and name).
const HEAD_H: i32 = 30;
/// How long a row stays lit after a search led to it.
const FLASH_MS: u64 = 1400;

/// The choices for locking after a while without input (minutes; 0: never).
const LOCK_AFTER: [(u32, &str); 4] = [(0, "Mai"), (2, "2 min"), (5, "5 min"), (15, "15 min")];

/// The sizes of the interface (pixels per unit; 0: by the screen).
const SIZES: [(u32, &str); 3] = [(0, "Auto"), (1, "100%"), (2, "200%")];

/// The choices of "Fluidità", as `pace::CHOICES` (0: automatic).
const FLUIDITY: [&str; 5] = ["Automatica", "60 Hz", "120 Hz", "144 Hz", "240 Hz"];

/// A section another part of the system asked to show (0: none; 1 + the
/// section's place in [`SECTIONS`]): the next window opened, or the one
/// open at its next tick, shows it.
static REQUESTED: AtomicU8 = AtomicU8::new(0);

fn request(s: Section) {
    let n = SECTIONS.iter().position(|x| *x == s).unwrap_or(0) + 1;
    REQUESTED.store(n as u8, Ordering::Relaxed);
    crate::dev::input::wake_gui_soon();
}

/// The settings open at Aspetto (the dock's menu).
pub fn request_look() {
    request(Section::Look);
}

/// The settings open at Schermo (the desktop's menu).
pub fn request_screen() {
    request(Section::Screen);
}

/// The settings open at Wi-Fi (the control centre's network tile).
pub fn request_wifi() {
    request(Section::Wifi);
}

fn take_requested() -> Option<Section> {
    match REQUESTED.swap(0, Ordering::Relaxed) {
        0 => None,
        n => SECTIONS.get(n as usize - 1).copied(),
    }
}

/// The themes of windows and panels.
const THEMES: [(ui::Theme, &str); 2] = [(ui::Theme::Dark, "Scuro"), (ui::Theme::Light, "Chiaro")];

/// The keyboard layouts.
const LAYOUTS: [(Layout, &str); 2] = [(Layout::It, "Italiano"), (Layout::Us, "English (US)")];

// The labels of the rows a note can appear under.
const L_NAME: &str = "Il tuo nome";
const L_DEVICE: &str = "Nome del dispositivo";
const L_SETUP: &str = "Configurazione iniziale";
const L_MODES: &str = "Risoluzione";
const L_MONITORS: &str = "Monitor";
const L_SWAP: &str = "Disposizione";
const L_MIRROR: &str = "Duplica";

/// The height of a monitor's picture.
const MON_H: i32 = 96;
const L_SCALE: &str = "Dimensione interfaccia";
const L_LAYOUT: &str = "Disposizione dei tasti";
const L_PASSWORD: &str = "Password";

/// The sections, in the order of the sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    General,
    Wifi,
    Look,
    Screen,
    Sound,
    Keyboard,
    Lock,
    Privacy,
}

const SECTIONS: [Section; 8] =
    [Section::General, Section::Wifi, Section::Look, Section::Screen, Section::Sound, Section::Keyboard, Section::Lock, Section::Privacy];

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::General => "Generale",
            Section::Wifi => "Wi-Fi",
            Section::Look => "Aspetto",
            Section::Screen => "Schermo",
            Section::Sound => "Suono",
            Section::Keyboard => "Tastiera",
            Section::Lock => "Blocco e password",
            Section::Privacy => "Privacy",
        }
    }

    /// The gradient of its tile, top and bottom.
    fn grad(self) -> (u32, u32) {
        match self {
            Section::General => (0xFFA0_A0A6, 0xFF62_6267),
            Section::Wifi => (0xFF4F_C3F7, 0xFF02_77BD),
            Section::Look => (0xFFA7_8BFA, 0xFF6D_3FE0),
            Section::Screen => (0xFF5A_A9FF, 0xFF25_63EB),
            Section::Sound => (0xFFFF_6B81, 0xFFE1_1D48),
            Section::Keyboard => (0xFF5F_E08A, 0xFF1F_A64F),
            Section::Lock => ui::ORANGE_GRAD,
            Section::Privacy => (0xFF7C_8CFF, 0xFF3F_4FD8),
        }
    }

    /// Its symbol in soft pixels, five cells by five.
    fn glyph(self) -> &'static [&'static str; 5] {
        match self {
            // A cog.
            Section::General => &["#.#.#", ".###.", "##.##", ".###.", "#.#.#"],
            // Waves over a dot.
            Section::Wifi => &[".###.", "#...#", ".###.", ".....", "..#.."],
            // A circle half dark, half light.
            Section::Look => &[".###.", "##..#", "##..#", "##..#", ".###."],
            // A screen on its stand.
            Section::Screen => &["#####", "#...#", "#####", "..#..", ".###."],
            // A loudspeaker.
            Section::Sound => &["..#..", "###.#", "###.#", "###.#", "..#.."],
            // Keys and the space bar.
            Section::Keyboard => &["#.#.#", ".....", "#.#.#", ".....", ".###."],
            // A padlock.
            Section::Lock => &[".###.", ".#.#.", "#####", "##.##", "#####"],
            // A raised hand.
            Section::Privacy => &["#.#.#", "#.#.#", "#####", "#####", ".###."],
        }
    }

    /// A few words under the title, if any.
    fn intro(self) -> &'static str {
        match self {
            Section::Privacy => "Le app non hanno nessun permesso finché non lo consenti tu: la prima volta che ne serve uno, te lo chiedono. Qui cambi idea.",
            Section::Wifi => "Le password delle reti a cui ti colleghi restano su questo computer, dove le app non arrivano. Senza connessione, justOS si collega da solo alla rete conosciuta più forte.",
            _ => "",
        }
    }
}

/// The loudspeaker next to the volume, and the same when it is silent.
const GLYPH_VOLUME: [&str; 5] = ["..#..", "###.#", "###.#", "###.#", "..#.."];
const GLYPH_MUTE: [&str; 5] = ["..#..", "###..", "###..", "###..", "..#.."];

/// The control of a row: what it shows and what it changes.
#[derive(Clone, PartialEq)]
enum Ctl {
    /// A value to read, at the right.
    Info(String),
    /// Nothing: the row is a line of text.
    Text,
    /// The name field and "Salva".
    Name,
    /// The computer's name, its field and "Salva".
    Device,
    /// The first-start setup again, at the next start.
    Setup,
    /// Dark or light: a small picture of a window in each.
    Themes,
    /// The wallpapers, a grid of previews.
    Wallpapers,
    /// The resolutions the display offers.
    Modes,
    /// Several monitors: a picture of each, left to right as they stand; a
    /// click makes one the primary.
    Monitors,
    /// The monitors left and right swapped.
    Swap,
    /// Every monitor showing the same ("Duplica").
    Mirror,
    /// The size of the interface.
    Scale,
    /// The volume: cells that fill up.
    Volume,
    /// A short tone.
    Test,
    /// The sound when the desktop appears.
    Chime,
    /// The dock small at rest, and its icons growing under the pointer.
    DockCompact,
    DockMagnify,
    /// The most frames a second.
    Fluidity,
    /// The keyboard layout.
    Layout,
    /// The password field and its button.
    Password,
    /// When the screen locks by itself.
    LockAfter,
    /// What app `i` was allowed, asked again.
    Forget(usize),
    /// Permission `j` of app `i`, on or off.
    Perm(usize, usize),
    /// A permission: the apps that asked for it, on a page of their own.
    PermView(String),
    /// Back from that page to all of Privacy.
    Back,
    /// A search result: the section it is in, and the row's label.
    Go(Section, String),
    /// The Wi-Fi connection: what it is doing, and "Cerca" or "Disconnetti".
    WifiState,
    /// Network `i` heard: its signal and "Connetti".
    Net(usize),
    /// The password of the network being joined, and "Connetti".
    NetPassword,
    /// Known network `i`: "Dimentica".
    Known(usize),
}

impl Ctl {
    /// Laid out under its label, the whole width of the card.
    fn block(&self) -> bool {
        matches!(self, Ctl::Wallpapers | Ctl::Modes | Ctl::Monitors)
    }
}

/// A row of a card: a label, a line under it (may be empty) and a control.
#[derive(Clone)]
struct Row {
    label: String,
    desc: String,
    ctl: Ctl,
}

fn row(label: &str, desc: &str, ctl: Ctl) -> Row {
    Row { label: String::from(label), desc: String::from(desc), ctl }
}

/// A card of rows; search results have their section above them.
struct Card {
    head: Option<Section>,
    rows: Vec<Row>,
}

fn card(rows: Vec<Row>) -> Card {
    Card { head: None, rows }
}

/// A row where it is drawn.
struct Placed {
    r: Rect,
    ctl_r: Rect,
    /// The top-left corner of the label, and how wide it and its line may be.
    text_x: i32,
    text_y: i32,
    text_w: i32,
    /// The first of its card (no line above it).
    first: bool,
    row: Row,
}

/// The content pane laid out.
struct Page {
    title: String,
    title_y: i32,
    intro: Vec<String>,
    intro_y: i32,
    cards: Vec<(Rect, Option<Section>)>,
    rows: Vec<Placed>,
    /// The left edge and width of the column.
    x: i32,
    w: i32,
    /// Everything from the top, margins included.
    height: i32,
}

/// What is under a point of the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Search,
    Side(Section),
    /// Part `k` of the control of row `i` of the page (a segment, a
    /// wallpaper, a field or the button beside it...).
    Ctl(usize, usize),
    /// Search result row `i` of the page.
    Result(usize),
}

pub struct Prefs {
    section: Section,
    /// What is typed in the search field, and whether keys go there.
    query: String,
    searching: bool,
    /// The search result the keyboard is on.
    pick: usize,
    name: String,
    editing: bool,
    /// The computer's name being typed.
    device: String,
    editing_device: bool,
    /// A new password being typed.
    password: String,
    editing_password: bool,
    /// A word in place of a row's line after a change: the row's label and
    /// the word.
    note: (&'static str, &'static str),
    /// The wallpaper previews, and the wallpaper behind the pictures of the
    /// themes (which one, and the picture).
    thumbs: Vec<Canvas>,
    backdrop: Option<(String, Canvas)>,
    /// The wallpaper in the pictures of the monitors.
    screens: Option<(String, Canvas)>,
    /// Resolutions the display driver offers (empty without one).
    modes: Vec<DisplayMode>,
    /// The volume is being dragged.
    sliding: bool,
    /// The apps and their permissions, and the permission whose apps are
    /// shown (None: all of Privacy).
    privacy: Vec<AppPerms>,
    privacy_view: Option<String>,
    scroll: i32,
    max_scroll: i32,
    /// The content pane's height at the last drawing.
    view_h: i32,
    /// What is under the pointer (only things that light up).
    hot: Option<Target>,
    /// A row lit for a moment after a search led to it: its label and
    /// until when; `reveal`: scroll to it at the next drawing.
    flash: Option<(String, u64)>,
    reveal: bool,
    /// Wi-Fi: the networks heard, the known ones, the station's state (and
    /// when they were last looked at); the network whose password is being
    /// asked for, and the password typed.
    nets: Vec<crate::net::wifi::Network>,
    known: Vec<String>,
    wifi: Option<crate::net::wifi::Status>,
    wifi_seen: u64,
    asking: Option<usize>,
    net_password: String,
    editing_net: bool,
}

/// An app as Privacy shows it: its permissions with what the user said
/// (None: not asked yet, but its manifest says it may ask).
#[derive(Clone)]
struct AppPerms {
    id: String,
    name: String,
    perms: Vec<(String, Option<Answer>)>,
}

/// The apps the user answered something about, and the installed apps
/// whose manifest lists permissions, by name.
fn privacy_apps() -> Vec<AppPerms> {
    let mut v: Vec<AppPerms> = crate::sandbox::all()
        .into_iter()
        .map(|g| AppPerms { id: g.id, name: g.name, perms: g.perms.into_iter().map(|(p, a)| (p, Some(a))).collect() })
        .collect();
    for p in crate::store::installed_all().into_iter().filter(|p| p.native && !p.permissions.is_empty()) {
        let i = match v.iter().position(|a| a.id == p.app_id) {
            Some(i) => i,
            None => {
                v.push(AppPerms { id: p.app_id.clone(), name: p.name.clone(), perms: Vec::new() });
                v.len() - 1
            }
        };
        for perm in p.permissions {
            if !v[i].perms.iter().any(|q| q.0 == perm) {
                v[i].perms.push((perm, None));
            }
        }
    }
    v.sort_by_key(|a| fold(if a.name.is_empty() { &a.id } else { &a.name }));
    v
}

/// What the user said about a permission, as its row says it.
fn state_word(answer: Option<Answer>) -> &'static str {
    match answer {
        Some(Answer::Allow) => "Consentito",
        Some(Answer::DeniedOnce) => "Negato una volta: lo chiederà ancora una volta",
        Some(Answer::Deny) => "Negato",
        None => "Non ancora chiesto: lo chiederà quando serve",
    }
}

/// `s` in lower case without accents, for the search.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| match ch {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            ch => ch,
        })
        .collect()
}

/// The width of a button with `label`.
fn button_w(label: &str) -> i32 {
    (text::width(Font::Strong, label) + 36).max(88)
}

/// The width of a segmented control with `labels`.
fn segmented_w(labels: &[&str]) -> i32 {
    let widest = labels.iter().map(|l| text::width(Font::Strong, l)).max().unwrap_or(0);
    (widest + 28).max(64) * labels.len() as i32 + 4
}

/// The pictures of the themes in a row `inner` wide: their size.
fn theme_size(inner: i32) -> (i32, i32) {
    let w = ((inner - 16) / 2).min(168);
    (w, w * 5 / 8)
}

/// The wallpapers in a row `inner` wide: columns, and each one's size.
fn thumb_grid(inner: i32) -> (i32, i32, i32) {
    let cols = ((inner + 12) / (96 + 12)).clamp(1, 4);
    let w = ((inner - (cols - 1) * 12) / cols).min(160);
    (cols, w, w * 5 / 8)
}

/// The resolutions in a row `inner` wide: columns and the buttons' width.
fn mode_grid(inner: i32) -> (i32, i32) {
    let cols = ((inner + 8) / (112 + 8)).clamp(1, 4);
    (cols, ((inner - (cols - 1) * 8) / cols).min(150))
}

/// The pictures of the monitors in a control at `r`, left to right as they
/// stand: each one's output and place. Same-sized monitors, in their shape,
/// as big as fits.
fn monitor_tiles(r: Rect) -> Vec<(u32, Rect)> {
    let order = crate::dev::display::order();
    let (mw, mh) = crate::dev::display::monitor_size().unwrap_or((16, 10));
    let n = order.len().max(1) as i32;
    let gap = 16;
    let (mut tw, mut th) = (MON_H * mw as i32 / (mh as i32).max(1), MON_H);
    let room = (r.w - (n - 1) * gap) / n;
    if tw > room {
        th = th * room / tw.max(1);
        tw = room;
    }
    let x0 = r.x + (r.w - n * tw - (n - 1) * gap) / 2;
    order.iter().enumerate().map(|(i, &o)| (o, Rect::new(x0 + i as i32 * (tw + gap), r.y, tw, th))).collect()
}

/// A text field with its button at the right.
fn field_and_button(r: Rect, label: &str) -> (Rect, Rect) {
    let bw = button_w(label).min(100);
    (Rect::new(r.x, r.y, r.w - bw - 10, r.h), Rect::new(r.right() - bw, r.y + 2, bw, r.h - 4))
}

/// The cells of the volume, right of its loudspeaker.
fn volume_bar(r: Rect) -> Rect {
    Rect::new(r.x + 30, r.y, r.w - 30, r.h)
}

/// A small picture of a window in the theme of palette `p`, over the
/// wallpaper `backdrop` (as big as `r`). The first-start setup shows them
/// too.
pub(crate) fn draw_theme(c: &mut Canvas, r: Rect, p: &ui::Palette, backdrop: Option<&Canvas>) {
    match backdrop {
        Some(b) => c.blit_round_from(b, 0, 0, r, 12, 0),
        None => c.fill_round_grad(r, 12, 0xFF3B_2E5A, 0xFFE0_8F6E),
    }
    let saved = c.clip();
    c.set_clip(saved.intersect(&r));
    let win = Rect::new(r.x + r.w * 12 / 100, r.y + r.h * 17 / 100, r.w * 76 / 100, r.h * 70 / 100);
    let opaque = |col: u32| col | 0xFF00_0000;
    c.shadow(win, 8, 6, 70);
    c.fill_round_grad(win, 8, opaque(p.panel_top), opaque(p.panel_bottom));
    c.stroke_round(win, 8, 1, p.line);
    c.sheen_round(win, 8, 1, p.sheen);
    // The title and the window's buttons.
    let k = 6;
    for i in 0..3 {
        let kr = Rect::new(win.right() - 6 - (3 - i) * (k + 3) + 3, win.y + 5, k, k);
        let g = if i == 2 { ui::ORANGE_GRAD } else { (p.knob_top, p.knob_bottom) };
        c.fill_round_grad(kr, 2, g.0, g.1);
    }
    c.fill_round(Rect::new(win.x + 8, win.y + 6, win.w / 5, 4), 2, p.text_dim);
    // A sidebar with a row chosen, and the content: a few lines and a
    // button.
    let body = Rect::new(win.x + 4, win.y + 16, win.w - 8, win.h - 20);
    let side = Rect::new(body.x, body.y, body.w * 30 / 100, body.h);
    let main = Rect::new(side.right() + 3, body.y, body.right() - side.right() - 3, body.h);
    for pane in [side, main] {
        c.fill_round(pane, 5, p.well);
        c.stroke_round(pane, 5, 1, p.line);
    }
    for i in 0..3 {
        let rr = Rect::new(side.x + 3, side.y + 4 + i * 9, side.w - 6, 7);
        if i == 0 {
            c.fill_round(rr, 3, p.sel);
        }
        c.fill_round(Rect::new(rr.x + 3, rr.y + 2, rr.w * 3 / 5, 3), 1, if i == 0 { p.text } else { p.text_dim });
    }
    c.fill_round(Rect::new(main.x + 6, main.y + 6, main.w * 45 / 100, 4), 2, p.text);
    c.fill_round(Rect::new(main.x + 6, main.y + 15, main.w * 70 / 100, 3), 1, p.text_dim);
    c.fill_round(Rect::new(main.x + 6, main.y + 22, main.w * 55 / 100, 3), 1, p.text_dim);
    let btn = Rect::new(main.x + 6, main.bottom() - 12, main.w * 30 / 100, 7);
    c.fill_round_grad(btn, 3, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
    c.set_clip(saved);
}

/// A section's tile with its symbol.
fn draw_tile(c: &mut Canvas, r: Rect, s: Section) {
    ui::tile(c, r, r.w / 4, s.grad());
    ui::soft_pixels_in(c, s.glyph(), r, r.w * 130, ui::WHITE);
}

impl Prefs {
    pub fn new() -> Prefs {
        let offered = crate::dev::display::modes();
        let modes =
            COMMON_MODES.iter().map(|&(width, height)| DisplayMode { width, height }).filter(|m| offered.contains(m)).take(12).collect();
        Prefs {
            section: take_requested().unwrap_or(Section::General),
            query: String::new(),
            searching: false,
            pick: 0,
            name: settings::get().name,
            editing: false,
            device: crate::host::name(),
            editing_device: false,
            password: String::new(),
            editing_password: false,
            note: ("", ""),
            thumbs: Vec::new(),
            backdrop: None,
            screens: None,
            modes,
            sliding: false,
            privacy: privacy_apps(),
            privacy_view: None,
            scroll: 0,
            max_scroll: 0,
            view_h: 0,
            hot: None,
            flash: None,
            reveal: false,
            nets: Vec::new(),
            known: Vec::new(),
            wifi: None,
            wifi_seen: 0,
            asking: None,
            net_password: String::new(),
            editing_net: false,
        }
    }

    /// Looks at the Wi-Fi station again: true if something changed.
    fn refresh_wifi(&mut self) -> bool {
        use crate::net::wifi;
        self.wifi_seen = crate::time::uptime_ms();
        let nets = wifi::networks();
        let known: Vec<String> = wifi::saved().iter().map(|k| String::from_utf8_lossy(&k.ssid).into_owned()).collect();
        let status = wifi::status();
        let sig = |n: &[wifi::Network]| n.iter().map(|n| (n.name.clone(), n.bars, n.saved, n.connected)).collect::<Vec<_>>();
        let st = |s: &Option<wifi::Status>| s.as_ref().map(|s| (s.text(), s.ssid.clone(), s.scanning));
        let changed = sig(&nets) != sig(&self.nets) || known != self.known || st(&status) != st(&self.wifi);
        if let Some(i) = self.asking {
            // Keep asking for the same network if the list moved.
            let name = self.nets.get(i).map(|n| n.ssid.clone());
            self.asking = name.and_then(|ssid| nets.iter().position(|n| n.ssid == ssid));
        }
        // A wrong password: ask for it again.
        let wrong = |s: &Option<wifi::Status>| s.as_ref().filter(|s| s.failure == Some(justwifi::sta::Failure::WrongPassword)).map(|s| s.ssid.clone());
        if let Some(name) = wrong(&status).filter(|_| wrong(&self.wifi).is_none()) {
            if let Some(i) = nets.iter().position(|n| n.name == name) {
                self.asking = Some(i);
                self.editing_net = true;
                self.net_password.clear();
            }
        }
        self.nets = nets;
        self.known = known;
        self.wifi = status;
        changed
    }

    /// The cards of Wi-Fi.
    fn wifi_cards(&self) -> Vec<Card> {
        let Some(st) = &self.wifi else {
            return vec![card(vec![row(
                "Nessuna scheda Wi-Fi trovata.",
                "justOS ha il driver delle Realtek RTL8852BE; le altre schede non ancora.",
                Ctl::Text,
            )])];
        };
        let addr = crate::net::config(st.iface).filter(|c| c.1).map(|c| format!("{}", c.0.ip));
        let state = match (st.state, addr) {
            (justwifi::sta::State::Connected, Some(a)) => format!("{} a {} · {a}", st.text(), st.ssid),
            (justwifi::sta::State::Connected, None) => format!("{} a {} · in attesa di un indirizzo", st.text(), st.ssid),
            _ if st.scanning => String::from("Ricerca delle reti…"),
            _ => st.text(),
        };
        let mut cards = vec![card(vec![row("Wi-Fi", &state, Ctl::WifiState)])];
        let mut rows = Vec::new();
        for (i, n) in self.nets.iter().enumerate() {
            let mut desc = String::from(n.security.name());
            if n.connected {
                desc = format!("Connessa · {desc}");
            } else if n.saved {
                desc = format!("{desc} · conosciuta");
            }
            rows.push(row(&n.name, &desc, Ctl::Net(i)));
            if self.asking == Some(i) {
                rows.push(row("", "", Ctl::NetPassword));
            }
        }
        if rows.is_empty() {
            rows.push(row(if st.scanning { "Ricerca delle reti…" } else { "Nessuna rete trovata." }, "", Ctl::Text));
        }
        cards.push(card(rows));
        if !self.known.is_empty() {
            cards.push(card(self.known.iter().enumerate().map(|(i, k)| row(k, "Rete conosciuta", Ctl::Known(i))).collect()));
        }
        cards
    }

    /// Joins network `i` of the list (its password typed, or known).
    fn join(&mut self, i: usize, password: Option<String>) {
        let Some(n) = self.nets.get(i).cloned() else { return };
        let name = String::from_utf8_lossy(&n.ssid).into_owned();
        if password.is_none() && n.security.needs_password() && !n.saved {
            self.asking = Some(i);
            self.editing_net = true;
            self.net_password.clear();
            return;
        }
        self.asking = None;
        self.editing_net = false;
        let _ = crate::net::wifi::connect(&name, password.as_deref());
        self.net_password.clear();
        self.refresh_wifi();
    }

    // ------------------------------------------------------------------
    // What the sections hold

    /// The cards of section `s`.
    fn cards(&self, s: Section) -> Vec<Card> {
        match s {
            Section::General => vec![
                card(vec![
                    row(L_NAME, "Come ti saluta justOS", Ctl::Name),
                    row(L_DEVICE, &format!("In rete: {}", crate::host::hostname()), Ctl::Device),
                ]),
                card(vec![
                    row(L_SETUP, "La rifai al prossimo avvio", Ctl::Setup),
                    row("Versione", "", Ctl::Info(format!("justOS {}", crate::VERSION))),
                ]),
            ],
            Section::Look => {
                vec![
                    card(vec![row("Tema", "Finestre, menu e pannelli", Ctl::Themes)]),
                    card(vec![
                        row("Dock compatto", "Piccolo a riposo, grande sotto il puntatore", Ctl::DockCompact),
                        row("Ingrandimento icone", "L'icona sotto il puntatore cresce", Ctl::DockMagnify),
                    ]),
                    card(vec![row("Sfondo", "", Ctl::Wallpapers)]),
                ]
            }
            Section::Screen => {
                let modes = if self.modes.is_empty() {
                    let (w, h) = crate::dev::display::monitor_size().unwrap_or((0, 0));
                    row(L_MODES, "Scelta dal firmware: nessun driver video la può cambiare.", Ctl::Info(format!("{w} × {h}")))
                } else {
                    row(L_MODES, &crate::dev::display::controller().unwrap_or_default(), Ctl::Modes)
                };
                let scale = if crate::gui::can_double() {
                    row(L_SCALE, "Testo e finestre più grandi", Ctl::Scale)
                } else {
                    row(L_SCALE, "Il 200% vuole uno schermo di almeno 1600 × 1200", Ctl::Info(String::from("100%")))
                };
                // The display does not say its refresh rate: automatic is 60.
                let auto = crate::gui::pace::display_hz().unwrap_or(60);
                let fluid = row("Fluidità", &format!("Fotogrammi al secondo al massimo (Automatica: {auto} Hz)"), Ctl::Fluidity);
                let mut cards = vec![card(vec![modes]), card(vec![scale]), card(vec![fluid])];
                if crate::dev::display::outputs().len() > 1 {
                    let mirror = crate::gui::monitor_choice().2;
                    let mut rows = vec![if mirror {
                        row(L_MONITORS, "Tutti mostrano la stessa scrivania", Ctl::Monitors)
                    } else {
                        row(L_MONITORS, "Tocca un monitor per renderlo principale: lì stanno orologio e dock", Ctl::Monitors)
                    }];
                    if !mirror {
                        rows.push(row(L_SWAP, "Il monitor a sinistra passa a destra", Ctl::Swap));
                    }
                    rows.push(row(L_MIRROR, "Tutti i monitor mostrano la stessa cosa", Ctl::Mirror));
                    cards.insert(1, card(rows));
                }
                cards
            }
            Section::Sound => {
                if crate::dev::audio::available() {
                    vec![
                        card(vec![row("Volume", "", Ctl::Volume), row("Suono di prova", "Un breve tono al volume scelto", Ctl::Test)]),
                        card(vec![row("Suono all'avvio", "Quando compare la scrivania", Ctl::Chime)]),
                    ]
                } else {
                    vec![card(vec![row("Nessuna scheda audio trovata.", "Volume e suoni compaiono quando ce n'è una.", Ctl::Text)])]
                }
            }
            Section::Keyboard => vec![card(vec![row(L_LAYOUT, "Le lettere e i simboli dei tasti", Ctl::Layout)])],
            Section::Lock => {
                let pw =
                    if settings::has_password() { "Chiesta per sbloccare il computer" } else { "Nessuna: il computer si sblocca senza" };
                vec![card(vec![row(L_PASSWORD, pw, Ctl::Password), row("Blocca dopo", "Senza usare il computer", Ctl::LockAfter)])]
            }
            Section::Privacy => match &self.privacy_view {
                Some(perm) => self.perm_cards(perm),
                None => self.privacy_cards(),
            },
            Section::Wifi => self.wifi_cards(),
        }
    }

    /// All of Privacy: a card of the permissions, each opening the apps
    /// that asked for it, then a card per app with its switches.
    fn privacy_cards(&self) -> Vec<Card> {
        let mut kinds: Vec<String> = crate::sandbox::DEVICES.iter().map(|d| String::from(d.0)).collect();
        for a in &self.privacy {
            for (p, _) in &a.perms {
                if !kinds.contains(p) {
                    kinds.push(p.clone());
                }
            }
        }
        let mut cards =
            vec![card(kinds.iter().map(|p| row(&crate::sandbox::title(p), &self.summary(p), Ctl::PermView(p.clone()))).collect())];
        if self.privacy.is_empty() {
            cards.push(card(vec![row("Nessuna app ha ancora chiesto qualcosa.", "", Ctl::Text)]));
        }
        for (i, a) in self.privacy.iter().enumerate() {
            let name = if a.name.is_empty() { a.id.as_str() } else { a.name.as_str() };
            let mut rows = vec![row(name, &a.id, Ctl::Forget(i))];
            for (j, (perm, answer)) in a.perms.iter().enumerate() {
                rows.push(row(&crate::sandbox::title(perm), state_word(*answer), Ctl::Perm(i, j)));
            }
            cards.push(card(rows));
        }
        cards
    }

    /// The page of one permission: the apps that asked for it (or may),
    /// each with its switch.
    fn perm_cards(&self, perm: &str) -> Vec<Card> {
        let mut rows = Vec::new();
        for (i, a) in self.privacy.iter().enumerate() {
            if let Some(j) = a.perms.iter().position(|(p, _)| p == perm) {
                let name = if a.name.is_empty() { a.id.as_str() } else { a.name.as_str() };
                rows.push(row(name, state_word(a.perms[j].1), Ctl::Perm(i, j)));
            }
        }
        if rows.is_empty() {
            rows.push(row("Nessuna app l'ha chiesto.", "Quando un'app lo chiederà, la troverai qui.", Ctl::Text));
        }
        vec![card(vec![row("Tutti i permessi", "Torna a Privacy", Ctl::Back)]), card(rows)]
    }

    /// How many apps were allowed permission `perm`, and how many denied.
    fn summary(&self, perm: &str) -> String {
        let answers: Vec<Option<Answer>> = self.privacy.iter().filter_map(|a| a.perms.iter().find(|p| p.0 == perm).map(|p| p.1)).collect();
        if answers.is_empty() {
            return String::from("Nessuna app l'ha chiesto");
        }
        let allowed = answers.iter().filter(|a| **a == Some(Answer::Allow)).count();
        let denied = answers.iter().filter(|a| matches!(a, Some(Answer::Deny | Answer::DeniedOnce))).count();
        let mut parts = Vec::new();
        if allowed > 0 {
            parts.push(format!("{allowed} app {}", if allowed == 1 { "consentita" } else { "consentite" }));
        }
        if denied > 0 {
            parts.push(format!("{denied} {}", if denied == 1 { "negata" } else { "negate" }));
        }
        if parts.is_empty() {
            return format!("{} app, non ancora chiesto", answers.len());
        }
        parts.join(", ")
    }

    /// The rows whose label (or section) has what is typed in the search
    /// field, a card per section. A line of text ("no audio card") is no
    /// setting: it is found only by its section's name.
    fn results(&self) -> Vec<Card> {
        let q = fold(self.query.trim());
        let mut out = Vec::new();
        for s in SECTIONS {
            let all = fold(s.name()).contains(q.as_str());
            let rows: Vec<Row> = self
                .cards(s)
                .into_iter()
                .flat_map(|c| c.rows)
                .filter(|r| !r.label.is_empty() && (all || (r.ctl != Ctl::Text && fold(&r.label).contains(q.as_str()))))
                .map(|r| row(&r.label, &r.desc, Ctl::Go(s, r.label.clone())))
                .collect();
            if !rows.is_empty() {
                out.push(Card { head: Some(s), rows });
            }
        }
        out
    }

    /// The sections in the sidebar: those with results while searching.
    fn visible(&self) -> Vec<Section> {
        if self.query.trim().is_empty() {
            return SECTIONS.to_vec();
        }
        self.results().iter().filter_map(|c| c.head).collect()
    }

    // ------------------------------------------------------------------
    // Layout

    fn search_rect(area: Rect) -> Rect {
        Rect::new(area.x + 12, area.y + 14, SIDEBAR - 24, 34)
    }

    fn side_rect(area: Rect, k: usize) -> Rect {
        Rect::new(area.x + 8, area.y + SIDE_TOP + k as i32 * SIDE_STEP, SIDEBAR - 16, SIDE_ROW)
    }

    /// The content pane, right of the sidebar.
    fn main(area: Rect) -> Rect {
        Rect::new(area.x + SIDEBAR + GAP, area.y, area.w - SIDEBAR - GAP, area.h)
    }

    /// The choices of a segmented control, and the one chosen.
    fn choices(ctl: &Ctl) -> (Vec<&'static str>, usize) {
        let s = settings::get();
        match ctl {
            Ctl::Scale => (SIZES.iter().map(|(_, l)| *l).collect(), SIZES.iter().position(|(k, _)| *k == s.scale).unwrap_or(0)),
            Ctl::Layout => (LAYOUTS.iter().map(|(_, l)| *l).collect(), LAYOUTS.iter().position(|(k, _)| *k == s.layout).unwrap_or(0)),
            Ctl::LockAfter => {
                (LOCK_AFTER.iter().map(|(_, l)| *l).collect(), LOCK_AFTER.iter().position(|(m, _)| *m == s.lock_after).unwrap_or(2))
            }
            Ctl::Fluidity => {
                let now = crate::gui::pace::choice();
                (FLUIDITY.to_vec(), crate::gui::pace::CHOICES.iter().position(|&hz| hz == now).unwrap_or(0))
            }
            _ => (Vec::new(), 0),
        }
    }

    /// The size of a control in a card `inner` wide.
    fn ctl_size(&self, ctl: &Ctl, inner: i32) -> (i32, i32) {
        let (bh, _) = text::metrics(Font::Body);
        let (sh, _) = text::metrics(Font::Small);
        match ctl {
            Ctl::Info(s) => (text::width(Font::Body, s).min(inner), bh),
            Ctl::Text => (0, 0),
            Ctl::Name | Ctl::Device | Ctl::Password => (inner.min(320), 38),
            Ctl::Setup => (button_w("Ripeti"), 34),
            Ctl::Test => (button_w("Prova"), 34),
            Ctl::Forget(_) => (button_w("Chiedi di nuovo"), 34),
            Ctl::Back => (button_w("Indietro"), 34),
            Ctl::PermView(_) => (text::width(Font::Heading, "›"), bh),
            Ctl::Chime | Ctl::Perm(..) | Ctl::DockCompact | Ctl::DockMagnify | Ctl::Mirror => (52, 30),
            Ctl::Swap => (button_w("Scambia"), 34),
            Ctl::Monitors => {
                let th = monitor_tiles(Rect::new(0, 0, inner, 0)).first().map_or(MON_H, |t| t.1.h);
                (inner, th + 10 + bh)
            }
            Ctl::Volume => (inner.min(290), 26),
            Ctl::Scale | Ctl::Layout | Ctl::LockAfter | Ctl::Fluidity => (segmented_w(&Self::choices(ctl).0).min(inner), 32),
            Ctl::Go(..) => (text::width(Font::Heading, "›"), bh),
            Ctl::WifiState => {
                let connected = self.wifi.as_ref().is_some_and(|s| s.state != justwifi::sta::State::Idle);
                (button_w(if connected { "Disconnetti" } else { "Cerca" }), 34)
            }
            Ctl::Net(_) => (BARS_W + 12 + button_w("Connetti"), 34),
            Ctl::NetPassword => (inner.min(380), 38),
            Ctl::Known(_) => (button_w("Dimentica"), 34),
            Ctl::Themes => {
                let (tw, th) = theme_size(inner);
                (2 * tw + 16, th + 10 + bh)
            }
            Ctl::Wallpapers => {
                let (cols, _, th) = thumb_grid(inner);
                let rows = (res::WALLPAPERS.len() as i32 + cols - 1) / cols;
                (inner, rows * (th + 6 + sh) + (rows - 1) * 14)
            }
            Ctl::Modes => {
                let (cols, _) = mode_grid(inner);
                let rows = (self.modes.len() as i32 + cols - 1) / cols;
                (inner, rows * 34 + (rows - 1).max(0) * 8)
            }
        }
    }

    /// Places a row at `y` in a card at `x`, `w` wide: the control at the
    /// right of the label when both fit, else under it.
    fn place(&self, row: Row, x: i32, y: i32, w: i32, first: bool) -> Placed {
        let inner = w - 2 * INSET;
        let (bh, _) = text::metrics(Font::Body);
        let (sh, _) = text::metrics(Font::Small);
        let text_h = match (row.label.is_empty(), row.desc.is_empty()) {
            (true, _) => 0,
            (false, true) => bh,
            (false, false) => bh + 2 + sh,
        };
        let (cw, ch) = self.ctl_size(&row.ctl, inner);
        let tx = x + INSET;
        let label_w = text::width(Font::Body, &row.label).max(text::width(Font::Small, &row.desc).min(150));
        if !row.ctl.block() && (cw == 0 || label_w + 24 + cw <= inner) {
            let h = (text_h.max(ch) + 24).max(ROW_MIN);
            let ctl_r = Rect::new(x + w - INSET - cw, y + (h - ch) / 2, cw, ch);
            let text_w = if cw > 0 { ctl_r.x - 16 - tx } else { inner };
            // Beside a tall control the label stays at the top.
            let text_y = if ch > 48 { ctl_r.y + 4 } else { y + (h - text_h) / 2 };
            return Placed { r: Rect::new(x, y, w, h), ctl_r, text_x: tx, text_y, text_w, first, row };
        }
        let top = if text_h > 0 { 14 } else { INSET };
        let gap = if text_h > 0 { 12 } else { 0 };
        let h = top + text_h + gap + ch + INSET;
        let ctl_r = Rect::new(tx, y + top + text_h + gap, cw.min(inner), ch);
        Placed { r: Rect::new(x, y, w, h), ctl_r, text_x: tx, text_y: y + top, text_w: inner, first, row }
    }

    /// The content pane laid out: the section, or the search results.
    fn page(&self, area: Rect) -> Page {
        let main = Self::main(area);
        let margin = if main.w >= 480 { MARGIN } else { 16 };
        let w = (main.w - 2 * margin).clamp(120, COLUMN);
        let x = main.x + (main.w - w) / 2;
        let top = main.y - self.scroll;
        let searching = !self.query.trim().is_empty();
        let view = self.privacy_view.as_deref().filter(|_| self.section == Section::Privacy && !searching);
        let (title, cards) = if searching {
            (format!("Risultati per “{}”", self.query.trim()), self.results())
        } else if let Some(perm) = view {
            (crate::sandbox::title(perm), self.cards(self.section))
        } else {
            (String::from(self.section.name()), self.cards(self.section))
        };
        let (dh, _) = text::metrics(Font::Display);
        let (bh, _) = text::metrics(Font::Body);
        let title_y = top + 22;
        let mut y = title_y + dh + 16;
        let intro = match (view, self.section.intro()) {
            (Some(perm), _) => {
                let s = format!("Le app che possono chiedere di {}. Spento: negato, senza più domande.", crate::sandbox::describe(perm));
                text::wrap(Font::Body, &s, w - 8)
            }
            (None, s) if !searching && !s.is_empty() => text::wrap(Font::Body, s, w - 8),
            _ => Vec::new(),
        };
        let intro_y = y - 4;
        if !intro.is_empty() {
            y += intro.len() as i32 * (bh + 3) + 12;
        }
        let mut rows = Vec::new();
        let mut boxes = Vec::new();
        for card in cards {
            if card.head.is_some() {
                y += HEAD_H;
            }
            let start = y;
            for (i, r) in card.rows.into_iter().enumerate() {
                let p = self.place(r, x, y, w, i == 0);
                y = p.r.bottom();
                rows.push(p);
            }
            boxes.push((Rect::new(x, start, w, y - start), card.head));
            y += CARD_GAP;
        }
        Page { title, title_y, intro, intro_y, cards: boxes, rows, x, w, height: y - CARD_GAP + MARGIN - top }
    }

    /// Where the parts of a row's control are (what a click can hit).
    fn parts(&self, p: &Placed) -> Vec<Rect> {
        let r = p.ctl_r;
        match &p.row.ctl {
            Ctl::Name | Ctl::Device => {
                let (f, b) = field_and_button(r, "Salva");
                vec![f, b]
            }
            Ctl::Password => {
                let (f, b) = field_and_button(r, "Imposta");
                vec![f, b]
            }
            Ctl::Setup | Ctl::Test | Ctl::Forget(_) | Ctl::Chime | Ctl::Perm(..) | Ctl::DockCompact | Ctl::DockMagnify | Ctl::Back => vec![r],
            // The whole row opens the permission's page.
            Ctl::PermView(_) => vec![p.r],
            Ctl::Swap | Ctl::Mirror => vec![r],
            // Mirrored, no monitor is the primary: nothing to click.
            Ctl::Monitors if crate::gui::monitor_choice().2 => Vec::new(),
            Ctl::Monitors => monitor_tiles(r).iter().map(|t| t.1).collect(),
            Ctl::Volume => vec![volume_bar(r).grow(8)],
            Ctl::Scale | Ctl::Layout | Ctl::LockAfter | Ctl::Fluidity => ui::segments(r, Self::choices(&p.row.ctl).0.len()),
            Ctl::Themes => {
                let (tw, _) = theme_size(r.w);
                (0..THEMES.len() as i32).map(|i| Rect::new(r.x + i * (tw + 16), r.y, tw, r.h)).collect()
            }
            Ctl::Wallpapers => {
                let (sh, _) = text::metrics(Font::Small);
                let (cols, tw, th) = thumb_grid(r.w);
                (0..res::WALLPAPERS.len() as i32)
                    .map(|i| Rect::new(r.x + (i % cols) * (tw + 12), r.y + (i / cols) * (th + 6 + sh + 14), tw, th))
                    .collect()
            }
            Ctl::Modes => {
                let (cols, bw) = mode_grid(r.w);
                (0..self.modes.len() as i32).map(|i| Rect::new(r.x + (i % cols) * (bw + 8), r.y + (i / cols) * 42, bw, 34)).collect()
            }
            Ctl::Info(_) | Ctl::Text | Ctl::Go(..) => Vec::new(),
            Ctl::WifiState | Ctl::Known(_) => vec![r],
            // The connected network has no button.
            Ctl::Net(i) if self.nets.get(*i).is_some_and(|n| n.connected) => Vec::new(),
            Ctl::Net(_) => vec![Rect::new(r.x + BARS_W + 12, r.y, r.w - BARS_W - 12, r.h)],
            Ctl::NetPassword => {
                let (f, b) = field_and_button(r, "Connetti");
                vec![f, b]
            }
        }
    }

    /// What is at (`ax`, `ay`).
    fn hit(&self, page: &Page, area: Rect, ax: i32, ay: i32) -> Option<Target> {
        if Self::search_rect(area).contains(ax, ay) {
            return Some(Target::Search);
        }
        if let Some((_, s)) = self.visible().into_iter().enumerate().find(|(k, _)| Self::side_rect(area, *k).contains(ax, ay)) {
            return Some(Target::Side(s));
        }
        if !Self::main(area).contains(ax, ay) {
            return None;
        }
        for (i, p) in page.rows.iter().enumerate() {
            if matches!(p.row.ctl, Ctl::Go(..)) {
                if p.r.contains(ax, ay) {
                    return Some(Target::Result(i));
                }
                continue;
            }
            if let Some(k) = self.parts(p).iter().position(|r| r.contains(ax, ay)) {
                return Some(Target::Ctl(i, k));
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Drawing

    fn draw_sidebar(&self, c: &mut Canvas, area: Rect) {
        ui::search_field(c, Self::search_rect(area), &self.query, "Cerca", self.searching);
        let searching = !self.query.trim().is_empty();
        let (bh, _) = text::metrics(Font::Body);
        for (k, s) in self.visible().into_iter().enumerate() {
            let r = Self::side_rect(area, k);
            let chosen = !searching && s == self.section;
            if chosen {
                c.fill_round(r, 12, ui::sel());
            } else if self.hot == Some(Target::Side(s)) {
                c.fill_round(r, 12, ui::hover());
            }
            draw_tile(c, Rect::new(r.x + 7, r.y + (r.h - 26) / 2, 26, 26), s);
            let font = if chosen { Font::Strong } else { Font::Body };
            let name = text::fit(font, s.name(), r.w - 52);
            text::draw(c, font, r.x + 44, r.y + (r.h - bh) / 2, &name, ui::text());
        }
    }

    /// The label of a placed row and the line under it (or the note about
    /// the last change there).
    fn draw_label(&self, c: &mut Canvas, p: &Placed) {
        let (bh, _) = text::metrics(Font::Body);
        let fg = if p.row.ctl == Ctl::Text { ui::text_dim() } else { ui::text() };
        let label = text::fit(Font::Body, &p.row.label, p.text_w);
        text::draw(c, Font::Body, p.text_x, p.text_y, &label, fg);
        let (desc, color) = if !self.note.0.is_empty() && self.note.0 == p.row.label {
            (self.note.1, ui::ORANGE)
        } else {
            (p.row.desc.as_str(), ui::text_dim())
        };
        if !desc.is_empty() {
            let desc = text::fit(Font::Small, desc, p.text_w);
            text::draw(c, Font::Small, p.text_x, p.text_y + bh + 2, &desc, color);
        }
    }

    /// The control of placed row `i`.
    fn draw_ctl(&mut self, c: &mut Canvas, i: usize, p: &Placed) {
        let r = p.ctl_r;
        let hot = |k: usize| self.hot == Some(Target::Ctl(i, k));
        match &p.row.ctl {
            Ctl::Info(s) => {
                let s = text::fit(Font::Body, s, r.w + 2);
                text::draw_right(c, Font::Body, r.right(), r.y, &s, ui::text_dim());
            }
            Ctl::Text => {}
            Ctl::Name => {
                let (f, b) = field_and_button(r, "Salva");
                ui::text_field(c, f, &self.name, "Nome", self.editing);
                ui::button(c, b, "Salva", true, hot(1));
            }
            Ctl::Device => {
                let (f, b) = field_and_button(r, "Salva");
                ui::text_field(c, f, &self.device, crate::host::DEFAULT, self.editing_device);
                ui::button(c, b, "Salva", true, hot(1));
            }
            Ctl::Password => {
                let has = settings::has_password();
                let dots: String = self.password.chars().map(|_| '•').collect();
                let (f, b) = field_and_button(r, "Imposta");
                ui::text_field(c, f, &dots, if has { "Nuova password" } else { "Nessuna" }, self.editing_password);
                let label = if self.editing_password || !has { "Imposta" } else { "Togli" };
                ui::button(c, b, label, self.editing_password, hot(1));
            }
            Ctl::Setup => ui::button(c, r, "Ripeti", false, hot(0)),
            Ctl::Test => ui::button(c, r, "Prova", false, hot(0)),
            Ctl::Forget(_) => ui::button(c, r, "Chiedi di nuovo", false, hot(0)),
            Ctl::Back => ui::button(c, r, "Indietro", false, hot(0)),
            Ctl::WifiState => {
                let connected = self.wifi.as_ref().is_some_and(|s| s.state != justwifi::sta::State::Idle);
                ui::button(c, r, if connected { "Disconnetti" } else { "Cerca" }, false, hot(0));
            }
            Ctl::Known(_) => ui::button(c, r, "Dimentica", false, hot(0)),
            Ctl::Net(i) => {
                let Some(n) = self.nets.get(*i) else { return };
                draw_bars(c, Rect::new(r.x, r.y + (r.h - 18) / 2, BARS_W, 18), n.bars);
                let b = Rect::new(r.x + BARS_W + 12, r.y, r.w - BARS_W - 12, r.h);
                if n.connected {
                    text::draw_right(c, Font::Strong, b.right(), b.y + (b.h - text::metrics(Font::Strong).0) / 2, "Connessa", ui::ORANGE);
                } else {
                    ui::button(c, b, "Connetti", self.asking == Some(*i), hot(0));
                }
            }
            Ctl::NetPassword => {
                let (f, b) = field_and_button(r, "Connetti");
                let dots: String = self.net_password.chars().map(|_| '•').collect();
                ui::text_field(c, f, &dots, "Password della rete", self.editing_net);
                ui::button(c, b, "Connetti", true, hot(1));
            }
            Ctl::PermView(_) => {
                let (hh, _) = text::metrics(Font::Heading);
                let color = if hot(0) { ui::text() } else { ui::text_dim() };
                text::draw(c, Font::Heading, r.x, r.y + (r.h - hh) / 2 - 1, "›", color);
            }
            Ctl::Chime => ui::toggle(c, r, settings::get().startup_sound),
            Ctl::DockCompact => ui::toggle(c, r, settings::dock_compact()),
            Ctl::DockMagnify => ui::toggle(c, r, settings::dock_magnify()),
            Ctl::Perm(a, j) => {
                let on = self.privacy.get(*a).and_then(|g| g.perms.get(*j)).is_some_and(|(_, answer)| *answer == Some(Answer::Allow));
                ui::toggle(c, r, on);
            }
            Ctl::Volume => {
                let v = crate::dev::audio::volume();
                let glyph = if v == 0 { &GLYPH_MUTE } else { &GLYPH_VOLUME };
                ui::soft_pixels_in(c, glyph, Rect::new(r.x, r.y, 18, r.h), 3200, ui::text_dim());
                ui::slider(c, volume_bar(r), v * 10);
            }
            Ctl::Scale | Ctl::Layout | Ctl::LockAfter | Ctl::Fluidity => {
                let (labels, sel) = Self::choices(&p.row.ctl);
                ui::segmented(c, r, &labels, sel);
            }
            Ctl::Go(..) => {
                let (hh, _) = text::metrics(Font::Heading);
                text::draw(c, Font::Heading, r.x, r.y + (r.h - hh) / 2 - 1, "›", ui::text_dim());
            }
            Ctl::Themes => {
                let (tw, th) = theme_size(r.w);
                let wallpaper = settings::get().wallpaper;
                let stale = match &self.backdrop {
                    Some((n, b)) => *n != wallpaper || b.w != tw || b.h != th || b.scale() != c.scale(),
                    None => true,
                };
                if stale {
                    let pic = res::wallpaper_thumb(&wallpaper, tw, th);
                    self.backdrop = Some((wallpaper, pic));
                }
                let backdrop = self.backdrop.as_ref().map(|(_, b)| b);
                for (k, part) in self.parts(p).iter().enumerate() {
                    let pic = Rect::new(part.x, part.y, tw, th);
                    let (theme, label) = THEMES[k];
                    let pal = if theme == ui::Theme::Light { &ui::LIGHT } else { &ui::DARK };
                    draw_theme(c, pic, pal, backdrop);
                    let chosen = theme == ui::theme();
                    if chosen {
                        c.stroke_round(pic.grow(4), 16, 2, ui::ORANGE);
                    } else {
                        c.stroke_round(pic, 12, 1, ui::line());
                    }
                    let font = if chosen { Font::Strong } else { Font::Body };
                    let color = if chosen { ui::text() } else { ui::text_dim() };
                    text::draw_centered(c, font, pic.x + tw / 2, pic.bottom() + 10, label, color);
                }
            }
            Ctl::Wallpapers => {
                let (_, tw, th) = thumb_grid(r.w);
                if self.thumbs.first().is_none_or(|t| t.w != tw || t.h != th || t.scale() != c.scale()) {
                    // A new size, or the interface changed size: sharper (or
                    // smaller) previews.
                    self.thumbs = res::WALLPAPERS.iter().map(|(n, _)| res::wallpaper_thumb(n, tw, th)).collect();
                }
                let current = settings::get().wallpaper;
                for (k, t) in self.parts(p).iter().enumerate() {
                    let (name, label) = res::WALLPAPERS[k];
                    if name == current {
                        c.stroke_round(t.grow(4), 15, 2, ui::ORANGE);
                    }
                    c.blit_round_from(&self.thumbs[k], 0, 0, *t, 11, 0);
                    let color = if name == current { ui::text() } else { ui::text_dim() };
                    text::draw_centered(c, Font::Small, t.x + t.w / 2, t.bottom() + 6, label, color);
                }
            }
            Ctl::Swap => ui::button(c, r, "Scambia", false, hot(0)),
            Ctl::Mirror => ui::toggle(c, r, crate::gui::monitor_choice().2),
            Ctl::Monitors => self.draw_monitors(c, r, i),
            Ctl::Modes => {
                let (w, h) = crate::dev::display::monitor_size().unwrap_or((0, 0));
                let current = DisplayMode { width: w as u32, height: h as u32 };
                for (k, b) in self.parts(p).iter().enumerate() {
                    let m = self.modes[k];
                    ui::button(c, *b, &format!("{}×{}", m.width, m.height), m == current, hot(k));
                }
            }
        }
    }

    /// The pictures of the monitors (the control of placed row `i` at `r`):
    /// the wallpaper on each, the clock and the dock on the primary, and
    /// its number.
    fn draw_monitors(&mut self, c: &mut Canvas, r: Rect, i: usize) {
        let tiles = monitor_tiles(r);
        let Some(&(_, first)) = tiles.first() else { return };
        let wallpaper = settings::get().wallpaper;
        let stale = match &self.screens {
            Some((n, b)) => *n != wallpaper || b.w != first.w || b.h != first.h || b.scale() != c.scale(),
            None => true,
        };
        if stale {
            let pic = res::wallpaper_thumb(&wallpaper, first.w, first.h);
            self.screens = Some((wallpaper, pic));
        }
        let mirror = crate::gui::monitor_choice().2;
        let main = crate::gui::monitors().get(crate::gui::primary()).map(|m| m.0);
        let (hh, _) = text::metrics(Font::Heading);
        for (k, &(output, t)) in tiles.iter().enumerate() {
            let primary = mirror || Some(output) == main;
            if primary && !mirror {
                c.stroke_round(t.grow(4), 15, 2, ui::ORANGE);
            } else if self.hot == Some(Target::Ctl(i, k)) {
                c.stroke_round(t.grow(4), 15, 2, ui::line());
            }
            match &self.screens {
                Some((_, pic)) => c.blit_round_from(pic, 0, 0, t, 11, 0),
                None => c.fill_round_grad(t, 11, 0xFF3B_2E5A, 0xFFE0_8F6E),
            }
            let cx = t.x + t.w / 2;
            if primary {
                // The clock at the top and the dock at the bottom.
                c.fill_round(Rect::new(cx - t.w / 10, t.y + 7, t.w / 5, 6), 3, 0xB3FF_FFFF);
                c.fill_round(Rect::new(cx - t.w / 4, t.bottom() - 14, t.w / 2, 8), 4, 0x8CFF_FFFF);
            }
            // Its number on a dark badge, clear over any wallpaper.
            let number = format!("{}", output + 1);
            c.fill_circle(cx, t.y + t.h / 2, hh * 3 / 4, 0x9910_0C0A);
            text::draw_centered(c, Font::Heading, cx, t.y + (t.h - hh) / 2, &number, ui::WHITE);
            c.stroke_round(t, 11, 1, ui::line());
            if primary && !mirror {
                text::draw_centered(c, Font::Strong, cx, t.bottom() + 10, "Principale", ui::text());
            } else {
                text::draw_centered(c, Font::Body, cx, t.bottom() + 10, &format!("Monitor {}", output + 1), ui::text_dim());
            }
        }
    }

    /// The monitors arranged anew (`order` left to right, the primary's
    /// output, all the same or not); the row `label` says if it cannot be.
    fn arrange(&mut self, order: Vec<u32>, primary: u32, mirror: bool, label: &'static str, done: &'static str) {
        self.note = match crate::gui::change_monitors(order, primary, mirror) {
            Ok(()) => (label, done),
            Err(_) => (label, "Non si può cambiare ora"),
        };
    }

    // ------------------------------------------------------------------
    // Doing

    /// Shows section `s`.
    fn open(&mut self, s: Section) {
        self.section = s;
        self.query.clear();
        self.searching = false;
        self.pick = 0;
        self.scroll = 0;
        self.hot = None;
        if s == Section::Privacy {
            self.privacy = privacy_apps();
            self.privacy_view = None;
        }
        if s == Section::Wifi {
            crate::net::wifi::scan();
            self.asking = None;
            self.refresh_wifi();
        }
    }

    /// Shows the row a search result stands for, lit for a moment.
    fn go(&mut self, s: Section, label: &str) {
        self.open(s);
        self.flash = Some((String::from(label), crate::time::uptime_ms() + FLASH_MS));
        self.reveal = true;
    }

    /// The search result the keyboard is on: its section and label.
    fn picked(&self) -> Option<(Section, String)> {
        self.results().into_iter().flat_map(|c| c.rows).nth(self.pick).and_then(|r| match r.ctl {
            Ctl::Go(s, label) => Some((s, label)),
            _ => None,
        })
    }

    fn set_volume(v: i32) {
        crate::gui::change_volume(v.clamp(0, 100) as u32);
    }

    fn save_password(&mut self) {
        let removing = self.password.is_empty();
        if removing && !settings::has_password() {
            self.editing_password = true; // nothing to remove: type one
            return;
        }
        settings::set_password(&self.password);
        self.password.clear();
        self.editing_password = false;
        self.note = (L_PASSWORD, if removing { "Password tolta" } else { "Password impostata" });
    }

    fn save_device(&mut self) {
        if let Some(problem) = crate::host::problem(&self.device) {
            self.note = (L_DEVICE, problem);
            return;
        }
        let device = self.device.clone();
        settings::update(|s| s.device_name = device);
        self.editing_device = false;
        self.note = (L_DEVICE, "Nome salvato");
    }

    fn save_name(&mut self) {
        let name = String::from(self.name.trim());
        settings::update(|s| s.name = name);
        self.editing = false;
        self.note = (L_NAME, "Nome salvato");
    }

    /// A click on part `k` of the control of placed row `p`, at `ax`.
    fn act(&mut self, p: &Placed, k: usize, ax: i32) -> Action {
        match &p.row.ctl {
            Ctl::Name => {
                if k == 0 {
                    self.editing = true;
                } else {
                    self.save_name();
                }
            }
            Ctl::Device => {
                if k == 0 {
                    self.editing_device = true;
                } else {
                    self.save_device();
                }
            }
            Ctl::Password => {
                if k == 0 {
                    self.editing_password = true;
                } else {
                    // "Togli" with an empty field removes the password.
                    self.save_password();
                }
            }
            Ctl::Setup => {
                settings::update(|s| s.setup_done = false);
                self.note = (L_SETUP, "Al prossimo avvio");
            }
            Ctl::Themes => {
                settings::update(|s| s.theme = THEMES[k].0);
                // Every window and panel changes colour.
                return Action::RedrawAll;
            }
            Ctl::Wallpapers => {
                settings::update(|s| s.wallpaper = String::from(res::WALLPAPERS[k].0));
                return Action::Wallpaper;
            }
            Ctl::Monitors | Ctl::Swap | Ctl::Mirror => {
                let mut order = crate::dev::display::order();
                let (_, chosen, mirror) = crate::gui::monitor_choice();
                let primary = crate::gui::monitors().get(crate::gui::primary()).map(|m| m.0).or(chosen).unwrap_or(0);
                match &p.row.ctl {
                    Ctl::Monitors => {
                        let Some(&(output, _)) = monitor_tiles(p.ctl_r).get(k) else { return Action::None };
                        self.arrange(order, output, mirror, L_MONITORS, "Monitor principale cambiato");
                    }
                    Ctl::Swap => {
                        order.rotate_right(1);
                        self.arrange(order, primary, mirror, L_SWAP, "Monitor scambiati");
                    }
                    _ => self.arrange(order, primary, !mirror, L_MIRROR, if mirror { "Monitor affiancati" } else { "Monitor duplicati" }),
                }
            }
            Ctl::Modes => {
                if crate::gui::change_resolution(self.modes[k]).is_err() {
                    self.note = (L_MODES, "Risoluzione non disponibile");
                }
            }
            Ctl::Scale => {
                if crate::gui::change_scale(SIZES[k].0).is_err() {
                    self.note = (L_SCALE, "Dimensione non disponibile");
                }
            }
            Ctl::Volume => {
                self.sliding = true;
                Self::set_volume((ui::slider_value(volume_bar(p.ctl_r), ax) / 10) as i32);
            }
            Ctl::Test => {
                let _ = crate::dev::audio::play(crate::dev::audio::tone(660, 300));
                return Action::None;
            }
            Ctl::Chime => settings::update(|s| s.startup_sound = !s.startup_sound),
            // The desktop sees the change at its next tick.
            Ctl::DockCompact => settings::update(|s| s.dock_compact = !s.dock_compact),
            Ctl::DockMagnify => settings::update(|s| s.dock_magnify = !s.dock_magnify),
            Ctl::Fluidity => {
                let _ = crate::gui::set_fluidity(crate::gui::pace::CHOICES[k.min(crate::gui::pace::CHOICES.len() - 1)]);
            }
            Ctl::Layout => {
                settings::update(|s| s.layout = LAYOUTS[k].0);
                self.note = (L_LAYOUT, "Tastiera cambiata");
            }
            Ctl::LockAfter => settings::update(|s| s.lock_after = LOCK_AFTER[k].0),
            Ctl::Forget(i) => {
                if let Some(g) = self.privacy.get(*i) {
                    crate::sandbox::forget(&g.id);
                }
                self.privacy = privacy_apps();
            }
            Ctl::Perm(i, j) => {
                // On is "Consenti"; off is denied for good, without
                // questions, until it is switched on again.
                if let Some(g) = self.privacy.get(*i) {
                    if let Some((perm, answer)) = g.perms.get(*j) {
                        let next = if *answer == Some(Answer::Allow) { Answer::Deny } else { Answer::Allow };
                        crate::sandbox::set(&g.id, &g.name, perm, Some(next));
                    }
                }
                self.privacy = privacy_apps();
            }
            Ctl::PermView(perm) => {
                self.privacy_view = Some(perm.clone());
                self.scroll = 0;
                self.hot = None;
            }
            Ctl::Back => {
                self.privacy_view = None;
                self.scroll = 0;
                self.hot = None;
            }
            Ctl::WifiState => {
                let connected = self.wifi.as_ref().is_some_and(|s| s.state != justwifi::sta::State::Idle);
                if connected {
                    crate::net::wifi::disconnect();
                } else {
                    crate::net::wifi::resume_auto();
                    crate::net::wifi::scan();
                }
                self.refresh_wifi();
            }
            Ctl::Net(i) => {
                let i = *i;
                self.join(i, None);
            }
            Ctl::NetPassword => {
                if k == 0 {
                    self.editing_net = true;
                } else if let Some(i) = self.asking {
                    let pw = self.net_password.clone();
                    self.join(i, Some(pw));
                }
            }
            Ctl::Known(i) => {
                if let Some(name) = self.known.get(*i).cloned() {
                    crate::net::wifi::forget(&name);
                }
                self.refresh_wifi();
            }
            Ctl::Info(_) | Ctl::Text | Ctl::Go(..) => {}
        }
        Action::Redraw
    }
}

/// The width of a network's signal.
const BARS_W: i32 = 26;

/// A network's signal: four bars, `bars` of them lit.
fn draw_bars(c: &mut Canvas, r: Rect, bars: u8) {
    let w = (r.w - 3 * 2) / 4;
    for k in 0..4 {
        let h = r.h * (k + 1) / 4;
        let b = Rect::new(r.x + k * (w + 2), r.bottom() - h, w, h);
        c.fill_round(b, 1, if (k as u8) < bars { ui::text() } else { ui::text_faint() });
    }
}

impl App for Prefs {
    fn title(&self) -> String {
        String::from("Impostazioni")
    }

    fn icon(&self) -> &str {
        "settings"
    }

    fn kind(&self) -> &'static str {
        "settings"
    }

    fn size(&self) -> (i32, i32) {
        (900, 580)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn sidebar(&self) -> i32 {
        SIDEBAR
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        self.draw_sidebar(c, area);
        let main = Self::main(area);
        let mut page = self.page(area);
        self.view_h = main.h;
        self.max_scroll = (page.height - main.h).max(0);
        let mut scroll = self.scroll.clamp(0, self.max_scroll);
        if self.reveal {
            // Scroll to the row a search led to, when it is out of sight.
            self.reveal = false;
            let label = self.flash.as_ref().map(|(l, _)| l.as_str()).unwrap_or("");
            if let Some(p) = page.rows.iter().find(|p| p.row.label == label) {
                if p.r.bottom() > main.bottom() - 16 {
                    scroll = (p.r.y - main.y - 90).clamp(0, self.max_scroll);
                }
            }
        }
        if scroll != self.scroll {
            self.scroll = scroll;
            page = self.page(area);
        }
        let saved = c.clip();
        c.set_clip(saved.intersect(&main.grow(-1)));
        let (bh, _) = text::metrics(Font::Body);
        let title = text::fit(Font::Display, &page.title, page.w);
        text::draw(c, Font::Display, page.x + 2, page.title_y, &title, ui::text());
        for (k, line) in page.intro.iter().enumerate() {
            text::draw(c, Font::Body, page.x + 2, page.intro_y + k as i32 * (bh + 3), line, ui::text_dim());
        }
        if page.rows.is_empty() && !self.query.trim().is_empty() {
            text::draw(c, Font::Body, page.x + 2, page.intro_y + 6, "Nessuna impostazione trovata.", ui::text_dim());
        }
        for (r, head) in &page.cards {
            if let Some(s) = head {
                let tile = Rect::new(r.x + 2, r.y - HEAD_H + 2, 20, 20);
                draw_tile(c, tile, *s);
                text::draw(c, Font::Strong, tile.right() + 10, tile.y + (20 - bh) / 2, s.name(), ui::text());
            }
            ui::well_pane(c, *r, CARD_RAD);
            c.fill_round(*r, CARD_RAD, ui::pane());
        }
        let searching = !self.query.trim().is_empty();
        for (i, p) in page.rows.iter().enumerate() {
            if p.r.bottom() < main.y || p.r.y > main.bottom() {
                continue;
            }
            let lit = if searching { i == self.pick } else { self.flash.as_ref().is_some_and(|(l, _)| *l == p.row.label) };
            if lit {
                c.fill_round(p.r.grow(-4), 12, ui::sel());
            } else if self.hot == Some(Target::Result(i)) {
                c.fill_round(p.r.grow(-4), 12, ui::hover());
            }
            if !p.first {
                c.fill(Rect::new(p.r.x + INSET, p.r.y, p.r.w - 2 * INSET, 1), ui::line());
            }
            self.draw_label(c, p);
            self.draw_ctl(c, i, p);
        }
        if self.max_scroll > 0 {
            // Where the view is in the page.
            let track = main.h - 24;
            let h = (track * main.h / page.height.max(1)).max(28);
            let y = main.y + 12 + (track - h) * self.scroll / self.max_scroll;
            c.fill_round(Rect::new(main.right() - 9, y, 4, h), 2, ui::text_faint());
        }
        c.set_clip(saved);
    }

    fn press(&mut self, x: i32, y: i32, _double: bool, area: Rect) -> Action {
        let (ax, ay) = (area.x + x, area.y + y);
        let page = self.page(area);
        let target = self.hit(&page, area, ax, ay);
        self.note = ("", "");
        self.flash = None;
        // A click anywhere else leaves the fields.
        self.editing = false;
        self.editing_device = false;
        self.editing_password = false;
        self.editing_net = false;
        self.searching = false;
        match target {
            None => Action::Redraw,
            Some(Target::Search) => {
                self.searching = true;
                Action::Redraw
            }
            Some(Target::Side(s)) => {
                self.open(s);
                Action::Redraw
            }
            Some(Target::Result(i)) => {
                if let Ctl::Go(s, label) = &page.rows[i].row.ctl {
                    self.go(*s, label);
                }
                Action::Redraw
            }
            Some(Target::Ctl(i, k)) => self.act(&page.rows[i], k, ax),
        }
    }

    fn drag(&mut self, x: i32, _y: i32, area: Rect) -> bool {
        if self.sliding {
            let page = self.page(area);
            if let Some(p) = page.rows.iter().find(|p| p.row.ctl == Ctl::Volume) {
                Self::set_volume((ui::slider_value(volume_bar(p.ctl_r), area.x + x) / 10) as i32);
            }
        }
        self.sliding
    }

    fn release(&mut self, _area: Rect) -> Action {
        self.sliding = false;
        Action::None
    }

    fn hover(&mut self, at: Option<(i32, i32)>, area: Rect) -> bool {
        let t = at.and_then(|(x, y)| {
            let page = self.page(area);
            let t = self.hit(&page, area, area.x + x, area.y + y)?;
            // Only what lights up under the pointer.
            let lights = match t {
                Target::Side(_) | Target::Result(_) => true,
                Target::Ctl(i, k) => match page.rows[i].row.ctl {
                    Ctl::Name | Ctl::Device | Ctl::Password | Ctl::NetPassword => k == 1,
                    Ctl::WifiState | Ctl::Net(_) | Ctl::Known(_) => true,
                    Ctl::Setup | Ctl::Test | Ctl::Forget(_) | Ctl::Modes | Ctl::Swap | Ctl::Monitors | Ctl::Back | Ctl::PermView(_) => true,
                    _ => false,
                },
                Target::Search => false,
            };
            lights.then_some(t)
        });
        let changed = t != self.hot;
        self.hot = t;
        changed
    }

    fn scroll(&mut self, lines: i32) -> bool {
        let old = self.scroll;
        self.scroll = (self.scroll + lines * 40).clamp(0, self.max_scroll);
        old != self.scroll
    }

    fn tick(&mut self) -> bool {
        // Opened again from somewhere that wants a section shown.
        if let Some(s) = take_requested() {
            self.open(s);
            return true;
        }
        if self.flash.as_ref().is_some_and(|(_, until)| crate::time::uptime_ms() >= *until) {
            self.flash = None;
            return true;
        }
        // Wi-Fi: networks come and go, a connection is made.
        if self.section == Section::Wifi && crate::time::uptime_ms() >= self.wifi_seen + 700 {
            return self.refresh_wifi();
        }
        false
    }

    fn key(&mut self, ev: KeyEvent) -> Action {
        if self.editing_net {
            match ev.key {
                Key::Enter => {
                    if let Some(i) = self.asking {
                        let pw = self.net_password.clone();
                        self.join(i, Some(pw));
                    }
                }
                Key::Escape => {
                    self.net_password.clear();
                    self.editing_net = false;
                    self.asking = None;
                }
                _ => {
                    ui::edit_text(&mut self.net_password, &ev, 63);
                }
            }
            return Action::Redraw;
        }
        if self.editing_password {
            match ev.key {
                Key::Enter => self.save_password(),
                Key::Escape => {
                    self.password.clear();
                    self.editing_password = false;
                }
                _ => {
                    ui::edit_text(&mut self.password, &ev, 64);
                }
            }
            return Action::Redraw;
        }
        if self.editing_device {
            match ev.key {
                Key::Enter => self.save_device(),
                Key::Escape => {
                    self.device = crate::host::name();
                    self.editing_device = false;
                }
                Key::Backspace => {
                    self.device.pop();
                }
                Key::Char(ch) if !ev.ctrl && self.device.len() < crate::host::MAX => {
                    if let Some(ch) = crate::host::name_char(ch) {
                        self.device.push(ch);
                    }
                }
                _ => {}
            }
            return Action::Redraw;
        }
        if self.editing {
            match ev.key {
                Key::Enter => self.save_name(),
                Key::Escape => {
                    self.name = settings::get().name;
                    self.editing = false;
                }
                _ => {
                    ui::edit_text(&mut self.name, &ev, 32);
                }
            }
            return Action::Redraw;
        }
        let searching = !self.query.trim().is_empty();
        match ev.key {
            Key::Escape if self.searching || !self.query.is_empty() => {
                self.query.clear();
                self.searching = false;
                self.pick = 0;
                self.scroll = 0;
            }
            Key::Up | Key::Down if searching => {
                let n = self.results().iter().map(|c| c.rows.len()).sum::<usize>();
                self.pick = if ev.key == Key::Up { self.pick.saturating_sub(1) } else { (self.pick + 1).min(n.saturating_sub(1)) };
            }
            Key::Up | Key::Down => {
                let i = SECTIONS.iter().position(|s| *s == self.section).unwrap_or(0);
                let j = if ev.key == Key::Up { i.saturating_sub(1) } else { (i + 1).min(SECTIONS.len() - 1) };
                if i == j {
                    return Action::None;
                }
                self.open(SECTIONS[j]);
            }
            Key::Enter if searching => match self.picked() {
                Some((s, label)) => self.go(s, &label),
                None => return Action::None,
            },
            Key::PageUp | Key::PageDown => {
                let step = (self.view_h - 60).max(40);
                let d = if ev.key == Key::PageUp { -step } else { step };
                let old = self.scroll;
                self.scroll = (self.scroll + d).clamp(0, self.max_scroll);
                if old == self.scroll {
                    return Action::None;
                }
            }
            Key::Backspace if self.searching => {
                self.query.pop();
                self.pick = 0;
                self.scroll = 0;
            }
            // Typing with nothing else to type into searches.
            Key::Char(ch) if !ev.ctrl && !ev.alt && !ch.is_control() && self.query.chars().count() < 40 => {
                self.searching = true;
                self.query.push(ch);
                self.pick = 0;
                self.scroll = 0;
            }
            _ => return Action::None,
        }
        Action::Redraw
    }
}
