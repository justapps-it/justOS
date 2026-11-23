//! JustOS Connect: a phone on the same network as a companion of the
//! computer. Files go both ways, the phone is a trackpad and a keyboard,
//! watches the screen, and brings its notifications, battery and clipboard.
//!
//! Phones find the computer with a datagram to port 47800
//! (`JUSTOS-CONNECT?`, answered with `JUSTOS-CONNECT {json}`) and connect
//! to port 47801. Every message is a frame: a 32-bit big-endian length,
//! then the type (one byte) and its payload. The first two frames (HELLO,
//! CHALLENGE) carry P-256 public keys; from there on every frame is sealed
//! with AES-256-GCM, a key for each direction. A new phone pairs with a
//! six-digit code the computer shows; it gets a token, and proves it knows
//! it at every later connection (an HMAC of the keys exchanged).

pub mod screen;

use crate::dev::input::{self, Key, KeyEvent};
use crate::net::{self, tcp};
use crate::sync::{IrqLock, Mutex};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use justjson::Value;
use justtls::aes::Gcm;
use justtls::ec::{self, Curve};
use justtls::hash::{self, Alg};
use justtls::sha2::Sha256;

pub const DISCOVERY_PORT: u16 = 47800;
pub const PORT: u16 = 47801;
const VERSION: u32 = 1;
const CONFIG_PATH: &str = "sys:/system/connect";
const DOWNLOADS: &str = "sys:/home/Scaricati";
/// File data goes in pieces of this size.
const CHUNK: usize = 64 * 1024;
/// The largest frame accepted (a picture of the screen fits).
const MAX_FRAME: usize = 4 << 20;
/// A phone that says nothing for this long is gone (it pings every 10 s).
const SILENCE_MS: u64 = 45_000;

// Message types.
pub const HELLO: u8 = 0x01;
pub const CHALLENGE: u8 = 0x02;
pub const AUTH: u8 = 0x03;
pub const PAIR: u8 = 0x04;
pub const CODE: u8 = 0x05;
pub const WELCOME: u8 = 0x06;
pub const DENIED: u8 = 0x07;
pub const MOVE: u8 = 0x10;
pub const BUTTON: u8 = 0x11;
pub const SCROLL: u8 = 0x12;
pub const TEXT: u8 = 0x13;
pub const KEY: u8 = 0x14;
pub const TAP: u8 = 0x15;
pub const FILE_BEGIN: u8 = 0x20;
pub const FILE_DATA: u8 = 0x21;
pub const FILE_END: u8 = 0x22;
pub const FILE_DONE: u8 = 0x23;
pub const FILE_CANCEL: u8 = 0x24;
pub const SCREEN: u8 = 0x30;
pub const FRAME: u8 = 0x31;
pub const FRAME_ACK: u8 = 0x32;
pub const NOTIFY: u8 = 0x40;
pub const BATTERY: u8 = 0x41;
pub const CLIP: u8 = 0x42;
pub const PING: u8 = 0x50;
pub const PONG: u8 = 0x51;
pub const RING: u8 = 0x52;

// ---------------------------------------------------------------------------
// Settings: the computer's id, on or off, the phones paired.

struct Device {
    id: String,
    token: [u8; 32],
    name: String,
}

