//! Ports: messages between programs, and between programs and the kernel's
//! services.
//!
//! A port is a queue of messages owned by one process, which alone receives
//! from it. Others reach it through a handle: from `PORT_LOOKUP` for a named
//! port (`just.notify`), or handed to them with a message (the reply port of
//! a call). The kernel stamps every message with the sender's process id,
//! so a service knows who asks; programs cannot forge it.
//!
//! Ports are counted: a port with no name, nothing queued and nobody left
//! able to send to it is `Closed` for its receiver, which is how a caller
//! learns that a service ended without answering. When the owner ends, the
//! port goes away and sending to it fails with `Closed`.
//!
//! Every send and close wakes the one wait queue; receivers look again.
//! Waits are cut into short slices so a stopped program does not hang in
//! one.

use crate::sync::IrqLock;
use crate::task::WaitQueue;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use justdk::syscall::{self as sys, Error};

/// A queued message.
pub struct Message {
    pub data: Vec<u8>,
    /// Process id of the sender (0: the kernel).
    pub sender: u32,
    /// A port the answer goes to; the message holds a right to send there,
    /// which passes to whoever takes the message.
    pub reply: Option<u32>,
}

struct Port {
    owner: u32,
    name: Option<String>,
    queue: VecDeque<Message>,
    /// Handles (and queued messages) that may send here.
    senders: usize,
}

struct Table {
    ports: BTreeMap<u32, Port>,
    names: BTreeMap<String, u32>,
    next: u32,
}

static TABLE: IrqLock<Table> = IrqLock::new(Table { ports: BTreeMap::new(), names: BTreeMap::new(), next: 1 });
static CHANGED: WaitQueue = WaitQueue::new();
/// How long a wait sleeps before looking whether its program was stopped.
const SLICE_MS: u64 = 50;

/// Names are lowercase words joined by dots: `just.notify`, `org.example.chat`.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'))
}

/// Makes a port owned by `owner`, named or not.
pub fn create(owner: u32, name: Option<&str>) -> Result<u32, Error> {
    if name.is_some_and(|n| !valid_name(n)) {
        return Err(Error::InvalidName);
    }
    let mut t = TABLE.lock();
    if name.is_some_and(|n| t.names.contains_key(n)) {
        return Err(Error::Exists);
    }
    let id = t.next;
    t.next += 1;
    if let Some(n) = name {
        t.names.insert(String::from(n), id);
    }
    t.ports.insert(id, Port { owner, name: name.map(String::from), queue: VecDeque::new(), senders: 0 });
    Ok(id)
}

/// A right to send to a named port.
pub fn lookup(name: &str) -> Result<u32, Error> {
    let mut t = TABLE.lock();
    let id = *t.names.get(name).ok_or(Error::NotFound)?;
    if let Some(p) = t.ports.get_mut(&id) {
        p.senders += 1;
    }
    Ok(id)
}

/// A right to send was given up (a handle closed, a message dropped).
pub fn drop_sender(id: u32) {
    let mut t = TABLE.lock();
    if let Some(p) = t.ports.get_mut(&id) {
        p.senders = p.senders.saturating_sub(1);
    }
    drop(t);
    CHANGED.notify_all();
}

/// The owner is done with a port: it goes, with what was queued.
pub fn close(id: u32) {
    let port = {
        let mut t = TABLE.lock();
        let p = t.ports.remove(&id);
        if let Some(n) = p.as_ref().and_then(|p| p.name.as_ref()) {
            t.names.remove(n);
        }
        p
    };
    // Unanswered requests: their callers learn the port is gone.
    for m in port.into_iter().flat_map(|p| p.queue) {
        if let Some(r) = m.reply {
            drop_sender(r);
        }
    }
    CHANGED.notify_all();
}

