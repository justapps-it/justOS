//! Debian's order of versions, as dpkg has it (deb-version(7)).
//!
//! A version is `[epoch:]upstream[-revision]`. The epoch is a number and
//! wins over everything else; the upstream part and the revision are each
//! compared as alternating runs of non-digits and digits. In the non-digit
//! runs letters sort before other symbols and `~` sorts before everything,
//! even the end of the string, so that `1.0~rc1` comes before `1.0`. The
//! digit runs compare as numbers.

use core::cmp::Ordering;

/// The three parts of a version: epoch, upstream version, revision (empty
/// when there is none, which compares like "0").
pub fn split(v: &str) -> (u64, &str, &str) {
    let v = v.trim();
    let (epoch, rest) = match v.split_once(':') {
        Some((e, r)) if !e.is_empty() && e.bytes().all(|c| c.is_ascii_digit()) => (e.parse().unwrap_or(u64::MAX), r),
        _ => (0, v),
    };
    match rest.rsplit_once('-') {
        Some((u, r)) => (epoch, u, r),
        None => (epoch, rest, ""),
    }
}

/// Where a character of a non-digit run sorts: `~` first, then the end of
/// the run (and digits, which end it), letters, then everything else.
fn weight(c: Option<u8>) -> i32 {
    match c {
        Some(b'~') => -1,
        None => 0,
        Some(c) if c.is_ascii_digit() => 0,
        Some(c) if c.is_ascii_alphabetic() => c as i32,
        Some(c) => c as i32 + 256,
    }
}

/// dpkg's `verrevcmp`: one part (upstream or revision) against another.
fn part(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        // The non-digit run, character by character.
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let (x, y) = (weight(a.get(i).copied()), weight(b.get(j).copied()));
            if x != y {
                return x.cmp(&y);
            }
            i += 1;
            j += 1;
        }
        // The digit run, as a number: leading zeros do not count, a longer
        // number is a bigger one, and between two of the same length the
        // first digit that differs decides.
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        let (si, sj) = (i, j);
        while i < a.len() && a[i].is_ascii_digit() {
            i += 1;
        }
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let o = (i - si).cmp(&(j - sj)).then_with(|| a[si..i].cmp(&b[sj..j]));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Compares two versions.
pub fn compare(a: &str, b: &str) -> Ordering {
    let ((ea, ua, ra), (eb, ub, rb)) = (split(a), split(b));
    ea.cmp(&eb).then_with(|| part(ua, ub)).then_with(|| part(ra, rb))
}

/// A version relation of a dependency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    /// `<<`
    Less,
    /// `<=` (and the old `<`)
    LessEq,
    /// `=`
    Eq,
    /// `>=` (and the old `>`)
    GreaterEq,
    /// `>>`
    Greater,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "<<" => Op::Less,
            "<=" | "<" => Op::LessEq,
            "=" => Op::Eq,
            ">=" | ">" => Op::GreaterEq,
            ">>" => Op::Greater,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Op::Less => "<<",
            Op::LessEq => "<=",
            Op::Eq => "=",
            Op::GreaterEq => ">=",
            Op::Greater => ">>",
        }
    }

    /// Whether `have` stands in this relation to `want`.
    pub fn holds(self, have: &str, want: &str) -> bool {
        let o = compare(have, want);
        match self {
            Op::Less => o == Ordering::Less,
            Op::LessEq => o != Ordering::Greater,
            Op::Eq => o == Ordering::Equal,
            Op::GreaterEq => o != Ordering::Less,
            Op::Greater => o == Ordering::Greater,
        }
    }
}
