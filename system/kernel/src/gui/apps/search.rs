//! What the launcher finds besides apps, as in Spotlight: the result of a
//! calculation, settings, files and folders in the home by name and, a few
//! files at a time while the launcher stays open, text files by what they
//! say.

use super::Launch;
use crate::fs;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Files found by name at most.
const MAX_FILES: usize = 5;
/// Folders looked into at most, so a big home does not stall the typing.
const MAX_DIRS: usize = 300;
/// Files found by what they say at most.
const MAX_CONTENT: usize = 3;
/// Text files looked into at most for one query, the nearest first.
const MAX_SCANNED: usize = 200;
/// Text files larger than this are not looked into.
const MAX_TEXT: u64 = 256 << 10;
/// Bytes of text kept while the launcher is open, so the next letter typed
/// searches memory instead of the disk.
const MAX_KEPT: usize = 4 << 20;
/// How long one step of the search inside files may go on, in nanoseconds:
/// a quarter of a frame at 60 Hz, so typing never waits for it.
const STEP_NS: u64 = 4_000_000;
/// A query is looked for inside files from this many letters on.
const MIN_CONTENT_QUERY: usize = 3;
/// The extensions of the files looked into.
const TEXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "rs", "c", "h", "cc", "cpp", "hpp", "py", "js", "ts", "css", "json", "csv", "tsv", "html", "htm", "xml", "svg", "conf", "cfg", "ini", "toml",
    "yaml", "yml", "sh", "log", "tex", "srt",
];
/// Characters of the matching line shown before the match, at most.
const LEAD: usize = 24;
/// Characters of the line shown after the match, at most.
const TAIL: usize = 80;

pub struct Found {
    /// The heading of its group: "Calcolo", "Impostazioni", "File",
    /// "Contenuto".
    pub section: &'static str,
    pub name: String,
    pub detail: String,
    /// The bytes of `detail` that matched the query, drawn brighter (the
    /// words found in a file's line).
    pub mark: Option<(usize, usize)>,
    pub icon: &'static str,
    pub launch: Option<Launch>,
    /// A calculation's result, copied when chosen.
    pub copy: Option<String>,
}

/// Settings by the words people look for them with: words, name, where,
/// app.
const SETTINGS: &[(&str, &str, &str, &str)] = &[
    ("sfondo wallpaper immagine desktop", "Sfondo", "Impostazioni", "settings"),
    ("risoluzione schermo display monitor", "Risoluzione dello schermo", "Impostazioni", "settings"),
    ("volume audio suono", "Volume", "Impostazioni", "settings"),
    ("suono avvio chime", "Suono all'avvio", "Impostazioni", "settings"),
    ("tastiera keyboard layout lingua", "Tastiera", "Impostazioni", "settings"),
    ("password sicurezza account", "Password", "Impostazioni", "settings"),
    ("blocco schermo lock automatico", "Blocco automatico", "Impostazioni", "settings"),
    ("nome utente account", "Nome", "Impostazioni", "settings"),
    ("info sistema processore memoria versione cpu ram", "Info sistema", "Processore, memoria, versione", "about"),
    ("monitor attività processi cpu prestazioni", "Monitor attività", "Processi e uso del processore", "monitor"),
    ("aggiornamenti update app negozio store", "Negozio", "App e aggiornamenti", "store"),
    ("cestino trash eliminati", "Cestino", "File eliminati", "trash"),
];

/// A file or folder of the home, for searching by name.
pub struct Item {
    path: String,
    /// The name in lower case.
    key: String,
    dir: bool,
    size: u64,
}

/// What [`find`] and [`step`] need to know about the files: built on the
/// first search and kept while the launcher is open.
#[derive(Default)]
pub struct Index {
    /// The files and folders of the home.
    items: Option<Vec<Item>>,
    /// Text files read already, by their place in `items` (None: not
    /// readable as text).
    texts: BTreeMap<usize, Option<Text>>,
    /// Bytes in `texts`.
    kept: usize,
    /// The search inside files for the query typed, while it goes on.
    scan: Option<Scan>,
}

/// A text file's words: as written, and folded for comparing.
struct Text {
    body: String,
    /// `body` in lower case, every letter the same number of bytes as in
    /// `body` (see [`fold`]): a match here is at the same place there.
    folded: String,
}

/// The search inside files for one query.
struct Scan {
    /// The query, folded.
    query: String,
    /// The next item of the index to look at.
    next: usize,
    /// Text files looked into so far.
    looked: usize,
    /// Content matches found so far.
    found: usize,
    /// Files already found by name, not shown twice.
    named: Vec<String>,
}

