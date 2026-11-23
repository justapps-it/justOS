//! Who an app is, and what it may touch.
//!
//! An installed justOS app (a `.app` in `sys:/apps` with its `.info`, or a
//! program that runs others' code and enters an app with `APP_ENTER`) runs
//! as its `app_id`, such as `dev.justos.disegno`, in a sandbox:
//!
//! - its own folder, `sys:/home/.apps/<app_id>`, is all its, and it starts
//!   there;
//! - it may read the installed programs and the system's fonts, pictures,
//!   sounds and certificates (not the settings, which hold the password);
//! - a folder of the home (Documenti, Immagini...) or another disk needs the
//!   user's permission;
//! - single files come from the system's file picker (`PICK_FILE`), which
//!   runs outside the app: what the user picks, the app may use, as picking
//!   it is the user's consent.
//!
//! Everything else is `Denied`. System programs (a `.app` without `.info`,
//! the shell, the desktop and its own apps: File, Impostazioni...) are part
//! of the system and have no sandbox; a program an app starts runs in the
//! app's. Devices (camera, microphone, location, network, notifications)
//! are asked for through the `just.permissions` port ([`crate::ipc`]), or
//! by the kernel's own services with [`device_allowed`].
//!
//! Permissions work as on a phone. An app has none until the user says so:
//! not at install, not because its manifest lists them (the manifest only
//! says what it may ask for, shown in the Negozio), not from the store. The
//! first time an app needs one, a popup over its window asks ("Consentire a
//! Disegno di usare la fotocamera?"), and the app waits for the answer:
//!
//! - **Consenti**: allowed for good;
//! - **Solo questa volta**: allowed until this run of the program ends;
//! - **Non consentire**: denied for the rest of this run; the next run asks
//!   once more, and a second "Non consentire" in a row is for good;
//! - closing the popup denies this run without counting as an answer.
//!
//! Answers for good are kept in `sys:/system/permissions.json`, and
//! Impostazioni, Privacy, changes them. Linux and Windows programs have no
//! sandbox for files, but their devices are asked for in the same way,
//! under the name and icon the desktop gives them ([`who`]): Firefox asks
//! before it may record from the microphone.

use crate::fs::{self, Path};
use crate::sync::{IrqLock, Mutex};
use crate::task::WaitQueue;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use justdk::syscall::Error;
use justjson::Value;

/// Where apps keep their own files.
pub const DATA: &str = "sys:/home/.apps";
const FILE: &str = "sys:/system/permissions.json";

/// What an app can ask for besides folders: its key, how the settings name
/// it, and what the question asks to do.
pub const DEVICES: [(&str, &str, &str); 6] = [
    ("camera", "Fotocamera", "usare la fotocamera"),
    ("microphone", "Microfono", "usare il microfono"),
    ("location", "Posizione", "conoscere la tua posizione"),
    ("network", "Rete", "usare la rete"),
    ("bluetooth", "Bluetooth", "usare il Bluetooth"),
    ("notifications", "Notifiche", "inviarti notifiche"),
];

/// What every app may read (paths in `sys:`, lowercase).
const SHARED: [&str; 7] = ["/apps", "/system/fonts.bin", "/system/fonts@2x.bin", "/system/certs.bin", "/system/gui", "/system/sounds", "/etc/ssl"];

/// An app, as the sandbox knows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct App {
    pub id: String,
    /// Its name for people ("Disegno").
    pub name: String,
    /// Its icon (an icon name, or "@file").
    pub icon: String,
}

/// Is `id` a good app id: reverse-DNS (`org.example.app`; letters, digits,
/// `-` and `_`), outside `just.`, which is the system's? Ids are kept in
/// lowercase, as file names do not mind case.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 128
        && parts.len() >= 2
        && !parts[0].eq_ignore_ascii_case("just")
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// `name` as the last part of an app id: lowercase letters and digits, a
/// `-` for anything else ("Firefox ESR" is `firefox-esr`).
fn id_part(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    String::from(s.trim_end_matches('-'))
}

/// The id of a package that does not name one: `dev.justos.<name>`.
pub fn default_id(name: &str) -> String {
    let s = id_part(name);
    match s.bytes().next() {
        None => String::from("dev.justos.app"),
        Some(b) if b.is_ascii_digit() => format!("dev.justos.app-{s}"),
        Some(_) => format!("dev.justos.{s}"),
    }
}

