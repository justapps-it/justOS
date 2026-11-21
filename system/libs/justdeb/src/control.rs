//! Debian's control files (deb822): paragraphs of `Field: value` lines,
//! continuation lines starting with a space, paragraphs apart by an empty
//! line. `Packages` indexes, dpkg's `status` and a package's own `control`
//! are all written so. The relation fields in them (`Depends`,
//! `Pre-Depends`, `Provides`, `Conflicts`, `Breaks`) are parsed into
//! [`Group`]s of [`Dep`]s.

use crate::version::Op;
use alloc::string::String;
use alloc::vec::Vec;

/// The paragraphs of a control file (empty ones left out).
pub fn paragraphs(text: &str) -> impl Iterator<Item = &str> {
    // A line of only spaces separates paragraphs too; "\r\n" is tolerated.
    let mut rest = text;
    core::iter::from_fn(move || loop {
        if rest.is_empty() {
            return None;
        }
        // The first blank line ends the paragraph.
        let (mut end, mut next) = (rest.len(), rest.len());
        let mut at = 0;
        while at < rest.len() {
            let line_end = rest[at..].find('\n').map_or(rest.len(), |i| at + i);
            if rest[at..line_end].trim().is_empty() {
                (end, next) = (at, (line_end + 1).min(rest.len()));
                break;
            }
            at = line_end + 1;
        }
        let par = &rest[..end];
        rest = &rest[next..];
        if !par.trim().is_empty() {
            return Some(par);
        }
    })
}

/// A field of a paragraph (names compare without case), with its
/// continuation lines joined by "\n" and each line trimmed.
pub fn field(par: &str, name: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for line in par.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(v) = out.as_mut() {
                v.push('\n');
                v.push_str(line.trim());
            }
            continue;
        }
        if out.is_some() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                out = Some(String::from(v.trim()));
            }
        }
    }
    out
}

/// All fields of a paragraph, in order (continuation lines as in [`field`]).
pub fn fields(par: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in par.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = out.last_mut() {
                v.push('\n');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.push((String::from(k.trim()), String::from(v.trim())));
        }
    }
    out
}

/// One alternative of a relation: `name[:arch] [(op version)]`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Dep {
    pub name: String,
    /// The architecture qualifier (`any`, `native`, `amd64`…), if any.
    pub arch: Option<String>,
    pub rule: Option<(Op, String)>,
}

/// A relation's alternatives (`a | b`): one of them is enough.
pub type Group = Vec<Dep>;

impl Dep {
    /// Whether version `v` meets the rule (no rule: any version).
    pub fn accepts(&self, v: &str) -> bool {
        self.rule.as_ref().is_none_or(|(op, want)| op.holds(v, want))
    }
}

impl core::fmt::Display for Dep {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.name)?;
        if let Some(a) = &self.arch {
            write!(f, ":{a}")?;
        }
        if let Some((op, v)) = &self.rule {
            write!(f, " ({} {v})", op.as_str())?;
        }
        Ok(())
    }
}

/// Writes a group as the index has it: "a (>= 1) | b".
pub fn group_text(g: &Group) -> String {
    let parts: Vec<String> = g.iter().map(|d| alloc::format!("{d}")).collect();
    parts.join(" | ")
}

/// Whether an architecture list (`[amd64 !i386]`) takes in amd64.
fn arch_list_has_amd64(list: &str) -> bool {
    let words: Vec<&str> = list.split_whitespace().collect();
    if words.is_empty() {
        return true;
    }
    let matches = |w: &str| matches!(w, "amd64" | "any" | "linux-any" | "any-amd64" | "linux-amd64");
    if words.iter().all(|w| w.starts_with('!')) {
        !words.iter().any(|w| matches(&w[1..]))
    } else {
        words.iter().any(|w| matches(w))
    }
}

/// One alternative's text, or None when its architecture list leaves
/// amd64 out (a source package's relation; binary ones have none).
fn parse_alt(text: &str) -> Option<Dep> {
    let mut s = String::new();
    let mut arch_list = String::new();
    let (mut in_list, mut in_profile) = (false, false);
    for c in text.chars() {
        match c {
            '[' => in_list = true,
            ']' => in_list = false,
            // Build profiles ("<!nocheck>") come after the rule's ")",
            // so a "<" before it is part of the rule.
            '<' if !in_list && (!s.contains('(') || s.contains(')')) => in_profile = true,
            '>' if in_profile => in_profile = false,
            _ if in_profile => {}
            c if in_list => arch_list.push(c),
            c => s.push(c),
        }
    }
    if !arch_list_has_amd64(&arch_list) {
        return None;
    }
    let (head, rule) = match s.split_once('(') {
        Some((n, r)) => {
            let r = r.split(')').next().unwrap_or("").trim();
            let cut = r.find(|c: char| !matches!(c, '<' | '>' | '=')).unwrap_or(r.len());
            let (op, version) = (r[..cut].trim(), r[cut..].trim());
            // "(1.2)" with no relation is an exact version.
            let op = if op.is_empty() { Some(Op::Eq) } else { Op::parse(op) };
            (n, op.filter(|_| !version.is_empty()).map(|op| (op, String::from(version))))
        }
        None => (s.as_str(), None),
    };
    let head = head.trim();
    let (name, arch) = match head.split_once(':') {
        Some((n, a)) => (n.trim(), Some(String::from(a.trim()))),
        None => (head, None),
    };
    (!name.is_empty()).then(|| Dep { name: String::from(name), arch, rule })
}

/// A relation field: groups apart by ",", alternatives by "|".
pub fn relations(text: &str) -> Vec<Group> {
    let mut out = Vec::new();
    for group in text.split(',') {
        let alts: Group = group.split('|').filter_map(parse_alt).collect();
        if !alts.is_empty() {
            out.push(alts);
        }
    }
    out
}
