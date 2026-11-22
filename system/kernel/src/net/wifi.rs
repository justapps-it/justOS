//! Wi-Fi: a station ([`justwifi::sta::Station`]) on every radio
//! ([`crate::dev::wifi`]), each with a network interface of its own for the
//! IP stack. The `wifid` thread feeds the station the radio's frames and
//! carries out what it asks (send, tune, frames for IP, link up or down);
//! frames IP sends go through the station, which turns them into 802.11
//! and encrypts them.
//!
//! The networks joined are remembered, with their passwords, in
//! `sys:/system/wifi.json`: a system file, which the sandbox keeps from
//! apps (as it does the settings that hold the user's password). When no
//! network is joined, the station looks every half minute and joins the
//! strongest one it knows.

use crate::dev::net as nic;
use crate::dev::wifi as radio;
use crate::fs;
use crate::sync::Mutex;
use crate::task;
use crate::time;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use justjson::Value;
use justwifi::frame::Security;
use justwifi::sta::{Failure, Out, State, Station};

const SAVED: &str = "sys:/system/wifi.json";
/// How often to look for known networks while not joined (ms).
const RESCAN_MS: u64 = 30_000;

struct Dev {
    radio: u32,
    iface: u32,
    sta: Station,
    /// Join known networks by itself (not after the user disconnected).
    auto: bool,
    /// When to look for known networks next.
    next_scan: u64,
    /// A scan the station is making to join a known network.
    auto_scan: bool,
    /// The connection being made was asked by the user (to save it once up).
    asked: Option<(Vec<u8>, Option<Vec<u8>>)>,
    /// Known networks whose password was refused: not tried by themselves
    /// again until the user gives one.
    refused: Vec<Vec<u8>>,
}

static DEVS: Mutex<Vec<Dev>> = Mutex::new(Vec::new());
static WAKE: task::Event = task::Event::new();
static SAVED_LOCK: Mutex<Option<Vec<Saved>>> = Mutex::new(None);

#[derive(Clone)]
pub struct Saved {
    pub ssid: Vec<u8>,
    pub password: Option<Vec<u8>>,
}

/// Something to do for the Wi-Fi thread (safe in interrupt handlers).
pub fn kick() {
    WAKE.signal();
}

pub fn start() {
    if task::spawn_big("wifid", 256 * 1024, run).is_none() {
        crate::warn!("wifi", "could not start the Wi-Fi thread");
    }
}

fn run() {
    loop {
        adopt();
        let now = time::uptime_ms();
        let mut due = now + 1000;
        {
            let mut devs = DEVS.lock();
            for d in devs.iter_mut() {
                let mut n = 0;
                while let Some((f, info)) = radio::poll(d.radio) {
                    d.sta.input(&f, info, time::uptime_ms());
                    n += 1;
                    if n % 16 == 0 {
                        carry_out(d);
                    }
                }
                if n > 0 {
                    crate::rand::mix(n);
                }
                let now = time::uptime_ms();
                d.sta.poll(now);
                carry_out(d);
                auto_join(d, now);
                carry_out(d);
                if let Some(t) = d.sta.deadline() {
                    due = due.min(t);
                }
                if d.auto && d.sta.status().state == State::Idle {
                    due = due.min(d.next_scan);
                }
            }
        }
        let now = time::uptime_ms();
        if due > now {
            WAKE.wait(Some(due - now));
        }
    }
}

/// A station for every radio that has none yet.
fn adopt() {
    let have: Vec<u32> = DEVS.lock().iter().map(|d| d.radio).collect();
    for r in radio::list() {
        if have.contains(&r.index) {
            continue;
        }
        let mut seed = [0u8; 32];
        crate::rand::fill(&mut seed);
        let sta = Station::new(r.mac, &seed, &r.channels);
        let iface = nic::register_kernel(r.device, r.mac, 1500, send_from_ip);
        crate::info!("wifi", "wifi{}: station on net{iface}", r.index);
        DEVS.lock().push(Dev {
            radio: r.index,
            iface,
            sta,
            auto: true,
            next_scan: 0,
            auto_scan: false,
            asked: None,
            refused: Vec::new(),
        });
    }
}

/// Radio `n` went away (its driver let go).
pub fn radio_gone(n: u32) {
    let mut devs = DEVS.lock();
    if let Some(at) = devs.iter().position(|d| d.radio == n) {
        let d = devs.remove(at);
        nic::set_link(d.iface, false);
    }
}

/// IP sends a frame on a Wi-Fi interface.
fn send_from_ip(iface: u32, frame: &[u8]) -> justdk::Status {
    let mut devs = DEVS.lock();
    let Some(d) = devs.iter_mut().find(|d| d.iface == iface) else { return justdk::E_INVAL };
    d.sta.send(frame);
    carry_out(d);
    justdk::OK
}

