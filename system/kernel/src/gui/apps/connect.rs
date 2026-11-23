//! JustOS Connect: the phones connected and paired, the code for pairing a
//! new one, files and rings to the phone. The service is
//! [`crate::connect`]; this is its window.

use super::picker::{Picked, Picker};
use crate::dev::input::KeyEvent;
use crate::gui::gfx::{Canvas, Rect};
use crate::gui::res;
use crate::gui::text::{self, Font};
use crate::gui::ui;
use crate::gui::wm::{Action, App};
use crate::util::Size;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const PAD: i32 = 20;

/// Something on the window that can be pressed.
#[derive(Clone, PartialEq)]
enum Hit {
    Toggle,
    Send(u32),
    Ring(u32),
    Forget(String),
}

pub struct Connect {
    /// What was drawn last, to redraw only when it changes.
    shown: String,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Rect>,
    picker: Option<Picker>,
    /// The phone the picked file goes to.
    send_to: Option<u32>,
    note: String,
}

impl Connect {
    pub fn new() -> Connect {
        Connect { shown: String::new(), hits: Vec::new(), hover: None, picker: None, send_to: None, note: String::new() }
    }

    /// Everything the window shows, as text: when it changes, it is drawn again.
    fn state() -> String {
        let mut s = format!("{} {:?} {:?}", crate::connect::enabled(), crate::connect::pairing_code(), crate::net::address().map(|ip| ip.0));
        for p in crate::connect::phones() {
            s.push_str(&format!("|{} {} {} {} {} {:?}", p.sid, p.name, p.battery, p.charging, p.watching, p.transfer));
        }
        for (id, name) in crate::connect::paired() {
            s.push_str(&format!("|{id} {name}"));
        }
        s
    }

    fn small_button(c: &mut Canvas, r: Rect, label: &str, primary: bool, hover: bool) {
        ui::button(c, r, label, primary, hover);
    }
}

impl App for Connect {
    fn title(&self) -> String {
        String::from("JustOS Connect")
    }

    fn icon(&self) -> &str {
        "connect"
    }

