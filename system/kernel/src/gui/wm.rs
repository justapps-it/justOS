//! Windows, as in the dynamic bar's design (docs/design/dynamic-bar): a
//! light frosted frame with the app's name at the top left and a pill of
//! three round buttons at the top right (minimise, full screen, close in
//! orange); the content sits in a pane inside the frame, two panes for an
//! app with a sidebar. Eight handles resize it; maximised it fills the
//! desktop above the bar, in full screen all of the screen (square corners,
//! the bar hides). Their content is an [`App`].

use super::gfx::{Canvas, Rect};
use super::icons;
use super::text::{self, Font};
use super::ui;
use crate::dev::input::KeyEvent;
use alloc::boxed::Box;
use alloc::string::String;
pub use justshell::wm::{Handle, Mode, R};
use justshell::wm::{handle_at, MIN_H, MIN_W};

/// The title row.
pub const TITLE_H: i32 = 60;
/// The frame around the content pane, and the gap between panes.
pub const GAP: i32 = 9;
const FRAME_RAD: i32 = 26;
const PANE_RAD: i32 = 18;
const PANE_JOIN: i32 = 6;
/// The buttons: 28 across, 9 apart, in a pill with 10 and 8 of padding.
const CTL_BTN: i32 = 28;
const CTL_GAP: i32 = 9;
const CTL_PAD_X: i32 = 10;
const CTL_PAD_Y: i32 = 8;
// rgba(238,228,230,.9) over the blurred wallpaper, then a faint gradient
// from rgb(242,236,240) to rgb(220,218,222) (the design's 160deg one).
const FRAME_TINT: u32 = 0xE6EE_E4E6;
const FRAME_TOP: u32 = 0x33F2_ECF0;
const FRAME_BOTTOM: u32 = 0x33DC_DADE;
const FRAME_EDGE: u32 = 0x8CFF_FFFF;
const TITLE_INK: u32 = 0xFF1C_1C20;
const TITLE_INK_DIM: u32 = 0x991C_1C20;
const CTL_PILL: u32 = 0x8CFF_FFFF;
const CTL_PILL_EDGE: u32 = 0x99FF_FFFF;
const CTL_BTN_BG: u32 = 0xFF4B_4B50;
const CTL_BTN_HOT: u32 = 0xFF2E_2E33;
const CTL_CLOSE: u32 = 0xFFFF_5A1F;
const CTL_CLOSE_HOT: u32 = 0xFFE8_4A12;

