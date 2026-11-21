//! Dialogs the system shows for programs, outside them: may this app have a
//! permission ([`crate::sandbox`]), and the file picker that hands an app
//! the file the user chooses (`PICK_FILE`). Each is a small window of its
//! own over the window of the program that asked, which waits for the
//! answer.
//!
//! The permission popup is as on a phone: the app's icon with the
//! permission's symbol on it, the question ("Consentire a Disegno di usare
//! la fotocamera?") and three big buttons, "Consenti", "Solo questa volta"
//! and "Non consentire". The keyboard starts on "Non consentire", so that
//! Invio never says yes by mistake; Esc says no too.

use super::picker::{Picked, Picker};
use crate::dev::input::{Key, KeyEvent};
use crate::gui::gfx::{Canvas, Rect};
use crate::gui::res;
use crate::gui::text::{self, Font};
use crate::gui::ui;
use crate::gui::wm::{Action, App};
use crate::sandbox::Choice;
use crate::sync::{IrqLock, Mutex};
use crate::task::WaitQueue;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

enum Ask {
    /// May `app` have `perm`? `pids`: the asking process and the ones it
    /// came from, whose window the popup goes over.
    Permission { app: crate::sandbox::App, perm: String, pids: Vec<u32> },
    Pick { app: String, save: bool, name: String },
}

enum Answer {
    Chose(Choice),
    Path(String),
    /// Closed, or the program stopped.
    Cancel,
}

struct Request {
    ask: Ask,
    answer: Option<Answer>,
    /// The answer was taken (or the asker gave up): the window goes.
    done: bool,
}

type Shared = Arc<Mutex<Request>>;

/// Dialogs for the desktop to open.
static NEW: IrqLock<Vec<Shared>> = IrqLock::new(Vec::new());
static ANSWERED: WaitQueue = WaitQueue::new();
/// The permission questions waiting for an answer, for [`pending`].
static OPEN: IrqLock<Vec<Shared>> = IrqLock::new(Vec::new());

/// Shows the dialog and waits for the answer.
fn ask(req: Shared) -> Answer {
    // A question asked for a process (through `just.permissions`, by a
    // thread of the kernel) goes when that process ends.
    let asker = match &req.lock().ask {
        Ask::Permission { pids, .. } => pids.first().copied().filter(|p| *p != 0),
        Ask::Pick { .. } => None,
    };
    NEW.lock().push(req.clone());
    crate::dev::input::wake_gui_soon();
    loop {
        let got = ANSWERED.wait_for_timeout(100, || {
            let mut r = req.lock();
            r.answer.take().map(|a| {
                r.done = true;
                a
            })
        });
        if let Some(a) = got {
            return a;
        }
        if crate::user::kill_requested() || asker.is_some_and(|p| !crate::user::alive(p)) {
            req.lock().done = true;
            return Answer::Cancel;
        }
    }
}

/// Asks whether `app` may have `perm`, over the window of one of `pids`.
pub fn permission(app: &crate::sandbox::App, perm: &str, pids: &[u32]) -> Choice {
    let req = Arc::new(Mutex::new(Request {
        ask: Ask::Permission { app: app.clone(), perm: String::from(perm), pids: pids.to_vec() },
        answer: None,
        done: false,
    }));
    OPEN.lock().push(req.clone());
    let answer = ask(req.clone());
    OPEN.lock().retain(|r| !Arc::ptr_eq(r, &req));
    match answer {
        Answer::Chose(c) => c,
        _ => Choice::Dismiss,
    }
}

/// The permission question on the screen, if one is: the app's id and
/// name, and the permission. For `sandbox test`, which answers it as the
/// user would ([`answer`]).
pub fn pending() -> Option<(String, String, String)> {
    let open: Vec<Shared> = OPEN.lock().clone();
    open.iter().find_map(|r| match &r.lock().ask {
        Ask::Permission { app, perm, .. } => Some((app.id.clone(), app.name.clone(), perm.clone())),
        Ask::Pick { .. } => None,
    })
}

/// Answers the permission question on the screen as a click on one of its
/// buttons would. False if there is none.
pub fn answer(choice: Choice) -> bool {
    let open: Vec<Shared> = OPEN.lock().clone();
    let Some(req) = open.first() else { return false };
    let mut r = req.lock();
    if r.answer.is_some() || r.done {
        return false;
    }
    r.answer = Some(Answer::Chose(choice));
    drop(r);
    ANSWERED.notify_all();
    true
}

pub fn pick(app: &str, save: bool, name: &str) -> Option<String> {
    let req = Arc::new(Mutex::new(Request { ask: Ask::Pick { app: String::from(app), save, name: String::from(name) }, answer: None, done: false }));
    match ask(req) {
        Answer::Path(p) => Some(p),
        _ => None,
    }
}

