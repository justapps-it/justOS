//! The way from a Linux program to the desktop's "open": a program writes
//! `open <path>` or `show <path>` to `/dev/justos/open` (the helper
//! `justos-open` does, for GLib's default application and for xdg-open),
//! and the desktop opens the file as a double click in the Files app would,
//! or shows its folder. The path is the Linux one (or a `file://` address,
//! or an http(s) one); here it becomes a justOS path.

use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::IrqLock;

/// What the desktop has not looked at yet: (show the folder, justOS path).
static QUEUE: IrqLock<Vec<(bool, String)>> = IrqLock::new(Vec::new());

/// The calling program wrote `data`: one request per line. Bytes taken.
pub fn write(data: &[u8]) -> i64 {
    let Ok(text) = core::str::from_utf8(data) else { return -super::err::EINVAL };
    for line in text.lines() {
        let line = line.trim();
        let (show, target) = match line.split_once(' ') {
            Some(("open", rest)) => (false, rest.trim()),
            Some(("show", rest)) => (true, rest.trim()),
            _ => (false, line),
        };
        if let Some(path) = translate(target) {
            let mut q = QUEUE.lock();
            if q.len() < 16 {
                q.push((show, path));
            }
        }
    }
    data.len() as i64
}

/// Requests for the desktop to carry out.
pub fn take() -> Vec<(bool, String)> {
    let mut q = QUEUE.lock();
    if q.is_empty() {
        return Vec::new();
    }
    core::mem::take(&mut *q)
}

/// A Linux path, `file://` address or web address as the justOS path (or
/// the web address) the desktop opens.
fn translate(target: &str) -> Option<String> {
    if target.is_empty() {
        return None;
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        return Some(String::from(target));
    }
    let text = match target.strip_prefix("file://") {
        Some(rest) => unescape(rest.strip_prefix("localhost").unwrap_or(rest)),
        None => String::from(target),
    };
    let rooted = super::rooted(&text);
    let path = super::linux_path(&rooted).ok()?;
    Some(alloc::format!("{path}"))
}

/// `%20` and the like in a `file://` address.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
