//! The Bluetooth pairing popup: what a pairing needs the user for
//! ([`crate::bt::prompt`]), wherever it was started (Impostazioni, the
//! shell's `bt pair`, an app through `just.bluetooth`). As the permission
//! popup: the Bluetooth symbol, the question, the number big, and the
//! buttons.
//!
//! - Confirm: "Il codice è lo stesso?" with "Associa" and "Annulla";
//! - Display: type the number on the device; only "Annulla" (the device
//!   says when it is done);
//! - Enter: a field for the number the device shows.
//!
//! The window goes when the question is answered, or the pairing ends.

use crate::bt::{self, PromptKind};
use crate::dev::input::{Key, KeyEvent};
use crate::gui::gfx::{Canvas, Rect};
use crate::gui::text::{self, Font};
use crate::gui::ui;
use crate::gui::wm::{Action, App};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const WIDTH: i32 = 380;
const PAD: i32 = 24;
const ICON: i32 = 64;
const BUTTON_H: i32 = 44;
const BUTTON_GAP: i32 = 10;
const FIELD_H: i32 = 44;
/// The Bluetooth rune, as on the control centre's tile.
const GLYPH: [&str; 5] = ["..#..", "#.##.", ".##..", "#.##.", "..#.."];
const BLUE: (u32, u32) = (0xFF5A_A9FF, 0xFF25_63EB);

/// The last prompt a window was opened for.
static SHOWN: AtomicU32 = AtomicU32::new(0);

/// A window for a new pairing question, if there is one.
pub fn take_new() -> Vec<Box<dyn App>> {
    let Some(p) = bt::prompt() else { return Vec::new() };
    if SHOWN.swap(p.id, Ordering::Relaxed) == p.id {
        return Vec::new();
    }
    alloc::vec![Box::new(Pair { id: p.id, name: p.name, kind: p.kind, typed: String::new(), hover: None }) as Box<dyn App>]
}

struct Pair {
    id: u32,
    name: String,
    kind: PromptKind,
    /// The number typed (Enter).
    typed: String,
    hover: Option<usize>,
}

struct Layout {
    icon: Rect,
    question: Vec<String>,
    question_y: i32,
    number_y: i32,
    detail: Vec<String>,
    detail_y: i32,
    field: Option<Rect>,
    buttons: Vec<Rect>,
}

impl Pair {
    fn question(&self) -> String {
        format!("Associare {}?", self.name)
    }

    fn detail(&self) -> &'static str {
        match self.kind {
            PromptKind::Confirm(_) => "Associa solo se il dispositivo mostra lo stesso codice.",
            PromptKind::Display(_) => "Scrivi il codice sul dispositivo, poi premi Invio lì. La finestra si chiude da sola.",
            PromptKind::Enter => "Scrivi il codice che mostra il dispositivo.",
        }
    }

    fn number(&self) -> Option<u32> {
        match self.kind {
            PromptKind::Confirm(n) | PromptKind::Display(n) => Some(n),
            PromptKind::Enter => None,
        }
    }

    /// The buttons' labels, the first the main one.
    fn labels(&self) -> &'static [&'static str] {
        match self.kind {
            PromptKind::Display(_) => &["Annulla"],
            _ => &["Associa", "Annulla"],
        }
    }

    fn layout(&self, area: Rect) -> Layout {
        let inner = area.w - 2 * PAD;
        let icon = Rect::new(area.x + (area.w - ICON) / 2, area.y + PAD, ICON, ICON);
        let question = text::wrap(Font::Heading, &self.question(), inner);
        let ql = text::metrics(Font::Heading).0 + 6;
        let question_y = icon.bottom() + 20;
        let mut y = question_y + question.len() as i32 * ql + 8;
        let number_y = y;
        if self.number().is_some() {
            y += text::metrics(Font::Title).0 + 10;
        }
        let detail = text::wrap(Font::Small, self.detail(), inner);
        let detail_y = y;
        y += detail.len() as i32 * 19 + 20;
        let field = (self.kind == PromptKind::Enter).then(|| {
            let f = Rect::new(area.x + PAD, y, inner, FIELD_H);
            y += FIELD_H + 14;
            f
        });
        let mut buttons = Vec::new();
        for _ in self.labels() {
            buttons.push(Rect::new(area.x + PAD, y, inner, BUTTON_H));
            y += BUTTON_H + BUTTON_GAP;
        }
        Layout { icon, question, question_y, number_y, detail, detail_y, field, buttons }
    }

    fn still_asked(&self) -> bool {
        bt::prompt().is_some_and(|p| p.id == self.id)
    }

    fn press_button(&mut self, i: usize) {
        let yes = self.labels()[i] == "Associa";
        if yes && self.kind == PromptKind::Enter && self.typed.is_empty() {
            return;
        }
        bt::answer(self.id, yes, self.typed.parse().unwrap_or(0));
    }
}

