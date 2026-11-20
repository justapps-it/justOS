//! Installing Debian packages (.deb), with a window of justOS's own.
//!
//! The work is Debian's: the image carries `dpkg` (and `tar`, `coreutils`…),
//! and a database of what the image has (`/var/lib/dpkg`, made when the
//! image is built, see tools/linux/debroot.py). A double click on a `.deb`
//! opens this window; it asks `dpkg-deb -f` for the package's control data
//! (xz and zstd are unpacked by dpkg's own libraries, justOS needs no
//! decoder), checks what can be checked before anything is written (the
//! architecture, the packages it depends on, the space), and waits for the
//! click on "Installa". Then `dpkg -i` runs through the Linux layer, with
//! the maintainer scripts, while the bar follows what it writes to the
//! system volume; the result is `dpkg`'s: dependencies are checked, files of
//! the base system are not overwritten, a later `dpkg -r` (or `deb remove`
//! in the shell) takes the package away again.
//!
//! Packages whose dependencies are not in the system are not installed:
//! nothing is downloaded (yet), the window names what is missing.

use crate::dev::input::{Key, KeyEvent};
use crate::fs;
use crate::gui::gfx::{Canvas, Rect};
use crate::gui::res;
use crate::gui::text::{self, Font};
use crate::gui::ui;
use crate::gui::wm::{Action, App};
use crate::sync::IrqLock;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering};

/// Where the helper script, its log and a copy of a package from another
/// volume are (as justOS and as the Linux programs see it).
const WORK: &str = "sys:/home/.cache/debinst";
const WORK_LINUX: &str = "/home/.cache/debinst";
/// The base system's package list (written by debroot.py), and dpkg's
/// database.
const BASE: &str = "sys:/usr/PACKAGES";
const STATUS: &str = "sys:/var/lib/dpkg/status";
/// Room to keep free besides what a package needs.
const SLACK: u64 = 24 << 20;
/// The longest an installation may take.
const GIVE_UP_MS: u64 = 40 * 60 * 1000;
const POLL_MS: u64 = 400;

/// One installation at a time: dpkg's database is locked while it runs.
static BUSY: AtomicBool = AtomicBool::new(false);

// ----------------------------------------------------------------------------
// dpkg's data

/// A package of the database.
#[derive(Clone)]
struct Installed {
    name: String,
    version: String,
    /// The virtual packages it stands for, with the version it gives them
    /// (`Provides: foo (= 1.2)`), if any.
    provides: Vec<(String, Option<String>)>,
    description: String,
}

/// The packages dpkg knows as installed, in the database's order.
fn status_db() -> Vec<Installed> {
    let Some(text) = read_text(STATUS) else { return Vec::new() };
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let f = fields(block);
        let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        // "install ok installed": the last word is the state.
        if get("Status").rsplit(' ').next() != Some("installed") {
            continue;
        }
        let provides = parse_depends(&get("Provides")).into_iter().flatten().map(|d| (d.name, d.rule.filter(|r| r.0 == "=").map(|r| r.1))).collect();
        let description = get("Description").lines().next().unwrap_or("").trim().into();
        out.push(Installed { name: get("Package"), version: get("Version"), provides, description });
    }
    out
}