/// Does what the station asked.
fn carry_out(d: &mut Dev) {
    for o in d.sta.take() {
        match o {
            Out::Tx(f) => {
                radio::send(d.radio, &f);
            }
            Out::Channel(c) => {
                radio::set_channel(d.radio, c);
            }
            Out::Ether(e) => nic::receive(d.iface, &e),
            Out::Link(l) => {
                radio::set_link(d.radio, l.as_ref());
                nic::set_link(d.iface, l.is_some());
                if let Some(l) = &l {
                    let name = String::from_utf8_lossy(&l.ssid).into_owned();
                    crate::info!("wifi", "wifi{}: connected to \"{name}\" ({}), channel {}", d.radio, justwifi::sta::mac_text(&l.bssid), l.channel);
                    // Joined: remember it (with the password the user gave).
                    if let Some((ssid, pw)) = d.asked.take() {
                        if ssid == l.ssid {
                            remember(&ssid, pw.as_deref());
                        }
                    }
                    d.refused.retain(|s| *s != l.ssid);
                }
                crate::net::EVENTS.notify_all();
            }
            Out::Log(s) => crate::info!("wifi", "wifi{}: {s}", d.radio),
        }
    }
    let st = d.sta.status();
    if st.state == State::Idle && st.failure == Some(Failure::WrongPassword) && !st.ssid.is_empty() && !d.refused.contains(&st.ssid) {
        d.refused.push(st.ssid.clone());
        d.asked = None;
    }
}

/// Not joined: look for known networks now and then, and join the
/// strongest one heard.
fn auto_join(d: &mut Dev, now: u64) {
    let st = d.sta.status();
    if !d.auto || st.state != State::Idle || st.scanning {
        if !st.scanning && d.auto_scan {
            d.auto_scan = false;
        }
        return;
    }
    let known = saved();
    if known.is_empty() {
        return;
    }
    if d.auto_scan {
        // The scan is over: join the strongest known network.
        d.auto_scan = false;
        let nets = d.sta.networks();
        let pick = nets.iter().find_map(|b| known.iter().find(|k| k.ssid == b.ssid && !d.refused.contains(&k.ssid)));
        if let Some(k) = pick {
            crate::info!("wifi", "wifi{}: joining the known network \"{}\"", d.radio, String::from_utf8_lossy(&k.ssid));
            d.asked = None;
            d.sta.connect(&k.ssid, k.password.as_deref(), now);
        }
        d.next_scan = now + RESCAN_MS;
        return;
    }
    if now >= d.next_scan {
        d.auto_scan = true;
        d.next_scan = now + RESCAN_MS;
        d.sta.scan(now);
    }
}

// ----------------------------------------------------------------------------
// Saved networks

fn load() -> Vec<Saved> {
    let Ok(text) = fs::abs(SAVED).and_then(|p| fs::read_file(&p)) else { return Vec::new() };
    let v = justjson::parse_bytes(&text).unwrap_or(Value::Null);
    v.get("networks")
        .as_array()
        .iter()
        .filter_map(|n| {
            let ssid = n.get("ssid").as_str()?.as_bytes().to_vec();
            let password = n.get("password").as_str().map(|p| p.as_bytes().to_vec());
            Some(Saved { ssid, password })
        })
        .collect()
}

fn with_saved<T>(f: impl FnOnce(&mut Vec<Saved>) -> T) -> T {
    let mut g = SAVED_LOCK.lock();
    if g.is_none() {
        *g = Some(load());
    }
    f(g.as_mut().unwrap())
}

fn store(list: &[Saved]) {
    let nets = list
        .iter()
        .map(|s| {
            let mut fields = alloc::vec![("ssid", Value::string(&String::from_utf8_lossy(&s.ssid)))];
            if let Some(p) = &s.password {
                fields.push(("password", Value::string(&String::from_utf8_lossy(p))));
            }
            Value::object(fields)
        })
        .collect();
    let text = alloc::format!("{}\n", Value::object(alloc::vec![("networks", Value::Arr(nets))]));
    if let Err(e) = fs::abs(SAVED).and_then(|p| fs::write_file(&p, text.as_bytes())) {
        crate::warn!("wifi", "cannot save the Wi-Fi networks: {e:?}");
    }
}

/// The known networks, most recently joined first.
pub fn saved() -> Vec<Saved> {
    with_saved(|l| l.clone())
}