/// What an app asks the desktop to do after an event.
pub enum Action {
    None,
    /// Draw the content again.
    Redraw,
    /// Type a command line into the terminal (opening it).
    Run(String),
    /// Open a file with the app made for it.
    OpenFile(String),
    /// Open a built-in app.
    Open(&'static str),
    /// Start an installed program that opens its own window.
    Spawn(String),
    /// Show a notification: title, text, icon.
    Notify(String, String, &'static str),
    /// The wallpaper changed: draw everything again.
    Wallpaper,
    /// The look changed (the theme): draw the whole screen again.
    RedrawAll,
}

/// What an app is doing that the dock shows while it lasts (see
/// [`App::activity`]).
#[derive(Clone, PartialEq, Eq)]
pub enum Live {
    /// A song loaded in the player: its title, its album, where it is and
    /// how long it is (seconds), whether it plays, and the player's
    /// shuffle and repeat.
    Music { title: String, album: String, pos: u32, len: u32, playing: bool, shuffle: bool, repeat: bool },
    /// A timer: the time left ("04:59"), its name, and whether it rings.
    Timer { left: String, label: String, ringing: bool },
}

/// The content of a window. Positions handed to it are relative to the
/// content area unless said otherwise.
/// What the bar's music player asks of the app playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Toggle,
    Next,
    Prev,
    /// Go to this thousandth of the song.
    Seek(u32),
    Shuffle,
    Repeat,
}

pub trait App {
    fn title(&self) -> String;
    fn icon(&self) -> &str;
    /// The size of the content area the app would like.
    fn size(&self) -> (i32, i32);
    /// Draws the content; `blur` is the blurred wallpaper (for glass).
    fn draw(&mut self, c: &mut Canvas, area: Rect, blur: &Canvas, focused: bool);
    fn press(&mut self, _x: i32, _y: i32, _double: bool, _area: Rect) -> Action {
        Action::None
    }
    /// The pointer moved with the button down, after a press on the content.
    fn drag(&mut self, _x: i32, _y: i32, _area: Rect) -> bool {
        false
    }
    fn release(&mut self, _area: Rect) -> Action {
        Action::None
    }
    /// The pointer moved over the content (`None`: it left).
    fn hover(&mut self, _at: Option<(i32, i32)>, _area: Rect) -> bool {
        false
    }
    /// The right button over the content.
    fn menu(&mut self, _x: i32, _y: i32, _area: Rect) -> Action {
        Action::None
    }
    fn key(&mut self, _ev: KeyEvent) -> Action {
        Action::None
    }
    /// Called ten times a second; true if the content changed.
    fn tick(&mut self) -> bool {
        false
    }
    /// After a [`tick`](Self::tick) that returned true: the part of the
    /// content that changed, when it is not all of it (on the screen;
    /// `area` is the content area). A blinking caret is a few pixels, not
    /// a window.
    fn changed(&mut self, _area: Rect) -> Option<Rect> {
        None
    }
    /// Does the app want a call to [`frame`](Self::frame) on every frame?
    fn animating(&self) -> bool {
        false
    }
    /// A frame while [`animating`](Self::animating); true to redraw.
    fn frame(&mut self) -> bool {
        false
    }
    /// A short name, to find an open window of the same kind.
    fn kind(&self) -> &'static str;
    /// Mouse wheel over the content.
    fn scroll(&mut self, _lines: i32) -> bool {
        false
    }
    /// Can the window be made bigger than [`size`](Self::size)?
    fn resizable(&self) -> bool {
        false
    }
    /// Width of a sidebar drawn as a card of its own at the left of the
    /// content area (0: none). The app still draws into the whole area;
    /// its main content starts at `sidebar() + GAP`.
    fn sidebar(&self) -> i32 {
        0
    }
    /// The window is closing (save what needs saving).
    fn close(&mut self) {}
    /// False when the window should go (its program ended).
    fn alive(&self) -> bool {
        true
    }
    /// Asked after each [`drag`](Self::drag): files the user started to
    /// carry out of the window (absolute paths), if any. The desktop then
    /// carries them until the button goes up.
    fn drag_files(&mut self) -> Option<alloc::vec::Vec<String>> {
        None
    }
    /// Files are carried over the content (`None`: they left); the app may
    /// light up the place they would land. True to redraw.
    fn drop_over(&mut self, _at: Option<(i32, i32)>, _paths: &[String], _area: Rect) -> bool {
        false
    }
    /// Files were let go over the content. `None`: the app does not take
    /// them (the desktop says so); otherwise what to do next.
    fn drop_files(&mut self, _paths: &[String], _x: i32, _y: i32, _area: Rect) -> Option<Action> {
        None
    }
    /// Files changed somewhere (a drop moved or copied some): show them again.
    fn refresh(&mut self) {}
    /// Something going on that the dock shows (music playing, a timer
    /// running), asked about ten times a second; None most of the time.
    fn activity(&self) -> Option<Live> {
        None
    }
    /// The bar's player controls the song (only an app that shows
    /// [`Live::Music`]); true if it did something.
    fn media(&mut self, _m: Media) -> bool {
        false
    }
    /// A number for the window that no other window has, for
    /// [`parent`](Self::parent) to name it (0: none).
    fn ident(&self) -> u64 {
        0
    }
    /// The [`ident`](Self::ident) of the window this one belongs to (a
    /// dialog's window), or 0. It opens over that window, stays above it,
    /// and is minimised and brought back with it.
    fn parent(&self) -> u64 {
        0
    }
}

/// A window: its frame on the screen (title row included), its content and
/// where it is.
pub struct Window {
    pub id: u32,
    pub app: Box<dyn App>,
    pub rect: Rect,
    pub minimized: bool,
    /// Normal, maximised (the bar stays) or full screen (the bar hides).
    pub mode: Mode,
    /// Snapped to half the screen: the frame it had before.
    pub snapped: Option<Rect>,
    /// The virtual desktop it is on.
    pub space: usize,
    /// The window button under the pointer.
    pub hover: Option<usize>,
    /// The title last drawn (a program may change it at any time).
    pub drawn_title: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Title,
    Minimize,
    /// The full-screen button.
    Full,
    Close,
    /// One of the eight resize handles.
    Edge(Handle),
    Body(i32, i32),
}

/// A rectangle for the shell's geometry.
pub fn to_r(r: Rect) -> R {
    R::new(r.x, r.y, r.w, r.h)
}

pub fn from_r(r: R) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// The two panes of a window with a sidebar: big corners outside, small
/// ones where they meet (`left`: the sidebar).
pub fn pane(c: &mut Canvas, r: Rect, left: bool, right: bool) {
    let col = ui::well();
    c.fill_round(r, PANE_RAD, col);
    // Square-ish corners on the inner side: cover them with a small radius.
    let half = r.w / 2;
    if !right {
        c.fill_round(Rect::new(r.x + half, r.y, r.w - half, r.h), PANE_JOIN, col);
    }
    if !left {
        c.fill_round(Rect::new(r.x, r.y, half, r.h), PANE_JOIN, col);
    }
}

impl Window {
    pub fn new(id: u32, app: Box<dyn App>, x: i32, y: i32) -> Window {
        let (w, h) = app.size();
        let (w, h) = Window::outer_size(w, h);
        Window { id, app, rect: Rect::new(x, y, w, h), minimized: false, mode: Mode::Normal, snapped: None, space: 0, hover: None, drawn_title: String::new() }
    }

