//! An archive's `InRelease` (once its signature is checked, see
//! [`crate::pgp`]) and the `sources.list` that names the archives.

use crate::control;
use alloc::string::String;
use alloc::vec::Vec;

/// A file an `InRelease` lists: its path under `dists/<suite>/`, size and
/// SHA-256 (hex).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Listed {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default)]
pub struct Release {
    pub suite: String,
    pub codename: String,
    /// Seconds since 1970 (UTC), 0 when absent or unreadable.
    pub date: u64,
    pub valid_until: u64,
    /// The indexes can be fetched by their hash (`by-hash/SHA256/<sum>`),
    /// which never changes under a download that runs while the archive
    /// is updated.
    pub by_hash: bool,
    pub files: Vec<Listed>,
}

impl Release {
    /// Reads the (signed) text of an `InRelease` file.
    pub fn parse(text: &str) -> Release {
        let par = control::paragraphs(text).next().unwrap_or("");
        let get = |k: &str| control::field(par, k).unwrap_or_default();
        let files = get("SHA256")
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                let (sum, size, path) = (w.next()?, w.next()?, w.next()?);
                let size = size.parse().ok()?;
                (sum.len() == 64).then(|| Listed { path: String::from(path), size, sha256: sum.to_ascii_lowercase() })
            })
            .collect();
        Release {
            suite: get("Suite"),
            codename: get("Codename"),
            date: parse_date(&get("Date")).unwrap_or(0),
            valid_until: parse_date(&get("Valid-Until")).unwrap_or(0),
            by_hash: get("Acquire-By-Hash").eq_ignore_ascii_case("yes"),
            files,
        }
    }

    pub fn file(&self, path: &str) -> Option<&Listed> {
        self.files.iter().find(|f| f.path == path)
    }

    /// The index of a component's amd64 packages to fetch: the gzip one if
    /// the archive has it (the main archive's), else the plain one (the
    /// small indexes of trixie-updates and trixie-security come only plain
    /// and xz). The xz one is never taken: justOS has no xz decoder.
    pub fn packages_index(&self, component: &str) -> Option<&Listed> {
        let base = alloc::format!("{component}/binary-amd64/Packages");
        self.file(&alloc::format!("{base}.gz")).or_else(|| self.file(&base))
    }
}

/// Days from 1970-01-01 to a date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An RFC 2822 date as the archive writes it ("Sat, 21 Nov 2026 02:10:46
/// UTC"), in seconds since 1970.
pub fn parse_date(s: &str) -> Option<u64> {
    let s = s.split_once(',').map_or(s, |(_, r)| r);
    let w: Vec<&str> = s.split_whitespace().collect();
    let (day, month, year, time) = (w.first()?.parse::<u32>().ok()?, *w.get(1)?, w.get(2)?.parse::<i64>().ok()?, *w.get(3)?);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let m = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(month))? as u32 + 1;
    let mut t = time.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
    // "UTC", "GMT", "+0000" or an offset.
    let offset = match w.get(4) {
        Some(z) if (z.starts_with('+') || z.starts_with('-')) && z.len() == 5 => {
            let v: i64 = z[1..].parse().ok()?;
            let secs = (v / 100) * 3600 + (v % 100) * 60;
            if z.starts_with('-') {
                -secs
            } else {
                secs
            }
        }
        _ => 0,
    };
    let secs = days_from_civil(year, m, day) * 86_400 + hh * 3600 + mm * 60 + ss - offset;
    u64::try_from(secs).ok()
}

/// An archive of `sources.list`: `deb [options] <uri> <suite> <component>…`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Source {
    pub uri: String,
    pub suite: String,
    pub components: Vec<String>,
}

impl Source {
    /// Where the suite's files are: `<uri>/dists/<suite>`.
    pub fn dists(&self) -> String {
        alloc::format!("{}/dists/{}", self.uri.trim_end_matches('/'), self.suite)
    }

    /// Where a file of the pool is (`Filename:` of the index).
    pub fn pool(&self, filename: &str) -> String {
        alloc::format!("{}/{}", self.uri.trim_end_matches('/'), filename.trim_start_matches('/'))
    }
}

/// The binary archives of a `sources.list` (one-line style); `deb-src`
/// lines, comments and options in brackets are skipped.
pub fn sources_list(text: &str) -> Vec<Source> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("deb ").or_else(|| line.strip_prefix("deb\t")) else { continue };
        let mut rest = rest.trim_start();
        if rest.starts_with('[') {
            rest = rest.split_once(']').map_or("", |(_, r)| r.trim_start());
        }
        let mut w = rest.split_whitespace();
        let (Some(uri), Some(suite)) = (w.next(), w.next()) else { continue };
        let components: Vec<String> = w.map(String::from).collect();
        if !components.is_empty() {
            out.push(Source { uri: String::from(uri), suite: String::from(suite), components });
        }
    }
    out
}

/// The archives of a deb822 `.sources` file (`Types: deb`, `URIs:`,
/// `Suites:`, `Components:`), as Debian's own installer writes them now.
pub fn sources_deb822(text: &str) -> Vec<Source> {
    let mut out = Vec::new();
    for par in control::paragraphs(text) {
        let get = |k: &str| control::field(par, k).unwrap_or_default();
        if !get("Types").split_whitespace().any(|t| t == "deb") || get("Enabled").eq_ignore_ascii_case("no") {
            continue;
        }
        let components: Vec<String> = get("Components").split_whitespace().map(String::from).collect();
        for uri in get("URIs").split_whitespace() {
            for suite in get("Suites").split_whitespace() {
                out.push(Source { uri: String::from(uri), suite: String::from(suite), components: components.clone() });
            }
        }
    }
    out
}