struct Config {
    pc_id: String,
    enabled: bool,
    devices: Vec<Device>,
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

fn load() -> Config {
    let mut c = Config { pc_id: String::new(), enabled: true, devices: Vec::new() };
    if let Ok(bytes) = crate::fs::abs(CONFIG_PATH).and_then(|p| crate::fs::read_file(&p)) {
        for line in String::from_utf8_lossy(&bytes).lines() {
            let mut parts = line.splitn(4, ' ');
            match parts.next() {
                Some("id") => c.pc_id = parts.next().unwrap_or("").into(),
                Some("off") => c.enabled = false,
                Some("device") => {
                    let (Some(id), Some(tok)) = (parts.next(), parts.next()) else { continue };
                    let Some(tok) = unhex(tok).filter(|t| t.len() == 32) else { continue };
                    let mut token = [0u8; 32];
                    token.copy_from_slice(&tok);
                    c.devices.push(Device { id: id.into(), token, name: parts.next().unwrap_or("").into() });
                }
                _ => {}
            }
        }
    }
    if c.pc_id.is_empty() {
        let mut b = [0u8; 8];
        crate::rand::fill(&mut b);
        c.pc_id = hex(&b);
        save(&c);
    }
    c
}

fn save(c: &Config) {
    let mut s = format!("id {}\n", c.pc_id);
    if !c.enabled {
        s.push_str("off\n");
    }
    for d in &c.devices {
        s.push_str(&format!("device {} {} {}\n", d.id, hex(&d.token), d.name));
    }
    if let Ok(p) = crate::fs::abs(CONFIG_PATH) {
        if let Err(e) = crate::fs::write_file(&p, s.as_bytes()) {
            crate::warn!("connect", "settings not saved: {e:?}");
        }
    }
}

fn with_config<T>(f: impl FnOnce(&mut Config) -> T) -> T {
    let mut g = CONFIG.lock();
    let c = g.get_or_insert_with(load);
    f(c)
}

// ---------------------------------------------------------------------------
// What the rest of the system sees and asks.

/// A phone connected now.
#[derive(Clone)]
pub struct Phone {
    pub sid: u32,
    pub name: String,
    pub addr: String,
    /// Battery percentage, or -1 before the phone said.
    pub battery: i32,
    pub charging: bool,
    pub watching: bool,
    /// A file on its way, either direction: (name, done, size, to the phone).
    pub transfer: Option<(String, u64, u64, bool)>,
}

enum Out {
    File(String),
    Ring,
}

static PHONES: IrqLock<Vec<Phone>> = IrqLock::new(Vec::new());
static OUTBOX: IrqLock<Vec<(u32, Out)>> = IrqLock::new(Vec::new());
/// The code shown for a phone pairing now: (code, phone's name, until).
static PAIRING: IrqLock<Option<(String, String, u64)>> = IrqLock::new(None);
static NEXT_SID: AtomicU32 = AtomicU32::new(1);
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    with_config(|c| c.enabled)
}

/// Turns Connect on or off (off ends the connections and stops answering).
pub fn set_enabled(on: bool) {
    with_config(|c| {
        c.enabled = on;
        save(c);
    });
}

pub fn phones() -> Vec<Phone> {
    PHONES.lock().clone()
}

/// The phones paired: (id, name).
pub fn paired() -> Vec<(String, String)> {
    with_config(|c| c.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect())
}

/// Forgets a paired phone (by id, or by name); whether there was one.
pub fn forget(who: &str) -> bool {
    with_config(|c| {
        let before = c.devices.len();
        c.devices.retain(|d| d.id != who && d.name != who);
        let gone = c.devices.len() != before;
        if gone {
            save(c);
        }
        gone
    })
}

/// The pairing code on the screen now, with the phone's name.
pub fn pairing_code() -> Option<(String, String)> {
    let p = PAIRING.lock();
    p.as_ref().filter(|p| p.2 > crate::time::uptime_ms()).map(|p| (p.0.clone(), p.1.clone()))
}

/// The phone a request goes to: the one given, or the only one connected.
fn target(sid: Option<u32>) -> Result<u32, &'static str> {
    let phones = PHONES.lock();
    match sid {
        Some(s) if phones.iter().any(|p| p.sid == s) => Ok(s),
        Some(_) => Err("telefono non connesso"),
        None => phones.first().map(|p| p.sid).ok_or("nessun telefono connesso"),
    }
}

/// Sends a file to a connected phone.
pub fn send_file(sid: Option<u32>, path: &str) -> Result<(), &'static str> {
    let p = crate::fs::abs(path).map_err(|_| "percorso non valido")?;
    if !crate::fs::exists(&p) || crate::fs::is_dir(&p) {
        return Err("file non trovato");
    }
    let s = target(sid)?;
    OUTBOX.lock().push((s, Out::File(path.into())));
    Ok(())
}

/// Makes a connected phone ring (to find it).
pub fn ring(sid: Option<u32>) -> Result<(), &'static str> {
    let s = target(sid)?;
    OUTBOX.lock().push((s, Out::Ring));
    Ok(())
}

fn update_phone(sid: u32, f: impl FnOnce(&mut Phone)) {
    if let Some(p) = PHONES.lock().iter_mut().find(|p| p.sid == sid) {
        f(p);
    }
}

// ---------------------------------------------------------------------------
// The service: discovery and connections.

/// Starts the service (once the network is up).
pub fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::task::spawn("connectd", serve);
}

