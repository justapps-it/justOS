//! The user's settings, kept in `sys:/system/settings` as `key=value` lines.

use super::ui::{set_theme, Theme};
use crate::dev::input;
use crate::dev::keymap::Layout;
use crate::fs;
use crate::sync::IrqLock;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};

const FILE: &str = "sys:/system/settings";

#[derive(Clone)]
pub struct Settings {
    pub name: String,
    pub layout: Layout,
    pub wallpaper: String,
    pub setup_done: bool,
    /// Screen resolution chosen by the user (needs a display driver).
    pub resolution: Option<(u32, u32)>,
    /// Sound volume, 0 to 100.
    pub volume: u32,
    /// Play a sound when the desktop appears.
    pub startup_sound: bool,
    /// The password that unlocks the computer, as "salt:hash" in hex
    /// (empty: no password).
    pub password: String,
    /// The password is a PIN of this many digits (0: it is not): the lock
    /// screen shows a keypad.
    pub pin: u32,
    /// Lock the screen after this many minutes without input (0: never).
    pub lock_after: u32,
    /// Pixels per unit: 0 chooses by the screen, 1 or 2 as the user wants
    /// (the size of the interface).
    pub scale: u32,
    /// The widgets (greeting, month, system) on the desktop.
    pub widgets: bool,
    /// Dark or light windows, menus and panels.
    pub theme: Theme,
    /// The apps kept in the dock, in order (None: the usual ones): app ids,
    /// "launcher", or "spawn:" and a program's file.
    pub dock: Option<alloc::vec::Vec<String>>,
    /// The most frames a second ("Fluidità"; 0: automatic).
    pub fluidity: u32,
    /// The computer's name, as the user wrote it (empty: the default);
    /// [`crate::host`] makes the host name from it.
    pub device_name: String,
    /// Several monitors: their outputs from left to right (None: in the
    /// controller's order).
    pub monitors: Option<alloc::vec::Vec<u32>>,
    /// The output of the primary monitor, with the clock and the dock
    /// (None: the first output's).
    pub primary: Option<u32>,
    /// Every monitor shows the same picture ("Duplica").
    pub mirror: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            name: String::new(),
            layout: Layout::It,
            wallpaper: String::from(super::res::WALLPAPERS[0].0),
            setup_done: false,
            resolution: None,
            volume: 70,
            startup_sound: true,
            password: String::new(),
            pin: 0,
            lock_after: 5,
            scale: 0,
            widgets: false,
            theme: Theme::Dark,
            dock: None,
            fluidity: 0,
            device_name: String::new(),
            monitors: None,
            primary: None,
            mirror: false,
        }
    }
}

static CURRENT: IrqLock<Option<Settings>> = IrqLock::new(None);

// What the desktop reads on every frame, without a copy of the settings: a
// count of the changes (it looks again at the rest when it moves).
static CHANGES: AtomicU32 = AtomicU32::new(0);

/// How many times the settings changed since the start.
pub fn changes() -> u32 {
    CHANGES.load(Ordering::Relaxed)
}

/// What takes effect at once outside the settings.
fn apply(s: &Settings) {
    super::pace::set_choice(s.fluidity);
    crate::host::set(&s.device_name);
    CHANGES.fetch_add(1, Ordering::Relaxed);
}

fn load() -> Settings {
    let mut s = Settings::default();
    let Ok(bytes) = fs::abs(FILE).and_then(|p| fs::read_file(&p)) else { return s };
    for line in String::from_utf8_lossy(&bytes).lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "name" => s.name = String::from(v),
            "layout" => s.layout = if v == "us" { Layout::Us } else { Layout::It },
            "wallpaper" => s.wallpaper = String::from(v),
            "setup" => s.setup_done = v == "done",
            "resolution" => {
                s.resolution = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
            }
            "volume" => s.volume = v.parse::<u32>().unwrap_or(70).min(100),
            "startup-sound" => s.startup_sound = v != "off",
            "password" => s.password = String::from(v),
            "pin" => s.pin = v.parse::<u32>().unwrap_or(0).min(8),
            "lock-after" => s.lock_after = v.parse().unwrap_or(5),
            "scale" => s.scale = v.parse::<u32>().unwrap_or(0).min(2),
            "widgets" => s.widgets = v == "on",
            "theme" => s.theme = Theme::from_name(v),
            "dock" => s.dock = Some(v.split(';').filter(|e| !e.is_empty()).map(String::from).collect()),
            "fluidity" => s.fluidity = v.parse().unwrap_or(0),
            "device-name" => s.device_name = String::from(v),
            "monitors" => s.monitors = Some(v.split(',').filter_map(|o| o.trim().parse().ok()).collect()),
            "primary" => s.primary = v.parse().ok(),
            "mirror" => s.mirror = v == "on",
            _ => {}
        }
    }
    s
}