/// Queues a message. The right to send to `reply` moves into the message.
pub fn send(id: u32, data: Vec<u8>, sender: u32, reply: Option<u32>) -> Result<(), Error> {
    if data.len() as u64 > sys::PORT_MSG_MAX {
        return Err(Error::TooLarge);
    }
    let mut t = TABLE.lock();
    let p = t.ports.get_mut(&id).ok_or(Error::Closed)?;
    if p.queue.len() >= sys::PORT_QUEUE_MAX {
        return Err(Error::Full);
    }
    p.queue.push_back(Message { data, sender, reply });
    drop(t);
    CHANGED.notify_all();
    Ok(())
}

/// Takes the next message of an own port, if there is one.
fn take(id: u32, owner: u32) -> Option<Result<Message, Error>> {
    let mut t = TABLE.lock();
    let Some(p) = t.ports.get_mut(&id) else { return Some(Err(Error::Closed)) };
    if p.owner != owner {
        return Some(Err(Error::BadHandle));
    }
    if let Some(m) = p.queue.pop_front() {
        return Some(Ok(m));
    }
    // Nobody can send any more.
    if p.name.is_none() && p.senders == 0 {
        return Some(Err(Error::Closed));
    }
    None
}

/// Waits for a message on an own port, for up to `timeout_ms` (`None`:
/// forever). `stop` is asked between slices whether to give up.
pub fn recv(id: u32, owner: u32, timeout_ms: Option<u64>, stop: &dyn Fn() -> bool) -> Result<Message, Error> {
    let until = timeout_ms.map(|ms| crate::time::uptime_ms().saturating_add(ms));
    loop {
        let left = until.map_or(SLICE_MS, |u| u.saturating_sub(crate::time::uptime_ms()).min(SLICE_MS));
        if let Some(r) = CHANGED.wait_for_timeout(left, || take(id, owner)) {
            return r;
        }
        if until.is_some_and(|u| crate::time::uptime_ms() >= u) {
            return Err(Error::Timeout);
        }
        if stop() {
            return Err(Error::Closed);
        }
    }
}

/// Sends a request and waits for its answer, on a private port made for it.
pub fn call(id: u32, data: Vec<u8>, sender: u32, timeout_ms: Option<u64>, stop: &dyn Fn() -> bool) -> Result<Vec<u8>, Error> {
    let reply = create(sender, None)?;
    TABLE.lock().ports.get_mut(&reply).expect("just made").senders = 1;
    if let Err(e) = send(id, data, sender, Some(reply)) {
        close(reply);
        return Err(e);
    }
    let r = recv(reply, sender, timeout_ms, stop);
    close(reply);
    r.map(|m| {
        // An answer that asks for an answer: not taken up.
        if let Some(r) = m.reply {
            drop_sender(r);
        }
        m.data
    })
}

/// The ports a process owns, for when it ends.
pub fn owned_by(pid: u32) -> Vec<u32> {
    TABLE.lock().ports.iter().filter(|(_, p)| p.owner == pid).map(|(&id, _)| id).collect()
}

/// Named ports and who owns them, for the shell.
pub fn list() -> Vec<(String, u32, usize)> {
    let t = TABLE.lock();
    t.names.iter().filter_map(|(n, id)| t.ports.get(id).map(|p| (n.clone(), p.owner, p.queue.len()))).collect()
}

// ----------------------------------------------------------------------------
// Services of the kernel

/// Starts the kernel's own services.
pub fn start_services() {
    if crate::task::spawn("just.notify", notify_service).is_none() {
        crate::warn!("ipc", "could not start just.notify");
    }
    if crate::task::spawn("just.permissions", permissions_service).is_none() {
        crate::warn!("ipc", "could not start just.permissions");
    }
    if crate::task::spawn("just.bluetooth", bluetooth_service).is_none() {
        crate::warn!("ipc", "could not start just.bluetooth");
    }
    if crate::task::spawn("just.net", net_service).is_none() {
        crate::warn!("ipc", "could not start just.net");
    }
}