fn serve() {
    let _ = with_config(|c| c.enabled);
    if let Err(e) = net::udp_bind(DISCOVERY_PORT) {
        crate::warn!("connect", "discovery port: {e}");
    }
    let listener = match tcp::listen(PORT) {
        Ok(l) => l,
        Err(e) => {
            crate::warn!("connect", "port {PORT}: {e}");
            return;
        }
    };
    loop {
        let on = enabled();
        while let Some((ip, port, data)) = net::udp_try_recv(DISCOVERY_PORT) {
            if on && data.starts_with(b"JUSTOS-CONNECT?") {
                let id = with_config(|c| c.pc_id.clone());
                let info = Value::object(vec![
                    ("id", Value::string(&id)),
                    ("name", Value::string(&crate::host::name())),
                    ("port", Value::Num(PORT as f64)),
                    ("ver", Value::Num(VERSION as f64)),
                ]);
                let reply = format!("JUSTOS-CONNECT {info}");
                let _ = net::send_udp(ip, DISCOVERY_PORT, port, reply.as_bytes());
            }
        }
        while let Some(conn) = tcp::try_accept(listener) {
            if !on {
                tcp::close(conn);
                continue;
            }
            let sid = NEXT_SID.fetch_add(1, Ordering::Relaxed);
            if crate::task::spawn("connect", move || session(sid, conn)).is_none() {
                tcp::close(conn);
            }
        }
        crate::task::sleep_ms(100);
    }
}

// ---------------------------------------------------------------------------
// Frames on a connection.

struct Link {
    conn: tcp::Conn,
    buf: Vec<u8>,
    eof: bool,
    seal: Option<Gcm>,
    open: Option<Gcm>,
    sent: u64,
    received: u64,
}

fn nonce(n: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    b
}