    /// Is it on desktop `space` and not minimised?
    pub fn shown(&self, space: usize) -> bool {
        !self.minimized && self.space == space
    }

    /// The frame around a content area of `w` x `h`.
    pub fn outer_size(w: i32, h: i32) -> (i32, i32) {
        (w + 2 * GAP, h + TITLE_H + GAP)
    }

    /// Full screen (the bar hides over it).
    pub fn full(&self) -> bool {
        self.mode.is_full()
    }

    /// Maximised or full screen: it does not move or resize by hand.
    pub fn zoomed(&self) -> bool {
        !self.mode.free()
    }

    pub fn title_rect(&self) -> Rect {
        Rect::new(self.rect.x, self.rect.y, self.rect.w, TITLE_H)
    }

    pub fn body(&self) -> Rect {
        Rect::new(self.rect.x + GAP, self.rect.y + TITLE_H, self.rect.w - 2 * GAP, self.rect.h - TITLE_H - GAP)
    }

    /// The pill of window buttons at the right of the title row.
    pub fn controls(&self) -> Rect {
        let w = 3 * CTL_BTN + 2 * CTL_GAP + 2 * CTL_PAD_X;
        let h = CTL_BTN + 2 * CTL_PAD_Y;
        Rect::new(self.rect.right() - GAP - w, self.rect.y + (TITLE_H - h) / 2, w, h)
    }

    /// Minimise, full screen, close.
    pub fn buttons(&self) -> [Rect; 3] {
        let p = self.controls();
        let b = |i: i32| Rect::new(p.x + CTL_PAD_X + i * (CTL_BTN + CTL_GAP), p.y + CTL_PAD_Y, CTL_BTN, CTL_BTN);
        [b(0), b(1), b(2)]
    }

    /// What it covers on the screen, its shadow included.
    pub fn bounds(&self) -> Rect {
        if self.full() {
            self.rect
        } else {
            shadow_area(self.rect)
        }
    }

    /// The resize handle under (`x`, `y`): only for a window that can be
    /// resized and is neither maximised nor full screen.
    pub fn handle_at(&self, x: i32, y: i32) -> Option<Handle> {
        if !self.app.resizable() || self.zoomed() {
            return None;
        }
        handle_at(to_r(self.rect), x, y)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        if let Some(h) = self.handle_at(x, y) {
            return Some(Hit::Edge(h));
        }
        let [min, full, close] = self.buttons();
        if close.contains(x, y) {
            return Some(Hit::Close);
        } else if full.contains(x, y) {
            return Some(Hit::Full);
        } else if min.contains(x, y) {
            return Some(Hit::Minimize);
        } else if self.title_rect().contains(x, y) {
            return Some(Hit::Title);
        }
        let b = self.body();
        if self.rect.contains(x, y) {
            Some(Hit::Body((x - b.x).clamp(0, b.w - 1), (y - b.y).clamp(0, b.h - 1)))
        } else {
            None
        }
    }