/// `just.bluetooth`, the Bluetooth stack ([`crate::bt`]) for apps:
///
/// - `{"state": true}`: `{"present", "name", "address", "on", "scanning",
///   "devices": [{"address", "name", "kind", "le", "paired",
///   "connected"}], "prompt": {"id", "address", "name", "kind":
///   "confirm" | "show" | "enter", "number"} | null}`
/// - `{"scan": seconds}`: `{"found": n}` when the scan is over
/// - `{"pair" | "connect" | "disconnect" | "forget": address}`: `{"ok":
///   true}` once done (pairing waits for the user's answers)
/// - `{"power": true | false}`
/// - `{"answer": id, "ok": bool, "passkey": n}`: answers the prompt
///
/// Errors come as `{"error": text}`. Apps in a sandbox need the
/// "bluetooth" permission (asked the first time). Every request runs in a
/// thread of its own: scans and pairings take a while.
fn bluetooth_service() {
    let port = match create(0, Some("just.bluetooth")) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("ipc", "just.bluetooth: {}", e.as_str());
            return;
        }
    };
    loop {
        let Ok(m) = recv(port, 0, None, &|| false) else { continue };
        let (to, sender, data) = (m.reply, m.sender, m.data);
        let started = crate::task::spawn("just.bluetooth.req", move || {
            if let Some(app) = crate::user::app_of(sender) {
                if !crate::sandbox::allowed(&app, sender, "bluetooth") {
                    send_text(to, "{\"error\":\"permesso negato\"}");
                    return;
                }
            }
            let v = justjson::parse_bytes(&data).unwrap_or(justjson::Value::Null);
            reply(to, &bluetooth_answer(&v));
        });
        if started.is_none() {
            send_text(m.reply, "{\"error\":\"occupato\"}");
        }
    }
}

fn bluetooth_answer(v: &justjson::Value) -> justjson::Value {
    use crate::bt;
    use justjson::Value;
    let ok = || Value::object(alloc::vec![("ok", Value::Bool(true))]);
    let error = |e: &str| Value::object(alloc::vec![("error", Value::string(e))]);
    let done = |r: Result<(), String>| match r {
        Ok(()) => ok(),
        Err(e) => error(&e),
    };
    let target = |key: &str| v.get(key).as_str().and_then(bt::find).and_then(|a| bt::parse_addr(&a));
    if v.get("state").as_bool() == Some(true) {
        let adapter = bt::adapter();
        let devices = bt::devices()
            .into_iter()
            .map(|d| {
                Value::object(alloc::vec![
                    ("address", Value::string(&d.addr)),
                    ("name", Value::string(&d.name)),
                    ("kind", Value::string(d.kind.name())),
                    ("le", Value::Bool(d.le)),
                    ("paired", Value::Bool(d.paired)),
                    ("connected", Value::Bool(d.connected)),
                ])
            })
            .collect();
        let prompt = bt::prompt().map_or(Value::Null, |p| {
            let (kind, n) = match p.kind {
                bt::PromptKind::Confirm(n) => ("confirm", n as f64),
                bt::PromptKind::Display(n) => ("show", n as f64),
                bt::PromptKind::Enter => ("enter", 0.0),
            };
            Value::object(alloc::vec![
                ("id", Value::Num(p.id as f64)),
                ("address", Value::string(&bt::addr_text(&p.addr))),
                ("name", Value::string(&p.name)),
                ("kind", Value::string(kind)),
                ("number", Value::Num(n)),
            ])
        });
        let (name, address, on) = adapter.clone().unwrap_or_default();
        return Value::object(alloc::vec![
            ("present", Value::Bool(adapter.is_some())),
            ("name", Value::string(&name)),
            ("address", Value::string(&address)),
            ("on", Value::Bool(on)),
            ("scanning", Value::Bool(bt::scanning())),
            ("devices", Value::Arr(devices)),
            ("prompt", prompt),
        ]);
    }
    if let Some(secs) = v.get("scan").as_i64() {
        return match bt::scan(secs.clamp(2, 30) as u32) {
            Ok(n) => Value::object(alloc::vec![("found", Value::Num(n as f64))]),
            Err(e) => error(&e),
        };
    }
    if let Some(on) = v.get("power").as_bool() {
        bt::set_powered(on);
        return ok();
    }
    if let Some(id) = v.get("answer").as_i64() {
        bt::answer(id as u32, v.get("ok").as_bool().unwrap_or(false), v.get("passkey").as_i64().unwrap_or(0) as u32);
        return ok();
    }
    if v.get("pair").as_str().is_some() {
        return match target("pair") {
            Some(a) => done(bt::pair(&bt::addr_text(&a))),
            None => error("dispositivo non trovato"),
        };
    }
    for key in ["connect", "disconnect", "forget"] {
        if v.get(key).as_str().is_none() {
            continue;
        }
        let Some(a) = target(key) else { return error("dispositivo non trovato") };
        return match key {
            "connect" => done(bt::connect(&a)),
            "disconnect" => {
                bt::disconnect(&a);
                ok()
            }
            _ => {
                bt::forget(&a);
                ok()
            }
        };
    }
    error("richiesta sconosciuta")
}

