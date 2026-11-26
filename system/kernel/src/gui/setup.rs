//! The first start: a short guided setup shown until it has been completed
//! once. Ten steps: welcome, look, keyboard, network, account, the
//! computer's name, date and time, privacy and updates, wallpaper, done.
//! Nothing is kept until the last step ("Inizia"); an interrupted setup
//! starts again from the beginning.

use super::apps::prefs::draw_theme;
use super::gfx::{Canvas, Rect};
use super::res;
use super::settings;
use super::splash::draw_logo;
use super::text::{self, Font};
use super::ui::{self, Anim, Event, Looper, Theme, View};
use super::Display;
use crate::dev::input::{self, Key};
use crate::dev::keymap::Layout;
use crate::fs;
use crate::host;
use crate::net;
use crate::time;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

const STEPS: usize = 10;
const WELCOME: usize = 0;
const LOOK: usize = 1;
const KEYS: usize = 2;
const NET: usize = 3;
const ACCOUNT: usize = 4;
const DEVICE: usize = 5;
const CLOCK: usize = 6;
const PRIVACY: usize = 7;
const WALL: usize = 8;
const DONE: usize = 9;

/// How long a step takes to slide in.
const SLIDE_MS: u64 = 200;

/// The internet check of the network step: 0 not started, 1 running,
/// 2 works, 3 local network only.
static CHECK: AtomicU8 = AtomicU8::new(0);

/// The titles and the line under them.
const HEADS: [(&str, &str); STEPS] = [
    ("", ""),
    ("Come lo preferisci?", "Scegli l'aspetto: lo vedi subito."),
    ("Scegli la tastiera", "Potrai cambiarla dopo nelle Impostazioni."),
    ("Rete", "justOS cerca da solo la rete."),
    ("Il tuo account", "Il nome compare sulla scrivania. La password è facoltativa."),
    ("Nome del dispositivo", "Così il computer si chiama in casa e in rete."),
    ("Data e ora", "Controlla che l'orologio sia giusto."),
    ("Privacy e aggiornamenti", "Decidi che cosa può fare da solo."),
    ("Scegli lo sfondo", "Ne puoi scegliere un altro quando vuoi."),
    ("", ""),
];

/// How an interface is doing, for the network page.
#[derive(Clone, Copy, PartialEq)]
enum Link {
    Up,
    Asking,
    Down,
}

struct Row {
    title: String,
    detail: String,
    link: Link,
}

struct Setup {
    step: usize,
    /// A slide from step `.1` (direction .2: 1 forward, -1 back).
    trans: Option<(Anim, usize, i32)>,
    name: String,
    layout: Layout,
    theme: Theme,
    test: String,
    password: String,
    confirm: String,
    focus: usize,
    device: String,
    device_edited: bool,
    /// Day, month, year, hour, minute as shown; changed by the user or
    /// following the clock.
    clock: [u64; 5],
    clock_edited: bool,
    clock_focus: usize,
    /// Automatic updates, startup sound, widgets.
    switches: [bool; 3],
    switch_focus: usize,
    wall: usize,
    w: i32,
    h: i32,
    sharp: Canvas,
    blur: Canvas,
    thumbs: Vec<Canvas>,
    mouse: (i32, i32),
    done: bool,
}

const SWITCHES: [(&str, &str); 3] = [
    ("Aggiornamenti automatici", "Le app si aggiornano da sole, senza chiederti ogni volta."),
    ("Suono all'avvio", "Un breve suono quando appare la scrivania."),
    ("Widget sulla scrivania", "Saluto, calendario e sistema accanto all'orologio."),
];

fn sh(r: Rect, ox: i32) -> Rect {
    r.offset(ox, 0)
}

fn clock_now() -> [u64; 5] {
    let t = time::from_epoch(time::now());
    [t.day, t.month, t.year, t.hour, t.min]
}