/// The fields of a control stanza (continuation lines joined with "\n").
fn fields(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in block.lines() {
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

fn read_text(path: &str) -> Option<String> {
    let p = fs::abs(path).ok()?;
    fs::read_file(&p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The packages of the base system (what the image was built from).
fn base_names() -> Vec<String> {
    read_text(BASE).map(|t| t.lines().filter_map(|l| l.split_whitespace().next()).map(String::from).collect()).unwrap_or_default()
}

/// Debian's order of versions ("1:2.3-1" against "1:2.10-1").
fn version_cmp(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (u64, &str, &str) {
        let (epoch, rest) = match v.split_once(':') {
            Some((e, r)) if e.bytes().all(|c| c.is_ascii_digit()) => (e.parse().unwrap_or(0), r),
            _ => (0, v),
        };
        match rest.rsplit_once('-') {
            Some((u, r)) => (epoch, u, r),
            None => (epoch, rest, ""),
        }
    }
    fn weight(c: Option<u8>) -> i32 {
        match c {
            None => 0,
            Some(b'~') => -1,
            Some(c) if c.is_ascii_alphabetic() => c as i32,
            Some(c) => c as i32 + 256,
        }
    }
    fn part(a: &str, b: &str) -> Ordering {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            // The non-digit prefix, letters before symbols, "~" before all.
            while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
                let (x, y) = (weight(a.get(i).copied().filter(|c| !c.is_ascii_digit())), weight(b.get(j).copied().filter(|c| !c.is_ascii_digit())));
                if x != y {
                    return x.cmp(&y);
                }
                if i < a.len() && !a[i].is_ascii_digit() {
                    i += 1;
                }
                if j < b.len() && !b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            // The number.
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (x, y) = (core::str::from_utf8(&a[si..i]).unwrap_or("").trim_start_matches('0'), core::str::from_utf8(&b[sj..j]).unwrap_or("").trim_start_matches('0'));
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    }
    let ((ea, ua, ra), (eb, ub, rb)) = (split(a), split(b));
    ea.cmp(&eb).then_with(|| part(ua, ub)).then_with(|| part(ra, rb))
}

/// One alternative of a dependency: name and an optional version rule
/// (relation, version).
struct Dep {
    name: String,
    rule: Option<(String, String)>,
}

/// A Depends (or Pre-Depends, Provides) field of a binary package as groups
/// of alternatives: "a (>= 1.2) | b, c:any [amd64]". What a binary package
/// can have there is a name, an architecture qualifier (":any"), a rule in
/// parentheses and a list of architectures in brackets; build profiles
/// ("<!nocheck>") are for source packages only, so a "<" is always part of
/// a rule ("<<", "<="). The architecture lists are dropped.
fn parse_depends(text: &str) -> Vec<Vec<Dep>> {
    let mut out = Vec::new();
    for group in text.split(',') {
        let mut alts = Vec::new();
        for alt in group.split('|') {
            let mut s = String::new();
            let mut arch_list = false;
            for c in alt.chars() {
                match c {
                    '[' => arch_list = true,
                    ']' => arch_list = false,
                    c if !arch_list => s.push(c),
                    _ => {}
                }
            }
            let (head, rule) = match s.split_once('(') {
                Some((n, r)) => {
                    let r = r.split(')').next().unwrap_or("").trim();
                    let (op, version) = r.split_at(r.find(|c: char| !matches!(c, '<' | '>' | '=')).unwrap_or(r.len()));
                    let version = version.trim();
                    // "(1.2)" with no relation is an exact version.
                    let op = if op.is_empty() { "=" } else { op };
                    (n, (!version.is_empty()).then(|| (String::from(op), String::from(version))))
                }
                None => (s.as_str(), None),
            };
            let name = head.trim().split(':').next().unwrap_or("").trim();
            if !name.is_empty() {
                alts.push(Dep { name: String::from(name), rule });
            }
        }
        if !alts.is_empty() {
            out.push(alts);
        }
    }
    out
}

/// Whether version `have` meets a rule. "<" and ">" are the old spellings
/// of "<=" and ">=".
fn rule_holds(have: &str, rule: &(String, String)) -> bool {
    let o = version_cmp(have, &rule.1);
    match rule.0.as_str() {
        "<<" => o == Ordering::Less,
        "<=" | "<" => o != Ordering::Greater,
        "=" => o == Ordering::Equal,
        ">=" | ">" => o != Ordering::Less,
        ">>" => o == Ordering::Greater,
        _ => true,
    }
}

/// Whether an installed package meets one alternative: by its name and
/// version, or by what it provides (a versioned rule only by a provide with
/// a version, as dpkg has it).
fn meets(p: &Installed, d: &Dep) -> bool {
    if p.name == d.name && d.rule.as_ref().is_none_or(|r| rule_holds(&p.version, r)) {
        return true;
    }
    p.provides.iter().any(|(name, version)| {
        *name == d.name
            && match (&d.rule, version) {
                (None, _) => true,
                (Some(r), Some(v)) => rule_holds(v, r),
                (Some(_), None) => false,
            }
    })
}

/// The dependencies of a package that the system does not meet, each as
/// the first alternative's name (with its rule when the package is there
/// but too old).
fn unmet(depends: &str, db: &[Installed]) -> Vec<String> {
    let mut out = Vec::new();
    for group in parse_depends(depends) {
        let met = group.iter().any(|d| db.iter().any(|p| meets(p, d)));
        if !met {
            let d = &group[0];
            out.push(match &d.rule {
                Some((op, v)) if db.iter().any(|p| p.name == d.name) => format!("{} (serve la versione {op} {v})", d.name),
                _ => d.name.clone(),
            });
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Running dpkg

/// The script every step runs (a file: no quoting through the arguments).
/// `MODE`, `DEB`/`PKG` and `OUT` (where its output goes, one name per run)
/// come in the environment. dpkg and the package's scripts run in
/// English (LC_ALL=C.UTF-8), although Linux programs otherwise get
/// Italian (`linux::default_env`): the window reads dpkg's messages
/// ("Unpacking", "Setting up", "dependency problems") to show how far it
/// is and what went wrong, and scripts parse what the tools print.
const SCRIPT: &str = "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
DEBIAN_FRONTEND=noninteractive
export DEBIAN_FRONTEND
LC_ALL=C.UTF-8
export LC_ALL
case \"$MODE\" in
info) exec dpkg-deb -f \"$DEB\" > \"$OUT.control\" 2> \"$OUT.log\" ;;
install) exec dpkg --force-confold -i \"$DEB\" > \"$OUT.log\" 2>&1 ;;
remove) exec dpkg --purge \"$PKG\" > \"$OUT.log\" 2>&1 ;;
undo) exec dpkg --purge --force-remove-reinstreq --force-depends \"$PKG\" > \"$OUT.log\" 2>&1 ;;
esac
exit 2
";

fn make_work() -> bool {
    for d in ["sys:/home/.cache", WORK] {
        if let Ok(p) = fs::abs(d) {
            if !fs::exists(&p) && fs::make_dir(&p).is_err() {
                return false;
            }
        }
    }
    let Ok(script) = fs::abs(&format!("{WORK}/run.sh")) else { return false };
    fs::write_file(&script, SCRIPT.as_bytes()).is_ok()
}

/// One run of dpkg: its process and the name of its output files.
struct Step {
    pid: u32,
    out: String,
}

impl Step {
    /// What dpkg has written so far (`log`) or the control data (`control`).
    fn read(&self, what: &str) -> String {
        read_text(&format!("{WORK}/{}.{what}", self.out)).unwrap_or_default()
    }

    /// Waits for the end (or kills it after GIVE_UP_MS); the exit code.
    fn finish(&self) -> Result<i64, String> {
        let began = crate::time::uptime_ms();
        while crate::user::is_running(self.pid) {
            if crate::time::uptime_ms().saturating_sub(began) > GIVE_UP_MS {
                crate::user::kill(self.pid);
                let _ = crate::user::wait(self.pid);
                return Err(String::from("ci mette troppo"));
            }
            crate::task::sleep_ms(POLL_MS);
        }
        Ok(crate::user::wait(self.pid).unwrap_or(-1))
    }

    /// Takes its output files away.
    fn clean(&self) {
        for what in ["log", "control"] {
            if let Ok(p) = fs::abs(&format!("{WORK}/{}.{what}", self.out)) {
                let _ = fs::remove(&p);
            }
        }
    }
}

/// Starts a step.
fn start(mode: &str, deb: &str, pkg: &str) -> Result<Step, String> {
    static RUNS: AtomicU32 = AtomicU32::new(0);
    if !make_work() {
        return Err(String::from("non riesco a preparare la cartella di lavoro"));
    }
    let dash = fs::abs("sys:/usr/bin/dash").ok().filter(fs::exists).ok_or_else(|| String::from("dpkg non c'è in questo sistema"))?;
    let out = format!("{mode}-{}", RUNS.fetch_add(1, AtomicOrdering::Relaxed));
    let env = alloc::vec![format!("MODE={mode}"), format!("DEB={deb}"), format!("PKG={pkg}"), format!("OUT={WORK_LINUX}/{out}")];
    let cwd = fs::abs("sys:/home").unwrap_or_else(|_| fs::Path::root("sys"));
    let pid = crate::user::spawn_env(&dash, &format!("{WORK_LINUX}/run.sh"), cwd, env).map_err(|e| format!("dpkg non parte: {}", e.as_str()))?;
    Ok(Step { pid, out })
}

/// Runs a step to its end; the exit code and the log.
fn run(mode: &str, deb: &str, pkg: &str) -> Result<(i64, String), String> {
    let step = start(mode, deb, pkg)?;
    let code = step.finish();
    let log = step.read("log");
    step.clean();
    Ok((code?, log))
}

/// What a package file says of itself (`dpkg-deb -f`).
fn read_control(deb: &str) -> Result<Info, (String, String)> {
    let step = start("info", deb, "").map_err(|e| (format!("Non riesco a leggere il pacchetto: {e}."), String::new()))?;
    let code = step.finish();
    let (control, log) = (step.read("control"), step.read("log"));
    step.clean();
    let f = fields(&control);
    let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    if code != Ok(0) || get("Package").is_empty() {
        return Err((String::from("Il file non è un pacchetto Debian valido, o è danneggiato."), last_lines(&log, 2)));
    }
    let desc = get("Description");
    let (short, long) = match desc.split_once('\n') {
        Some((s, l)) => (String::from(s.trim()), l.lines().map(|x| if x.trim() == "." { "" } else { x.trim() }).collect::<Vec<_>>().join(" ").trim().into()),
        None => (desc.trim().into(), String::new()),
    };
    let depends = [get("Pre-Depends"), get("Depends")].iter().filter(|d| !d.is_empty()).cloned().collect::<Vec<_>>().join(", ");
    Ok(Info { name: get("Package"), version: get("Version"), arch: get("Architecture"), short, long, kib: get("Installed-Size").parse().unwrap_or(0), depends })
}

/// Why a package cannot be installed on this system, checked before
/// anything is written: its processor, the packages it needs, the space.
fn problem(info: &Info, db: &[Installed]) -> Option<String> {
    if info.arch != "amd64" && info.arch != "all" {
        return Some(if info.arch == "i386" {
            String::from("Questo pacchetto è per programmi a 32 bit (i386): justOS installa solo pacchetti amd64 o «all».")
        } else {
            format!("Questo pacchetto è per un altro tipo di processore ({}): justOS installa solo pacchetti amd64 o «all».", info.arch)
        });
    }
    let missing = unmet(&info.depends, db);
    if !missing.is_empty() {
        return Some(format!(
            "Mancano dei pacchetti di cui ha bisogno: {}. justOS non li scarica ancora da solo: servono i loro file .deb, da installare prima.",
            missing.join(", ")
        ));
    }
    let need = info.kib * 1024 + SLACK;
    if free() < need {
        return Some(format!("Non c'è abbastanza spazio: servono circa {} liberi.", size_text(need / 1024)));
    }
    None
}

/// Takes an installed package away, with its settings (`deb remove` in the
/// shell). Only what was installed from a .deb: the base system's packages
/// stay.
pub fn remove(name: &str) -> Result<String, String> {
    if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'+')) {
        return Err(String::from("nome di pacchetto non valido"));
    }
    if base_names().iter().any(|n| n == name) {
        return Err(format!("«{name}» fa parte del sistema: non si toglie"));
    }
    if !status_db().iter().any(|p| p.name == name) {
        return Err(format!("«{name}» non è installato"));
    }
    if BUSY.swap(true, AtomicOrdering::SeqCst) {
        return Err(String::from("un'altra installazione è in corso"));
    }
    let r = run("remove", "", name);
    BUSY.store(false, AtomicOrdering::SeqCst);
    match r {
        Ok((0, log)) => Ok(log),
        Ok((code, log)) => Err(format!("dpkg è finito con {code}: {}", last_lines(&log, 3))),
        Err(e) => Err(e),
    }
}

/// Installs a package file without a window (`deb install` in the shell),
/// with the window's checks first; dpkg's output.
pub fn install_blocking(path: &str) -> Result<String, String> {
    let deb = linux_path(path)?;
    let info = read_control(&deb).map_err(|(why, detail)| if detail.is_empty() { why } else { format!("{why} ({detail})") })?;
    let db = status_db();
    if let Some(why) = problem(&info, &db) {
        return Err(why);
    }
    let existing = db.iter().any(|p| p.name == info.name);
    if BUSY.swap(true, AtomicOrdering::SeqCst) {
        return Err(String::from("un'altra installazione è in corso"));
    }
    let r = run("install", &deb, "");
    let r = match r {
        Ok((0, log)) => Ok(log),
        Ok((code, log)) => {
            // Half-installed: taken away again (an earlier version is
            // dpkg's to keep).
            if !existing {
                let _ = run("undo", "", &info.name);
            }
            Err(format!("{} (dpkg è finito con {code}: {})", explain(&log).0, last_lines(&log, 3)))
        }
        Err(e) => Err(e),
    };
    BUSY.store(false, AtomicOrdering::SeqCst);
    r
}

/// The packages installed after the image was made: name, version, and
/// what they are.
pub fn user_packages() -> Vec<(String, String, String)> {
    let base = base_names();
    status_db().into_iter().filter(|p| !base.contains(&p.name)).map(|p| (p.name, p.version, p.description)).collect()
}

fn last_lines(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" ")
}