/// `just.net`, the network settings, for the system's programs (an app in a
/// sandbox gets `{"error": …}`):
///
/// - `{"wifi": "status"}`: `{"present", "state", "text", "ssid", "channel",
///   "rssi", "address"}`;
/// - `{"wifi": "scan", "wait": true}`: `{"networks": [{"ssid", "security",
///   "password", "rssi", "bars", "channel", "saved", "connected"}]}`
///   (without `wait`, what was heard lately);
/// - `{"wifi": "connect", "ssid": …, "password": …, "wait": true}`: joins
///   (a known network without the password), and remembers it once joined;
/// - `{"wifi": "disconnect"}`, `{"wifi": "forget", "ssid": …}`;
/// - `{"wifi": "saved"}`: the names of the known networks (never their
///   passwords, which stay in `sys:/system/wifi.json`).
fn net_service() {
    use justjson::Value;
    let port = match create(0, Some("just.net")) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("ipc", "just.net: {}", e.as_str());
            return;
        }
    };
    loop {
        let Ok(m) = recv(port, 0, None, &|| false) else { continue };
        let v = justjson::parse_bytes(&m.data).unwrap_or(Value::Null);
        if crate::user::app_of(m.sender).is_some() {
            reply(m.reply, &Value::object(alloc::vec![("error", Value::string("solo i programmi di sistema"))]));
            continue;
        }
        // Connecting and scanning take seconds: answered from a thread.
        let to = m.reply;
        let asked = crate::task::spawn_big("just.net.ask", 128 * 1024, move || reply(to, &crate::net::wifi::request(&v)));
        if asked.is_none() {
            reply(m.reply, &Value::object(alloc::vec![("error", Value::string("occupato"))]));
        }
    }
}

/// `just.permissions`, the sandbox's table ([`crate::sandbox`]):
///
/// - `{"request": "camera"}`: may the sending app use it? The first time
///   the user is asked in the popup, and the answer waits for theirs.
///   `{"granted": true}`; programs without a sandbox may always.
/// - `{"check": "files:Immagini"}`: `{"state": "allow" | "deny" | "ask"}`,
///   without asking.
///
/// And from programs without a sandbox only (the system's tools):
/// `{"list": true}` (every answer: `"allow"`, `"denied_once"` or
/// `"deny"`), `{"set": perm, "app": id, "allow": true | false | null}`,
/// `{"forget": id}`; `{"request": perm, "for": pid}` asks as the PulseAudio
/// server does for process `pid`, and `{"request": perm, "program":
/// "sys:/usr/bin/…"}` as that Linux program would; `{"pending": true}` says
/// which question the popup asks (`{"pending": {"app", "name", "perm"}}`
/// or null), and `{"answer": "allow" | "once" | "deny" | "dismiss"}`
/// answers it as its buttons do (for `sandbox test`).
///
/// A question waits in a thread of its own, so that the service answers
/// others meanwhile.
fn permissions_service() {
    use justjson::Value;
    let port = match create(0, Some("just.permissions")) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("ipc", "just.permissions: {}", e.as_str());
            return;
        }
    };
    loop {
        let Ok(m) = recv(port, 0, None, &|| false) else { continue };
        let v = justjson::parse_bytes(&m.data).unwrap_or(Value::Null);
        match permissions_answer(&v, m.sender) {
            Ok(answer) => reply(m.reply, &answer),
            Err((app, pid, perm)) => {
                // The user is asked: the answer comes from another thread.
                let to = m.reply;
                let asked = crate::task::spawn("just.permissions.ask", move || {
                    let granted = crate::sandbox::allowed(&app, pid, &perm);
                    reply(to, &Value::object(alloc::vec![("granted", Value::Bool(granted))]));
                });
                if asked.is_none() {
                    reply(m.reply, &Value::object(alloc::vec![("granted", Value::Bool(false))]));
                }
            }
        }
    }
}