    pub fn draw(&mut self, c: &mut Canvas, blur: &Canvas, focused: bool, clip: Rect) {
        let body = self.body();
        let full = self.full();
        let frame = self.rect;
        let rad = if full { 0 } else { FRAME_RAD };
        if clip.overlaps(&self.bounds()) {
            // Light frosted glass: the blurred wallpaper under a pale tint,
            // a faint warm gradient, a white line inside the edge.
            if !full {
                c.shadow(frame, rad, 40, if focused { 120 } else { 80 });
                c.shadow(frame, rad, 8, if focused { 70 } else { 40 });
            }
            c.blit_round(blur, frame, rad, FRAME_TINT);
            c.fill_round_grad(frame, rad, FRAME_TOP, FRAME_BOTTOM);
            c.stroke_round(frame, rad, 1, FRAME_EDGE);
            let t = self.title_rect();
            let (lh, _) = text::metrics(Font::Title15);
            let pill = self.controls();
            self.drawn_title = self.app.title();
            let name = text::fit(Font::Title15, &self.drawn_title, pill.x - t.x - 34);
            let fg = if focused { TITLE_INK } else { TITLE_INK_DIM };
            text::draw(c, Font::Title15, t.x + 17, t.y + (TITLE_H - lh) / 2, &name, fg);
            c.fill_round(pill, pill.h / 2, CTL_PILL);
            c.stroke_round(pill, pill.h / 2, 1, CTL_PILL_EDGE);
            let full_icon = if full { "unmax" } else { "max" };
            for (i, (b, (icon, size))) in self.buttons().iter().zip([("chev-d", 16), (full_icon, 13), ("x", 15)]).enumerate() {
                let hot = self.hover == Some(i);
                let bg = match (i == 2, hot) {
                    (true, false) => CTL_CLOSE,
                    (true, true) => CTL_CLOSE_HOT,
                    (false, false) => CTL_BTN_BG,
                    (false, true) => CTL_BTN_HOT,
                };
                c.fill_circle(b.x + b.w / 2, b.y + b.h / 2, b.w / 2, bg);
                icons::centered(c, icon, size, b.x + b.w / 2, b.y + b.h / 2, ui::WHITE);
            }
        }
        if clip.overlaps(&body) {
            let side = self.app.sidebar();
            if side > 0 {
                let sb = Rect::new(body.x, body.y, side, body.h);
                let main = Rect::new(body.x + side + GAP, body.y, body.w - side - GAP, body.h);
                pane(c, sb, true, false);
                pane(c, main, false, true);
            } else {
                c.fill_round(body, PANE_RAD, ui::well());
            }
            let saved = c.clip();
            c.set_clip(saved.intersect(&body));
            self.app.draw(c, body, blur, focused);
            c.set_clip(saved);
        }
    }

    /// The window drawn on its own (for the animations): the picture and
    /// where it goes on the screen.
    pub fn snapshot(&mut self, blur: &Canvas, focused: bool) -> (Canvas, Rect) {
        let b = self.bounds();
        let local_blur = blur.crop(b);
        let mut layer = Canvas::new_layer_at(b.w, b.h, blur.scale());
        let saved = self.rect;
        self.rect = self.rect.offset(-b.x, -b.y);
        let all = layer.bounds();
        self.draw(&mut layer, &local_blur, focused, all);
        self.rect = saved;
        (layer, b)
    }

    /// The smallest frame: 460 x 320 for a window that can be resized (or
    /// its own size, if smaller), its size for one that cannot.
    pub fn min_size(&self) -> (i32, i32) {
        let (w, h) = Window::outer_size(self.app.size().0, self.app.size().1);
        if self.app.resizable() {
            (MIN_W.min(w), MIN_H.min(h))
        } else {
            (w, h)
        }
    }
}

/// What a framed window's shadow reaches.
pub fn shadow_area(r: Rect) -> Rect {
    r.grow(42).offset(0, 14)
}