// ----------------------------------------------------------------------------
// The state a window shows and the threads change

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Asking the package what it is.
    Reading,
    /// Known: waiting for the click.
    Ready,
    Installing,
    Done,
    /// Cannot be installed, or the installation failed: `why` says.
    Failed,
}

#[derive(Clone, Default)]
struct Info {
    name: String,
    version: String,
    arch: String,
    short: String,
    long: String,
    /// What it takes once installed (KiB, the package's own figure).
    kib: u64,
    depends: String,
}

/// A program the package brought: name, icon, the file that starts it.
#[derive(Clone)]
struct Program {
    name: String,
    icon: String,
    path: String,
}

struct Shared {
    phase: Phase,
    /// The package file, as the Linux programs see it (once read).
    deb: String,
    info: Option<Info>,
    /// The installed version of the same package, if any.
    existing: Option<String>,
    why: String,
    detail: String,
    /// Where the bar should be (0..=1000), what is being done.
    target: u32,
    step: &'static str,
    began: u64,
    program: Option<Program>,
    /// The command it brought, for a package without a window.
    command: Option<String>,
    closed: bool,
}

type Handle = Arc<IrqLock<Shared>>;

// ----------------------------------------------------------------------------
// The threads

/// The Linux path of a package file: a file of the system volume is where it
/// is; one from another volume (a USB stick) is copied in first.
fn linux_path(path: &str) -> Result<String, String> {
    let p = fs::abs(path).map_err(|_| String::from("non trovo il file"))?;
    if p.volume == "sys" {
        return Ok(p.inner);
    }
    if !make_work() {
        return Err(String::from("non riesco a preparare la cartella di lavoro"));
    }
    let to = fs::abs(&format!("{WORK}/copy.deb")).map_err(|_| String::from("non trovo il file"))?;
    fs::copy_file(&p, &to).map_err(|_| String::from("non riesco a copiare il pacchetto nel sistema (spazio?)"))?;
    Ok(format!("{WORK_LINUX}/copy.deb"))
}