/// Sends `answer` back on reply port `to`, if there is one.
fn reply(to: Option<u32>, answer: &justjson::Value) {
    if let Some(r) = to {
        let _ = send(r, alloc::format!("{answer}").into_bytes(), 0, None);
        drop_sender(r);
    }
}

/// What `just.permissions` answers to request `v` from process `sender`
/// at once; `Err` with the app, the asking process and the permission when
/// the user has to be asked first.
fn permissions_answer(v: &justjson::Value, sender: u32) -> Result<justjson::Value, (crate::sandbox::App, u32, String)> {
    use crate::sandbox::{self, Answer, Choice};
    use justjson::Value;
    let error = |e: &str| Value::object(alloc::vec![("error", Value::string(e))]);
    let system = crate::user::app_of(sender).is_none();
    if let Some(perm) = v.get("request").as_str() {
        if !sandbox::known(perm) {
            return Ok(error("permesso sconosciuto"));
        }
        // On behalf of another process, or as a Linux program: only for
        // the system's tools.
        let (app, pid) = match (v.get("for").as_i64().filter(|p| *p > 0), v.get("program").as_str()) {
            (Some(pid), _) if system => (sandbox::who(pid as u32), pid as u32),
            (_, Some(program)) if system => (Some(sandbox::linux_app(program, None)), sender),
            (None, None) => (sandbox::who(sender), sender),
            _ => return Ok(error("solo i programmi di sistema")),
        };
        let Some(app) = app else { return Ok(Value::object(alloc::vec![("granted", Value::Bool(true))])) };
        return match sandbox::settled(&app, pid, perm) {
            Some(granted) => Ok(Value::object(alloc::vec![("granted", Value::Bool(granted))])),
            None => Err((app, pid, String::from(perm))),
        };
    }
    if let Some(perm) = v.get("check").as_str() {
        if !sandbox::known(perm) {
            return Ok(error("permesso sconosciuto"));
        }
        let state = match sandbox::who(sender).map(|a| sandbox::settled(&a, sender, perm)) {
            None | Some(Some(true)) => "allow",
            Some(Some(false)) => "deny",
            Some(None) => "ask",
        };
        return Ok(Value::object(alloc::vec![("state", Value::string(state))]));
    }
    if !system {
        return Ok(error("solo i programmi di sistema"));
    }
    if v.get("list").as_bool() == Some(true) {
        let apps = sandbox::all()
            .into_iter()
            .map(|g| {
                let perms = g.perms.iter().map(|(p, a)| (p.as_str(), Value::string(a.key()))).collect();
                Value::object(alloc::vec![("id", Value::string(&g.id)), ("name", Value::string(&g.name)), ("perms", Value::object(perms))])
            })
            .collect();
        return Ok(Value::object(alloc::vec![("apps", Value::Arr(apps))]));
    }
    if let Some(perm) = v.get("set").as_str() {
        let id = v.get("app").str();
        if !sandbox::valid_id(id) || !sandbox::known(perm) {
            return Ok(error("app o permesso non validi"));
        }
        let answer = v.get("allow").as_bool().map(|a| if a { Answer::Allow } else { Answer::Deny });
        sandbox::set(id, v.get("name").str(), perm, answer);
        return Ok(Value::object(alloc::vec![("ok", Value::Bool(true))]));
    }
    if let Some(id) = v.get("forget").as_str() {
        return Ok(Value::object(alloc::vec![("ok", Value::Bool(sandbox::forget(id)))]));
    }
    if v.get("pending").as_bool() == Some(true) {
        let pending = match crate::gui::permission_pending() {
            Some((id, name, perm)) => {
                Value::object(alloc::vec![("app", Value::string(&id)), ("name", Value::string(&name)), ("perm", Value::string(&perm))])
            }
            None => Value::Null,
        };
        return Ok(Value::object(alloc::vec![("pending", pending)]));
    }
    if let Some(choice) = v.get("answer").as_str() {
        let choice = match choice {
            "allow" => Choice::Allow,
            "once" => Choice::Once,
            "deny" => Choice::Deny,
            _ => Choice::Dismiss,
        };
        return Ok(Value::object(alloc::vec![("ok", Value::Bool(crate::gui::answer_permission(choice)))]));
    }
    Ok(error("atteso {\"request\": …} o {\"check\": …}"))
}