fn remember(ssid: &[u8], password: Option<&[u8]>) {
    with_saved(|l| {
        l.retain(|s| s.ssid != ssid);
        l.insert(0, Saved { ssid: ssid.to_vec(), password: password.map(|p| p.to_vec()) });
        store(l);
    });
}

/// Forgets a known network (and leaves it if joined). False if unknown.
pub fn forget(ssid: &str) -> bool {
    let known = with_saved(|l| {
        let before = l.len();
        l.retain(|s| s.ssid != ssid.as_bytes());
        let gone = l.len() != before;
        if gone {
            store(l);
        }
        gone
    });
    let mut devs = DEVS.lock();
    for d in devs.iter_mut() {
        if d.sta.status().ssid == ssid.as_bytes() && d.sta.status().state != State::Idle {
            d.sta.disconnect();
            carry_out(d);
        }
    }
    drop(devs);
    kick();
    known
}

// ----------------------------------------------------------------------------
// For the shell, the just.net service and the desktop

/// One network heard, as the lists show it.
#[derive(Clone)]
pub struct Network {
    pub ssid: Vec<u8>,
    pub name: String,
    pub security: Security,
    pub rssi: i8,
    pub bars: u8,
    pub channel: u8,
    pub saved: bool,
    pub connected: bool,
}

/// How the first radio's station is doing.
#[derive(Clone)]
pub struct Status {
    pub radio: u32,
    pub iface: u32,
    pub mac: [u8; 6],
    pub state: State,
    pub ssid: String,
    pub channel: u8,
    pub rssi: i8,
    pub security: Option<Security>,
    pub failure: Option<Failure>,
    pub scanning: bool,
}

impl Status {
    /// What the lists say of the connection, in Italian.
    pub fn text(&self) -> String {
        match self.state {
            State::Idle => match self.failure {
                Some(f) => f.text(),
                None => String::from("Non connessa"),
            },
            State::Searching => String::from("Ricerca della rete…"),
            State::Authenticating | State::Associating => String::from("Connessione…"),
            State::Handshake => String::from("Verifica della password…"),
            State::Connected => String::from("Connessa"),
        }
    }
}

/// Is there any Wi-Fi radio?
pub fn present() -> bool {
    !DEVS.lock().is_empty() || !radio::list().is_empty()
}

pub fn status() -> Option<Status> {
    let devs = DEVS.lock();
    let d = devs.first()?;
    let s = d.sta.status();
    Some(Status {
        radio: d.radio,
        iface: d.iface,
        mac: d.sta.mac,
        state: s.state,
        ssid: String::from_utf8_lossy(&s.ssid).into_owned(),
        channel: s.channel,
        rssi: s.rssi,
        security: s.security,
        failure: s.failure,
        scanning: s.scanning,
    })
}

/// The networks heard lately, strongest first.
pub fn networks() -> Vec<Network> {
    let known = saved();
    let devs = DEVS.lock();
    let Some(d) = devs.first() else { return Vec::new() };
    let st = d.sta.status();
    d.sta
        .networks()
        .into_iter()
        .map(|b| Network {
            name: b.name(),
            security: b.security(),
            rssi: b.rssi,
            bars: b.bars(),
            channel: b.channel,
            saved: known.iter().any(|k| k.ssid == b.ssid),
            connected: st.state == State::Connected && st.ssid == b.ssid,
            ssid: b.ssid,
        })
        .collect()
}

/// Starts a scan (the lists fill as it goes).
pub fn scan() -> bool {
    let mut devs = DEVS.lock();
    let Some(d) = devs.first_mut() else { return false };
    d.sta.scan(time::uptime_ms());
    carry_out(d);
    drop(devs);
    kick();
    true
}

/// Waits up to `ms` for the scan to end.
pub fn wait_scan(ms: u64) {
    let until = time::uptime_ms() + ms;
    task::sleep_ms(50);
    while time::uptime_ms() < until && status().is_some_and(|s| s.scanning) {
        task::sleep_ms(50);
    }
}

/// Joins network `ssid`; with no password given, a known network's is used.
pub fn connect(ssid: &str, password: Option<&str>) -> Result<(), &'static str> {
    let known = saved().into_iter().find(|s| s.ssid == ssid.as_bytes());
    let pw: Option<Vec<u8>> = match password {
        Some(p) => Some(p.as_bytes().to_vec()),
        None => known.and_then(|k| k.password),
    };
    if pw.as_ref().is_some_and(|p| p.len() > 128) {
        return Err("password too long");
    }
    let mut devs = DEVS.lock();
    let Some(d) = devs.first_mut() else { return Err("no Wi-Fi card") };
    d.auto = true;
    d.auto_scan = false;
    d.refused.retain(|s| s != ssid.as_bytes() || password.is_none());
    d.asked = Some((ssid.as_bytes().to_vec(), pw.clone()));
    d.sta.connect(ssid.as_bytes(), pw.as_deref(), time::uptime_ms());
    carry_out(d);
    drop(devs);
    kick();
    Ok(())
}