fn fail(h: &Handle, why: &str, detail: &str) {
    if detail.is_empty() {
        crate::println!("debinst: {why}");
    } else {
        crate::println!("debinst: {why} ({detail})");
    }
    let mut s = h.lock();
    s.phase = Phase::Failed;
    s.why = String::from(why);
    s.detail = String::from(detail);
}

fn free() -> u64 {
    fs::volume("sys").map_or(0, |v| v.space().1)
}

/// Reads the package and decides whether it can be installed.
fn read_package(h: Handle, path: String) {
    let deb = match linux_path(&path) {
        Ok(d) => d,
        Err(e) => return fail(&h, &e, ""),
    };
    h.lock().deb = deb.clone();
    let info = match read_control(&deb) {
        Ok(i) => i,
        Err((why, detail)) => return fail(&h, &why, &detail),
    };
    let db = status_db();
    let existing = db.iter().find(|p| p.name == info.name).map(|p| p.version.clone());
    let why = problem(&info, &db);
    if why.is_none() {
        crate::println!("debinst: {} {} can be installed", info.name, info.version);
    }
    {
        let mut s = h.lock();
        s.info = Some(info);
        s.existing = existing;
    }
    match why {
        Some(why) => fail(&h, &why, ""),
        None => h.lock().phase = Phase::Ready,
    }
}

