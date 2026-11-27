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
//! What the package needs and the system does not have comes from Debian's
//! archive, as with `apt install ./file.deb`: [`crate::apt`] fetches the
//! signed index and justdeb's resolver decides which packages; the window
//! says how many and how big ("Servono anche 12 pacchetti, 34 MB") before
//! anything is downloaded, then downloads them (each checked against the
//! signed index) and gives them all to dpkg, in as few `dpkg -i` runs as
//! their Pre-Depends allow. `deb install` and `deb get <name>` in the shell
//! do the same without the window.

use crate::apt;
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
use justdeb::index::Pkg;
use justdeb::release::Source;
use justdeb::resolve::{Plan, LOCAL};
use justdeb::version::compare as version_cmp;

/// Where the helper script, its log and a copy of a package from another
/// volume are (as justOS and as the Linux programs see it).
const WORK: &str = "sys:/home/.cache/debinst";
const WORK_LINUX: &str = "/home/.cache/debinst";
/// The packages downloaded from the archive, until dpkg has them.
const ARCHIVES: &str = "sys:/home/.cache/debinst/archives";
const ARCHIVES_LINUX: &str = "/home/.cache/debinst/archives";
/// The base system's package list (written by debroot.py).
const BASE: &str = "sys:/usr/PACKAGES";
/// Room to keep free besides what a package needs.
const SLACK: u64 = 24 << 20;
/// The longest an installation may take.
const GIVE_UP_MS: u64 = 40 * 60 * 1000;
const POLL_MS: u64 = 400;

/// One installation at a time: dpkg's database is locked while it runs.
static BUSY: AtomicBool = AtomicBool::new(false);

// ----------------------------------------------------------------------------
// dpkg's data