/// The app a program file is: an installed justOS package in `sys:/apps`.
pub fn for_program(path: &Path) -> Option<App> {
    if !path.volume.eq_ignore_ascii_case("sys") {
        return None;
    }
    let inner = path.inner.to_ascii_lowercase();
    let stem = inner.strip_prefix("/apps/")?.strip_suffix(".app")?;
    if stem.contains('/') {
        return None;
    }
    let p = crate::store::installed(stem)?;
    if p.platform != crate::store::Platform::Justos {
        return None;
    }
    let icon = crate::store::icon(stem).unwrap_or_else(|| String::from("window"));
    Some(App { id: p.app_id, name: p.name, icon })
}

/// Who process `pid` is for its permissions: the app it runs as, or for a
/// Linux or Windows program the app the desktop shows it as (Firefox, from
/// its .desktop file). None for the system's own programs, which may use
/// everything.
pub fn who(pid: u32) -> Option<App> {
    if let Some(app) = crate::user::app_of(pid) {
        return Some(app);
    }
    if !crate::user::is_linux(pid) {
        return None;
    }
    let run = crate::user::run_of(pid);
    let exe = crate::user::exe_of(run)?;
    Some(linux_app(&format!("{exe}"), crate::user::thread_name(run).as_deref()))
}

/// The app a Linux program file is (`exe`, "sys:/usr/lib/firefox-esr/
/// firefox-esr"): `linux.<file>`, named as its .desktop file names it. Wine
/// runs Windows programs, which are `windows.<program>`, by the name Wine
/// gives the thread (`thread`, "notepad.exe").
pub fn linux_app(exe: &str, thread: Option<&str>) -> App {
    let file = exe.rsplit('/').next().unwrap_or(exe);
    let windows = thread.filter(|t| file.to_ascii_lowercase().starts_with("wine") && t.to_ascii_lowercase().ends_with(".exe"));
    if let Some(program) = windows {
        let stem = &program[..program.len() - 4];
        let (name, icon) = crate::gui::program_face(exe, Some(program)).unwrap_or_else(|| (String::from(stem), String::from("window")));
        let part = id_part(stem);
        return App { id: format!("windows.{}", if part.is_empty() { "programma" } else { &part }), name, icon };
    }
    let (name, icon) = crate::gui::program_face(exe, None).unwrap_or_else(|| (String::from(file), String::from("window")));
    let part = id_part(file);
    App { id: format!("linux.{}", if part.is_empty() { "programma" } else { &part }), name, icon }
}

/// The app's own folder, made if it is not there yet.
pub fn prepare(app: &App) -> Option<Path> {
    let data = fs::abs(DATA).ok()?;
    let home = data.join(&app.id);
    for dir in [Path::parse("..", &data).ok()?, data, home.clone()] {
        if !fs::exists(&dir) {
            fs::make_dir(&dir).ok()?;
        }
    }
    Some(home)
}

enum Rule {
    Allow,
    Deny,
    /// Allowed if the user agrees to this permission.
    Ask(String),
}

/// `inner` is `base` or inside it.
fn under(inner: &str, base: &str) -> bool {
    inner == base || (inner.starts_with(base) && inner.as_bytes().get(base.len()) == Some(&b'/'))
}

fn same(a: &Path, b: &Path) -> bool {
    a.volume.eq_ignore_ascii_case(&b.volume) && a.inner.eq_ignore_ascii_case(&b.inner)
}

/// A home folder's name as it is on disk (names do not mind case).
fn folder_name(typed: &str) -> String {
    let home = Path::root("sys").join("home");
    fs::list_dir(&home)
        .ok()
        .and_then(|l| l.into_iter().map(|(e, _)| e.name).find(|n| n.eq_ignore_ascii_case(typed)))
        .unwrap_or_else(|| String::from(typed))
}

fn rule(app: &App, picked: &[(Path, bool)], path: &Path, write: bool) -> Rule {
    if picked.iter().any(|(p, w)| same(p, path) && (*w || !write)) {
        return Rule::Allow;
    }
    if !path.volume.eq_ignore_ascii_case("sys") {
        return Rule::Ask(format!("files:{}:", path.volume));
    }
    let inner = path.inner.to_ascii_lowercase();
    if under(&inner, &format!("/home/.apps/{}", app.id)) {
        return Rule::Allow;
    }
    if SHARED.iter().any(|b| under(&inner, b)) {
        return if write { Rule::Deny } else { Rule::Allow };
    }
    if let Some(rest) = inner.strip_prefix("/home/") {
        // A folder of the home and what is in it (lowercasing keeps the
        // lengths, so the name can be cut from the path as written).
        let len = rest.find('/').unwrap_or(rest.len());
        let folder = &path.inner["/home/".len().."/home/".len() + len];
        let file_in_home = len == rest.len() && fs::stat(path).is_ok_and(|s| s.kind != justfs::Kind::Dir);
        if !folder.is_empty() && !folder.starts_with('.') && !file_in_home {
            return Rule::Ask(format!("files:{}", folder_name(folder)));
        }
    }
    Rule::Deny
}