pub fn take_new() -> Vec<Box<dyn App>> {
    let list = core::mem::take(&mut *NEW.lock());
    list.into_iter()
        .map(|req| {
            let (picker, pids) = match &req.lock().ask {
                Ask::Pick { save: true, name, .. } => (Some(Picker::save("sys:/home/Documenti", name)), Vec::new()),
                Ask::Pick { save: false, .. } => (Some(Picker::open("sys:/home/Documenti", &[])), Vec::new()),
                Ask::Permission { pids, .. } => (None, pids.clone()),
            };
            // Over the asking program's window: one of its own, or one its
            // Linux client showed (Firefox's).
            let parent = if pids.is_empty() {
                0
            } else {
                crate::gui::window::ident_for(&pids).or_else(|| crate::linux::wayland::ident_for(&pids)).unwrap_or(0)
            };
            Box::new(Prompt { req, picker, hover: None, focus: DENY, parent }) as Box<dyn App>
        })
        .collect()
}

// ----------------------------------------------------------------------------
// The permission popup

/// The popup's content width, the margin inside it, the app's icon and the
/// permission's badge on it.
const WIDTH: i32 = 380;
const PAD: i32 = 24;
const ICON: i32 = 64;
const BADGE: i32 = 30;
/// The buttons: their height and the gap between them.
const BUTTON_H: i32 = 44;
const BUTTON_GAP: i32 = 10;
/// The buttons, top to bottom, and what each answers.
const BUTTONS: [(&str, Choice); 3] = [("Consenti", Choice::Allow), ("Solo questa volta", Choice::Once), ("Non consentire", Choice::Deny)];
/// The button the keyboard starts on.
const DENY: usize = 2;

/// The symbol of a permission in soft pixels, for the popup's badge and
/// the settings.
pub fn glyph(perm: &str) -> &'static [&'static str] {
    match perm {
        // A camera with its lens.
        "camera" => &["..###..", "#######", "###.###", "##...##", "###.###", "#######"],
        // A microphone on its stand.
        "microphone" => &["..###..", "..###..", "#.###.#", "#.###.#", ".#...#.", "..###..", "...#..."],
        // A map pin.
        "location" => &[".###.", "#####", "##.##", "#####", ".###.", "..#.."],
        // Waves of a network.
        "network" => &[".#####.", "#.....#", "..###..", ".#...#.", ".......", "...#..."],
        // A bell.
        "notifications" => &["..#..", ".###.", ".###.", ".###.", "#####", "..#.."],
        // A disk.
        p if p.ends_with(':') => &["#######", "#######", ".......", "####.##", "#######"],
        // A folder.
        _ => &["###....", "#######", "#######", "#######", "#######"],
    }
}

/// Where the popup's parts are.
struct Layout {
    icon: Rect,
    question: Vec<String>,
    question_y: i32,
    detail: Vec<String>,
    detail_y: i32,
    buttons: [Rect; 3],
}

fn question_line() -> i32 {
    text::metrics(Font::Heading).0 + 6
}

const DETAIL_LINE: i32 = 19;

/// The popup laid out in `area`, for the question `q` and the line `d`.
fn layout(area: Rect, q: &str, d: &str) -> Layout {
    let inner = area.w - 2 * PAD;
    let icon = Rect::new(area.x + (area.w - ICON) / 2, area.y + PAD, ICON, ICON);
    let question = text::wrap(Font::Heading, q, inner);
    let question_y = icon.bottom() + 20;
    let detail = text::wrap(Font::Small, d, inner);
    let detail_y = question_y + question.len() as i32 * question_line() + 6;
    let mut y = detail_y + detail.len() as i32 * DETAIL_LINE + 22;
    let mut buttons = [Rect::new(0, 0, 0, 0); 3];
    for b in &mut buttons {
        *b = Rect::new(area.x + PAD, y, inner, BUTTON_H);
        y += BUTTON_H + BUTTON_GAP;
    }
    Layout { icon, question, question_y, detail, detail_y, buttons }
}

/// The popup's content height for the question `q` and the line `d`.
fn height(q: &str, d: &str) -> i32 {
    let l = layout(Rect::new(0, 0, WIDTH, 0), q, d);
    l.buttons[2].bottom() + PAD - 4
}

struct Prompt {
    req: Shared,
    picker: Option<Picker>,
    /// The button under the pointer, and the one the keyboard is on.
    hover: Option<usize>,
    focus: usize,
    /// The window it belongs to ([`App::parent`]; 0: none).
    parent: u64,
}

impl Prompt {
    fn answer(&self, a: Answer) {
        let mut r = self.req.lock();
        if r.answer.is_none() && !r.done {
            r.answer = Some(a);
        }
        drop(r);
        ANSWERED.notify_all();
    }

    /// The app, its icon's name, the permission, the question and the line
    /// under it (None for the picker).
    fn texts(&self) -> Option<(String, String, String, String)> {
        match &self.req.lock().ask {
            Ask::Permission { app, perm, .. } => Some((
                app.icon.clone(),
                perm.clone(),
                crate::sandbox::question(&app.name, perm),
                format!("{} Puoi cambiare idea in Impostazioni, Privacy.", crate::sandbox::detail(perm)),
            )),
            Ask::Pick { .. } => None,
        }
    }

