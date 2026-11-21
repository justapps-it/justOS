//! Programs installed inside Wine's C: drive, for the launcher.
//!
//! When an installer makes a Start-menu shortcut, Wine's `winemenubuilder`
//! writes a `.desktop` file (and the program's icon as PNG files) under
//! `$HOME/.local/share/applications/wine` and `.../icons/hicolor`: that is
//! the source here, as Wine has parsed the shortcut and pulled the icon out
//! of the program. An entry only counts while the shortcut it names still
//! exists (an uninstaller removes it, `winemenubuilder` leaves its files).
//! Shortcuts on the Windows desktop (`users/*/Desktop/*.lnk`) are listed too,
//! with the generic icon when Wine made none. A click opens the shortcut
//! through Wine's `start` ([`super::for_file`]), so its folder and arguments
//! are Wine's.

use super::{Entry, Launch};
use crate::fs;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// A Windows program the launcher lists.
#[derive(Clone)]
pub struct Program {
    pub name: String,
    /// The icon as the desktop names pictures ("@sys:/…png"), or "window".
    pub icon: String,
    /// The shortcut (a Linux path on the system volume, "sys:/…").
    pub lnk: String,
    /// The name Wine gives the program's windows (`StartupWMClass`, the
    /// program's file name), when known.
    pub class: String,
}

/// Sizes of icon to look for, best first.
const SIZES: [&str; 5] = ["128x128", "256x256", "64x64", "48x48", "32x32"];

/// Entries that are not programs.
const NOISE: [&str; 14] = [
    "uninstall",
    "disinstall",
    "remove",
    "rimuovi",
    "readme",
    "leggimi",
    "manual",
    "help",
    "website",
    "sito web",
    "documentation",
    "licen",
    "release notes",
    "guida",
];

fn home() -> String {
    crate::linux::extra_env().iter().find_map(|v| v.strip_prefix("HOME=").map(String::from)).unwrap_or_else(|| String::from("/home"))
}

fn prefix() -> String {
    let dir = crate::linux::extra_env().iter().find_map(|v| v.strip_prefix("WINEPREFIX=").map(String::from));
    format!("sys:{}", dir.unwrap_or_else(|| format!("{}/.wine", home())))
}

/// Whether `path` is a shortcut in the Wine prefix (the launcher's way to
/// name a Windows program).
pub fn is_link(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".lnk") && path.starts_with(&prefix())
}

fn exists(path: &str) -> bool {
    fs::abs(path).is_ok_and(|p| fs::exists(&p))
}

/// A Windows path from an Exec line ("C:\\\\users\\\\a\\\\x.lnk", spaces as
/// "\\ ") to the Linux path of the file on the system volume.
fn link_path(exec: &str) -> Option<String> {
    let end = exec.rfind(".lnk")? + 4;
    let b = exec.as_bytes();
    let start = (0..end.saturating_sub(2)).find(|&i| b[i].is_ascii_alphabetic() && b[i + 1] == b':' && b[i + 2] == b'\\')?;
    let raw = &exec[start..end];
    // Runs of backslashes: one separator, or the escape of a space.
    let mut win = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            while chars.peek() == Some(&'\\') {
                chars.next();
            }
            win.push(if chars.peek() == Some(&' ') { ' ' } else { '\\' });
            if chars.peek() == Some(&' ') {
                chars.next();
            }
        } else {
            win.push(c);
        }
    }
    let (drive, rest) = win.split_at(2);
    let rest = rest.trim_start_matches('\\').replace('\\', "/");
    Some(if drive.eq_ignore_ascii_case("c:") {
        format!("{}/drive_c/{rest}", prefix())
    } else if drive.eq_ignore_ascii_case("z:") {
        format!("sys:/{rest}")
    } else {
        return None;
    })
}

fn noise(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    NOISE.iter().any(|w| n.contains(w))
}

/// The best PNG of an icon Wine made.
fn icon_file(icon: &str) -> Option<String> {
    let base = format!("sys:{}/.local/share/icons/hicolor", home());
    SIZES.iter().map(|s| format!("{base}/{s}/apps/{icon}.png")).find(|f| exists(f)).map(|f| format!("@{f}"))
}

fn walk(dir: &str, out: &mut Vec<String>, depth: u32) {
    let Some(list) = fs::abs(dir).ok().and_then(|p| fs::list_dir(&p).ok()) else { return };
    for (e, s) in list {
        let path = format!("{dir}/{}", e.name);
        if s.kind == justfs::Kind::Dir && depth < 5 {
            walk(&path, out, depth + 1);
        } else if e.name.ends_with(".desktop") {
            out.push(path);
        }
    }
}

