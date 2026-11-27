//! Getting Wine's C: drive ready, shown by the dynamic bar.
//!
//! The first Windows program opened makes Wine fill its prefix (`$HOME/.wine`),
//! which takes a minute or more, and Wine would tell it with a grey dialog
//! of its own. The owner is to see justOS instead: the bar turns into the
//! preparation (see `view`).
//! So the preparation is a step of its own, before the program starts:
//! `wineboot --init` runs with no display to show anything on (Wine falls
//! back to its null driver: the dialog is made and never mapped) and with no
//! Mono or Gecko prompt (`WINEDLLOVERRIDES`); when it and Wine's server are
//! done the programs that waited start as usual, and find the prefix ready.
//!
//! The same step runs when the prefix is out of date (Wine's own files were
//! updated: Wine would show the same dialog). Which one is decided by
//! [`needed`], by Wine's own rule: no prefix, or the date of `wine.inf`
//! differs from the one in the prefix's `.update-timestamp`.
//!
//! The bar measures what is written to the system volume since the
//! preparation began, against what the last one wrote for this build of
//! Wine (kept in a file), or else against an estimate from the size of
//! Wine's own files. It never goes back; the last tenth is for Wine's server
//! settling.

use crate::fs;
use crate::sync::IrqLock;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Wine's files in the system: its loader, and the file that fills a prefix.
const WINE: &str = "sys:/usr/lib/wine/wine64";
const INF: &str = "sys:/usr/share/wine/wine.inf";
/// The Windows programs Wine ships (what a new prefix is made from).
const PE_DIRS: [&str; 2] = ["sys:/usr/lib/x86_64-linux-gnu/wine/x86_64-windows", "sys:/usr/lib/x86_64-linux-gnu/wine/i386-windows"];
/// What the last preparation wrote, for the bar: "<stamp> <bytes>".
const LEARNED: &str = "sys:/home/.cache/justos-winprep";


/// The longest a preparation may take before it is given up.
const GIVE_UP_MS: u64 = 25 * 60 * 1000;
/// How often the thread looks at what happened.
const POLL_MS: u64 = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    /// Being stopped and cleaned up.
    Cancelling,
    Failed,
}

struct State {
    phase: Phase,
    /// The prefix is being brought up to date, not made.
    update: bool,
    /// Programs waiting for it (what was opened).
    queue: Vec<String>,
    /// Programs to start now: the preparation ended.
    ready: Vec<String>,
    /// Why it failed.
    why: String,
    /// A window shows it (or is on its way).
    window: bool,
    /// Where the bar should be (0..=1000) and the time the work began.
    target: u32,
    began: u64,
    /// Wine's server is settling after `wineboot` ended.
    settling: bool,
    cancel: bool,
}

static STATE: IrqLock<State> = IrqLock::new(State {
    phase: Phase::Idle,
    update: false,
    queue: Vec::new(),
    ready: Vec::new(),
    why: String::new(),
    window: false,
    target: 0,
    began: 0,
    settling: false,
    cancel: false,
});

fn var(name: &str) -> Option<String> {
    crate::linux::extra_env().iter().find_map(|v| v.strip_prefix(name).and_then(|v| v.strip_prefix('=')).map(String::from))
}

/// Wine's prefix: `$WINEPREFIX`, or `$HOME/.wine`.
fn prefix() -> String {
    let dir = var("WINEPREFIX").unwrap_or_else(|| format!("{}/.wine", var("HOME").unwrap_or_else(|| String::from("/home"))));
    format!("sys:{dir}")
}

fn read_text(path: &str) -> Option<String> {
    let p = fs::abs(path).ok()?;
    fs::read_file(&p).ok().map(|b| String::from_utf8_lossy(&b).trim().into())
}

fn exists(path: &str) -> bool {
    fs::abs(path).is_ok_and(|p| fs::exists(&p))
}

/// What identifies this build of Wine's prefix contents: size and date of
/// its `wine.inf`.
fn stamp() -> Option<String> {
    let s = fs::stat(&fs::abs(INF).ok()?).ok()?;
    Some(format!("{}:{}", s.size, s.modified))
}

/// Seconds between 1970 (the date Wine keeps) and 2000 (the one the file
/// system keeps).
const EPOCH_2000: u64 = 946_684_800;