impl Link {
    fn send(&mut self, ty: u8, payload: &[u8]) -> Result<(), &'static str> {
        let mut body = Vec::with_capacity(payload.len() + 21);
        body.push(ty);
        body.extend_from_slice(payload);
        if let Some(k) = &self.seal {
            k.seal(&nonce(self.sent), &[], &mut body);
            self.sent += 1;
        }
        let mut out = Vec::with_capacity(body.len() + 4);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        tcp::send(self.conn, &out, 15_000).map(|_| ())
    }

    fn send_json(&mut self, ty: u8, v: Value) -> Result<(), &'static str> {
        self.send(ty, v.to_string().as_bytes())
    }

    /// A whole frame from what has come, waiting at most `wait_ms` for more.
    fn poll(&mut self, wait_ms: u64) -> Result<Option<(u8, Vec<u8>)>, &'static str> {
        if let Some(f) = self.take()? {
            return Ok(Some(f));
        }
        if self.eof {
            return Err("closed");
        }
        let mut tmp = [0u8; 16384];
        // Everything already there, then wait if nothing was.
        loop {
            match tcp::try_recv(self.conn, &mut tmp)? {
                tcp::Read::Data(n) => self.buf.extend_from_slice(&tmp[..n]),
                tcp::Read::Eof => {
                    self.eof = true;
                    break;
                }
                tcp::Read::Empty => break,
            }
            if self.buf.len() > MAX_FRAME + 64 {
                break;
            }
        }
        if let Some(f) = self.take()? {
            return Ok(Some(f));
        }
        if self.eof {
            return Err("closed");
        }
        if wait_ms > 0 {
            match tcp::recv(self.conn, &mut tmp, wait_ms) {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err("timed out") => {}
                Err(e) => return Err(e),
            }
            return self.take();
        }
        Ok(None)
    }

    fn take(&mut self) -> Result<Option<(u8, Vec<u8>)>, &'static str> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len == 0 || len > MAX_FRAME + 32 {
            return Err("bad frame");
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let mut body: Vec<u8> = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        if let Some(k) = &self.open {
            if !k.open(&nonce(self.received), &[], &mut body) {
                return Err("bad seal");
            }
            self.received += 1;
        }
        if body.is_empty() {
            return Err("bad frame");
        }
        let ty = body.remove(0);
        Ok(Some((ty, body)))
    }

    /// The next frame, waiting at most `ms` for it.
    fn wait(&mut self, ms: u64) -> Result<(u8, Vec<u8>), &'static str> {
        let end = crate::time::uptime_ms() + ms;
        loop {
            let now = crate::time::uptime_ms();
            if now >= end {
                return Err("timed out");
            }
            if let Some(f) = self.poll((end - now).min(200))? {
                return Ok(f);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A session with one phone.

fn session(sid: u32, conn: tcp::Conn) {
    let addr = tcp::endpoints(conn).map(|(_, ip, _)| format!("{ip}")).unwrap_or_default();
    let mut link = Link { conn, buf: Vec::new(), eof: false, seal: None, open: None, sent: 0, received: 0 };
    let mut s = Session::new(sid);
    match handshake(&mut link, sid, &addr) {
        Ok((id, name)) => {
            crate::info!("connect", "{name} connected from {addr}");
            crate::gui::notify("JustOS Connect", &format!("{name} è connesso"), "connect");
            s.device = id;
            if let Err(e) = s.run(&mut link) {
                if e != "closed" {
                    crate::info!("connect", "{name}: {e}");
                }
            }
            crate::info!("connect", "{name} disconnected");
        }
        Err(e) => crate::info!("connect", "{addr}: {e}"),
    }
    s.finish();
    PHONES.lock().retain(|p| p.sid != sid);
    OUTBOX.lock().retain(|(o, _)| *o != sid);
    tcp::close(conn);
}

fn mac(key: &[u8], label: &[u8], th: &[u8]) -> Vec<u8> {
    let mut data = label.to_vec();
    data.extend_from_slice(th);
    hash::hmac(Alg::Sha256, key, &data)
}

/// Keys, then the phone proves it is paired (or pairs). The phone's id and
/// name.
fn handshake(link: &mut Link, sid: u32, addr: &str) -> Result<(String, String), &'static str> {
    let (ty, p) = link.wait(10_000)?;
    if ty != HELLO {
        return Err("no hello");
    }
    let hello = justjson::parse_bytes(&p).ok_or("bad hello")?;
    let id: String = hello.get("id").str().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect();
    let name: String = hello.get("name").str().chars().filter(|c| !c.is_control()).take(40).collect();
    let name = if name.trim().is_empty() { String::from("Telefono") } else { String::from(name.trim()) };
    let phone_key = unhex(hello.get("key").str()).filter(|k| k.len() == 65 && k[0] == 4).ok_or("bad key")?;
    if id.is_empty() {
        return Err("no id");
    }

    let mut random = [0u8; 32];
    crate::rand::fill(&mut random);
    let (k, pc_key) = ec::keypair(Curve::P256, &random);
    let shared = ec::shared(Curve::P256, &k, &phone_key).ok_or("bad key")?;
    let (pc_id, token) = with_config(|c| (c.pc_id.clone(), c.devices.iter().find(|d| d.id == id).map(|d| d.token)));
    link.send_json(
        CHALLENGE,
        Value::object(vec![
            ("id", Value::string(&pc_id)),
            ("name", Value::string(&crate::host::name())),
            ("key", Value::string(&hex(&pc_key))),
            ("known", Value::Bool(token.is_some())),
            ("ver", Value::Num(VERSION as f64)),
        ]),
    )?;
    let mut h = Sha256::new();
    h.update(&phone_key);
    h.update(&pc_key);
    let th = h.finish();
    let prk = hash::hkdf_extract(Alg::Sha256, b"justos-connect 1", &shared);
    let keys = hash::hkdf_expand(Alg::Sha256, &prk, &th, 64);
    link.open = Some(Gcm::new(&keys[..32]));
    link.seal = Some(Gcm::new(&keys[32..]));

    let (ty, p) = link.wait(60_000)?;
    let (token, fresh) = match ty {
        AUTH => {
            let token = token.ok_or("unknown phone")?;
            let v = justjson::parse_bytes(&p).ok_or("bad auth")?;
            let got = unhex(v.get("mac").str()).unwrap_or_default();
            if !justtls::ct_eq(&got, &mac(&token, b"auth", &th)) {
                let _ = link.send_json(DENIED, Value::object(vec![("reason", Value::string("auth"))]));
                return Err("wrong token");
            }
            (token, false)
        }
        PAIR => {
            let code = format!("{:06}", crate::rand::u64() % 1_000_000);
            *PAIRING.lock() = Some((code.clone(), name.clone(), crate::time::uptime_ms() + 120_000));
            crate::gui::notify("JustOS Connect", &format!("Codice per {name}: {code}"), "connect");
            crate::info!("connect", "pairing code for {name} ({addr}): {code}");
            let r = link.wait(120_000);
            *PAIRING.lock() = None;
            let (ty, p) = r?;
            let v = justjson::parse_bytes(&p).unwrap_or(Value::Null);
            let got = unhex(v.get("mac").str()).unwrap_or_default();
            if ty != CODE || !justtls::ct_eq(&got, &mac(code.as_bytes(), b"pair", &th)) {
                let _ = link.send_json(DENIED, Value::object(vec![("reason", Value::string("code"))]));
                return Err("wrong pairing code");
            }
            let mut token = [0u8; 32];
            crate::rand::fill(&mut token);
            with_config(|c| {
                c.devices.retain(|d| d.id != id);
                c.devices.push(Device { id: id.clone(), token, name: name.clone() });
                save(c);
            });
            crate::info!("connect", "paired with {name}");
            (token, true)
        }
        _ => return Err("no auth"),
    };
    let (_, _, w, hh) = screen::geometry();
    let mut welcome = vec![
        ("id", Value::string(&pc_id)),
        ("name", Value::string(&crate::host::name())),
        ("w", Value::Num(w as f64)),
        ("h", Value::Num(hh as f64)),
        ("gui", Value::Bool(crate::gui::active())),
        ("mac", Value::string(&hex(&mac(&token, b"pc", &th)))),
    ];
    if fresh {
        welcome.push(("token", Value::string(&hex(&token))));
    }
    link.send_json(WELCOME, Value::object(welcome))?;
    PHONES.lock().push(Phone {
        sid,
        name: name.clone(),
        addr: addr.into(),
        battery: -1,
        charging: false,
        watching: false,
        transfer: None,
    });
    Ok((id, name))
}

struct Incoming {
    path: String,
    name: String,
    size: u64,
    got: u64,
}

struct Outgoing {
    path: String,
    size: u64,
    sent: u64,
}

struct Session {
    sid: u32,
    device: String,
    watching: bool,
    quality: u32,
    seen: u64,
    awaiting_ack: bool,
    incoming: Option<Incoming>,
    outgoing: Option<Outgoing>,
    clip: String,
    clip_checked: u64,
    heard: u64,
    checked: u64,
}

impl Session {
    fn new(sid: u32) -> Session {
        Session {
            sid,
            device: String::new(),
            watching: false,
            quality: 70,
            seen: 0,
            awaiting_ack: false,
            incoming: None,
            outgoing: None,
            clip: crate::gui::clipboard(),
            clip_checked: 0,
            heard: crate::time::uptime_ms(),
            checked: 0,
        }
    }

    fn run(&mut self, link: &mut Link) -> Result<(), &'static str> {
        loop {
            let mut busy = false;
            for _ in 0..64 {
                match link.poll(0)? {
                    Some((ty, p)) => {
                        busy = true;
                        self.heard = crate::time::uptime_ms();
                        self.handle(link, ty, &p)?;
                    }
                    None => break,
                }
            }
            let now = crate::time::uptime_ms();
            if now - self.heard > SILENCE_MS {
                return Err("no answer");
            }
            if now - self.checked > 1000 {
                self.checked = now;
                let id = self.device.clone();
                if !with_config(|c| c.enabled && c.devices.iter().any(|d| d.id == id)) {
                    return Err("forgotten or turned off");
                }
            }
            // The screen, a picture once the last one was shown.
            if self.watching && !self.awaiting_ack {
                if let Some(f) = screen::newer(self.seen) {
                    let jpeg = screen::encode(&f, self.quality);
                    let mut payload = Vec::with_capacity(jpeg.len() + 8);
                    for v in [f.w as u16, f.h as u16, f.sw as u16, f.sh as u16] {
                        payload.extend_from_slice(&v.to_be_bytes());
                    }
                    payload.extend_from_slice(&jpeg);
                    link.send(FRAME, &payload)?;
                    self.seen = f.seq;
                    self.awaiting_ack = true;
                    busy = true;
                }
            }
            // Requests from the computer.
            let mine: Vec<Out> = {
                let mut q = OUTBOX.lock();
                let mut mine = Vec::new();
                let mut i = 0;
                while i < q.len() {
                    if q[i].0 == self.sid && (self.outgoing.is_none() || matches!(q[i].1, Out::Ring)) {
                        mine.push(q.remove(i).1);
                        if matches!(mine.last(), Some(Out::File(_))) {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                mine
            };
            for o in mine {
                match o {
                    Out::Ring => link.send(RING, &[])?,
                    Out::File(path) => self.begin_send(link, &path)?,
                }
            }
            if self.outgoing.is_some() {
                self.send_chunk(link)?;
                busy = true;
            }
            // The clipboard, when it changed here.
            if now - self.clip_checked > 500 {
                self.clip_checked = now;
                let c = crate::gui::clipboard();
                if c != self.clip && !c.is_empty() && c.len() < 64 * 1024 {
                    link.send(CLIP, c.as_bytes())?;
                    self.clip = c;
                }
            }
            if !busy {
                // Rest until the phone says something (or a moment passes).
                if let Some((ty, p)) = link.poll(if self.watching { 15 } else { 40 })? {
                    self.heard = crate::time::uptime_ms();
                    self.handle(link, ty, &p)?;
                }
            }
        }
    }

    fn handle(&mut self, link: &mut Link, ty: u8, p: &[u8]) -> Result<(), &'static str> {
        let i16_at = |i: usize| p.get(i..i + 2).map(|b| i16::from_be_bytes([b[0], b[1]]) as i32).unwrap_or(0);
        let u16_at = |i: usize| p.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as i32).unwrap_or(0);
        match ty {
            MOVE => move_pointer(i16_at(0), i16_at(2)),
            BUTTON if p.len() >= 2 && p[0] < 3 => {
                if crate::gui::active() {
                    input::driver_event(0, justdk::INPUT_BUTTON, p[0] as u32, p[1] as i32);
                }
            }
            SCROLL => {
                if crate::gui::active() {
                    input::driver_event(0, justdk::INPUT_REL, justdk::AXIS_WHEEL, i16_at(0));
                }
            }
            TAP => {
                let (x, y, w, h) = screen::geometry();
                if w > 0 && h > 0 {
                    let tx = (u16_at(0) as i64 * w as i64 / 65536) as i32;
                    let ty = (u16_at(2) as i64 * h as i64 / 65536) as i32;
                    move_pointer(tx - x, ty - y);
                }
            }
            TEXT => {
                for ch in String::from_utf8_lossy(p).chars() {
                    let key = match ch {
                        '\n' | '\r' => Key::Enter,
                        '\t' => Key::Tab,
                        c if c.is_control() => continue,
                        c => Key::Char(c),
                    };
                    input::remote_key(KeyEvent { key, ctrl: false, alt: false, shift: false });
                }
            }
            KEY if p.len() >= 2 => {
                let ch = core::str::from_utf8(&p[2..]).ok().and_then(|s| s.chars().next());
                if let Some(key) = key_of(p[0], ch) {
                    input::remote_key(KeyEvent { key, ctrl: p[1] & 1 != 0, alt: p[1] & 2 != 0, shift: p[1] & 4 != 0 });
                }
            }
            FILE_BEGIN => self.begin_receive(link, p)?,
            FILE_DATA => self.receive(link, p)?,
            FILE_END => self.end_receive(link)?,
            FILE_CANCEL => {
                if let Some(inc) = self.incoming.take() {
                    remove(&inc.path);
                }
                if self.outgoing.take().is_some() {
                    update_phone(self.sid, |ph| ph.transfer = None);
                }
            }
            FILE_DONE => {
                let v = justjson::parse_bytes(p).unwrap_or(Value::Null);
                let ok = v.get("ok").as_bool().unwrap_or(false);
                let name = v.get("name").str();
                let text = if ok { format!("{name} inviato") } else { format!("{name} non inviato") };
                crate::gui::notify("JustOS Connect", &text, "connect");
                crate::info!("connect", "{text}");
            }
            SCREEN => {
                let v = justjson::parse_bytes(p).unwrap_or(Value::Null);
                let on = v.get("on").as_bool().unwrap_or(false);
                self.quality = v.get("q").as_i64().unwrap_or(70).clamp(20, 95) as u32;
                if on && !self.watching {
                    screen::watch(v.get("w").as_i64().unwrap_or(1280).clamp(160, 4096) as u32);
                    self.awaiting_ack = false;
                    self.seen = 0;
                } else if on {
                    screen::refresh();
                } else if self.watching {
                    screen::unwatch();
                }
                self.watching = on;
                update_phone(self.sid, |ph| ph.watching = on);
            }
            FRAME_ACK => self.awaiting_ack = false,
            NOTIFY => {
                let v = justjson::parse_bytes(p).unwrap_or(Value::Null);
                let (app, title, text) = (v.get("app").str(), v.get("title").str(), v.get("text").str());
                let head = if app.is_empty() || app == title { String::from(title) } else { format!("{app} · {title}") };
                crate::gui::notify(&clip_text(&head, 80), &clip_text(text, 240), "connect");
            }
            BATTERY => {
                let v = justjson::parse_bytes(p).unwrap_or(Value::Null);
                let level = v.get("level").as_i64().unwrap_or(-1).clamp(-1, 100) as i32;
                let charging = v.get("charging").as_bool().unwrap_or(false);
                update_phone(self.sid, |ph| {
                    ph.battery = level;
                    ph.charging = charging;
                });
            }
            CLIP => {
                let text = String::from_utf8_lossy(p).into_owned();
                crate::gui::set_clipboard(&text);
                self.clip = text;
            }
            PING => link.send(PONG, &[])?,
            _ => {}
        }
        Ok(())
    }

    fn begin_receive(&mut self, link: &mut Link, p: &[u8]) -> Result<(), &'static str> {
        if let Some(old) = self.incoming.take() {
            remove(&old.path);
        }
        let v = justjson::parse_bytes(p).unwrap_or(Value::Null);
        let name = clean_name(v.get("name").str());
        let size = v.get("size").as_f64().unwrap_or(0.0).max(0.0) as u64;
        let Some(path) = free_path(&name) else {
            return link.send_json(FILE_DONE, done(&name, false));
        };
        let created = crate::fs::abs(&path).and_then(|p| crate::fs::write_file(&p, &[]));
        if created.is_err() {
            return link.send_json(FILE_DONE, done(&name, false));
        }
        let shown = path.rsplit('/').next().unwrap_or(&name).to_string();
        update_phone(self.sid, |ph| ph.transfer = Some((shown.clone(), 0, size, false)));
        self.incoming = Some(Incoming { path, name: shown, size, got: 0 });
        Ok(())
    }

    fn receive(&mut self, link: &mut Link, p: &[u8]) -> Result<(), &'static str> {
        let Some(inc) = self.incoming.as_mut() else { return Ok(()) };
        let ok = crate::fs::abs(&inc.path).and_then(|path| crate::fs::write_at(&path, inc.got, p)).is_ok();
        if !ok {
            let inc = self.incoming.take().unwrap();
            remove(&inc.path);
            update_phone(self.sid, |ph| ph.transfer = None);
            return link.send_json(FILE_DONE, done(&inc.name, false));
        }
        inc.got += p.len() as u64;
        let (got, size) = (inc.got, inc.size);
        update_phone(self.sid, |ph| {
            if let Some(t) = ph.transfer.as_mut() {
                t.1 = got;
                t.2 = size.max(got);
            }
        });
        Ok(())
    }

    fn end_receive(&mut self, link: &mut Link) -> Result<(), &'static str> {
        let Some(inc) = self.incoming.take() else { return Ok(()) };
        update_phone(self.sid, |ph| ph.transfer = None);
        let ok = inc.got == inc.size;
        if ok {
            crate::info!("connect", "received {} ({} bytes)", inc.path, inc.got);
            crate::gui::notify("JustOS Connect", &format!("Ricevuto {} in Scaricati", inc.name), "connect");
        } else {
            remove(&inc.path);
        }
        link.send_json(FILE_DONE, done(&inc.name, ok))
    }

    fn begin_send(&mut self, link: &mut Link, path: &str) -> Result<(), &'static str> {
        let size = crate::fs::abs(path).and_then(|p| crate::fs::stat(&p)).map(|s| s.size);
        let name = path.rsplit(['/', ':']).next().unwrap_or("file").to_string();
        let Ok(size) = size else {
            crate::gui::notify("JustOS Connect", &format!("{name} non trovato"), "connect");
            return Ok(());
        };
        link.send_json(FILE_BEGIN, Value::object(vec![("name", Value::string(&name)), ("size", Value::Num(size as f64))]))?;
        update_phone(self.sid, |ph| ph.transfer = Some((name.clone(), 0, size, true)));
        self.outgoing = Some(Outgoing { path: path.into(), size, sent: 0 });
        Ok(())
    }

    fn send_chunk(&mut self, link: &mut Link) -> Result<(), &'static str> {
        let Some(out) = self.outgoing.as_mut() else { return Ok(()) };
        if out.sent >= out.size {
            self.outgoing = None;
            update_phone(self.sid, |ph| ph.transfer = None);
            return link.send(FILE_END, &[]);
        }
        let n = ((out.size - out.sent) as usize).min(CHUNK);
        let mut buf = vec![0u8; n];
        let read = crate::fs::abs(&out.path).and_then(|p| crate::fs::read_at(&p, out.sent, &mut buf));
        match read {
            Ok(r) if r > 0 => {
                buf.truncate(r);
                out.sent += r as u64;
                let (sent, size) = (out.sent, out.size);
                link.send(FILE_DATA, &buf)?;
                update_phone(self.sid, |ph| {
                    if let Some(t) = ph.transfer.as_mut() {
                        t.1 = sent;
                        t.2 = size;
                    }
                });
                Ok(())
            }
            _ => {
                self.outgoing = None;
                update_phone(self.sid, |ph| ph.transfer = None);
                link.send(FILE_CANCEL, &[])
            }
        }
    }

    fn finish(&mut self) {
        if self.watching {
            screen::unwatch();
            self.watching = false;
        }
        if let Some(inc) = self.incoming.take() {
            remove(&inc.path);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers.

/// Moves the pointer by (`dx`, `dy`) units, as a mouse would.
fn move_pointer(dx: i32, dy: i32) {
    if !crate::gui::active() || (dx == 0 && dy == 0) {
        return;
    }
    // A tablet's position is in pixels; a mouse's movement in units.
    let k = match input::pointer() {
        Some(p) if p.absolute => crate::gui::scale(),
        _ => 1,
    };
    let (x, y, w, h) = screen::geometry();
    screen::moved_to((x + dx).clamp(0, (w - 1).max(0)), (y + dy).clamp(0, (h - 1).max(0)));
    if dx != 0 {
        input::driver_event(0, justdk::INPUT_REL, justdk::AXIS_X, dx * k);
    }
    if dy != 0 {
        input::driver_event(0, justdk::INPUT_REL, justdk::AXIS_Y, dy * k);
    }
}

/// The phone's key codes.
fn key_of(code: u8, ch: Option<char>) -> Option<Key> {
    Some(match code {
        0 => Key::Char(ch?),
        1 => Key::Enter,
        2 => Key::Backspace,
        3 => Key::Delete,
        4 => Key::Tab,
        5 => Key::Escape,
        6 => Key::Up,
        7 => Key::Down,
        8 => Key::Left,
        9 => Key::Right,
        10 => Key::Home,
        11 => Key::End,
        12 => Key::PageUp,
        13 => Key::PageDown,
        14 => Key::VolumeUp,
        15 => Key::VolumeDown,
        16 => Key::Mute,
        17 => Key::Overview,
        0x21..=0x2C => Key::F(code - 0x20),
        _ => return None,
    })
}

fn done(name: &str, ok: bool) -> Value {
    Value::object(vec![("name", Value::string(name)), ("ok", Value::Bool(ok))])
}

fn clip_text(s: &str, max: usize) -> String {
    let mut out: String = s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// A file name from the phone, safe to put in a folder.
fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':')).take(120).collect();
    let n = n.trim().trim_start_matches('.').trim();
    if n.is_empty() {
        String::from("file")
    } else {
        String::from(n)
    }
}

/// A path in Scaricati for `name` not taken yet: `name`, `name (2)`, ...
fn free_path(name: &str) -> Option<String> {
    let dir = crate::fs::abs(DOWNLOADS).ok()?;
    if !crate::fs::exists(&dir) {
        crate::fs::make_dir(&dir).ok()?;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..1000 {
        let candidate = if n == 1 { format!("{DOWNLOADS}/{name}") } else { format!("{DOWNLOADS}/{stem} ({n}){ext}") };
        if !crate::fs::abs(&candidate).map(|p| crate::fs::exists(&p)).unwrap_or(true) {
            return Some(candidate);
        }
    }
    None
}

fn remove(path: &str) {
    if let Ok(p) = crate::fs::abs(path) {
        let _ = crate::fs::remove(&p);
    }
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