fn read_text(path: &str) -> Option<String> {
    let p = fs::abs(path).ok()?;
    fs::read_file(&p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The packages of the base system (what the image was built from).
fn base_names() -> Vec<String> {
    read_text(BASE).map(|t| t.lines().filter_map(|l| l.split_whitespace().next()).map(String::from).collect()).unwrap_or_default()
}

/// The installed version of a package, if any.
fn installed_version(name: &str) -> Option<String> {
    apt::installed().into_iter().find(|p| p.name == name).map(|p| p.version)
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
/// `install` takes in `DEB` a file that lists the packages, one per line
/// (a name may have spaces); `undo` takes the names in `PKG`, apart by
/// spaces (package names have none).
const SCRIPT: &str = "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
DEBIAN_FRONTEND=noninteractive
export DEBIAN_FRONTEND
UCF_FORCE_CONFFNEW=YES
export UCF_FORCE_CONFFNEW
LC_ALL=C.UTF-8
export LC_ALL
# What the scripts of the system's own packages would do when a package
# adds settings schemas, icons, MIME types, fonts or image loaders (their
# triggers): those came with the system unpacked, without their scripts.
caches() {
set +f
unset IFS
L=/usr/lib/x86_64-linux-gnu
[ -d /usr/share/glib-2.0/schemas ] && $L/glib-2.0/glib-compile-schemas /usr/share/glib-2.0/schemas
[ -d $L/gio/modules ] && $L/glib-2.0/gio-querymodules $L/gio/modules
for d in /usr/share/icons/*/; do [ -f \"$d/index.theme\" ] && gtk-update-icon-cache -q -f -t \"$d\"; done
[ -x /usr/bin/update-mime-database ] && update-mime-database /usr/share/mime
[ -x /usr/bin/update-desktop-database ] && update-desktop-database -q /usr/share/applications
[ -x $L/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders ] && $L/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders --update-cache
[ -x /usr/bin/fc-cache ] && fc-cache
ldconfig
return 0
}
case \"$MODE\" in
info) exec dpkg-deb -f \"$DEB\" > \"$OUT.control\" 2> \"$OUT.log\" ;;
install) IFS='
'
set -f
dpkg --force-confold -i $(cat \"$DEB\") > \"$OUT.log\" 2>&1
r=$?
caches > /dev/null 2>&1
exit $r ;;
remove) dpkg --purge \"$PKG\" > \"$OUT.log\" 2>&1
r=$?
caches > /dev/null 2>&1
exit $r ;;
undo) exec dpkg --purge --force-remove-reinstreq --force-depends $PKG > \"$OUT.log\" 2>&1 ;;
esac
exit 2
";

fn make_work() -> bool {
    for d in ["sys:/home/.cache", WORK, ARCHIVES] {
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
        for what in ["log", "control", "list"] {
            if let Ok(p) = fs::abs(&format!("{WORK}/{}.{what}", self.out)) {
                let _ = fs::remove(&p);
            }
        }
    }
}

static RUNS: AtomicU32 = AtomicU32::new(0);

/// Starts a step; `out` names its files (`next_out`).
fn start_as(mode: &str, deb: &str, pkg: &str, out: String) -> Result<Step, String> {
    if !make_work() {
        return Err(String::from("non riesco a preparare la cartella di lavoro"));
    }
    let dash = fs::abs("sys:/usr/bin/dash").ok().filter(fs::exists).ok_or_else(|| String::from("dpkg non c'è in questo sistema"))?;
    let env = alloc::vec![format!("MODE={mode}"), String::from("DEBIAN_FRONTEND=noninteractive"), format!("DEB={deb}"), format!("PKG={pkg}"), format!("OUT={WORK_LINUX}/{out}")];
    let cwd = fs::abs("sys:/home").unwrap_or_else(|_| fs::Path::root("sys"));
    let pid = crate::user::spawn_env(&dash, &format!("{WORK_LINUX}/run.sh"), cwd, env).map_err(|e| format!("dpkg non parte: {}", e.as_str()))?;
    Ok(Step { pid, out })
}

fn next_out(mode: &str) -> String {
    format!("{mode}-{}", RUNS.fetch_add(1, AtomicOrdering::Relaxed))
}

fn start(mode: &str, deb: &str, pkg: &str) -> Result<Step, String> {
    start_as(mode, deb, pkg, next_out(mode))
}

/// Runs a step to its end; the exit code and the log.
fn run(mode: &str, deb: &str, pkg: &str) -> Result<(i64, String), String> {
    let step = start(mode, deb, pkg)?;
    let code = step.finish();
    let log = step.read("log");
    step.clean();
    Ok((code?, log))
}

/// Starts `dpkg -i` on some package files (Linux paths), in this order.
fn start_install(files: &[String]) -> Result<Step, String> {
    if !make_work() {
        return Err(String::from("non riesco a preparare la cartella di lavoro"));
    }
    let out = next_out("install");
    let mut list = files.join("\n");
    list.push('\n');
    let file = fs::abs(&format!("{WORK}/{out}.list")).map_err(|_| String::from("non riesco a preparare la cartella di lavoro"))?;
    fs::write_file(&file, list.as_bytes()).map_err(|_| String::from("non riesco a preparare la cartella di lavoro"))?;
    start_as("install", &format!("{WORK_LINUX}/{out}.list"), "", out)
}

/// What a package file says of itself (`dpkg-deb -f`).
fn read_control(deb: &str) -> Result<Info, (String, String)> {
    let step = start("info", deb, "").map_err(|e| (format!("Non riesco a leggere il pacchetto: {e}."), String::new()))?;
    let code = step.finish();
    let (control, log) = (step.read("control"), step.read("log"));
    step.clean();
    let f = justdeb::control::fields(&control);
    let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    if code != Ok(0) || get("Package").is_empty() {
        return Err((String::from("Il file non è un pacchetto Debian valido, o è danneggiato."), last_lines(&log, 2)));
    }
    let desc = get("Description");
    let (short, long) = match desc.split_once('\n') {
        Some((s, l)) => (String::from(s.trim()), l.lines().map(|x| if x.trim() == "." { "" } else { x.trim() }).collect::<Vec<_>>().join(" ").trim().into()),
        None => (desc.trim().into(), String::new()),
    };
    let pkg = Pkg::from_paragraph(&control, LOCAL);
    Ok(Info { name: get("Package"), version: get("Version"), arch: get("Architecture"), short, long, kib: get("Installed-Size").parse().unwrap_or(0), pkg })
}

/// Why a package is not for this computer: its processor.
fn arch_problem(info: &Info) -> Option<String> {
    if info.arch == "amd64" || info.arch == "all" {
        return None;
    }
    Some(if info.arch == "i386" {
        String::from("Questo pacchetto è per programmi a 32 bit (i386): justOS installa solo pacchetti amd64 o «all».")
    } else {
        format!("Questo pacchetto è per un altro tipo di processore ({}): justOS installa solo pacchetti amd64 o «all».", info.arch)
    })
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
    if installed_version(name).is_none() {
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

/// The packages installed after the image was made: name, version, and
/// what they are.
pub fn user_packages() -> Vec<(String, String, String)> {
    let base = base_names();
    apt::installed().into_iter().filter(|p| !base.contains(&p.name)).map(|p| (p.name, p.version, p.summary)).collect()
}

fn last_lines(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" ")
}

// ----------------------------------------------------------------------------
// Preparing an installation: the package, what it needs, the space

/// What an installation will do, decided before anything is written.
#[derive(Clone)]
pub struct Prepared {
    pub info: Info,
    /// The package file (a Linux path), empty for one of the archive.
    deb: String,
    plan: Plan,
    /// The archives the plan's downloads come from.
    sources: Vec<Source>,
    /// The installed version of the same package, if any.
    existing: Option<String>,
}

impl Prepared {
    /// The packages besides the one asked for.
    fn extras(&self) -> Vec<&Pkg> {
        self.plan.packages.iter().filter(|p| p.name != self.info.name).collect()
    }

    /// "Servono anche 12 pacchetti, 34 MB da scaricare", or what is
    /// downloaded for a package of the archive alone; None when nothing is.
    pub fn summary(&self) -> Option<String> {
        let n = self.extras().len();
        let down = size_text(self.plan.download_size().div_ceil(1024));
        match n {
            0 if self.plan.download_size() == 0 => None,
            0 => Some(format!("Si scarica dall'archivio di Debian: {down}")),
            1 => Some(format!("Serve anche 1 pacchetto, {down} da scaricare")),
            n => Some(format!("Servono anche {n} pacchetti, {down} da scaricare")),
        }
    }

    /// The other packages by name, those that replace an installed version
    /// marked.
    pub fn names(&self) -> String {
        let names: Vec<String> = self
            .extras()
            .iter()
            .map(|p| if self.plan.upgrades.iter().any(|(n, _)| *n == p.name) { format!("{} (aggiornato)", p.name) } else { p.name.clone() })
            .collect();
        names.join(", ")
    }

    /// Whether anything comes from the archive.
    pub fn downloads(&self) -> bool {
        self.plan.downloads().next().is_some()
    }

    /// KiB the packages take once installed.
    pub fn installed_kib(&self) -> u64 {
        self.plan.installed_kib()
    }
}

/// The plan for a package, once its processor fits: what it needs, and
/// whether there is room for all of it.
fn prepare(info: Info, deb: String, planned: (Plan, Vec<Source>)) -> Result<Prepared, String> {
    let (plan, sources) = planned;
    let need = plan.installed_kib() * 1024 + plan.download_size() + SLACK;
    if free() < need {
        return Err(format!("Non c'è abbastanza spazio: servono circa {} liberi.", size_text(need / 1024)));
    }
    let existing = installed_version(&info.name);
    if plan.packages.len() > 1 {
        crate::println!("debinst: {} needs {} more packages, {} KiB to download", info.name, plan.packages.len() - 1, plan.download_size() / 1024);
    }
    Ok(Prepared { info, deb, plan, sources, existing })
}

/// Reads a package file and prepares its installation (`deb install`).
pub fn prepare_file(path: &str, step: &mut dyn FnMut(apt::Step<'_>)) -> Result<Prepared, String> {
    let deb = linux_path(path)?;
    let info = read_control(&deb).map_err(|(why, detail)| if detail.is_empty() { why } else { format!("{why} ({detail})") })?;
    prepare_local(info, deb, step)
}

/// A package file's installation: for this processor, with what the
/// system and the archives have.
fn prepare_local(info: Info, deb: String, step: &mut dyn FnMut(apt::Step<'_>)) -> Result<Prepared, String> {
    if let Some(why) = arch_problem(&info) {
        return Err(why);
    }
    let planned = apt::plan(alloc::vec![info.pkg.clone()], step)?;
    prepare(info, deb, planned)
}

/// Prepares the installation of a package of the archive (`deb get`).
pub fn prepare_name(name: &str, step: &mut dyn FnMut(apt::Step<'_>)) -> Result<Prepared, String> {
    // The index read once, for the name and for the plan.
    let (index, sources) = apt::load(false, step)?;
    let pkg = apt::find(&index, name)?;
    if installed_version(&pkg.name).is_some_and(|v| version_cmp(&v, &pkg.version) != Ordering::Less) {
        return Err(format!("«{}» è già installato, nella versione più recente ({})", pkg.name, pkg.version));
    }
    let planned = apt::plan_in(alloc::vec![pkg.clone()], &index, sources)?;
    prepare(Info::of(pkg), String::new(), planned)
}

// ----------------------------------------------------------------------------
// Installing what is prepared

/// How far an installation is (0..=1000) and what it is doing.
type Report<'a> = &'a mut dyn FnMut(u32, &str);

/// Downloads the plan's packages: their Linux paths, by package name.
fn fetch_all(p: &Prepared, report: Report<'_>, share: u32) -> Result<Vec<(String, String)>, String> {
    let downloads: Vec<&Pkg> = p.plan.downloads().collect();
    let total = p.plan.download_size().max(1);
    let mut done = 0u64;
    let mut out = Vec::new();
    // The first package ever is downloaded before dpkg has run once.
    if !downloads.is_empty() && !make_work() {
        return Err(String::from("non riesco a creare la cartella dei pacchetti"));
    }
    for (n, pkg) in downloads.iter().enumerate() {
        let doing = format!("Scarico {} ({} di {})…", pkg.name, n + 1, downloads.len());
        report((done * share as u64 / total) as u32, &doing);
        let file = apt::download(pkg, &p.sources, ARCHIVES, &mut |got, _| {
            report(((done + got).min(total) * share as u64 / total) as u32, &doing);
            true
        })?;
        done += pkg.size;
        out.push((pkg.name.clone(), format!("{ARCHIVES_LINUX}/{}", file.rsplit('/').next().unwrap_or(""))));
    }
    Ok(out)
}

/// Takes the downloaded packages away.
fn clean_archives() {
    if let Ok(dir) = fs::abs(ARCHIVES) {
        for (e, _) in fs::list_dir(&dir).unwrap_or_default() {
            let _ = fs::remove(&dir.join(&e.name));
        }
    }
}

/// How many of a log's lines start with `what` (dpkg's "Unpacking …").
fn count(log: &str, what: &str) -> u64 {
    log.lines().filter(|l| l.starts_with(what)).count() as u64
}

/// Runs one `dpkg -i` of a batch, reporting from `from` to `to`; the log.
fn dpkg_batch(files: &[String], report: Report<'_>, from: u32, to: u32) -> Result<String, (String, String)> {
    let step = start_install(files).map_err(|e| (format!("Non riesco ad avviare l'installazione: {e}."), String::new()))?;
    let began = crate::time::uptime_ms();
    let steps = files.len() as u64 * 2;
    while crate::user::is_running(step.pid) {
        crate::task::sleep_ms(POLL_MS);
        if crate::time::uptime_ms().saturating_sub(began) > GIVE_UP_MS {
            crate::user::kill(step.pid);
            break;
        }
        let log = step.read("log");
        let (unpacked, set_up) = (count(&log, "Unpacking "), count(&log, "Setting up "));
        let doing = if set_up > 0 { "Configuro…" } else if unpacked > 0 { "Copio i file…" } else { "Leggo i pacchetti…" };
        report(from + ((unpacked + set_up).min(steps) * (to - from) as u64 / steps) as u32, doing);
    }
    let code = crate::user::wait(step.pid).unwrap_or(-1);
    let log = step.read("log");
    step.clean();
    if code != 0 {
        crate::println!("debinst: dpkg -i ended with {code}: {}", last_lines(&log, 4));
        // The errors in full, for the system log.
        for l in log.lines().filter(|l| l.contains("rror") || l.contains("not found") || l.contains("warning") || l.contains("depends on")).take(40) {
            crate::debug!("debinst", "{l}");
        }
        return Err(explain(&log));
    }
    Ok(log)
}

/// Installs what is prepared: downloads, then one `dpkg -i` per batch. On
/// failure the packages that were not there before are taken away again
/// (an earlier version, if there was one, is dpkg's to keep). The log, or
/// why it failed and dpkg's last words.
pub fn install_prepared(p: &Prepared, report: Report<'_>) -> Result<String, (String, String)> {
    if BUSY.swap(true, AtomicOrdering::SeqCst) {
        return Err((String::from("Un'altra installazione è in corso: aspetta che finisca."), String::new()));
    }
    let r = install_locked(p, report);
    clean_archives();
    BUSY.store(false, AtomicOrdering::SeqCst);
    r
}

fn install_locked(p: &Prepared, report: Report<'_>) -> Result<String, (String, String)> {
    let share = if p.downloads() { 400 } else { 0 };
    let files = fetch_all(p, report, share).map_err(|e| (format!("Lo scaricamento non è riuscito: {e}."), String::new()))?;
    let path_of = |pkg: &Pkg| if pkg.source == LOCAL { Some(p.deb.clone()) } else { files.iter().find(|(n, _)| *n == pkg.name).map(|(_, f)| f.clone()) };
    let before = apt::installed();
    let new: Vec<String> = p.plan.packages.iter().filter(|q| !before.iter().any(|b| b.name == q.name)).map(|q| q.name.clone()).collect();
    crate::println!("debinst: installing {} {} with {} packages in {} runs", p.info.name, p.info.version, p.plan.packages.len(), p.plan.batches.len());
    let mut log = String::new();
    let batches = p.plan.batches.len() as u32;
    for (b, batch) in p.plan.batches.iter().enumerate() {
        let list: Vec<String> = batch.iter().filter_map(|&i| path_of(&p.plan.packages[i])).collect();
        let span = (985 - share) / batches.max(1);
        let from = share + b as u32 * span;
        match dpkg_batch(&list, report, from, from + span) {
            Ok(l) => log.push_str(&l),
            Err(e) => {
                if !new.is_empty() {
                    let mut names = new.clone();
                    names.reverse();
                    let _ = run("undo", "", &names.join(" "));
                }
                return Err(e);
            }
        }
    }
    // dpkg may have ended well without configuring everything (a trigger
    // left pending, say): what matters is that each package is there.
    let after = apt::installed();
    if let Some(q) = p.plan.packages.iter().find(|q| !after.iter().any(|a| a.name == q.name && a.version == q.version)) {
        return Err((format!("«{}» non risulta installato dopo dpkg.", q.name), last_lines(&log, 3)));
    }
    report(1000, "Fatto");
    Ok(log)
}

// ----------------------------------------------------------------------------
// The state a window shows and the threads change

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Asking the package what it is.
    Reading,
    /// Fetching the archive's index, to find what the package needs.
    Resolving,
    /// Known: waiting for the click.
    Ready,
    Installing,
    Done,
    /// Cannot be installed, or the installation failed: `why` says.
    Failed,
}

#[derive(Clone, Default)]
pub struct Info {
    pub name: String,
    pub version: String,
    arch: String,
    pub short: String,
    long: String,
    /// What it takes once installed (KiB, the package's own figure).
    kib: u64,
    /// Its control data, as the resolver reads it.
    pkg: Pkg,
}

impl Info {
    /// A package of the archive (no long description in the index).
    fn of(pkg: Pkg) -> Info {
        Info { name: pkg.name.clone(), version: pkg.version.clone(), arch: pkg.arch.clone(), short: pkg.summary.clone(), long: String::new(), kib: pkg.installed_kib, pkg }
    }
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
    info: Option<Info>,
    /// What the installation will do, once decided.
    prepared: Option<Prepared>,
    why: String,
    detail: String,
    /// Where the bar should be (0..=1000), what is being done.
    target: u32,
    step: String,
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

/// What the index's download looks like in the window and the shell.
pub fn step_text(step: &apt::Step<'_>) -> String {
    match step {
        apt::Step::Index(name, done, total) => format!("Scarico l'indice di {name}: {} di {}", size_text(done / 1024), size_text(total.div_ceil(1024))),
        apt::Step::Reading => String::from("Leggo l'indice dei pacchetti…"),
    }
}

/// Reads the package and decides whether, and with what, it can be
/// installed.
fn read_package(h: Handle, path: String) {
    let deb = match linux_path(&path) {
        Ok(d) => d,
        Err(e) => return fail(&h, &e, ""),
    };
    let info = match read_control(&deb) {
        Ok(i) => i,
        Err((why, detail)) => return fail(&h, &why, &detail),
    };
    h.lock().info = Some(info.clone());
    let hs = h.clone();
    let r = prepare_local(info, deb, &mut |step| {
        let mut s = hs.lock();
        s.phase = Phase::Resolving;
        if let apt::Step::Index(_, done, total) = step {
            s.target = (done * 1000 / total.max(1)).min(1000) as u32;
        }
        s.step = step_text(&step);
    });
    match r {
        Ok(p) => {
            crate::println!("debinst: {} {} can be installed", p.info.name, p.info.version);
            let mut s = h.lock();
            s.prepared = Some(p);
            s.phase = Phase::Ready;
        }
        Err(why) => fail(&h, &why, ""),
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
    // A command named as the package that the package set up through
    // update-alternatives (figlet's /usr/bin/figlet, to figlet-figlet) is
    // not in its file list, but it is the one to type.
    if !all.is_empty() && !all.iter().any(|(_, c)| c == name) && !crate::shell::shadowed(name) && crate::user::find_linux_program(name).is_some() {
        return Some(String::from(name));
    }
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
/// The program a `.desktop` file of /usr/share/applications starts, and
/// its icon (for the store's programs of Debian's archive).
pub fn desktop_program(file: &str) -> Option<(String, String)> {
    let text = read_text(&format!("sys:/usr/share/applications/{file}"))?;
    let e = super::linux_app(&text)?;
    match e.launch {
        super::Launch::Spawn(path) => Some((path, e.icon)),
        _ => None,
    }
}

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

/// Installs what is prepared, following it on the bar.
fn install(h: Handle, p: Prepared) {
    let began = h.lock().began;
    let hs = h.clone();
    let r = install_prepared(&p, &mut |at, doing| {
        let mut s = hs.lock();
        s.target = s.target.max(at).min(if at >= 1000 { 1000 } else { 985 });
        if s.step != doing {
            s.step = String::from(doing);
        }
    });
    if let Err((why, detail)) = r {
        return fail(&h, &why, &detail);
    }
    let name = &p.info.name;
    let program = program_of(name);
    let command = if program.is_none() { command_of(name) } else { None };
    crate::println!("debinst: {name} installed in {} s", (crate::time::uptime_ms() - began) / 1000);
    match &program {
        Some(pr) => crate::gui::notify(&format!("«{}» è installato", pr.name), "Lo trovi nel launcher.", &pr.icon),
        None => crate::gui::notify(&format!("«{name}» è installato"), &done_text(command.as_deref()), "store"),
    }
    let mut s = h.lock();
    s.target = 1000;
    s.program = program;
    s.command = command;
    s.phase = Phase::Done;
}

/// What to say of an installed package without a window.
pub fn done_text(command: Option<&str>) -> String {
    match command {
        Some(c) => format!("Non ha una finestra: si usa dal Terminale, con il comando «{c}»."),
        None => String::from("Non ha una finestra né un comando da avviare."),
    }
}

/// What the shell says once a package is installed: how to start it.
pub fn installed_text(name: &str) -> String {
    match program_of(name) {
        Some(p) => format!("«{}» è installato: lo trovi nel launcher.", p.name),
        None => format!("«{name}» è installato. {}", done_text(command_of(name).as_deref())),
    }
}

// ----------------------------------------------------------------------------
// The window

pub struct DebInstall {
    h: Handle,
    /// The bar as drawn (0..=1000), gliding to the target; never backwards
    /// within a phase.
    shown: u32,
    last: u64,
    last_phase: Phase,
    hover: Option<usize>,
}

/// Opens the window for a package file.
pub fn open(path: &str) -> alloc::boxed::Box<dyn App> {
    let h: Handle = Arc::new(IrqLock::new(Shared {
        phase: Phase::Reading,
        info: None,
        prepared: None,
        why: String::new(),
        detail: String::new(),
        target: 0,
        step: String::new(),
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
pub fn size_text(kib: u64) -> String {
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
            Phase::Reading | Phase::Resolving => alloc::vec![("Annulla", false)],
            Phase::Ready => {
                let p = s.prepared.as_ref();
                let label = match p.and_then(|p| p.existing.as_ref().map(|old| (old, &p.info.version))) {
                    Some((old, new)) if version_cmp(new, old) == Ordering::Equal => "Reinstalla",
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
        let (phase, label, program, prepared) = {
            let s = self.h.lock();
            let list = self.buttons(s.phase, &s);
            (s.phase, list.get(i).map_or("", |b| b.0), s.program.clone(), s.prepared.clone())
        };
        match (phase, label, prepared) {
            (Phase::Ready, "Installa" | "Reinstalla" | "Aggiorna", Some(p)) => {
                if BUSY.load(AtomicOrdering::SeqCst) {
                    fail(&self.h, "Un'altra installazione è in corso: aspetta che finisca.", "");
                    return Action::Redraw;
                }
                {
                    let mut s = self.h.lock();
                    s.phase = Phase::Installing;
                    s.target = 0;
                    s.step = String::from("Preparo…");
                    s.began = crate::time::uptime_ms();
                }
                self.shown = 0;
                let h = self.h.clone();
                if crate::task::spawn("debinstall", move || install(h, p)).is_none() {
                    fail(&self.h, "Memoria insufficiente.", "");
                }
                Action::Redraw
            }
            (Phase::Done, "Apri", _) => {
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

    /// The bar of a phase that takes a while, with what is being done.
    fn draw_bar(&self, c: &mut Canvas, area: Rect, step: &str) {
        let bar = Rect::new(area.x + 28, area.bottom() - 112, area.w - 56, 14);
        c.fill_round(bar, 7, ui::cell());
        let fill = (bar.w as i64 * self.shown.min(1000) as i64 / 1000) as i32;
        if fill > 0 {
            let r = Rect::new(bar.x, bar.y, fill.max(14), bar.h);
            c.fill_round_grad(r, 7, ui::ORANGE_GRAD.0, ui::ORANGE_GRAD.1);
            c.sheen_round(r, 7, 1, ui::TILE_SHEEN);
        }
        let pct = format!("{}%", self.shown / 10);
        text::draw(c, Font::Strong, bar.x, bar.bottom() + 10, &pct, ui::text());
        let room = bar.w - text::width(Font::Strong, &pct) - 16;
        text::draw_right(c, Font::Small, bar.right(), bar.bottom() + 13, &text::fit(Font::Small, step, room), ui::text_dim());
    }

    /// The package's description and, below it, what else comes with it.
    fn draw_ready(&self, c: &mut Canvas, x: i32, mut y: i32, w: i32, p: &Prepared) {
        let i = &p.info;
        let summary = p.summary();
        for line in text::wrap(Font::Strong, &i.short, w).into_iter().take(2) {
            text::draw(c, Font::Strong, x, y, &line, ui::text());
            y += 22;
        }
        y += 4;
        // Less of the description when there is a list to show.
        let room = if summary.is_some() { 2 } else { 4 };
        let lines = text::wrap(Font::Small, &i.long, w);
        for (n, line) in lines.iter().take(room).enumerate() {
            let line = if n + 1 == room && lines.len() > room { text::fit(Font::Small, &format!("{line} …"), w) } else { line.clone() };
            text::draw(c, Font::Small, x, y, &line, ui::text_dim());
            y += 19;
        }
        if let Some(sum) = &summary {
            y += 8;
            text::draw(c, Font::Strong, x, y, &text::fit(Font::Strong, sum, w), ui::ORANGE);
            y += 22;
            let names = text::wrap(Font::Small, &p.names(), w);
            for (n, line) in names.iter().take(2).enumerate() {
                let line = if n == 1 && names.len() > 2 { text::fit(Font::Small, &format!("{line} …"), w) } else { line.clone() };
                text::draw(c, Font::Small, x, y, &line, ui::text_dim());
                y += 19;
            }
        }
        y += 6;
        let note = match &p.existing {
            Some(old) if version_cmp(&i.version, old) == Ordering::Equal => String::from("È già installato, nella stessa versione."),
            Some(old) => format!("Ora c'è la versione {old}: verrà sostituita."),
            // Where the downloads come from, and why they can be trusted.
            None if p.downloads() => String::from("Dall'archivio di Debian: indice firmato, file controllati."),
            None => return,
        };
        let color = if p.existing.is_some() { ui::ORANGE } else { ui::text_faint() };
        text::draw(c, Font::Small, x, y, &text::fit(Font::Small, &note, w), color);
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
        (560, 360)
    }

    fn animating(&self) -> bool {
        matches!(self.h.lock().phase, Phase::Installing | Phase::Resolving)
    }

    fn frame(&mut self) -> bool {
        let now = crate::time::uptime_ms();
        let dt = now.saturating_sub(self.last).min(500);
        self.last = now;
        let target = self.h.lock().target;
        if target < self.shown {
            // A new phase started its bar over.
            self.shown = target;
            return true;
        }
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
        changed || matches!(p, Phase::Installing | Phase::Resolving)
    }

    fn draw(&mut self, c: &mut Canvas, area: Rect, _blur: &Canvas, _focused: bool) {
        let (phase, info, prepared, why, detail, step, program, command) = {
            let s = self.h.lock();
            (s.phase, s.info.clone(), s.prepared.clone(), s.why.clone(), s.detail.clone(), s.step.clone(), s.program.clone(), s.command.clone())
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
            Phase::Resolving => String::from("Cerco quello che gli serve…"),
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
                let kib = prepared.as_ref().map_or(i.kib, Prepared::installed_kib);
                let line = format!("Versione {} · occupa {}", i.version, size_text(kib));
                text::draw(c, Font::Small, x, y, &line, ui::text_faint());
                y += 22;
            }
        }
        match phase {
            Phase::Reading => {
                text::draw(c, Font::Small, x, y, "Un momento.", ui::text_dim());
            }
            Phase::Resolving => {
                let line = "Mancano dei pacchetti di cui ha bisogno: li cerco nell'archivio di Debian.";
                for line in text::wrap(Font::Small, line, w) {
                    text::draw(c, Font::Small, x, y, &line, ui::text_dim());
                    y += 20;
                }
                self.draw_bar(c, area, &step);
            }
            Phase::Ready => {
                if let Some(p) = &prepared {
                    self.draw_ready(c, x, y, w, p);
                }
            }
            Phase::Installing => {
                if let Some(sum) = prepared.as_ref().and_then(Prepared::summary) {
                    text::draw(c, Font::Small, x, y, &text::fit(Font::Small, &sum, w), ui::text_dim());
                }
                self.draw_bar(c, area, &step);
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