/// The programs Wine's shortcuts lead to.
pub fn programs() -> Vec<Program> {
    let mut v: Vec<Program> = Vec::new();
    let mut files = Vec::new();
    walk(&format!("sys:{}/.local/share/applications/wine", home()), &mut files, 0);
    for f in files {
        let Some(text) = fs::abs(&f).ok().and_then(|p| fs::read_file(&p).ok()) else { continue };
        let text = String::from_utf8_lossy(&text);
        let (mut name, mut exec, mut icon, mut class) = (String::new(), String::new(), String::new(), String::new());
        let mut hidden = false;
        let mut main = false;
        for line in text.lines() {
            if line.starts_with('[') {
                main = line.trim() == "[Desktop Entry]";
            } else if let (true, Some((k, val))) = (main, line.split_once('=')) {
                match k.trim() {
                    "Name" => name = val.trim().into(),
                    "Exec" => exec = val.trim().into(),
                    "Icon" => icon = val.trim().into(),
                    "StartupWMClass" => class = val.trim().into(),
                    "NoDisplay" | "Hidden" if val.trim() == "true" => hidden = true,
                    _ => {}
                }
            }
        }
        let Some(lnk) = link_path(&exec) else { continue };
        if hidden || name.is_empty() || noise(&name) || !exists(&lnk) || v.iter().any(|p| p.lnk == lnk) {
            continue;
        }
        let icon = icon_file(&icon).unwrap_or_else(|| String::from("window"));
        v.push(Program { name, icon, lnk, class });
    }
    // Shortcuts on the Windows desktop that Wine made no entry for.
    let users = format!("{}/drive_c/users", prefix());
    if let Some(list) = fs::abs(&users).ok().and_then(|p| fs::list_dir(&p).ok()) {
        for (u, _) in list {
            let desk = format!("{users}/{}/Desktop", u.name);
            for (e, s) in fs::abs(&desk).ok().and_then(|p| fs::list_dir(&p).ok()).unwrap_or_default() {
                let stem = e.name.strip_suffix(".lnk").unwrap_or("");
                let lnk = format!("{desk}/{}", e.name);
                if s.kind == justfs::Kind::File && !stem.is_empty() && !noise(stem) && !v.iter().any(|p| p.lnk == lnk || p.name == stem) {
                    v.push(Program { name: String::from(stem), icon: String::from("window"), lnk, class: String::new() });
                }
            }
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// The launcher entries.
pub fn entries() -> Vec<Entry> {
    programs().into_iter().map(|p| Entry { name: p.name, icon: p.icon, launch: Launch::Spawn(p.lnk) }).collect()
}

pub fn find(lnk: &str) -> Option<Program> {
    programs().into_iter().find(|p| p.lnk == lnk)
}

/// The icon for the window of a Windows program by the name its window
/// class has (its file name, "steam.exe").
pub fn icon_for_class(class: &str) -> Option<String> {
    programs().into_iter().find(|p| !p.class.is_empty() && p.class.eq_ignore_ascii_case(class) && p.icon != "window").map(|p| p.icon)
}

/// The name and icon of a Windows program by its window class, as the
/// launcher shows it ("Blocco note"), for the sandbox's questions.
pub fn face_for_class(class: &str) -> Option<(String, String)> {
    programs().into_iter().find(|p| !p.class.is_empty() && p.class.eq_ignore_ascii_case(class)).map(|p| (p.name, p.icon))
}

// ----------------------------------------------------------------------------
// New programs

/// The shortcuts already announced; None until the first look.
static KNOWN: crate::sync::IrqLock<Option<Vec<String>>> = crate::sync::IrqLock::new(None);
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Notifies about programs that are new since the last look (the first look
/// only remembers what is there). Returns their names.
pub fn announce() -> Vec<String> {
    let now = programs();
    let mut known = KNOWN.lock();
    let fresh: Vec<&Program> = match known.as_ref() {
        Some(k) => now.iter().filter(|p| !k.contains(&p.lnk)).collect(),
        None => Vec::new(),
    };
    let names: Vec<String> = fresh.iter().map(|p| p.name.clone()).collect();
    let icons: Vec<String> = fresh.iter().map(|p| p.icon.clone()).collect();
    *known = Some(now.iter().map(|p| p.lnk.clone()).collect());
    drop(known);
    for (n, i) in names.iter().zip(icons.iter()) {
        crate::gui::notify(&format!("«{n}» è installato"), "Lo trovi nel launcher.", i);
    }
    names
}

/// Starts looking, while Windows programs run, for programs an installer
/// makes: a notification for each new one.
pub fn watch() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    announce();
    let started = crate::task::spawn("winwatch", || {
        let mut idle = 0;
        loop {
            crate::task::sleep_ms(4000);
            // Shortcuts come while the installer is still going: let it
            // finish what it is writing.
            let before = programs().len();
            crate::task::sleep_ms(3000);
            if programs().len() == before {
                announce();
            }
            if crate::user::processes().iter().any(|p| p.name.starts_with("wine")) {
                idle = 0;
            } else {
                idle += 1;
                if idle > 15 {
                    break;
                }
            }
        }
        announce();
        WATCHING.store(false, Ordering::SeqCst);
    });
    if started.is_none() {
        WATCHING.store(false, Ordering::SeqCst);
    }
}