/// May the calling process touch `path` (to change it if `write`)? Asks
/// the user when the app needs a permission it was never given.
pub fn check(path: &Path, write: bool) -> Result<(), Error> {
    let Some((app, picked)) = crate::user::sandbox() else { return Ok(()) };
    match rule(&app, &picked, path, write) {
        Rule::Allow => Ok(()),
        Rule::Deny => Err(Error::Denied),
        Rule::Ask(perm) if allowed(&app, crate::task::current_pid(), &perm) => Ok(()),
        Rule::Ask(_) => Err(Error::Denied),
    }
}

/// May process `pid` use device `perm` ("microphone", "camera")? For the
/// kernel's services that hand devices to programs (the PulseAudio server
/// before it records): an app or a Linux or Windows program is asked about
/// in the popup the first time, and waits for the answer; the system's own
/// programs may. The PulseAudio server calls it before it records; nothing
/// hands a camera to Linux programs yet.
pub fn device_allowed(pid: u32, perm: &str) -> bool {
    match who(pid) {
        Some(app) => allowed(&app, pid, perm),
        None => true,
    }
}

/// How the settings name a permission: "Fotocamera", "Cartella Immagini",
/// "Disco ram:".
pub fn title(perm: &str) -> String {
    if let Some(f) = perm.strip_prefix("files:") {
        return match f.strip_suffix(':') {
            Some(v) => format!("Disco {v}:"),
            None => format!("Cartella {f}"),
        };
    }
    DEVICES.iter().find(|d| d.0 == perm).map_or_else(|| String::from(perm), |d| String::from(d.1))
}

/// What a permission lets an app do, as the question says it: "usare la
/// fotocamera", "accedere alla cartella Immagini".
pub fn describe(perm: &str) -> String {
    if let Some(f) = perm.strip_prefix("files:") {
        return match f.strip_suffix(':') {
            Some(v) => format!("accedere al disco {v}:"),
            None => format!("accedere alla cartella {f}"),
        };
    }
    DEVICES.iter().find(|d| d.0 == perm).map_or_else(|| String::from(perm), |d| String::from(d.2))
}

/// The popup's question: "Consentire a Disegno di usare la fotocamera?"
pub fn question(app: &str, perm: &str) -> String {
    format!("Consentire a {app} di {}?", describe(perm))
}

/// The line under the question: what saying yes means.
pub fn detail(perm: &str) -> &'static str {
    match perm {
        "camera" => "Potrà scattare foto e registrare video.",
        "microphone" => "Potrà registrare l'audio.",
        "location" => "Potrà sapere dove si trova il computer.",
        "network" => "Potrà collegarsi a Internet.",
        "bluetooth" => "Potrà cercare, associare e collegare dispositivi Bluetooth.",
        "notifications" => "Potrà mostrarti avvisi sulla scrivania.",
        _ => "Potrà leggere e cambiare i file che ci sono.",
    }
}

/// A permission there is: a device, or `files:<home folder>`, or
/// `files:<disk>:`.
pub fn known(perm: &str) -> bool {
    match perm.strip_prefix("files:") {
        Some(f) => !f.is_empty() && !f.contains('/') && !f.starts_with('.') && f.len() <= 64,
        None => DEVICES.iter().any(|d| d.0 == perm),
    }
}

// ----------------------------------------------------------------------------
// What the user said, per app

/// An answer kept for good.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// "Consenti", or on in Impostazioni.
    Allow,
    /// One "Non consentire": the next run of the app asks once more.
    DeniedOnce,
    /// Two "Non consentire" in a row, or off in Impostazioni: no more
    /// questions until it is changed there.
    Deny,
}

impl Answer {
    /// Its key in the table, and in `just.permissions`'s answers.
    pub fn key(self) -> &'static str {
        match self {
            Answer::Allow => "allow",
            Answer::DeniedOnce => "denied_once",
            Answer::Deny => "deny",
        }
    }
}