/// Whether the prefix has to be made (`Some(true)`) or brought up to date
/// (`Some(false)`) before a Windows program can start: what Wine would
/// show its dialog for. Wine's own rule: no prefix, or the date in the
/// prefix's `.update-timestamp` is not that of `wine.inf`.
pub fn needed() -> Option<bool> {
    let dir = prefix();
    if !exists(&format!("{dir}/system.reg")) {
        return Some(true);
    }
    let inf = fs::stat(&fs::abs(INF).ok()?).ok()?;
    let have = read_text(&format!("{dir}/.update-timestamp")).and_then(|t| t.parse::<u64>().ok());
    (have != Some(inf.modified + EPOCH_2000)).then_some(false)
}
/// A program to open once Wine is ready: starts the preparation if it is
/// not running. Whether a window must be opened for it (false: one is open
/// already, or the preparation cannot take it).
pub fn request(program: &str) -> bool {
    let update = needed() == Some(false);
    let mut st = STATE.lock();
    if st.phase == Phase::Cancelling {
        return false;
    }
    if !st.queue.iter().any(|q| q == program) {
        st.queue.push(String::from(program));
    }
    let start = matches!(st.phase, Phase::Idle | Phase::Failed);
    let open = !st.window;
    st.window = true;
    if start {
        st.update = update;
        st.phase = Phase::Running;
        st.target = 0;
        st.settling = false;
        st.cancel = false;
        st.why.clear();
        st.began = crate::time::uptime_ms();
        drop(st);
        if crate::task::spawn("winprep", prepare).is_none() {
            fail("memoria insufficiente");
        }
    }
    open
}

/// Programs that waited for the preparation and can start now.
pub fn take_ready() -> Vec<String> {
    let mut st = STATE.lock();
    if st.ready.is_empty() {
        return Vec::new();
    }
    core::mem::take(&mut st.ready)
}

fn fail(why: &str) {
    crate::println!("winprep: preparation failed: {why}");
    let mut st = STATE.lock();
    st.phase = Phase::Failed;
    st.why = String::from(why);
}

/// Processes of Wine (its loader, server and the programs they run all
/// carry the loader's name).
fn wine_running() -> bool {
    crate::user::processes().iter().any(|p| p.name.starts_with("wine"))
}

/// Free bytes on the system volume.
fn free() -> u64 {
    fs::volume("sys").map_or(0, |v| v.space().1)
}

/// Bytes in the files of a folder.
fn folder_bytes(dir: &str) -> u64 {
    fs::abs(dir).ok().and_then(|p| fs::list_dir(&p).ok()).map_or(0, |l| l.iter().map(|(_, s)| s.size).sum())
}

/// What a preparation is expected to write: what the last one did for this
/// build of Wine, else a guess from the size of the programs Wine ships.
fn expected() -> u64 {
    if let (Some(s), Some(text)) = (stamp(), read_text(LEARNED)) {
        if let Some((key, bytes)) = text.split_once(' ') {
            if key == s {
                if let Ok(b) = bytes.trim().parse::<u64>() {
                    return b.max(1 << 20);
                }
            }
        }
    }
    let tree: u64 = PE_DIRS.iter().map(|d| folder_bytes(d)).sum();
    (tree * ESTIMATE_PERCENT / 100).max(64 << 20)
}

/// A new prefix takes about this much (in percent) of the size of the
/// Windows programs Wine ships (measured on Wine 10, its DLLs without
/// their debugging sections: 335 MiB for 283).
const ESTIMATE_PERCENT: u64 = 118;

