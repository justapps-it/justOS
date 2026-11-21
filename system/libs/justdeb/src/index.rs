//! Packages, as the resolver sees them: from an archive's `Packages` index,
//! from dpkg's `status` (what is installed) and from a `.deb`'s control.
//!
//! The main archive's index is 56 MB of text; [`compact`] keeps the fields
//! that matter here (about 60 % of it), which is what justOS caches on
//! disk, and [`Index`] finds packages by name and by what they provide.

use crate::control::{self, Dep, Group};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The fields [`compact`] keeps.
const KEEP: &[&str] = &[
    "Package",
    "Version",
    "Architecture",
    "Multi-Arch",
    "Essential",
    "Pre-Depends",
    "Depends",
    "Provides",
    "Conflicts",
    "Breaks",
    "Filename",
    "Size",
    "SHA256",
    "Installed-Size",
    "Description",
];

/// A package (one version of it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pkg {
    pub name: String,
    pub version: String,
    pub arch: String,
    /// `same`, `foreign`, `allowed` or empty.
    pub multi_arch: String,
    pub essential: bool,
    pub pre_depends: Vec<Group>,
    pub depends: Vec<Group>,
    pub provides: Vec<Dep>,
    pub conflicts: Vec<Dep>,
    pub breaks: Vec<Dep>,
    /// Where it is in the archive (`pool/main/…`), empty for one that is
    /// not from an archive.
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    /// KiB, the package's own figure.
    pub installed_kib: u64,
    /// The first line of its description.
    pub summary: String,
    /// Which archive (of the list it was read with) it comes from.
    pub source: usize,
}

/// The architectures justOS installs: its own, and packages for all.
pub fn native_arch(arch: &str) -> bool {
    arch == "amd64" || arch == "all"
}

fn flat(text: &str) -> Vec<Dep> {
    control::relations(text).into_iter().flatten().collect()
}

impl Pkg {
    /// A package from a control paragraph.
    pub fn from_paragraph(par: &str, source: usize) -> Pkg {
        let f = control::fields(par);
        let get = |k: &str| f.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str()).unwrap_or("");
        Pkg {
            name: String::from(get("Package")),
            version: String::from(get("Version")),
            arch: String::from(get("Architecture")),
            multi_arch: String::from(get("Multi-Arch")),
            essential: get("Essential").eq_ignore_ascii_case("yes"),
            pre_depends: control::relations(get("Pre-Depends")),
            depends: control::relations(get("Depends")),
            provides: flat(get("Provides")),
            conflicts: flat(get("Conflicts")),
            breaks: flat(get("Breaks")),
            filename: String::from(get("Filename")),
            size: get("Size").parse().unwrap_or(0),
            sha256: get("SHA256").to_ascii_lowercase(),
            installed_kib: get("Installed-Size").parse().unwrap_or(0),
            summary: String::from(get("Description").lines().next().unwrap_or("").trim()),
            source,
        }
    }

    /// All its dependencies: Pre-Depends first (bool: whether a group is
    /// one of them).
    pub fn all_depends(&self) -> impl Iterator<Item = (&Group, bool)> {
        self.pre_depends.iter().map(|g| (g, true)).chain(self.depends.iter().map(|g| (g, false)))
    }

    /// Whether it meets one alternative of a dependency: by its own name
    /// and version, or by what it provides. A versioned dependency is met
    /// by a provide only if that one has a version (`Provides: foo (= 1)`),
    /// as dpkg has it. `name:any` asks for a package that allows it
    /// (`Multi-Arch: allowed`); justOS has only its own architecture, so
    /// `:native` and `:amd64` are the bare name.
    pub fn meets(&self, d: &Dep) -> bool {
        let arch_ok = match d.arch.as_deref() {
            None | Some("native") | Some("amd64") => true,
            Some("any") => self.multi_arch == "allowed",
            Some(_) => false,
        };
        if self.name == d.name && arch_ok && d.accepts(&self.version) {
            return true;
        }
        // A provide meets ":any" only from a Multi-Arch: foreign package
        // (dpkg's rule); justOS has one architecture, so that is enough.
        let provide_arch_ok = d.arch.as_deref() != Some("any") || self.multi_arch == "foreign" || self.multi_arch == "allowed";
        provide_arch_ok
            && self.provides.iter().any(|p| {
                p.name == d.name
                    && match (&d.rule, &p.rule) {
                        (None, _) => true,
                        (Some(_), Some((crate::version::Op::Eq, v))) => d.accepts(v),
                        _ => false,
                    }
            })
    }
}