pub fn find(query: &str, index: &mut Index) -> Vec<Found> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();
    index.scan = None;
    if q.is_empty() {
        return out;
    }
    if let Some(v) = calculate(&q) {
        out.push(Found {
            section: "Calcolo",
            name: format!("= {v}"),
            detail: String::from("Invio per copiarlo"),
            mark: None,
            icon: "calculator",
            launch: None,
            copy: Some(v),
        });
        return out;
    }
    for &(words, name, detail, app) in SETTINGS {
        if words.split(' ').any(|w| w.starts_with(&q)) || name.to_lowercase().contains(&q) {
            let icon = super::BUILT_IN.iter().find(|(id, _, _)| *id == app).map_or("settings", |(_, _, i)| i);
            out.push(Found {
                section: "Impostazioni",
                name: String::from(name),
                detail: String::from(detail),
                mark: None,
                icon,
                launch: Some(Launch::App(app)),
                copy: None,
            });
        }
    }
    if q.chars().count() >= 2 {
        let items = index.items.get_or_insert_with(home_files);
        let mut named = Vec::new();
        for it in items.iter().filter(|it| it.key.contains(&q)).take(MAX_FILES) {
            let name = String::from(it.path.rsplit('/').next().unwrap_or(&it.path));
            out.push(Found {
                section: "File",
                detail: String::from(folder_of(&it.path)),
                mark: None,
                icon: super::files::icon_for(&name, it.dir),
                name,
                launch: Some(Launch::File(it.path.clone())),
                copy: None,
            });
            named.push(it.path.clone());
        }
        if q.chars().count() >= MIN_CONTENT_QUERY {
            index.scan = Some(Scan { query: fold(query.trim()), next: 0, looked: 0, found: 0, named });
        }
    }
    out
}

/// The folder a file of the home is in, as shown: "Documenti/Lavoro", or
/// "Inizio" for the home itself.
fn folder_of(path: &str) -> &str {
    let shown = path.strip_prefix("sys:/home/").unwrap_or(path);
    shown.rsplit_once('/').map_or("Inizio", |(d, _)| d)
}

/// Is the search inside files still going on? (The launcher then calls
/// [`step`] on every frame.)
pub fn searching(index: &Index) -> bool {
    index.scan.is_some()
}

/// Goes on with the search inside files for a part of a frame
/// ([`STEP_NS`], at least one file) and gives the files found meanwhile,
/// for the section after the files found by name. A file is read once while
/// the launcher stays open (up to [`MAX_KEPT`] bytes of text), so each
/// letter typed after the first searches memory.
pub fn step(index: &mut Index) -> Vec<Found> {
    let mut out = Vec::new();
    let Index { items, texts, kept, scan } = index;
    let (Some(items), Some(s)) = (items.as_ref(), scan.as_mut()) else { return out };
    let start = crate::time::uptime_ns();
    while s.next < items.len() && s.looked < MAX_SCANNED && s.found < MAX_CONTENT {
        let i = s.next;
        s.next += 1;
        let it = &items[i];
        if it.dir || it.size > MAX_TEXT || !is_text(&it.key) || s.named.contains(&it.path) {
            continue;
        }
        s.looked += 1;
        let found = match texts.get(&i) {
            Some(text) => text.as_ref().and_then(|t| content_hit(it, t, &s.query)),
            None => {
                let text = read_text(&it.path);
                let found = text.as_ref().and_then(|t| content_hit(it, t, &s.query));
                // Kept for the next letters while there is room.
                let size = text.as_ref().map_or(0, |t| t.body.len() + t.folded.len());
                if *kept + size <= MAX_KEPT {
                    *kept += size;
                    texts.insert(i, text);
                }
                found
            }
        };
        if let Some(f) = found {
            s.found += 1;
            out.push(f);
        }
        if crate::time::uptime_ns().saturating_sub(start) > STEP_NS {
            return out;
        }
    }
    *scan = None;
    out
}

/// Is a file with this name (in lower case) one that is looked into?
fn is_text(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| TEXT.contains(&ext))
}

/// A file's text, unless it looks binary (a zero byte near the start).
fn read_text(path: &str) -> Option<Text> {
    let data = fs::abs(path).and_then(|p| fs::read_file(&p)).ok()?;
    if data[..data.len().min(1024)].contains(&0) {
        return None;
    }
    let body = String::from(String::from_utf8_lossy(&data));
    let folded = fold(&body);
    Some(Text { body, folded })
}