fn strength(p: &str) -> (&'static str, u32) {
    let classes = [
        p.chars().any(|c| c.is_ascii_lowercase()),
        p.chars().any(|c| c.is_ascii_uppercase()),
        p.chars().any(|c| c.is_ascii_digit()),
        p.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let n = p.chars().count();
    if n < 6 {
        ("Password debole: più lunga è meglio", ui::RED)
    } else if n < 10 || classes < 2 {
        ("Password discreta", ui::ORANGE)
    } else {
        ("Password buona", ui::GREEN)
    }
}

fn net_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (n, i) in crate::dev::net::list().into_iter().enumerate() {
        let card = crate::dev::get(i.device).map(|d| d.description()).unwrap_or_default();
        let title = if card.is_empty() { format!("Rete cablata {}", n + 1) } else { format!("Rete cablata · {card}") };
        let (detail, link) = match net::lease(i.index) {
            Some(net::Lease::Bound(c)) => (format!("Connesso · indirizzo {} (DHCP)", c.ip), Link::Up),
            Some(net::Lease::GaveUp) => (String::from("Nessuna risposta: il cavo è collegato?"), Link::Down),
            _ => (String::from("Cerco la rete…"), Link::Asking),
        };
        rows.push(Row { title, detail, link });
    }
    rows
}

fn start_check() {
    if CHECK.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        let started = crate::task::spawn("setup-net", || {
            let ok = net::internet().is_ok();
            CHECK.store(if ok { 2 } else { 3 }, Ordering::Release);
        });
        if started.is_none() {
            CHECK.store(3, Ordering::Release);
        }
    }
}

impl Setup {
    fn card(&self) -> Rect {
        let (cw, ch) = (660.min(self.w - 40), 540.min(self.h - 40));
        Rect::new((self.w - cw) / 2, (self.h - ch) / 2, cw, ch)
    }

    fn cx(&self) -> i32 {
        let c = self.card();
        c.x + c.w / 2
    }

    fn buttons(&self) -> (Rect, Option<Rect>) {
        let card = self.card();
        let cx = self.cx();
        let by = card.bottom() - 92;
        match self.step {
            WELCOME | DONE => (Rect::new(cx - 110, by, 220, 48), None),
            _ => (Rect::new(cx + 8, by, 190, 46), Some(Rect::new(cx - 198, by, 170, 46))),
        }
    }