    fn button_at(&self, area: Rect, x: i32, y: i32) -> Option<usize> {
        let (_, _, q, d) = self.texts()?;
        layout(area, &q, &d).buttons.iter().position(|b| b.contains(x, y))
    }

    fn picked(&mut self, p: Picked) -> Action {
        match p {
            Picked::Nothing => return Action::Redraw,
            Picked::Cancel => self.answer(Answer::Cancel),
            Picked::Open(path) | Picked::Save(path) => self.answer(Answer::Path(path)),
        }
        Action::Redraw
    }

    fn draw_permission(&self, c: &mut Canvas, area: Rect) {
        let Some((icon, perm, q, d)) = self.texts() else { return };
        let l = layout(area, &q, &d);
        res::draw_icon(c, &icon, ICON, l.icon.x, l.icon.y);
        // The permission's badge on the icon's corner, apart from it by a
        // ring of the window's colour.
        let badge = Rect::new(l.icon.right() - BADGE + 8, l.icon.bottom() - BADGE + 8, BADGE, BADGE);
        c.fill_round(badge.grow(3), 12, ui::well());
        ui::tile(c, badge, 9, ui::ORANGE_GRAD);
        let rows = glyph(&perm);
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(rows.len()) as i32;
        ui::soft_pixels_in(c, rows, badge, BADGE * 620 / cols, ui::WHITE);
        let cx = area.x + area.w / 2;
        for (i, line) in l.question.iter().enumerate() {
            text::draw_centered(c, Font::Heading, cx, l.question_y + i as i32 * question_line(), line, ui::text());
        }
        for (i, line) in l.detail.iter().enumerate() {
            text::draw_centered(c, Font::Small, cx, l.detail_y + i as i32 * DETAIL_LINE, line, ui::text_dim());
        }
        for (i, (b, (label, _))) in l.buttons.iter().zip(BUTTONS).enumerate() {
            ui::button(c, *b, label, i == 0, self.hover == Some(i));
            if i == self.focus {
                c.stroke_round(b.grow(3), 17, 2, ui::ACCENT);
            }
        }
    }
}

impl App for Prompt {
    fn title(&self) -> String {
        match &self.req.lock().ask {
            Ask::Permission { .. } => String::from("Privacy"),
            Ask::Pick { app, save: false, .. } => format!("{app}: apri"),
            Ask::Pick { app, save: true, .. } => format!("{app}: salva"),
        }
    }

    fn icon(&self) -> &str {
        "settings"
    }

    fn kind(&self) -> &'static str {
        "prompt"
    }

    fn parent(&self) -> u64 {
        self.parent
    }

    fn size(&self) -> (i32, i32) {
        if self.picker.is_some() {
            return (600, 410);
        }
        match self.texts() {
            Some((_, _, q, d)) => (WIDTH, height(&q, &d)),
            None => (WIDTH, 300),
        }
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        match &mut self.picker {
            Some(p) => p.draw(c, area),
            None => self.draw_permission(c, area),
        }
    }

    fn press(&mut self, x: i32, y: i32, double: bool, area: Rect) -> Action {
        let (ax, ay) = (area.x + x, area.y + y);
        if let Some(p) = &mut self.picker {
            let got = p.press(ax, ay, double, area);
            return self.picked(got);
        }
        if let Some(i) = self.button_at(area, ax, ay) {
            self.answer(Answer::Chose(BUTTONS[i].1));
        }
        Action::None
    }

    fn hover(&mut self, at: Option<(i32, i32)>, area: Rect) -> bool {
        let at = at.map(|(x, y)| (area.x + x, area.y + y));
        if let Some(p) = &mut self.picker {
            return p.hover(at, area);
        }
        let h = at.and_then(|(x, y)| self.button_at(area, x, y));
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    fn key(&mut self, ev: KeyEvent) -> Action {
        if let Some(p) = &mut self.picker {
            let got = p.key(ev);
            return self.picked(got);
        }
        let n = BUTTONS.len();
        match ev.key {
            // Escape says no; Invio and space press the button the
            // keyboard is on, "Non consentire" at first.
            Key::Escape => self.answer(Answer::Chose(Choice::Deny)),
            Key::Enter | Key::Char(' ') => self.answer(Answer::Chose(BUTTONS[self.focus].1)),
            Key::Tab if ev.shift => self.focus = (self.focus + n - 1) % n,
            Key::Up | Key::Left => self.focus = (self.focus + n - 1) % n,
            Key::Tab | Key::Down | Key::Right => self.focus = (self.focus + 1) % n,
            _ => return Action::None,
        }
        Action::Redraw
    }

    fn scroll(&mut self, lines: i32) -> bool {
        self.picker.as_mut().is_some_and(|p| p.scroll(lines))
    }

    fn close(&mut self) {
        // Closing the popup gives no answer (the app is denied this time
        // and may ask again); closing the picker cancels.
        let a = if self.picker.is_some() { Answer::Cancel } else { Answer::Chose(Choice::Dismiss) };
        self.answer(a);
    }

    fn alive(&self) -> bool {
        let r = self.req.lock();
        r.answer.is_none() && !r.done
    }
}
