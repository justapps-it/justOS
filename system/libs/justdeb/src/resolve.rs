//! Which packages a new one needs, as `apt install ./file.deb` decides it.
//!
//! The resolver is greedy, like apt's for a single request: each
//! dependency that the system (what is installed, plus what is already
//! chosen) does not meet is met by the first alternative that can be, with
//! the newest version of a package that fits, or else with a package that
//! provides it. A package whose newer version is needed replaces the
//! installed one, once the packages that depend on the old one are still
//! content with the new one. Conflicts and Breaks, either way between a new
//! package and the system, stop the plan: nothing is removed to make room.
//!
//! The order is dpkg's to work out within one `dpkg -i` call (it unpacks
//! all, then configures each after what it depends on), except for
//! Pre-Depends: a package that pre-depends on another needs that one
//! configured before it is even unpacked, so the plan comes in batches, one
//! `dpkg -i` each.

use crate::control::{group_text, Dep, Group};
use crate::index::{native_arch, Index, Pkg};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The `source` of a package that is installed.
pub const INSTALLED: usize = usize::MAX;
/// The `source` of a package file given by the user (not in an archive).
pub const LOCAL: usize = usize::MAX - 1;

/// Why there is no plan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Problem {
    /// Nothing meets a dependency (`what`, as the package writes it).
    Missing { what: String, needed_by: String },
    /// A new package and another (new or installed) do not go together.
    Conflict { new: String, other: String, breaks: bool },
    /// A newer version of `upgraded` is needed, and `installed` depends on
    /// the one there is (`what`).
    Upgrade { upgraded: String, installed: String, what: String },
    /// A package for another processor.
    Arch { name: String, arch: String },
}

impl core::fmt::Display for Problem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Problem::Missing { what, needed_by } => write!(f, "{needed_by} depends on {what}, which is not available"),
            Problem::Conflict { new, other, breaks } => write!(f, "{new} {} {other}", if *breaks { "breaks" } else { "conflicts with" }),
            Problem::Upgrade { upgraded, installed, what } => write!(f, "{installed} needs {what}, but {upgraded} must be upgraded"),
            Problem::Arch { name, arch } => write!(f, "{name} is for {arch}"),
        }
    }
}

/// What to install, in order, in batches of one `dpkg -i` each.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The packages, dependencies before what depends on them (the
    /// requested ones are in it too).
    pub packages: Vec<Pkg>,
    /// Indexes into `packages`: one `dpkg -i` per batch, in order.
    pub batches: Vec<Vec<usize>>,
    /// Installed packages that get a newer version: (name, old version).
    pub upgrades: Vec<(String, String)>,
}

impl Plan {
    /// The packages that come from the archive (to download).
    pub fn downloads(&self) -> impl Iterator<Item = &Pkg> {
        self.packages.iter().filter(|p| p.source != LOCAL && p.source != INSTALLED)
    }

    /// Bytes to download.
    pub fn download_size(&self) -> u64 {
        self.downloads().map(|p| p.size).sum()
    }

    /// KiB they take once installed.
    pub fn installed_kib(&self) -> u64 {
        self.packages.iter().map(|p| p.installed_kib).sum()
    }
}

struct Solver<'a> {
    installed: &'a [Pkg],
    index: &'a Index,
    chosen: Vec<Pkg>,
    by_name: BTreeMap<String, usize>,
}

/// Whether `p`'s Conflicts or Breaks hit `q` (a package never conflicts
/// with itself, though it may conflict with what it provides).
fn hits(p: &Pkg, q: &Pkg) -> Option<bool> {
    if p.name == q.name {
        return None;
    }
    if p.conflicts.iter().any(|d| q.meets(d)) {
        return Some(false);
    }
    p.breaks.iter().any(|d| q.meets(d)).then_some(true)
}