/// The result for a text file that says `query` (folded), if it does: the
/// file, and the line around the first match with the match brighter.
fn content_hit(it: &Item, t: &Text, query: &str) -> Option<Found> {
    let at = t.folded.find(query)?;
    let (line, mark) = snippet(&t.body, at, query.len());
    let name = String::from(it.path.rsplit('/').next().unwrap_or(&it.path));
    Some(Found {
        section: "Contenuto",
        detail: line,
        mark: Some(mark),
        icon: super::files::icon_for(&name, false),
        name,
        launch: Some(Launch::File(it.path.clone())),
        copy: None,
    })
}

/// The line of `text` around a match at byte `at`, `len` bytes long: at
/// most [`LEAD`] characters before it (with "…" when cut) and [`TAIL`]
/// after, tabs and line ends as spaces. Also where the match is in it.
fn snippet(text: &str, at: usize, len: usize) -> (String, (usize, usize)) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let plain = |s: &str| -> String { s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect() };
    let before = plain(text[start..at].trim_start());
    let n = before.chars().count();
    let mut out = String::new();
    if n > LEAD {
        out.push('…');
        out.extend(before.chars().skip(n - LEAD));
    } else {
        out.push_str(&before);
    }
    let from = out.len();
    out.push_str(&plain(&text[at..at + len]));
    let to = out.len();
    out.extend(plain(&text[at + len..end]).trim_end().chars().take(TAIL));
    (out, (from, to))
}

/// `s` in lower case for comparing, letter by letter: a letter whose lower
/// case takes a different number of bytes stays as it is, so that a place
/// in the result is the same place in `s` (Italian letters, accented ones
/// included, all fold).
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        let mut low = c.to_lowercase();
        match (low.next(), low.next()) {
            (Some(l), None) if l.len_utf8() == c.len_utf8() => out.push(l),
            _ => out.push(c),
        }
    }
    out
}

/// The files and folders under the home, nearer ones first.
fn home_files() -> Vec<Item> {
    let mut items = Vec::new();
    let mut queue: Vec<String> = alloc::vec![String::from("sys:/home")];
    let mut seen = 0;
    while !queue.is_empty() && seen < MAX_DIRS {
        let dir = queue.remove(0);
        seen += 1;
        let Ok(p) = fs::abs(&dir) else { continue };
        let Ok(list) = fs::list_dir(&p) else { continue };
        for (e, st) in list {
            if e.name.starts_with('.') {
                continue;
            }
            let path = format!("{dir}/{}", e.name);
            let dir = st.kind == justfs::Kind::Dir;
            if dir {
                queue.push(path.clone());
            }
            items.push(Item { key: e.name.to_lowercase(), path, dir, size: st.size });
        }
    }
    items
}

/// The value of an arithmetic expression (+ - * / % ^, brackets, decimals
/// with a dot or a comma), if `s` is one and not just a number.
pub fn calculate(s: &str) -> Option<String> {
    let s = s.strip_prefix('=').unwrap_or(s);
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).map(|c| if c == ',' { '.' } else { c }).collect();
    if chars.is_empty() || !chars.iter().any(|c| c.is_ascii_digit()) || !chars.iter().any(|c| "+-*/%^×÷(".contains(*c)) {
        return None;
    }
    let mut p = Parser { s: &chars, i: 0 };
    let v = p.sum()?;
    if p.i != chars.len() || !v.is_finite() {
        return None;
    }
    Some(show(v))
}

/// A number the short way: no ".0", at most nine decimals.
fn show(v: f64) -> String {
    if v == (v as i64) as f64 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    String::from(s).replace('.', ",")
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.product()?;
        while let Some(c @ ('+' | '-')) = self.peek() {
            self.i += 1;
            let r = self.product()?;
            v = if c == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn product(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        while let Some(c @ ('*' | '/' | '%' | 'x' | '×' | '÷')) = self.peek() {
            self.i += 1;
            let r = self.power()?;
            v = match c {
                '%' => v % r,
                '/' | '÷' => v / r,
                _ => v * r,
            };
        }
        Some(v)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.i += 1;
            let e = self.power()?;
            return Some(pow(base, e));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.i += 1;
                Some(-self.unary()?)
            }
            '+' => {
                self.i += 1;
                self.unary()
            }
            '(' => {
                self.i += 1;
                let v = self.sum()?;
                if self.peek() != Some(')') {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Option<f64> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        let text: String = self.s[start..self.i].iter().collect();
        text.parse().ok()
    }
}

/// `b` to the power `e`: whole exponents exactly, others not at all.
fn pow(b: f64, e: f64) -> f64 {
    if e != (e as i64) as f64 || e.abs() > 1024.0 {
        return f64::NAN;
    }
    let mut r = 1.0;
    for _ in 0..(e as i64).abs() {
        r *= b;
    }
    if e < 0.0 {
        1.0 / r
    } else {
        r
    }
}