/// The files an installed package brought (dpkg's list of them).
fn file_list(name: &str) -> String {
    ["", ":amd64"].iter().find_map(|a| read_text(&format!("sys:/var/lib/dpkg/info/{name}{a}.list"))).unwrap_or_default()
}

/// The command a package without a window brought: its first program in
/// Debian's PATH, if any.
fn command_of(name: &str) -> Option<String> {
    let list = file_list(name);
    let base = |l: &str| ["/usr/bin/", "/usr/games/", "/usr/sbin/"].iter().find_map(|d| l.strip_prefix(d)).filter(|c| !c.is_empty() && !c.contains('/')).map(String::from);
    let all: Vec<(&str, String)> = list.lines().filter_map(|l| Some((l, base(l)?))).collect();
    // The one named as the package, else the first.
    let (file, bare) = all.iter().find(|(_, c)| c == name).or(all.first())?;
    // Its bare name when that is what the shell runs; its whole path when
    // the shell would find something else first (justOS's own `hello`).
    let whole = format!("sys:{file}");
    let found = crate::user::find_linux_program(bare);
    let plain = !crate::shell::shadowed(bare) && found.is_some() && found == fs::abs(&whole).ok();
    Some(if plain { bare.clone() } else { whole })
}

/// The program a package brought: its first .desktop file (in the package's
/// file list) that the launcher lists.
fn program_of(name: &str) -> Option<Program> {
    let list = file_list(name);
    for line in list.lines() {
        if line.starts_with("/usr/share/applications/") && line.ends_with(".desktop") {
            let Some(text) = read_text(&format!("sys:{line}")) else { continue };
            if let Some(e) = super::linux_app(&text) {
                if let super::Launch::Spawn(path) = e.launch {
                    return Some(Program { name: e.name, icon: e.icon, path });
                }
            }
        }
    }
    None
}