impl Solver<'_> {
    /// The installed packages a choice has not replaced, then the chosen.
    fn world(&self) -> impl Iterator<Item = &Pkg> {
        self.installed.iter().filter(|p| !self.by_name.contains_key(&p.name)).chain(self.chosen.iter())
    }

    fn met(&self, g: &Group) -> bool {
        self.world().any(|p| g.iter().any(|d| p.meets(d)))
    }

    /// What in the world (but a package of the same name) does not go
    /// with `p`.
    fn clash(&self, p: &Pkg) -> Option<Problem> {
        for q in self.world().filter(|q| q.name != p.name) {
            if let Some(breaks) = hits(p, q) {
                return Some(Problem::Conflict { new: p.name.clone(), other: q.name.clone(), breaks });
            }
            if let Some(breaks) = hits(q, p) {
                return Some(Problem::Conflict { new: p.name.clone(), other: q.name.clone(), breaks });
            }
        }
        None
    }

    /// Packages that could meet one alternative: a package of that name
    /// (newest first; only the chosen version once one is chosen), then
    /// those that provide it.
    fn options(&self, d: &Dep) -> Vec<Pkg> {
        let mut out: Vec<Pkg> = match self.by_name.get(&d.name) {
            Some(&i) => alloc::vec![self.chosen[i].clone()],
            None => self.index.versions(&d.name),
        };
        let mut providers = self.index.providers(&d.name);
        providers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| crate::version::compare(&b.version, &a.version)));
        for p in providers {
            match self.by_name.get(&p.name) {
                Some(&i) if self.chosen[i].version != p.version => {}
                _ => out.push(p),
            }
        }
        out.retain(|p| p.meets(d) && native_arch(&p.arch));
        out
    }

    fn choose(&mut self, p: Pkg) -> usize {
        let at = self.chosen.len();
        self.by_name.insert(p.name.clone(), at);
        self.chosen.push(p);
        at
    }

    /// Meets one dependency group of `who`, choosing a package if needed.
    fn meet(&mut self, g: &Group, who: &str) -> Result<Option<usize>, Problem> {
        if self.met(g) {
            return Ok(None);
        }
        let mut clash = None;
        for d in g {
            for p in self.options(d) {
                if self.by_name.get(&p.name).is_some_and(|&i| self.chosen[i].version == p.version) {
                    // Chosen already (it meets the group: met() would have
                    // said so, unless the arch rule differs).
                    continue;
                }
                match self.clash(&p) {
                    Some(c) => clash = clash.or(Some(c)),
                    None => return Ok(Some(self.choose(p))),
                }
            }
        }
        Err(clash.unwrap_or_else(|| Problem::Missing { what: group_text(g), needed_by: String::from(who) }))
    }

    /// Chooses the dependencies of everything chosen, breadth first.
    fn close(&mut self) -> Result<(), Problem> {
        let mut next = 0;
        while next < self.chosen.len() {
            let p = self.chosen[next].clone();
            for (g, _) in p.all_depends() {
                self.meet(g, &p.name)?;
            }
            next += 1;
        }
        Ok(())
    }

    /// An installed package whose dependency an upgrade no longer meets.
    fn broken_by_upgrades(&self) -> Option<Problem> {
        let upgraded: Vec<&Pkg> = self.chosen.iter().filter(|p| self.installed.iter().any(|q| q.name == p.name)).collect();
        if upgraded.is_empty() {
            return None;
        }
        for q in self.installed.iter().filter(|q| !self.by_name.contains_key(&q.name)) {
            for (g, _) in q.all_depends() {
                // Only groups the old versions met and the new ones are
                // asked about: what was broken before is not this plan's.
                let before = self.installed.iter().any(|p| g.iter().any(|d| p.meets(d)));
                if before && !self.met(g) {
                    let up = upgraded.iter().find(|p| g.iter().any(|d| d.name == p.name || p.provides.iter().any(|v| v.name == d.name)));
                    return Some(Problem::Upgrade { upgraded: up.map_or_else(String::new, |p| p.name.clone()), installed: q.name.clone(), what: group_text(g) });
                }
            }
        }
        None
    }

    /// The chosen packages, each after the chosen ones it depends on
    /// (cycles are cut where they close: dpkg sorts those out itself).
    fn order(&self) -> Vec<usize> {
        let mut state = alloc::vec![0u8; self.chosen.len()];
        let mut out = Vec::new();
        for i in 0..self.chosen.len() {
            self.visit(i, &mut state, &mut out);
        }
        out
    }

    fn visit(&self, i: usize, state: &mut Vec<u8>, out: &mut Vec<usize>) {
        if state[i] != 0 {
            return;
        }
        state[i] = 1;
        for j in self.edges(i).into_iter().map(|(j, _)| j) {
            self.visit(j, state, out);
        }
        state[i] = 2;
        out.push(i);
    }

    /// The chosen packages that meet `i`'s dependencies (when nothing
    /// installed does): (index, whether through Pre-Depends).
    fn edges(&self, i: usize) -> Vec<(usize, bool)> {
        let p = &self.chosen[i];
        let mut out = Vec::new();
        for (g, pre) in p.all_depends() {
            let by_installed = self.installed.iter().filter(|q| !self.by_name.contains_key(&q.name)).any(|q| g.iter().any(|d| q.meets(d)));
            if by_installed {
                continue;
            }
            if let Some(j) = self.chosen.iter().position(|q| q.name != p.name && g.iter().any(|d| q.meets(d))) {
                out.push((j, pre));
            }
        }
        out
    }
}

/// The plan to install `wanted` (package files or packages of the
/// archive) on a system with `installed`, taking what is missing from
/// `index`.
pub fn plan(installed: &[Pkg], index: &Index, wanted: Vec<Pkg>) -> Result<Plan, Problem> {
    let mut s = Solver { installed, index, chosen: Vec::new(), by_name: BTreeMap::new() };
    for p in wanted {
        if !native_arch(&p.arch) {
            return Err(Problem::Arch { name: p.name, arch: p.arch });
        }
        if let Some(c) = s.clash(&p) {
            return Err(c);
        }
        s.choose(p);
    }
    s.close()?;
    // A later choice may clash with an earlier one: checked once more on
    // the whole.
    for p in &s.chosen {
        if let Some(c) = s.clash(p) {
            return Err(c);
        }
    }
    if let Some(problem) = s.broken_by_upgrades() {
        return Err(problem);
    }
    let order = s.order();
    // Batches: a package that pre-depends on one of the current batch
    // starts the next.
    let mut position = alloc::vec![0usize; s.chosen.len()];
    for (k, &i) in order.iter().enumerate() {
        position[i] = k;
    }
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        if s.edges(i).iter().any(|&(j, pre)| pre && current.contains(&position[j])) {
            batches.push(core::mem::take(&mut current));
        }
        current.push(k);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    let upgrades = s.chosen.iter().filter_map(|p| installed.iter().find(|q| q.name == p.name).map(|q| (q.name.clone(), q.version.clone()))).collect();
    let packages = order.iter().map(|&i| s.chosen[i].clone()).collect();
    Ok(Plan { packages, batches, upgrades })
}

/// The package of the archive that `name` asks for: the package of that
/// name, or the one package that provides it.
pub fn by_name(index: &Index, name: &str) -> Result<Pkg, String> {
    if let Some(p) = index.candidate(name) {
        return Ok(p);
    }
    let mut providers = index.providers(name);
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    providers.dedup_by(|a, b| a.name == b.name);
    match providers.len() {
        0 => Err(format!("{name}: not in the archive")),
        1 => Ok(providers.remove(0)),
        _ => Err(format!("{name} is provided by several packages: {}", providers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}