/// `just.notify`: `{"title": "…", "text": "…"}` shows a notification on the
/// desktop, under the sending program's name if there is no title. Answers
/// `{"ok":true}`, or `{"ok":false,"error":"…"}`. An app needs the
/// permission "notifications": the first one it sends asks the user (in a
/// thread of its own, as the service goes on), and without it nothing shows.
fn notify_service() {
    let port = match create(0, Some("just.notify")) {
        Ok(p) => p,
        Err(e) => {
            crate::warn!("ipc", "just.notify: {}", e.as_str());
            return;
        }
    };
    loop {
        let Ok(m) = recv(port, 0, None, &|| false) else { continue };
        let Some(v) = justjson::parse_bytes(&m.data).filter(|v| v.get("text").as_str().is_some()) else {
            send_text(m.reply, "{\"ok\":false,\"error\":\"atteso {\\\"title\\\": …, \\\"text\\\": …}\"}");
            continue;
        };
        let app = crate::user::app_of(m.sender);
        let from = app.as_ref().map(|a| a.name.clone()).or_else(|| crate::user::process_name(m.sender)).unwrap_or_default();
        let title = String::from(v.get("title").as_str().filter(|t| !t.is_empty()).unwrap_or(&from));
        let text = String::from(v.get("text").str());
        match app {
            Some(app) if crate::sandbox::settled(&app, m.sender, "notifications").is_none() => {
                let (to, sender) = (m.reply, m.sender);
                let asked = crate::task::spawn("just.notify.ask", move || {
                    let ok = crate::sandbox::allowed(&app, sender, "notifications");
                    notify_if(ok, &from, sender, &title, &text, to);
                });
                if asked.is_none() {
                    send_text(m.reply, "{\"ok\":false,\"error\":\"permesso negato\"}");
                }
            }
            Some(app) => {
                let ok = crate::sandbox::settled(&app, m.sender, "notifications") == Some(true);
                notify_if(ok, &from, m.sender, &title, &text, m.reply);
            }
            None => notify_if(true, &from, m.sender, &title, &text, m.reply),
        }
    }
}

/// Shows the notification if `ok`, and answers on `to`.
fn notify_if(ok: bool, from: &str, sender: u32, title: &str, text: &str, to: Option<u32>) {
    if !ok {
        send_text(to, "{\"ok\":false,\"error\":\"permesso negato\"}");
        return;
    }
    crate::debug!("ipc", "notification from {} ({}): {title}: {text}", from, sender);
    crate::gui::notify(title, text, "window");
    send_text(to, "{\"ok\":true}");
}

fn send_text(to: Option<u32>, answer: &str) {
    if let Some(r) = to {
        let _ = send(r, answer.as_bytes().to_vec(), 0, None);
        drop_sender(r);
    }
}