/// The current settings (read from disk the first time).
pub fn get() -> Settings {
    let mut cur = CURRENT.lock();
    if cur.is_none() {
        let s = load();
        if s.setup_done {
            input::set_layout(s.layout);
        }
        crate::dev::audio::set_volume(s.volume);
        set_theme(s.theme);
        apply(&s);
        *cur = Some(s);
    }
    cur.clone().unwrap()
}

/// Changes the settings, applies them and writes them to disk.
pub fn update(f: impl FnOnce(&mut Settings)) {
    let mut s = get();
    f(&mut s);
    input::set_layout(s.layout);
    crate::dev::audio::set_volume(s.volume);
    set_theme(s.theme);
    let mut text = format!(
        "name={}\nlayout={}\nwallpaper={}\nsetup={}\nvolume={}\nstartup-sound={}\nlock-after={}\ntheme={}\n",
        s.name,
        if s.layout == Layout::Us { "us" } else { "it" },
        s.wallpaper,
        if s.setup_done { "done" } else { "no" },
        s.volume,
        if s.startup_sound { "on" } else { "off" },
        s.lock_after,
        s.theme.name()
    );
    if !s.password.is_empty() {
        text.push_str(&format!("password={}\n", s.password));
    }
    if let Some((w, h)) = s.resolution {
        text.push_str(&format!("resolution={w}x{h}\n"));
    }
    if s.scale != 0 {
        text.push_str(&format!("scale={}\n", s.scale));
    }
    if s.widgets {
        text.push_str("widgets=on\n");
    }
    if let Some(dock) = &s.dock {
        text.push_str(&format!("dock={}\n", dock.join(";")));
    }
    if s.fluidity != 0 {
        text.push_str(&format!("fluidity={}\n", s.fluidity));
    }
    if !s.device_name.is_empty() {
        text.push_str(&format!("device-name={}\n", s.device_name));
    }
    if let Some(order) = &s.monitors {
        let list: alloc::vec::Vec<String> = order.iter().map(|o| format!("{o}")).collect();
        text.push_str(&format!("monitors={}\n", list.join(",")));
    }
    if let Some(p) = s.primary {
        text.push_str(&format!("primary={p}\n"));
    }
    if s.mirror {
        text.push_str("mirror=on\n");
    }
    if let Ok(p) = fs::abs(FILE) {
        if let Err(e) = fs::write_file(&p, text.as_bytes()) {
            crate::warn!("gui", "cannot save the settings: {e}");
        }
    }
    // Kept before the count of changes moves: whoever sees it sees these.
    *CURRENT.lock() = Some(s.clone());
    apply(&s);
}

/// Rounds of SHA-256 over the password: slow enough to make guessing
/// costly, quick enough to unlock without a wait.
const ROUNDS: u32 = 20_000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn stretch(salt: &str, password: &str) -> String {
    let mut h = justtls::sha2::Sha256::new();
    h.update(salt.as_bytes());
    h.update(password.as_bytes());
    let mut d = h.finish();
    for _ in 0..ROUNDS {
        let mut h = justtls::sha2::Sha256::new();
        h.update(&d);
        h.update(password.as_bytes());
        d = h.finish();
    }
    hex(&d)
}

/// What the settings keep of `password`: "salt:hash" in hex (empty for
/// none).
pub fn password_hash(password: &str) -> String {
    if password.is_empty() {
        return String::new();
    }
    let mut salt = [0u8; 16];
    crate::rand::fill(&mut salt);
    let salt = hex(&salt);
    let hash = stretch(&salt, password);
    format!("{salt}:{hash}")
}

/// Sets the password that unlocks the computer (empty removes it). Four
/// to eight digits make a PIN: the lock screen then shows a keypad.
pub fn set_password(password: &str) {
    let stored = password_hash(password);
    let pin = pin_len(password);
    update(|s| {
        s.password = stored;
        s.pin = pin;
    });
}

/// The digits of a password that is a PIN (four to eight digits), else 0.
pub fn pin_len(password: &str) -> u32 {
    let n = password.len();
    if (4..=8).contains(&n) && password.bytes().all(|b| b.is_ascii_digit()) {
        n as u32
    } else {
        0
    }
}

/// A password is set.
pub fn has_password() -> bool {
    !get().password.is_empty()
}

/// Whether `password` is the one set (true when none is).
pub fn check_password(password: &str) -> bool {
    let s = get();
    let Some((salt, hash)) = s.password.split_once(':') else { return s.password.is_empty() };
    // Compare every byte, however early they differ.
    let given = stretch(salt, password);
    given.len() == hash.len() && given.bytes().zip(hash.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}