/// The work of a preparation, on its own thread.
fn prepare() {
    let (update, began) = {
        let st = STATE.lock();
        (st.update, st.began)
    };
    let dir = prefix();
    let total = expected();
    let base = free();
    let env = alloc::vec![String::from("WAYLAND_DISPLAY="), String::from("XDG_RUNTIME_DIR="), String::from("WINEDLLOVERRIDES=mscoree,mshtml="), String::from("WINEDEBUG=-all")];
    let spawned = fs::abs(WINE)
        .map_err(|_| justdk::syscall::Error::NotFound)
        .and_then(|p| crate::user::spawn_env(&p, "wineboot --init", fs::abs("sys:/home").unwrap_or_else(|_| fs::Path::root("sys")), env));
    let pid = match spawned {
        Ok(pid) => pid,
        Err(e) => return fail(&format!("non parte: {}", e.as_str())),
    };
    crate::println!("winprep: {} the prefix {dir} (expecting {} MiB)", if update { "updating" } else { "making" }, total >> 20);
    let mut ended_at: Option<u64> = None;
    loop {
        crate::task::sleep_ms(POLL_MS);
        let now = crate::time::uptime_ms();
        if STATE.lock().cancel {
            crate::user::kill(pid);
            return cancelled(&dir, update);
        }
        if now.saturating_sub(began) > GIVE_UP_MS {
            crate::user::kill(pid);
            return fail("ci mette troppo");
        }
        let written = base.saturating_sub(free());
        // The files are the first nine tenths; a bigger prefix than
        // expected crawls up to 94 %.
        let frac = written * 1000 / total.max(1);
        let work = (if frac <= 1000 { frac * 900 / 1000 } else { 900 + 40 * (frac - 1000) / frac }) as u32;
        if crate::user::is_running(pid) {
            let mut st = STATE.lock();
            st.target = st.target.max(work);
            continue;
        }
        // `wineboot` is done; Wine's server goes a few seconds later.
        let since = *ended_at.get_or_insert(now);
        if !wine_running() || now.saturating_sub(since) > 90_000 {
            break;
        }
        let mut st = STATE.lock();
        st.settling = true;
        let creep = 900 + (now - since).min(30_000) as u32 / 400;
        st.target = st.target.max(work).max(creep).min(985);
    }
    let code = crate::user::wait(pid);
    if wine_running() {
        for p in crate::user::processes().iter().filter(|p| p.name.starts_with("wine")) {
            crate::user::kill(p.pid);
        }
    }
    let ok = exists(&format!("{dir}/system.reg")) && exists(&format!("{dir}/user.reg")) && exists(&format!("{dir}/userdef.reg"));
    if code != Some(0) || !ok {
        return fail(&format!("wineboot è finito con {code:?}, prefisso completo: {ok}"));
    }
    // Remember what it took, and that the prefix matches this Wine.
    let wrote = base.saturating_sub(free());
    if let Some(s) = stamp() {
        let write = |path: &str, text: &str| {
            if let Ok(p) = fs::abs(path) {
                let _ = fs::make_dir(&p.parent());

                let _ = fs::write_file(&p, text.as_bytes());
            }
        };
        write(LEARNED, &format!("{s} {wrote}"));
    }
    crate::println!("winprep: ready after {} s, {} MiB written", (crate::time::uptime_ms() - began) / 1000, wrote >> 20);
    let mut st = STATE.lock();
    st.target = 1000;
    st.ready = core::mem::take(&mut st.queue);
    st.phase = Phase::Idle;
    st.window = false;
    drop(st);
    crate::dev::input::wake_gui_soon();
}

/// Stopped by the user: what the preparation made is taken away, so that
/// the next try starts from nothing (Wine would take half a prefix for a
/// finished one).
fn cancelled(dir: &str, update: bool) {
    for _ in 0..40 {
        if !wine_running() {
            break;
        }
        for p in crate::user::processes().iter().filter(|p| p.name.starts_with("wine")) {
            crate::user::kill(p.pid);
        }
        crate::task::sleep_ms(250);
    }
    if let Ok(p) = fs::abs(dir) {
        if update {
            // An old prefix stays; only what says it is up to date goes.
            let _ = fs::remove(&p.join(".update-timestamp"));
        } else {
            let _ = fs::remove_all(&p);
        }
    }
    crate::println!("winprep: cancelled, the prefix {dir} was taken away");
    let mut st = STATE.lock();
    st.queue.clear();
    st.cancel = false;
    st.phase = Phase::Idle;
    st.window = false;
}

/// "Annulla" or closing the window: stops a running preparation.
fn cancel() {
    let mut st = STATE.lock();
    match st.phase {
        Phase::Running => {
            st.cancel = true;
            st.phase = Phase::Cancelling;
        }
        Phase::Failed => {
            st.queue.clear();
            st.phase = Phase::Idle;
            st.window = false;
        }
        _ => {}
    }
}

fn retry() {
    let first = STATE.lock().queue.first().cloned();
    if let Some(p) = first {
        STATE.lock().phase = Phase::Idle;
        request(&p);
    }
}

// ----------------------------------------------------------------------------
// What the bar shows

/// The preparation as the dynamic bar shows it.
pub struct View {
    pub update: bool,
    pub failed: bool,
    pub cancelling: bool,
    pub settling: bool,
    pub why: String,
    /// The first program waiting, and how many more.
    pub name: String,
    pub more: usize,
    /// Where the work is (0..=1000) and when it began.
    pub target: u32,
    pub began: u64,
}

/// None when nothing is being prepared.
pub fn view() -> Option<View> {
    let st = STATE.lock();
    if st.phase == Phase::Idle {
        return None;
    }
    let name = st.queue.first().map(|p| String::from(p.rsplit('/').next().unwrap_or(p))).unwrap_or_default();
    Some(View {
        update: st.update,
        failed: st.phase == Phase::Failed,
        cancelling: st.phase == Phase::Cancelling,
        settling: st.settling,
        why: st.why.clone(),
        name,
        more: st.queue.len().saturating_sub(1),
        target: st.target,
        began: st.began,
    })
}

/// "Annulla" (or "Chiudi" after a failure).
pub fn stop() {
    cancel();
}

/// "Riprova".
pub fn again() {
    retry();
}

/// "circa 2 minuti" for a time in seconds.
pub fn how_long(secs: u64) -> String {
    match secs {
        0..=44 => String::from("meno di un minuto"),
        45..=89 => String::from("circa 1 minuto"),
        s => format!("circa {} minuti", (s + 30) / 60),
    }
}