impl App for Pair {
    fn title(&self) -> String {
        String::from("Bluetooth")
    }

    fn icon(&self) -> &str {
        "settings"
    }

    fn kind(&self) -> &'static str {
        "prompt"
    }

    fn size(&self) -> (i32, i32) {
        let l = self.layout(Rect::new(0, 0, WIDTH, 0));
        (WIDTH, l.buttons.last().map_or(300, |b| b.bottom()) + PAD - 4)
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        let l = self.layout(area);
        ui::tile(c, l.icon, 18, BLUE);
        ui::soft_pixels_in(c, &GLYPH, l.icon, ICON * 130, ui::WHITE);
        let cx = area.x + area.w / 2;
        let ql = text::metrics(Font::Heading).0 + 6;
        for (i, line) in l.question.iter().enumerate() {
            text::draw_centered(c, Font::Heading, cx, l.question_y + i as i32 * ql, line, ui::text());
        }
        if let Some(n) = self.number() {
            // Grouped in threes, as phones show it.
            let s = format!("{:03} {:03}", n / 1000, n % 1000);
            text::draw_centered(c, Font::Title, cx, l.number_y, &s, ui::text());
        }
        for (i, line) in l.detail.iter().enumerate() {
            text::draw_centered(c, Font::Small, cx, l.detail_y + i as i32 * 19, line, ui::text_dim());
        }
        if let Some(f) = l.field {
            ui::text_field(c, f, &self.typed, "000000", true);
        }
        for (i, (b, label)) in l.buttons.iter().zip(self.labels()).enumerate() {
            ui::button(c, *b, label, i == 0 && self.labels().len() > 1, self.hover == Some(i));
        }
    }

    fn press(&mut self, x: i32, y: i32, _double: bool, area: Rect) -> Action {
        let l = self.layout(area);
        if let Some(i) = l.buttons.iter().position(|b| b.contains(area.x + x, area.y + y)) {
            self.press_button(i);
        }
        Action::Redraw
    }

    fn hover(&mut self, at: Option<(i32, i32)>, area: Rect) -> bool {
        let l = self.layout(area);
        let h = at.and_then(|(x, y)| l.buttons.iter().position(|b| b.contains(area.x + x, area.y + y)));
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    fn key(&mut self, ev: KeyEvent) -> Action {
        match ev.key {
            // Escape says no; Invio says yes only where there is a number
            // to type (the number to compare wants a look and a click).
            Key::Escape => bt::answer(self.id, false, 0),
            Key::Enter if self.kind == PromptKind::Enter => self.press_button(0),
            Key::Backspace => {
                self.typed.pop();
            }
            Key::Char(ch) if self.kind == PromptKind::Enter && ch.is_ascii_digit() && self.typed.len() < 6 => self.typed.push(ch),
            _ => return Action::None,
        }
        Action::Redraw
    }

    fn close(&mut self) {
        if self.still_asked() {
            bt::answer(self.id, false, 0);
        }
    }

    fn alive(&self) -> bool {
        self.still_asked()
    }
}