/// Waits up to `ms` for the connection being made: Ok once joined.
pub fn wait_connected(ms: u64) -> Result<(), Failure> {
    let until = time::uptime_ms() + ms;
    task::sleep_ms(50);
    loop {
        match status() {
            Some(s) if s.state == State::Connected => return Ok(()),
            Some(s) if s.state == State::Idle => return Err(s.failure.unwrap_or(Failure::NotFound)),
            None => return Err(Failure::NotFound),
            _ => {}
        }
        if time::uptime_ms() >= until {
            return Err(Failure::Timeout);
        }
        task::sleep_ms(50);
    }
}

/// Leaves the network, and stays off known networks until told to join one.
pub fn disconnect() {
    let mut devs = DEVS.lock();
    for d in devs.iter_mut() {
        d.auto = false;
        d.asked = None;
        d.sta.disconnect();
        carry_out(d);
        nic::set_link(d.iface, false);
    }
    drop(devs);
    kick();
}

/// Joins known networks by themselves again (after [`disconnect`]).
pub fn resume_auto() {
    for d in DEVS.lock().iter_mut() {
        d.auto = true;
        d.next_scan = 0;
    }
    kick();
}

/// The test radio over interface `iface` (`wifi sim net<N>`).
pub fn sim(iface: u32) -> Result<u32, &'static str> {
    let n = radio::add_sim(iface)?;
    adopt();
    Ok(n)
}

/// The English words for the shell.
pub fn failure_en(f: Failure) -> String {
    f.english()
}

/// `{"wifi": …}` requests of the `just.net` service (system programs only).
pub fn request(v: &Value) -> Value {
    let err = |e: &str| Value::object(alloc::vec![("error", Value::string(e))]);
    let ok = || Value::object(alloc::vec![("ok", Value::Bool(true))]);
    match v.get("wifi").str() {
        "status" => match status() {
            None => Value::object(alloc::vec![("present", Value::Bool(false))]),
            Some(s) => Value::object(alloc::vec![
                ("present", Value::Bool(true)),
                ("state", Value::string(&alloc::format!("{:?}", s.state).to_ascii_lowercase())),
                ("text", Value::string(&s.text())),
                ("ssid", Value::string(&s.ssid)),
                ("channel", Value::Num(s.channel as f64)),
                ("rssi", Value::Num(s.rssi as f64)),
                ("address", match crate::net::config(s.iface) {
                    Some((c, true)) => Value::string(&c.ip.to_string()),
                    _ => Value::Null,
                }),
            ]),
        },
        "scan" => {
            if v.get("wait").as_bool() == Some(true) {
                scan();
                wait_scan(8000);
            }
            let nets = networks()
                .iter()
                .map(|n| {
                    Value::object(alloc::vec![
                        ("ssid", Value::string(&n.name)),
                        ("security", Value::string(n.security.name())),
                        ("password", Value::Bool(n.security.needs_password())),
                        ("rssi", Value::Num(n.rssi as f64)),
                        ("bars", Value::Num(n.bars as f64)),
                        ("channel", Value::Num(n.channel as f64)),
                        ("saved", Value::Bool(n.saved)),
                        ("connected", Value::Bool(n.connected)),
                    ])
                })
                .collect();
            Value::object(alloc::vec![("networks", Value::Arr(nets))])
        }
        "connect" => {
            let ssid = v.get("ssid").str();
            if ssid.is_empty() || ssid.len() > 32 {
                return err("nome di rete non valido");
            }
            match connect(ssid, v.get("password").as_str()) {
                Ok(()) if v.get("wait").as_bool() == Some(true) => match wait_connected(20_000) {
                    Ok(()) => ok(),
                    Err(f) => err(&f.text()),
                },
                Ok(()) => ok(),
                Err(e) => err(e),
            }
        }
        "disconnect" => {
            disconnect();
            ok()
        }
        "forget" => Value::object(alloc::vec![("ok", Value::Bool(forget(v.get("ssid").str())))]),
        "saved" => {
            // Names only: the passwords stay in the system's file.
            let list = saved().iter().map(|s| Value::string(&String::from_utf8_lossy(&s.ssid))).collect();
            Value::object(alloc::vec![("saved", Value::Arr(list))])
        }
        _ => err("atteso {\"wifi\": \"status\" | \"scan\" | \"connect\" | \"disconnect\" | \"forget\" | \"saved\"}"),
    }
}