    fn kind(&self) -> &'static str {
        "connect"
    }

    fn size(&self) -> (i32, i32) {
        (540, 440)
    }

    fn resizable(&self) -> bool {
        true
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        self.shown = Self::state();
        self.hits.clear();
        let on = crate::connect::enabled();
        let x = area.x + PAD;
        let w = area.w - 2 * PAD;
        let mut y = area.y + PAD;

        // Heading: the icon, the name, on or off.
        res::draw_icon(c, "connect", 48, x, y);
        text::draw(c, Font::Heading, x + 62, y + 2, "JustOS Connect", ui::text());
        let ip = crate::net::address().map(|ip| format!("{ip}")).unwrap_or_else(|| String::from("nessuna rete"));
        let sub = if on { format!("{} · {ip}", crate::host::name()) } else { String::from("Spento: i telefoni non trovano questo computer") };
        text::draw(c, Font::Small, x + 62, y + 28, &text::fit(Font::Small, &sub, w - 140), ui::text_dim());
        let t = Rect::new(area.right() - PAD - 52, y + 10, 52, 30);
        ui::toggle(c, t, on);
        self.hits.push((t, Hit::Toggle));
        y += 68;

        // Pairing.
        if let Some((code, name)) = crate::connect::pairing_code() {
            let card = Rect::new(x, y, w, 96);
            c.fill_round(card, 16, 0x33FF_5A1F);
            text::draw(c, Font::Small, card.x + 18, card.y + 14, &format!("Codice per abbinare {name}"), ui::text_dim());
            let spaced: String = code.chars().flat_map(|ch| [ch, ' ']).collect();
            text::draw(c, Font::Display, card.x + 18, card.y + 36, spaced.trim_end(), ui::ACCENT);
            y += 108;
        }

        // Phones connected.
        let phones = crate::connect::phones();
        text::draw(c, Font::Caps, x, y, "CONNESSI", ui::text_faint());
        y += 22;
        if phones.is_empty() {
            let hint = if on {
                "Apri JustOS Connect sul telefono, sulla stessa rete Wi-Fi, e tocca questo computer."
            } else {
                "Accendi Connect per collegare un telefono."
            };
            for line in text::wrap(Font::Body, hint, w) {
                text::draw(c, Font::Body, x, y, &line, ui::text_dim());
                y += 22;
            }
            y += 10;
        }
        for p in &phones {
            let card = Rect::new(x, y, w, if p.transfer.is_some() { 112 } else { 84 });
            ui::well_pane(c, card, 16);
            text::draw(c, Font::Strong, card.x + 16, card.y + 14, &text::fit(Font::Strong, &p.name, w - 260), ui::text());
            let mut info = p.addr.clone();
            if p.battery >= 0 {
                info.push_str(&format!(" · batteria {}%{}", p.battery, if p.charging { " in carica" } else { "" }));
            }
            if p.watching {
                info.push_str(" · guarda lo schermo");
            }
            text::draw(c, Font::Small, card.x + 16, card.y + 38, &text::fit(Font::Small, &info, w - 260), ui::text_dim());
            let send = Rect::new(card.right() - 232, card.y + 18, 116, 32);
            let ring = Rect::new(card.right() - 108, card.y + 18, 92, 32);
            Self::small_button(c, send, "Invia file", true, self.hover == Some(send));
            Self::small_button(c, ring, "Squilla", false, self.hover == Some(ring));
            self.hits.push((send, Hit::Send(p.sid)));
            self.hits.push((ring, Hit::Ring(p.sid)));
            if let Some((name, done, size, out)) = &p.transfer {
                let bar = Rect::new(card.x + 16, card.y + 70, w - 32, 8);
                c.fill_round(bar, 4, ui::line());
                let filled = if *size > 0 { (bar.w as u64 * done.min(size) / size) as i32 } else { 0 };
                c.fill_round(Rect::new(bar.x, bar.y, filled.max(8), bar.h), 4, ui::ACCENT);
                let what = format!("{} {name} · {} di {}", if *out { "Invio" } else { "Ricezione" }, Size(*done), Size(*size));
                text::draw(c, Font::Small, card.x + 16, card.y + 84, &text::fit(Font::Small, &what, w - 32), ui::text_dim());
            }
            y += card.h + 10;
        }

        // Phones paired.
        let paired = crate::connect::paired();
        if !paired.is_empty() {
            y += 6;
            text::draw(c, Font::Caps, x, y, "ABBINATI", ui::text_faint());
            y += 22;
            for (id, name) in paired {
                let row = Rect::new(x, y, w, 40);
                c.fill(Rect::new(x, row.bottom(), w, 1), ui::line());
                text::draw(c, Font::Body, x + 4, y + 10, &text::fit(Font::Body, &name, w - 140), ui::text());
                let forget = Rect::new(row.right() - 110, y + 5, 110, 30);
                Self::small_button(c, forget, "Dimentica", false, self.hover == Some(forget));
                self.hits.push((forget, Hit::Forget(id)));
                y += 44;
            }
        }

        let foot = if self.note.is_empty() { "L'app per Android: justos.dev/connect" } else { &self.note };
        text::draw(c, Font::Small, x, area.bottom() - 28, foot, ui::text_faint());

        if let Some(p) = &mut self.picker {
            p.draw(c, area);
        }
    }

    fn press(&mut self, x: i32, y: i32, double: bool, area: Rect) -> Action {
        let (ax, ay) = (area.x + x, area.y + y);
        if let Some(p) = &mut self.picker {
            match p.press(ax, ay, double, area) {
                Picked::Nothing => {}
                Picked::Cancel => self.picker = None,
                Picked::Open(path) | Picked::Save(path) => {
                    self.picker = None;
                    self.note = match crate::connect::send_file(self.send_to, &path) {
                        Ok(()) => String::new(),
                        Err(e) => String::from(e),
                    };
                }
            }
            return Action::Redraw;
        }
        let Some((_, hit)) = self.hits.iter().find(|(r, _)| r.contains(ax, ay)).cloned() else { return Action::None };
        self.note.clear();
        match hit {
            Hit::Toggle => crate::connect::set_enabled(!crate::connect::enabled()),
            Hit::Send(sid) => {
                self.send_to = Some(sid);
                self.picker = Some(Picker::open("sys:/home", &[]));
            }
            Hit::Ring(sid) => {
                if let Err(e) = crate::connect::ring(Some(sid)) {
                    self.note = String::from(e);
                }
            }
            Hit::Forget(id) => {
                crate::connect::forget(&id);
            }
        }
        Action::Redraw
    }

    fn hover(&mut self, at: Option<(i32, i32)>, area: Rect) -> bool {
        if let Some(p) = &mut self.picker {
            return p.hover(at.map(|(x, y)| (area.x + x, area.y + y)), area);
        }
        let h = at.and_then(|(x, y)| self.hits.iter().find(|(r, _)| r.contains(area.x + x, area.y + y)).map(|(r, _)| *r));
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    fn key(&mut self, ev: KeyEvent) -> Action {
        if let Some(p) = &mut self.picker {
            match p.key(ev) {
                Picked::Cancel => self.picker = None,
                Picked::Open(path) | Picked::Save(path) => {
                    self.picker = None;
                    if let Err(e) = crate::connect::send_file(self.send_to, &path) {
                        self.note = String::from(e);
                    }
                }
                Picked::Nothing => {}
            }
            return Action::Redraw;
        }
        Action::None
    }

    fn scroll(&mut self, lines: i32) -> bool {
        match &mut self.picker {
            Some(p) => p.scroll(lines),
            None => false,
        }
    }

    fn tick(&mut self) -> bool {
        self.picker.is_none() && Self::state() != self.shown
    }
}