/// Writes the fields of a `Packages` index that [`Pkg`] reads, for the
/// packages justOS can install (amd64 and all), the description cut to its
/// first line.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    for par in control::paragraphs(text) {
        let arch = control::field(par, "Architecture").unwrap_or_default();
        if !native_arch(&arch) {
            continue;
        }
        let mut keep = false;
        for line in par.lines() {
            if line.starts_with([' ', '\t']) {
                // A continuation: a long Depends goes on, a description's
                // other lines do not.
                if keep {
                    out.push_str(line);
                    out.push('\n');
                }
                continue;
            }
            let name = line.split(':').next().unwrap_or("");
            keep = KEEP.iter().any(|k| k.eq_ignore_ascii_case(name)) && !name.eq_ignore_ascii_case("Description");
            if keep || name.eq_ignore_ascii_case("Description") {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

/// The packages dpkg has as installed (`Status: install ok installed`) in
/// its `status` file.
pub fn installed(status: &str) -> Vec<Pkg> {
    control::paragraphs(status)
        .filter(|par| control::field(par, "Status").is_some_and(|s| s.split_whitespace().last() == Some("installed")))
        .map(|par| Pkg::from_paragraph(par, usize::MAX))
        .collect()
}

/// Where a package's paragraph is: which text, from where to where, and
/// which archive it comes from.
#[derive(Clone, Copy)]
struct Entry {
    text: u32,
    start: u32,
    end: u32,
    source: u32,
}

/// The packages of one or more archives, by name and by the virtual
/// packages they provide. The texts are kept as they are and a package is
/// parsed when it is asked for: the main archive has 69 000 of them, and
/// the resolver looks at a few hundred.
#[derive(Default)]
pub struct Index {
    texts: Vec<String>,
    entries: Vec<Entry>,
    names: BTreeMap<String, Vec<u32>>,
    provided: BTreeMap<String, Vec<u32>>,
}

impl Index {
    pub fn new() -> Index {
        Index::default()
    }

    /// Adds the packages of an index text (full or [`compact`]) of archive
    /// number `source`.
    pub fn add(&mut self, text: String, source: usize) {
        let t = self.texts.len() as u32;
        let base = text.as_ptr() as usize;
        for par in control::paragraphs(&text) {
            let (Some(name), arch) = (control::field(par, "Package"), control::field(par, "Architecture").unwrap_or_default()) else { continue };
            if name.is_empty() || !native_arch(&arch) {
                continue;
            }
            let start = par.as_ptr() as usize - base;
            let at = self.entries.len() as u32;
            self.entries.push(Entry { text: t, start: start as u32, end: (start + par.len()) as u32, source: source as u32 });
            if let Some(p) = control::field(par, "Provides") {
                for v in flat(&p) {
                    self.provided.entry(v.name).or_default().push(at);
                }
            }
            self.names.entry(name).or_default().push(at);
        }
        self.texts.push(text);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn pkg(&self, i: u32) -> Pkg {
        let e = self.entries[i as usize];
        Pkg::from_paragraph(&self.texts[e.text as usize][e.start as usize..e.end as usize], e.source as usize)
    }

    /// The versions of a package, newest first.
    pub fn versions(&self, name: &str) -> Vec<Pkg> {
        let mut v: Vec<Pkg> = self.names.get(name).into_iter().flatten().map(|&i| self.pkg(i)).collect();
        v.sort_by(|a, b| crate::version::compare(&b.version, &a.version));
        v
    }

    /// The newest version of a package: what installing it means.
    pub fn candidate(&self, name: &str) -> Option<Pkg> {
        self.versions(name).into_iter().next()
    }

    /// The packages that provide a virtual package.
    pub fn providers(&self, name: &str) -> Vec<Pkg> {
        self.provided.get(name).into_iter().flatten().map(|&i| self.pkg(i)).collect()
    }

    /// Whether a name is a package or a virtual package of the archives.
    pub fn knows(&self, name: &str) -> bool {
        self.names.contains_key(name) || self.provided.contains_key(name)
    }
}