/// What the user chose in the popup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Allow,
    /// "Solo questa volta".
    Once,
    Deny,
    /// The popup was closed, or the program stopped: no answer.
    Dismiss,
}

#[derive(Clone)]
pub struct Grants {
    pub id: String,
    pub name: String,
    /// Permission and answer.
    pub perms: Vec<(String, Answer)>,
}

static GRANTS: IrqLock<Option<Vec<Grants>>> = IrqLock::new(None);
/// Writes of the table, one at a time.
static SAVING: Mutex<()> = Mutex::new(());

const KEYS: [Answer; 3] = [Answer::Allow, Answer::DeniedOnce, Answer::Deny];

fn load() -> Vec<Grants> {
    let Some(v) = fs::abs(FILE).and_then(|p| fs::read_file(&p)).ok().and_then(|b| justjson::parse_bytes(&b)) else {
        return Vec::new();
    };
    v.get("apps")
        .as_array()
        .iter()
        .filter(|a| valid_id(a.get("id").str()))
        .map(|a| {
            let mut perms: Vec<(String, Answer)> = Vec::new();
            for answer in KEYS {
                for p in a.get(answer.key()).as_array().iter().filter_map(Value::as_str).filter(|p| known(p)) {
                    if !perms.iter().any(|q| q.0 == p) {
                        perms.push((String::from(p), answer));
                    }
                }
            }
            Grants { id: a.get("id").str().to_ascii_lowercase(), name: a.get("name").str().to_string(), perms }
        })
        .collect()
}

fn with<T>(f: impl FnOnce(&mut Vec<Grants>) -> T) -> T {
    if GRANTS.lock().is_none() {
        // Read outside the lock: the file system may sleep.
        let loaded = load();
        let mut g = GRANTS.lock();
        if g.is_none() {
            *g = Some(loaded);
        }
    }
    let mut g = GRANTS.lock();
    f(g.get_or_insert_with(Vec::new))
}

fn save() {
    let _one = SAVING.lock();
    let apps: Vec<Value> = with(|g| {
        g.iter()
            .filter(|a| !a.perms.is_empty())
            .map(|a| {
                let list = |answer: Answer| Value::Arr(a.perms.iter().filter(|p| p.1 == answer).map(|p| Value::string(&p.0)).collect());
                let mut fields = alloc::vec![("id", Value::string(&a.id)), ("name", Value::string(&a.name))];
                fields.extend(KEYS.iter().map(|k| (k.key(), list(*k))));
                Value::object(fields)
            })
            .collect()
    });
    let text = format!("{}\n", Value::object(alloc::vec![("apps", Value::Arr(apps))]));
    if let Err(e) = fs::abs(FILE).and_then(|p| fs::write_file(&p, text.as_bytes())) {
        crate::warn!("sandbox", "cannot save the permissions: {e:?}");
    }
}

/// What the user answered for good about `perm` of app `id` (None: never
/// asked, or only for one run).
pub fn state(id: &str, perm: &str) -> Option<Answer> {
    with(|g| g.iter().find(|a| a.id.eq_ignore_ascii_case(id))?.perms.iter().find(|p| p.0 == perm).map(|p| p.1))
}

/// Records an answer (None forgets it: the app will ask again). What was
/// said for the run alone goes, so that a change in Impostazioni counts at
/// once.
pub fn set(id: &str, name: &str, perm: &str, answer: Option<Answer>) {
    let id = &id.to_ascii_lowercase();
    with(|g| {
        let i = match g.iter().position(|a| a.id == *id) {
            Some(i) => i,
            None => {
                g.push(Grants { id: String::from(id), name: String::from(name), perms: Vec::new() });
                g.len() - 1
            }
        };
        let a = &mut g[i];
        if !name.is_empty() {
            a.name = String::from(name);
        }
        a.perms.retain(|p| p.0 != perm);
        if let Some(answer) = answer {
            a.perms.push((String::from(perm), answer));
        }
    });
    RUNS.lock().retain(|r| !(r.id == *id && r.perm == perm));
    save();
}

/// Forgets every answer about an app.
pub fn forget(id: &str) -> bool {
    let had = with(|g| {
        let n = g.len();
        g.retain(|a| !a.id.eq_ignore_ascii_case(id));
        n != g.len()
    });
    RUNS.lock().retain(|r| !r.id.eq_ignore_ascii_case(id));
    save();
    had
}

/// Every app the user answered something about.
pub fn all() -> Vec<Grants> {
    with(|g| g.iter().filter(|a| !a.perms.is_empty()).cloned().collect())
}