    fn primary_label(&self) -> &'static str {
        match self.step {
            WELCOME => "INIZIA",
            DONE => "Inizia",
            NET if !net::online() => "Continua senza rete",
            _ => "Avanti",
        }
    }

    fn can_go_on(&self) -> bool {
        match self.step {
            ACCOUNT => !self.name.trim().is_empty() && self.password == self.confirm,
            DEVICE => host::problem(&self.device).is_none(),
            _ => true,
        }
    }

    /// The pieces of step `step` that react to the pointer.
    fn parts(&self, step: usize) -> Vec<Rect> {
        let card = self.card();
        let cx = self.cx();
        let top = card.y + 142;
        let mut v = Vec::new();
        match step {
            LOOK => {
                v.push(Rect::new(cx - 262, top + 6, 250, 168));
                v.push(Rect::new(cx + 12, top + 6, 250, 168));
            }
            KEYS => {
                v.push(Rect::new(cx - 236, top + 6, 228, 74));
                v.push(Rect::new(cx + 8, top + 6, 228, 74));
                v.push(Rect::new(cx - 190, top + 130, 380, 54));
            }
            NET => {
                let n = net_rows().len().clamp(1, 3) as i32;
                v.push(Rect::new(card.right() - 40 - 130, top + n * 70 + 2, 130, 38));
            }
            ACCOUNT => {
                for i in 0..3 {
                    v.push(Rect::new(cx - 190, top + 4 + i * 70, 380, 54));
                }
            }
            DEVICE => v.push(Rect::new(cx - 190, top + 20, 380, 54)),
            CLOCK => {
                for i in 0..5 {
                    let x = cx - 241 + i * 98;
                    v.push(Rect::new(x, top + 70, 90, 34));
                    v.push(Rect::new(x, top + 160, 90, 34));
                }
            }
            PRIVACY => {
                for i in 0..3 {
                    v.push(Rect::new(card.x + 40, top + i * 72, card.w - 80, 62));
                }
            }
            WALL => {
                for i in 0..res::WALLPAPERS.len() as i32 {
                    v.push(Rect::new(cx - 263 + (i % 4) * 134, top + 4 + (i / 4) * 116, 124, 78));
                }
            }
            _ => {}
        }
        v
    }

    fn enter(&mut self, step: usize) {
        match step {
            KEYS => input::set_layout(self.layout),
            NET => {
                if net::online() {
                    start_check();
                }
            }
            ACCOUNT => self.focus = 0,
            DEVICE => {
                if !self.device_edited {
                    self.device = host::suggest(&self.name);
                }
            }
            CLOCK => {
                if !self.clock_edited {
                    self.clock = clock_now();
                }
            }
            _ => {}
        }
    }

    fn go(&mut self, to: usize) {
        if to == self.step || self.trans.is_some() {
            return;
        }
        if self.step == CLOCK && to > CLOCK {
            self.apply_clock();
        }
        let from = self.step;
        self.step = to;
        self.enter(to);
        self.trans = Some((Anim::new(SLIDE_MS), from, if to > from { 1 } else { -1 }));
    }

    fn next(&mut self) {
        if !self.can_go_on() || self.trans.is_some() {
            return;
        }
        if self.step == DONE {
            self.finish();
        } else {
            self.go(self.step + 1);
        }
    }

    fn back(&mut self) {
        if self.step > 0 {
            self.go(self.step - 1);
        }
    }

    fn set_wallpaper(&mut self, i: usize) {
        if i != self.wall {
            self.wall = i;
            let (s, b) = res::wallpaper(res::WALLPAPERS[i].0, self.w, self.h);
            self.sharp = s;
            self.blur = b;
        }
    }

    fn set_theme(&mut self, t: Theme) {
        self.theme = t;
        ui::set_theme(t);
    }

    fn set_layout(&mut self, l: Layout) {
        self.layout = l;
        input::set_layout(l);
    }

    /// Sets the computer's clock if the user corrected it.
    fn apply_clock(&mut self) {
        if !self.clock_edited {
            return;
        }
        let [d, mo, y, h, mi] = self.clock;
        time::set_clock(time::to_epoch(y, mo, d.min(time::days_in_month(y, mo)), h, mi, 0));
        self.clock_edited = false;
    }

    fn adjust_clock(&mut self, i: usize, delta: i64) {
        let wrap = |v: u64, lo: u64, hi: u64| -> u64 {
            let span = (hi - lo + 1) as i64;
            (((v - lo) as i64 + delta).rem_euclid(span)) as u64 + lo
        };
        let c = &mut self.clock;
        match i {
            0 => c[0] = wrap(c[0], 1, time::days_in_month(c[2], c[1])),
            1 => c[1] = wrap(c[1], 1, 12),
            2 => c[2] = wrap(c[2], 2000, 2099),
            3 => c[3] = wrap(c[3], 0, 23),
            _ => c[4] = wrap(c[4], 0, 59),
        }
        c[0] = c[0].min(time::days_in_month(c[2], c[1]));
        self.clock_edited = true;
    }

    fn finish(&mut self) {
        let layout = self.layout;
        let (name, wall) = (String::from(self.name.trim()), res::WALLPAPERS[self.wall].0);
        let (theme, device) = (self.theme, self.device.clone());
        let hash = settings::password_hash(&self.password);
        let pin = settings::pin_len(&self.password);
        let [_, sound, widgets] = self.switches;
        settings::update(|s| {
            s.name = name;
            s.layout = layout;
            s.wallpaper = String::from(wall);
            s.theme = theme;
            s.password = hash;
            s.pin = pin;
            s.device_name = device;
            s.startup_sound = sound;
            s.widgets = widgets;
            s.setup_done = true;
        });
        crate::appupdate::set_auto(self.switches[0]);
        for dir in ["Scrivania", "Documenti", "Scaricati", "Immagini", "Musica", "Video", "Progetti"] {
            if let Ok(p) = fs::abs(&format!("sys:/home/{dir}")) {
                if !fs::exists(&p) {
                    let _ = fs::make_dir(&p);
                }
            }
        }
        self.done = true;
    }

    fn retry(&mut self) {
        net::retry();
        CHECK.store(0, Ordering::Release);
    }

    /// The text of the field with focus on the account page.
    fn account_field(&mut self) -> (&mut String, usize) {
        match self.focus {
            0 => (&mut self.name, 32),
            1 => (&mut self.password, 64),
            _ => (&mut self.confirm, 64),
        }
    }

    fn key(&mut self, k: input::KeyEvent) {
        match k.key {
            Key::Enter => return self.next(),
            Key::Escape => return self.back(),
            _ => {}
        }
        match self.step {
            LOOK => match k.key {
                Key::Left | Key::Up => self.set_theme(Theme::Dark),
                Key::Right | Key::Down => self.set_theme(Theme::Light),
                _ => {}
            },
            KEYS => match k.key {
                Key::Left if self.test.is_empty() || k.alt => self.set_layout(Layout::It),
                Key::Right if self.test.is_empty() || k.alt => self.set_layout(Layout::Us),
                _ => {
                    ui::edit_text(&mut self.test, &k, 40);
                }
            },
            NET => {
                if matches!(k.key, Key::Char(' ') | Key::Char('r')) {
                    self.retry();
                }
            }
            ACCOUNT => match k.key {
                Key::Tab if k.shift => self.focus = (self.focus + 2) % 3,
                Key::Tab | Key::Down => self.focus = (self.focus + 1) % 3,
                Key::Up => self.focus = (self.focus + 2) % 3,
                _ => {
                    let (f, max) = self.account_field();
                    ui::edit_text(f, &k, max);
                }
            },
            DEVICE => match k.key {
                Key::Backspace => {
                    self.device.pop();
                    self.device_edited = true;
                }
                Key::Char(ch) if !k.ctrl && self.device.len() < host::MAX => {
                    if let Some(ch) = host::name_char(ch) {
                        self.device.push(ch);
                        self.device_edited = true;
                    }
                }
                _ => {}
            },
            CLOCK => match k.key {
                Key::Left => self.clock_focus = (self.clock_focus + 4) % 5,
                Key::Right => self.clock_focus = (self.clock_focus + 1) % 5,
                Key::Tab if k.shift => self.clock_focus = (self.clock_focus + 4) % 5,
                Key::Tab => self.clock_focus = (self.clock_focus + 1) % 5,
                Key::Up => self.adjust_clock(self.clock_focus, 1),
                Key::Down => self.adjust_clock(self.clock_focus, -1),
                _ => {}
            },
            PRIVACY => match k.key {
                Key::Up => self.switch_focus = (self.switch_focus + 2) % 3,
                Key::Down | Key::Tab => self.switch_focus = (self.switch_focus + 1) % 3,
                Key::Char(' ') => self.switches[self.switch_focus] ^= true,
                _ => {}
            },
            WALL => {
                let n = res::WALLPAPERS.len();
                match k.key {
                    Key::Left if self.wall > 0 => self.set_wallpaper(self.wall - 1),
                    Key::Right if self.wall + 1 < n => self.set_wallpaper(self.wall + 1),
                    Key::Up if self.wall >= 4 => self.set_wallpaper(self.wall - 4),
                    Key::Down if self.wall + 4 < n => self.set_wallpaper(self.wall + 4),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn press(&mut self, x: i32, y: i32) {
        let (primary, back) = self.buttons();
        if primary.contains(x, y) {
            return self.next();
        }
        if back.is_some_and(|b| b.contains(x, y)) {
            return self.back();
        }
        let hit = self.parts(self.step).iter().position(|r| r.contains(x, y));
        let Some(i) = hit else { return };
        match self.step {
            LOOK => self.set_theme(if i == 0 { Theme::Dark } else { Theme::Light }),
            KEYS if i < 2 => self.set_layout(if i == 0 { Layout::It } else { Layout::Us }),
            NET => {
                if Self::can_retry(&net_rows()) {
                    self.retry();
                }
            }
            ACCOUNT => self.focus = i,
            CLOCK => {
                self.clock_focus = i / 2;
                self.adjust_clock(i / 2, if i % 2 == 0 { 1 } else { -1 });
            }
            PRIVACY => {
                self.switch_focus = i;
                self.switches[i] ^= true;
            }
            WALL => self.set_wallpaper(i),
            _ => {}
        }
    }

    fn damage(&self, l: &mut Looper) {
        l.damage(ui::with_shadow(self.card()));
    }

    /// Whether the retry button of the network page shows.
    fn can_retry(rows: &[Row]) -> bool {
        !rows.is_empty() && (rows.iter().any(|r| r.link == Link::Down) || CHECK.load(Ordering::Acquire) == 3)
    }
}

impl View for Setup {
    fn resized(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        let (sharp, blur) = res::wallpaper(res::WALLPAPERS[self.wall].0, w, h);
        self.sharp = sharp;
        self.blur = blur;
    }

    fn animating(&self) -> bool {
        self.trans.is_some()
    }

    fn event(&mut self, l: &mut Looper, ev: Event) -> bool {
        let before_wall = self.wall;
        let before_theme = self.theme;
        match ev {
            Event::Move { x, y, .. } => {
                let (primary, back) = self.buttons();
                let hit = |p: (i32, i32)| (primary.contains(p.0, p.1), back.is_some_and(|b| b.contains(p.0, p.1)));
                let was = hit(self.mouse);
                self.mouse = (x, y);
                if was != hit(self.mouse) {
                    self.damage(l);
                }
            }
            Event::Press { x, y, .. } if self.trans.is_none() => {
                self.press(x, y);
                self.damage(l);
            }
            Event::Key(k) if self.trans.is_none() => {
                self.key(k);
                self.damage(l);
            }
            Event::Frame => {
                if self.trans.as_ref().is_some_and(|t| t.0.done()) {
                    self.trans = None;
                }
                self.damage(l);
            }
            Event::Tick => {
                // The network and the clock change by themselves.
                match self.step {
                    NET => {
                        if net::online() {
                            start_check();
                        }
                        self.damage(l);
                    }
                    CLOCK if !self.clock_edited => {
                        let now = clock_now();
                        if now != self.clock {
                            self.clock = now;
                            self.damage(l);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if before_wall != self.wall || before_theme != self.theme {
            l.damage(Rect::new(0, 0, self.w, self.h));
        }
        self.done
    }

    fn draw(&mut self, c: &mut Canvas, r: Rect) {
        // The wallpaper, softened and darkened, behind the card.
        c.blit(&self.blur, r.x, r.y, r);
        c.fill(r, 0x5014_0E1C);
        let card = self.card();
        if !r.overlaps(&ui::with_shadow(card)) {
            return;
        }
        ui::glass(c, &self.blur, card, 30, 0xB023_1A2E);
        let saved = c.clip();
        c.set_clip(saved.intersect(&Rect::new(card.x + 2, card.y + 2, card.w - 4, card.h - 4)));
        match self.trans {
            Some((a, from, dir)) if !a.done() => {
                let slide = (card.w * a.out() as i32) / 1000;
                self.draw_step(c, from, -dir * slide);
                self.draw_step(c, self.step, dir * (card.w - slide));
            }
            _ => self.draw_step(c, self.step, 0),
        }
        c.set_clip(saved);
        self.draw_chrome(c);
    }
}

impl Setup {
    /// Small print, wrapped and centred; returns where the next line goes.
    fn note(&self, c: &mut Canvas, cx: i32, y: i32, width: i32, s: &str, color: u32) -> i32 {
        let (lh, _) = text::metrics(Font::Small);
        let mut y = y;
        for line in text::wrap(Font::Small, s, width) {
            text::draw_centered(c, Font::Small, cx, y, &line, color);
            y += lh + 4;
        }
        y
    }

    fn draw_step(&self, c: &mut Canvas, step: usize, ox: i32) {
        let card = self.card();
        let cx = self.cx() + ox;
        let top = card.y + 142;
        let parts: Vec<Rect> = self.parts(step).into_iter().map(|r| sh(r, ox)).collect();
        if matches!(step, WELCOME | DONE) {
            draw_logo(c, cx, card.y + 96, 35, ui::knob_glyph());
        } else {
            let (title, sub) = HEADS[step];
            text::draw_centered(c, Font::Title, cx, card.y + 34, title, ui::text());
            let sub = text::fit(Font::Body, sub, card.w - 60);
            text::draw_centered(c, Font::Body, cx, card.y + 92, &sub, ui::text_dim());
        }
        match step {
            WELCOME => {
                let y = card.y + 176;
                text::draw_centered(c, Font::Title, cx, y, "Benvenuto in justOS", ui::text());
                text::draw_centered(c, Font::Body, cx, y + 64, "Prepariamo il computer in pochi passi.", ui::text_dim());
                text::draw_centered(c, Font::Body, cx, y + 88, "Ci vuole meno di un minuto.", ui::text_dim());
                // The language: Italian is the only one for now.
                let pill = Rect::new(cx - 90, y + 134, 180, 34);
                c.fill_round(pill, 17, ui::well());
                c.stroke_round(pill, 17, 1, ui::line());
                text::draw_centered(c, Font::Small, cx, pill.y + 9, "Lingua: Italiano", ui::text_dim());
            }
            LOOK => {
                for (i, (p, label)) in [(&ui::DARK, "Scuro"), (&ui::LIGHT, "Chiaro")].into_iter().enumerate() {
                    let sel = (i == 0) == (self.theme == Theme::Dark);
                    draw_theme(c, parts[i].grow(-4), p, None);
                    if sel {
                        c.stroke_round(parts[i], 16, 3, ui::ACCENT);
                    }
                    text::draw_centered(
                        c,
                        Font::Strong,
                        parts[i].x + parts[i].w / 2,
                        parts[i].bottom() + 10,
                        label,
                        if sel { ui::text() } else { ui::text_dim() },
                    );
                }
            }
            KEYS => {
                for i in 0..2 {
                    let sel = (i == 0) == (self.layout == Layout::It);
                    option_tile(
                        c,
                        parts[i],
                        if i == 0 { "Italiano" } else { "English (US)" },
                        if i == 0 { "Layout italiano" } else { "US layout" },
                        sel,
                    );
                }
                ui::caps(c, parts[2].x + 4, parts[2].y - 26, "Prova a scrivere qui", ui::text_faint());
                ui::text_field(c, parts[2], &self.test, "Scrivi qualcosa", true);
            }
            NET => self.draw_net(c, cx, top, card, &parts, ox),
            ACCOUNT => {
                let dots = |s: &str| -> String { s.chars().map(|_| '•').collect() };
                ui::text_field(c, parts[0], &self.name, "Il tuo nome", self.focus == 0);
                ui::text_field(c, parts[1], &dots(&self.password), "Password (facoltativa)", self.focus == 1);
                ui::text_field(c, parts[2], &dots(&self.confirm), "Ripeti la password", self.focus == 2);
                let y = parts[2].bottom() + 14;
                let (msg, color) = if self.password.is_empty() && self.confirm.is_empty() {
                    ("Senza password: chiunque può aprire il computer.", ui::text_faint())
                } else if self.password != self.confirm {
                    ("Le password non coincidono.", ui::RED)
                } else {
                    strength(&self.password)
                };
                text::draw_centered(c, Font::Small, cx, y, msg, color);
            }
            DEVICE => {
                ui::text_field(c, parts[0], &self.device, host::DEFAULT, true);
                let y = parts[0].bottom() + 16;
                match host::problem(&self.device) {
                    Some(p) => text::draw_centered(c, Font::Small, cx, y, p, ui::RED),
                    None => {
                        let line = format!("Nome in rete: {}", self.device.to_ascii_lowercase());
                        text::draw_centered(c, Font::Strong, cx, y, &line, ui::text());
                    }
                }
                self.note(
                    c,
                    cx,
                    y + 44,
                    card.w - 140,
                    "Lettere senza accenti, cifre e trattini. Lo vedono il router, gli altri computer e i programmi Linux.",
                    ui::text_dim(),
                );
            }
            CLOCK => {
                let [d, mo, y, h, mi] = self.clock;
                let now = format!("{d:02}/{mo:02}/{y}   {h:02}:{mi:02}");
                text::draw_centered(c, Font::Title, cx, top - 6, &now, ui::text());
                const LABELS: [&str; 5] = ["Giorno", "Mese", "Anno", "Ore", "Minuti"];
                for i in 0..5 {
                    let (up, down) = (parts[2 * i], parts[2 * i + 1]);
                    let focused = self.clock_focus == i;
                    ui::caps(c, up.x + 4, up.y - 26, LABELS[i], ui::text_faint());
                    ui::button(c, up, "+", false, false);
                    let val = Rect::new(up.x, up.bottom() + 6, up.w, down.y - up.bottom() - 12);
                    c.fill_round(val, 14, ui::well());
                    c.stroke_round(val, 14, if focused { 2 } else { 1 }, if focused { ui::ACCENT } else { ui::line() });
                    let s = if i == 2 { format!("{}", self.clock[i]) } else { format!("{:02}", self.clock[i]) };
                    let (lh, _) = text::metrics(Font::Heading);
                    text::draw_centered(c, Font::Heading, val.x + val.w / 2, val.y + (val.h - lh) / 2, &s, ui::text());
                    ui::button(c, down, "-", false, false);
                }
                self.note(
                    c,
                    cx,
                    top + 226,
                    card.w - 140,
                    "justOS non ha ancora i fusi orari: l'orologio del computer è l'ora locale. Se è sbagliato, correggilo con + e -.",
                    ui::text_dim(),
                );
            }
            PRIVACY => {
                for (i, r) in parts.iter().enumerate() {
                    let focus = self.switch_focus == i;
                    c.fill_round(*r, 18, ui::well());
                    c.stroke_round(*r, 18, if focus { 2 } else { 1 }, if focus { ui::ACCENT } else { ui::line() });
                    text::draw(c, Font::Strong, r.x + 20, r.y + 10, SWITCHES[i].0, ui::text());
                    let sub = text::fit(Font::Small, SWITCHES[i].1, r.w - 110);
                    text::draw(c, Font::Small, r.x + 20, r.y + 36, &sub, ui::text_dim());
                    ui::toggle(c, Rect::new(r.right() - 76, r.y, 56, r.h), self.switches[i]);
                }
                self.note(
                    c,
                    cx,
                    top + 228,
                    card.w - 140,
                    "Le app del Negozio vedono solo la propria cartella: ti chiedono il permesso quando serve altro (Impostazioni, Privacy).",
                    ui::text_dim(),
                );
            }
            WALL => {
                for (i, o) in parts.iter().enumerate() {
                    let sel = i == self.wall;
                    if sel {
                        c.stroke_round(o.grow(4), 18, 3, ui::ACCENT);
                    }
                    draw_thumb(c, &self.thumbs[i], *o, 14);
                    text::draw_centered(
                        c,
                        Font::Strong,
                        o.x + o.w / 2,
                        o.bottom() + 8,
                        res::WALLPAPERS[i].1,
                        if sel { ui::text() } else { ui::text_dim() },
                    );
                }
            }
            _ => {
                let title = format!("Tutto pronto, {}!", self.name.trim());
                let y = card.y + 168;
                text::draw_centered(c, Font::Title, cx, y, &text::fit(Font::Title, &title, card.w - 60), ui::text());
                let rows = [
                    ("Dispositivo", self.device.clone()),
                    ("Password", String::from(if self.password.is_empty() { "Nessuna" } else { "Impostata" })),
                    ("Aspetto", String::from(if self.theme == Theme::Dark { "Scuro" } else { "Chiaro" })),
                    ("Tastiera", String::from(if self.layout == Layout::It { "Italiano" } else { "English (US)" })),
                    ("Rete", String::from(if net::online() { "Connesso" } else { "Nessuna rete" })),
                ];
                let mut ry = y + 70;
                for (k, v) in rows {
                    text::draw(c, Font::Body, cx - 150, ry, k, ui::text_dim());
                    let v = text::fit(Font::Strong, &v, 190);
                    text::draw_right(c, Font::Strong, cx + 150, ry, &v, ui::text());
                    ry += 32;
                }
            }
        }
    }

    fn draw_net(&self, c: &mut Canvas, cx: i32, top: i32, card: Rect, parts: &[Rect], ox: i32) {
        let rows = net_rows();
        let n = rows.len().clamp(1, 3) as i32;
        let (x, w) = (card.x + 40 + ox, card.w - 80);
        if rows.is_empty() {
            let r = Rect::new(x, top, w, 62);
            c.fill_round(r, 18, ui::well());
            c.stroke_round(r, 18, 1, ui::line());
            c.fill_circle(r.x + 28, r.y + 31, 7, ui::RED);
            text::draw(c, Font::Strong, r.x + 52, r.y + 10, "Nessuna scheda di rete", ui::text());
            text::draw(c, Font::Small, r.x + 52, r.y + 36, "Manca il driver, oppure il computer non ne ha una.", ui::text_dim());
        }
        for (i, row) in rows.iter().take(3).enumerate() {
            let r = Rect::new(x, top + i as i32 * 70, w, 62);
            c.fill_round(r, 18, ui::well());
            c.stroke_round(r, 18, 1, ui::line());
            let color = match row.link {
                Link::Up => ui::GREEN,
                Link::Asking => ui::ORANGE,
                Link::Down => ui::RED,
            };
            c.fill_circle(r.x + 28, r.y + 31, 7, color);
            text::draw(c, Font::Strong, r.x + 52, r.y + 10, &text::fit(Font::Strong, &row.title, w - 72), ui::text());
            text::draw(c, Font::Small, r.x + 52, r.y + 36, &text::fit(Font::Small, &row.detail, w - 72), ui::text_dim());
        }
        let y = top + n * 70 + 10;
        let up = rows.iter().any(|r| r.link == Link::Up);
        let (line, color) = if !up {
            ("Nessuna connessione", ui::text_dim())
        } else {
            match CHECK.load(Ordering::Acquire) {
                2 => ("Internet funziona", ui::GREEN),
                3 => ("Rete locale soltanto", ui::ORANGE),
                _ => ("Controllo Internet…", ui::text_dim()),
            }
        };
        text::draw(c, Font::Strong, x + 4, y, line, color);
        if Self::can_retry(&rows) {
            ui::button(c, parts[0], "Riprova", false, false);
        }
        let mut ny = y + 36;
        if !up {
            ny = self.note(c, cx, ny, card.w - 100, "Il Wi-Fi non è ancora supportato: collega un cavo di rete.", ui::text_dim());
        }
        self.note(c, cx, ny, card.w - 100, "Puoi continuare anche senza rete: la colleghi più tardi.", ui::text_faint());
    }

    /// The buttons and the dots: the same for every step.
    fn draw_chrome(&self, c: &mut Canvas) {
        let card = self.card();
        let cx = self.cx();
        let (primary, back) = self.buttons();
        let (mx, my) = self.mouse;
        if let Some(b) = back {
            ui::button(c, b, "Indietro", false, b.contains(mx, my));
        }
        if self.can_go_on() {
            ui::button(c, primary, self.primary_label(), true, primary.contains(mx, my));
        } else {
            c.fill_round(primary, primary.h / 2, ui::cell());
            let (lh, _) = text::metrics(Font::Strong);
            text::draw_centered(
                c,
                Font::Strong,
                primary.x + primary.w / 2,
                primary.y + (primary.h - lh) / 2,
                self.primary_label(),
                ui::text_faint(),
            );
        }
        // Where we are: a dot per step, the current one wider.
        let total = STEPS as i32 * 14 + 14;
        let mut dx = cx - total / 2;
        for i in 0..STEPS {
            let wide = i == self.step;
            let w = if wide { 26 } else { 8 };
            c.fill_round(Rect::new(dx, card.bottom() - 30, w, 8), 4, if wide { ui::ACCENT } else { ui::cell() });
            dx += w + 6;
        }
    }
}

fn option_tile(c: &mut Canvas, r: Rect, title: &str, sub: &str, selected: bool) {
    c.fill_round(r, 18, if selected { ui::sel() } else { ui::pane() });
    c.stroke_round(r, 18, if selected { 2 } else { 1 }, if selected { ui::ACCENT } else { ui::line() });
    text::draw(c, Font::Heading, r.x + 20, r.y + 14, title, ui::text());
    text::draw(c, Font::Small, r.x + 20, r.y + 42, sub, ui::text_dim());
    if selected {
        c.fill_circle(r.right() - 26, r.y + r.h / 2, 11, ui::ACCENT);
        c.fill_circle(r.right() - 26, r.y + r.h / 2, 5, ui::WHITE);
    }
}

/// A picture with rounded corners.
pub fn draw_thumb(c: &mut Canvas, img: &Canvas, r: Rect, rad: i32) {
    c.blit_round_from(img, 0, 0, r, rad, 0);
}

/// Whether the first-start setup has been completed.
pub fn done() -> bool {
    settings::get().setup_done
}

/// Marks the setup as completed without asking anything (for automation: the
/// shell's `setup skip`); the name stays as it is, or "justOS" if none.
pub fn skip() {
    settings::update(|s| {
        if s.name.trim().is_empty() {
            s.name = String::from("justOS");
        }
        s.setup_done = true;
    });
}

/// Changes the computer's name and keeps it with the settings.
pub fn device_name(name: &str) {
    settings::update(|s| s.device_name = String::from(name));
}

pub fn run(d: &mut Display) {
    let (w, h) = (d.width(), d.height());
    let s = settings::get();
    let wall = res::WALLPAPERS.iter().position(|(n, _)| *n == s.wallpaper).unwrap_or(0);
    let (sharp, blur) = res::wallpaper(res::WALLPAPERS[wall].0, w, h);
    let thumbs = res::WALLPAPERS.iter().map(|(n, _)| res::wallpaper_thumb(n, 124, 78)).collect();
    let mut setup = Setup {
        step: 0,
        trans: None,
        name: s.name.clone(),
        layout: s.layout,
        theme: ui::theme(),
        test: String::new(),
        password: String::new(),
        confirm: String::new(),
        focus: 0,
        device: host::name(),
        device_edited: false,
        clock: clock_now(),
        clock_edited: false,
        clock_focus: 3,
        switches: [crate::appupdate::auto(), s.startup_sound, s.widgets],
        switch_focus: 0,
        wall,
        w,
        h,
        sharp,
        blur,
        thumbs,
        mouse: (-1, -1),
        done: false,
    };
    let mut l = Looper::new(d);
    l.damage_all(d);
    l.run(d, &mut setup);
    let _ = &setup.sharp;
}