/// What a failed `dpkg -i` said, in the owner's words.
fn explain(log: &str) -> (String, String) {
    let detail = last_lines(log, 3);
    if let Some(i) = log.find("trying to overwrite '") {
        let rest = &log[i + 21..];
        let file = rest.split('\'').next().unwrap_or("");
        let other = rest.split("which is also in package ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or("un altro pacchetto");
        return (format!("Un file di questo pacchetto ({file}) appartiene già a «{other}»: non lo sovrascrivo."), detail);
    }
    if log.contains("dependency problems") || log.contains("depends on") {
        let mut names: Vec<String> = Vec::new();
        for l in log.lines() {
            if let Some((_, r)) = l.split_once(" depends on ") {
                let n = r.split([';', ' ', '(']).next().unwrap_or("");
                if !n.is_empty() && !names.iter().any(|x| x == n) {
                    names.push(String::from(n));
                }
            }
        }
        let list = if names.is_empty() { String::from("altri pacchetti") } else { names.join(", ") };
        return (format!("Non posso installarlo: dipende da {list}, che non vanno bene in questo sistema."), detail);
    }
    for word in [" conflicts with ", " breaks "] {
        if let Some(other) = log.lines().find_map(|l| l.split_once(word)).and_then(|(_, r)| r.split([' ', '(']).find(|s| !s.is_empty())) {
            return (format!("Non va d'accordo con «{other}», che è già installato."), detail);
        }
    }
    if log.contains("No space left") || log.contains("ENOSPC") {
        return (String::from("Lo spazio è finito durante l'installazione."), detail);
    }
    if log.contains("architecture") {
        return (String::from("Il pacchetto è per un altro tipo di processore."), detail);
    }
    if log.contains("subprocess") && log.contains("script") {
        return (String::from("Un passo dell'installazione (uno script del pacchetto) non è riuscito."), detail);
    }
    (String::from("L'installazione non è riuscita."), detail)
}

/// Installs the package: runs dpkg and follows what it writes.
fn install(h: Handle, deb: String) {
    let (info, existing) = {
        let s = h.lock();
        (s.info.clone().unwrap_or_default(), s.existing.clone())
    };
    let base = free();
    let expected = (info.kib * 1024).max(256 << 10);
    if base < expected + SLACK {
        BUSY.store(false, AtomicOrdering::SeqCst);
        return fail(&h, &format!("Non c'è abbastanza spazio: servono circa {} liberi.", size_text((expected + SLACK) / 1024)), "");
    }
    let step = match start("install", &deb, "") {
        Ok(s) => s,
        Err(e) => {
            BUSY.store(false, AtomicOrdering::SeqCst);
            return fail(&h, &format!("Non riesco ad avviare l'installazione: {e}."), "");
        }
    };
    crate::println!("debinst: installing {} {} ({} KiB)", info.name, info.version, info.kib);
    let began = h.lock().began;
    while crate::user::is_running(step.pid) {
        crate::task::sleep_ms(POLL_MS);
        if crate::time::uptime_ms().saturating_sub(began) > GIVE_UP_MS {
            crate::user::kill(step.pid);
            break;
        }
        let log = step.read("log");
        let written = base.saturating_sub(free());
        let (floor, doing) = if log.contains("Setting up") {
            (900, "Configuro il programma…")
        } else if log.contains("Unpacking") {
            (60 + (written * 780 / expected).min(780) as u32, "Copio i file…")
        } else if log.contains("Preparing to unpack") {
            (40, "Leggo il pacchetto…")
        } else {
            (10, "Preparo…")
        };
        let mut s = h.lock();
        s.target = s.target.max(floor).min(985);
        s.step = doing;
    }
    let code = crate::user::wait(step.pid).unwrap_or(-1);
    let log = step.read("log");
    step.clean();
    let ok = code == 0 && status_db().iter().any(|p| p.name == info.name && p.version == info.version);
    if !ok {
        crate::println!("debinst: dpkg -i ended with {code}: {}", last_lines(&log, 4));
        let (why, detail) = explain(&log);
        // A package left half-installed is taken away again (an earlier
        // version, if there was one, is dpkg's to keep).
        if existing.is_none() {
            let _ = run("undo", "", &info.name);
        }
        BUSY.store(false, AtomicOrdering::SeqCst);
        return fail(&h, &why, &detail);
    }
    let program = program_of(&info.name);
    let command = if program.is_none() { command_of(&info.name) } else { None };
    crate::println!("debinst: {} installed in {} s", info.name, (crate::time::uptime_ms() - began) / 1000);
    match &program {
        Some(p) => crate::gui::notify(&format!("«{}» è installato", p.name), "Lo trovi nel launcher.", &p.icon),
        None => crate::gui::notify(&format!("«{}» è installato", info.name), &done_text(command.as_deref()), "store"),
    }
    BUSY.store(false, AtomicOrdering::SeqCst);
    let mut s = h.lock();
    s.target = 1000;
    s.program = program;
    s.command = command;
    s.phase = Phase::Done;
}

/// What to say of an installed package without a window.
fn done_text(command: Option<&str>) -> String {
    match command {
        Some(c) => format!("Non ha una finestra: si usa dal Terminale, con il comando «{c}»."),
        None => String::from("Non ha una finestra né un comando da avviare."),
    }
}

// ----------------------------------------------------------------------------
// The window

pub struct DebInstall {
    h: Handle,
    /// The bar as drawn (0..=1000), gliding to the target; never backwards.
    shown: u32,
    last: u64,
    last_phase: Phase,
    hover: Option<usize>,
}

/// Opens the window for a package file.
pub fn open(path: &str) -> alloc::boxed::Box<dyn App> {
    let h: Handle = Arc::new(IrqLock::new(Shared {
        phase: Phase::Reading,
        deb: String::new(),
        info: None,
        existing: None,
        why: String::new(),
        detail: String::new(),
        target: 0,
        step: "",
        began: crate::time::uptime_ms(),
        program: None,
        command: None,
        closed: false,
    }));
    let (h2, p) = (h.clone(), String::from(path));
    if crate::task::spawn("debinfo", move || read_package(h2, p)).is_none() {
        fail(&h, "Memoria insufficiente.", "");
    }
    alloc::boxed::Box::new(DebInstall { h, shown: 0, last: crate::time::uptime_ms(), last_phase: Phase::Reading, hover: None })
}

/// "1,7 MB" / "340 kB" for a size in KiB.
fn size_text(kib: u64) -> String {
    if kib >= 1024 {
        let tenth = kib * 10 / 1024;
        format!("{},{} MB", tenth / 10, tenth % 10)
    } else {
        format!("{kib} kB")
    }
}

impl DebInstall {
    /// The buttons for a phase: label, primary.
    fn buttons(&self, phase: Phase, s: &Shared) -> Vec<(&'static str, bool)> {
        match phase {
            Phase::Reading => alloc::vec![("Annulla", false)],
            Phase::Ready => {
                let label = match &s.existing {
                    Some(old) if s.info.as_ref().is_some_and(|i| version_cmp(&i.version, old) == Ordering::Equal) => "Reinstalla",
                    Some(_) => "Aggiorna",
                    None => "Installa",
                };
                alloc::vec![("Annulla", false), (label, true)]
            }
            Phase::Installing => Vec::new(),
            Phase::Done if s.program.is_some() => alloc::vec![("Chiudi", false), ("Apri", true)],
            Phase::Done | Phase::Failed => alloc::vec![("Chiudi", true)],
        }
    }

    fn rects(&self, area: Rect, n: usize) -> Vec<Rect> {
        let y = area.bottom() - 58;
        (0..n).map(|i| Rect::new(area.right() - 28 - 132 - (n - 1 - i) as i32 * 142, y, 132, 38)).collect()
    }

    /// What a click on button `i` does.
    fn activate(&mut self, i: usize) -> Action {
        let (phase, label, program, deb) = {
            let s = self.h.lock();
            let list = self.buttons(s.phase, &s);
            (s.phase, list.get(i).map_or("", |b| b.0), s.program.clone(), s.deb.clone())
        };
        match (phase, label) {
            (Phase::Ready, "Installa" | "Reinstalla" | "Aggiorna") => {
                if BUSY.swap(true, AtomicOrdering::SeqCst) {
                    fail(&self.h, "Un'altra installazione è in corso: aspetta che finisca.", "");
                    return Action::Redraw;
                }
                {
                    let mut s = self.h.lock();
                    s.phase = Phase::Installing;
                    s.target = 0;
                    s.step = "Preparo…";
                    s.began = crate::time::uptime_ms();
                }
                let h = self.h.clone();
                if crate::task::spawn("debinstall", move || install(h, deb)).is_none() {
                    BUSY.store(false, AtomicOrdering::SeqCst);
                    fail(&self.h, "Memoria insufficiente.", "");
                }
                Action::Redraw
            }
            (Phase::Done, "Apri") => {
                self.h.lock().closed = true;
                match program {
                    Some(p) => Action::Spawn(p.path),
                    None => Action::Redraw,
                }
            }
            _ => {
                self.h.lock().closed = true;
                Action::Redraw
            }
        }
    }
}

impl App for DebInstall {
    fn title(&self) -> String {
        match self.h.lock().info.as_ref() {
            Some(i) => format!("Installazione di «{}»", i.name),
            None => String::from("Installazione"),
        }
    }

    fn icon(&self) -> &str {
        "store"
    }

    fn kind(&self) -> &'static str {
        "debinst"
    }

    fn size(&self) -> (i32, i32) {
        (540, 340)
    }

    fn animating(&self) -> bool {
        matches!(self.h.lock().phase, Phase::Installing)
    }

    fn frame(&mut self) -> bool {
        let now = crate::time::uptime_ms();
        let dt = now.saturating_sub(self.last).min(500);
        self.last = now;
        let target = self.h.lock().target;
        if target > self.shown {
            let step = ((target - self.shown) as u64 * dt / (dt + 450)).max(u64::from(dt > 0)) as u32;
            self.shown = (self.shown + step).min(target);
            return true;
        }
        false
    }

    fn tick(&mut self) -> bool {
        let p = self.h.lock().phase;
        let changed = p != self.last_phase;
        self.last_phase = p;
        changed || p == Phase::Installing
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        let (phase, info, existing, why, detail, step, program, command) = {
            let s = self.h.lock();
            (s.phase, s.info.clone(), s.existing.clone(), s.why.clone(), s.detail.clone(), s.step, s.program.clone(), s.command.clone())
        };
        // The package's picture: the program's icon once there is one, else
        // the store's box; when it fails, a white box in a grey tile, as the
        // Windows logo in winprep.
        let tile = Rect::new(area.x + 28, area.y + 28, 64, 64);
        match (&program, phase) {
            (Some(p), Phase::Done) if p.icon.starts_with('@') => res::draw_icon(c, &p.icon, 64, tile.x, tile.y),
            (_, Phase::Failed) => {
                c.fill_round_grad(tile, 18, 0xFF55_5260, 0xFF3A_3844);
                c.sheen_round(tile, 18, 1, ui::TILE_SHEEN);
                c.fill_round(Rect::new(tile.x + 14, tile.y + 17, 36, 9), 3, ui::WHITE);
                c.fill_round(Rect::new(tile.x + 17, tile.y + 28, 30, 20), 3, ui::WHITE);
                c.fill_round(Rect::new(tile.x + 26, tile.y + 32, 12, 4), 2, 0xFF48_4652);
            }
            _ => res::draw_icon(c, "store", 64, tile.x, tile.y),
        }
        let x = area.x + 112;
        let w = area.w - 112 - 28;
        let name = info.as_ref().map(|i| i.name.clone()).unwrap_or_default();
        let shown_name = program.as_ref().filter(|_| phase == Phase::Done).map(|p| p.name.clone()).unwrap_or_else(|| name.clone());
        let heading = match phase {
            Phase::Reading => String::from("Leggo il pacchetto…"),
            Phase::Ready => format!("Installare «{name}»?"),
            Phase::Installing => format!("Installo «{name}»…"),
            Phase::Done => format!("«{shown_name}» è installato"),
            Phase::Failed if info.is_some() => format!("«{name}» non si installa"),
            Phase::Failed => String::from("Non posso aprire il pacchetto"),
        };
        let mut y = area.y + 26;
        for line in text::wrap(Font::Heading, &heading, w) {
            text::draw(c, Font::Heading, x, y, &line, ui::text());
            y += 28;
        }
        y += 2;
        if let (Some(i), true) = (&info, phase != Phase::Failed || !why.is_empty()) {
            if !matches!(phase, Phase::Done) {
                let line = format!("Versione {} · occupa {}", i.version, size_text(i.kib));
                text::draw(c, Font::Small, x, y, &line, ui::text_faint());
                y += 22;
            }
        }
        match phase {
            Phase::Reading => {
                text::draw(c, Font::Small, x, y, "Un momento.", ui::text_dim());
            }
            Phase::Ready => {
                if let Some(i) = &info {
                    for line in text::wrap(Font::Strong, &i.short, w).into_iter().take(2) {
                        text::draw(c, Font::Strong, x, y, &line, ui::text());
                        y += 22;
                    }
                    y += 4;
                    let lines = text::wrap(Font::Small, &i.long, w);
                    for (n, line) in lines.iter().take(4).enumerate() {
                        let line = if n == 3 && lines.len() > 4 { text::fit(Font::Small, &format!("{line} …"), w) } else { line.clone() };
                        text::draw(c, Font::Small, x, y, &line, ui::text_dim());
                        y += 19;
                    }
                    if let Some(old) = &existing {
                        y += 6;
                        let note = if version_cmp(&i.version, old) == Ordering::Equal { String::from("È già installato, nella stessa versione.") } else { format!("Ora c'è la versione {old}: verrà sostituita.") };
                        text::draw(c, Font::Small, x, y, &note, ui::ORANGE);
                    }
                }
            }
            Phase::Installing => {
                let bar = Rect::new(area.x + 28, area.bottom() - 112, area.w - 56, 14);
                c.fill_round(bar, 7, ui::cell());
                let fill = (bar.w as i64 * self.shown.min(1000) as i64 / 1000) as i32;
                if fill > 0 {
                    let r = Rect::new(bar.x, bar.y, fill.max(14), bar.h);
                    c.fill_round_grad(r, 7, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
                    c.sheen_round(r, 7, 1, ui::TILE_SHEEN);
                }
                text::draw(c, Font::Strong, bar.x, bar.bottom() + 10, &format!("{}%", self.shown / 10), ui::text());
                text::draw_right(c, Font::Small, bar.right(), bar.bottom() + 13, step, ui::text_dim());
            }
            Phase::Done => {
                let line = match &program {
                    Some(_) => String::from("Lo trovi nel launcher."),
                    None => done_text(command.as_deref()),
                };
                for line in text::wrap(Font::Small, &line, w) {
                    text::draw(c, Font::Small, x, y, &line, ui::text_dim());
                    y += 20;
                }
            }
            Phase::Failed => {
                for line in text::wrap(Font::Small, &why, w) {
                    text::draw(c, Font::Small, x, y, &line, ui::text_dim());
                    y += 20;
                }
                if !detail.is_empty() {
                    y += 6;
                    for line in text::wrap(Font::Small, &detail, w).into_iter().take(3) {
                        text::draw(c, Font::Small, x, y, &line, ui::text_faint());
                        y += 18;
                    }
                }
            }
        }
        let list = {
            let s = self.h.lock();
            self.buttons(phase, &s)
        };
        let rects = self.rects(area, list.len());
        for (i, ((label, primary), r)) in list.iter().zip(rects.iter()).enumerate() {
            ui::button(c, *r, label, *primary, self.hover == Some(i));
        }
    }

    fn press(&mut self, x: i32, y: i32, _double: bool, area: Rect) -> Action {
        let (ax, ay) = (area.x + x, area.y + y);
        let n = {
            let s = self.h.lock();
            self.buttons(s.phase, &s).len()
        };
        match self.rects(area, n).iter().position(|r| r.contains(ax, ay)) {
            Some(i) => self.activate(i),
            None => Action::None,
        }
    }

    fn hover(&mut self, at: Option<(i32, i32)>, area: Rect) -> bool {
        let n = {
            let s = self.h.lock();
            self.buttons(s.phase, &s).len()
        };
        let h = at.and_then(|(x, y)| self.rects(area, n).iter().position(|r| r.contains(area.x + x, area.y + y)));
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    fn key(&mut self, ev: KeyEvent) -> Action {
        let n = {
            let s = self.h.lock();
            self.buttons(s.phase, &s).len()
        };
        match ev.key {
            Key::Enter if n > 0 => self.activate(n - 1),
            Key::Escape if self.h.lock().phase != Phase::Installing => {
                self.h.lock().closed = true;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn close(&mut self) {
        // Closing the window while dpkg works leaves it to finish: the
        // result comes as a notification.
        self.h.lock().closed = true;
    }

    fn alive(&self) -> bool {
        !self.h.lock().closed
    }
}