// ----------------------------------------------------------------------------
// What the user said for one run

/// An answer that lasts as long as one run of a program: "Solo questa
/// volta", a first "Non consentire", a closed popup.
struct Run {
    id: String,
    perm: String,
    /// The process the run is counted from ([`crate::user::run_of`]).
    pid: u32,
    allow: bool,
}

static RUNS: IrqLock<Vec<Run>> = IrqLock::new(Vec::new());

/// What was said about `perm` of app `id` for the run that started with
/// process `pid`. Answers of runs that ended are dropped first.
fn run_answer(id: &str, perm: &str, pid: u32) -> Option<bool> {
    let pids: Vec<u32> = RUNS.lock().iter().map(|r| r.pid).collect();
    // Asked outside the lock: the process table has its own.
    let ended: Vec<u32> = pids.into_iter().filter(|p| !crate::user::alive(*p)).collect();
    let mut runs = RUNS.lock();
    runs.retain(|r| !ended.contains(&r.pid));
    runs.iter().find(|r| r.id == id && r.perm == perm && r.pid == pid).map(|r| r.allow)
}

fn remember_run(id: &str, perm: &str, pid: u32, allow: bool) {
    let mut runs = RUNS.lock();
    runs.retain(|r| !(r.id == id && r.perm == perm && r.pid == pid));
    // A few hundred at most: answers of runs that ended are dropped as
    // they are looked at.
    if runs.len() < 512 {
        runs.push(Run { id: String::from(id), perm: String::from(perm), pid, allow });
    }
}

/// The answer that holds for `perm` of `app` in the run of process `pid`
/// without asking, if there is one.
pub fn settled(app: &App, pid: u32, perm: &str) -> Option<bool> {
    match state(&app.id, perm) {
        Some(Answer::Allow) => Some(true),
        Some(Answer::Deny) => Some(false),
        _ => run_answer(&app.id, perm, crate::user::run_of(pid)),
    }
}

/// One popup at a time.
static ASKING: IrqLock<bool> = IrqLock::new(false);
static ASKED: WaitQueue = WaitQueue::new();

/// Does `app` have `perm`, for process `pid`? Asks the user in the popup
/// when nothing settles it yet, waits, and remembers.
pub fn allowed(app: &App, pid: u32, perm: &str) -> bool {
    if let Some(answer) = settled(app, pid, perm) {
        return answer;
    }
    loop {
        {
            let mut a = ASKING.lock();
            if !*a {
                *a = true;
                break;
            }
        }
        if crate::user::kill_requested() {
            return false;
        }
        ASKED.wait_for_timeout(100, || (!*ASKING.lock()).then_some(()));
    }
    // Asked meanwhile, by another program of the same app?
    let answer = match settled(app, pid, perm) {
        Some(a) => a,
        None => {
            let choice = ask(app, pid, perm);
            record(app, crate::user::run_of(pid), perm, choice)
        }
    };
    *ASKING.lock() = false;
    ASKED.notify_all();
    answer
}

/// Keeps what the user chose for `perm` of `app` in the run that started
/// with process `run`, and says whether the app may now.
fn record(app: &App, run: u32, perm: &str, choice: Choice) -> bool {
    match choice {
        Choice::Allow => {
            set(&app.id, &app.name, perm, Some(Answer::Allow));
            true
        }
        Choice::Once => {
            // Saying yes, even once, ends a row of "Non consentire".
            if state(&app.id, perm) == Some(Answer::DeniedOnce) {
                set(&app.id, &app.name, perm, None);
            }
            remember_run(&app.id, perm, run, true);
            true
        }
        Choice::Deny => {
            let next = if state(&app.id, perm) == Some(Answer::DeniedOnce) { Answer::Deny } else { Answer::DeniedOnce };
            set(&app.id, &app.name, perm, Some(next));
            remember_run(&app.id, perm, run, false);
            false
        }
        Choice::Dismiss => {
            remember_run(&app.id, perm, run, false);
            false
        }
    }
}

fn ask(app: &App, pid: u32, perm: &str) -> Choice {
    if crate::gui::active() {
        return crate::gui::ask_permission(app, perm, &crate::user::lineage(pid));
    }
    crate::println!(
        "\x1b[93m{} ({}) vuole {}: negato.\x1b[0m Per consentirlo: perms allow {} {perm}",
        app.name,
        app.id,
        describe(perm),
        app.id
    );
    Choice::Dismiss
}
