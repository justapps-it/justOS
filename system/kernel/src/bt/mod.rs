//! Bluetooth: the host stack.
//!
//! A driver (btusb) registers a controller with [`register`] and hands
//! every HCI event and ACL packet to [`receive`]; the stack sends commands
//! and data back through the driver's [`HciDesc`]. One controller is used
//! at a time, the first that is ready.
//!
//! Three threads per controller: `bt-rx` takes the packets the driver
//! queued (command completions go to whoever waits for them, ACL data to
//! L2CAP, which answers the other side and feeds HID reports to the input
//! system), `bt-events` handles the other events (it may send commands and
//! wait for them), and each procedure (a scan, a pairing, a connection)
//! runs in the thread that asked for it, waiting on [`WAKE`].
//!
//! What it does: BR/EDR inquiry, Secure Simple Pairing, L2CAP, SDP and
//! HID (keyboards, mice); LE scanning and connections, SMP pairing
//! (Secure Connections and legacy, Just Works, passkey and numeric
//! comparison), a GATT client and HID over GATT. Paired devices are kept
//! in `sys:/system/bluetooth.json`.

mod classic;
mod crypto;
mod gatt;
mod hid;
mod l2cap;
mod smp;

use crate::sync::{IrqLock, Mutex};
use crate::task::{Event, WaitQueue};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use justdk::{HciDesc, Status, E_GONE, HCI_ACL, HCI_COMMAND, HCI_EVENT, OK};
use justjson::Value;

/// A device address, least significant byte first as HCI carries it.
pub type Addr = [u8; 6];

const STORE: &str = "sys:/system/bluetooth.json";

pub(crate) const EV_INQUIRY_COMPLETE: u8 = 0x01;
pub(crate) const EV_CONN_COMPLETE: u8 = 0x03;
pub(crate) const EV_CONN_REQUEST: u8 = 0x04;
pub(crate) const EV_DISCONN_COMPLETE: u8 = 0x05;
pub(crate) const EV_AUTH_COMPLETE: u8 = 0x06;
pub(crate) const EV_REMOTE_NAME: u8 = 0x07;
pub(crate) const EV_ENCRYPT_CHANGE: u8 = 0x08;
const EV_COMMAND_COMPLETE: u8 = 0x0E;
const EV_COMMAND_STATUS: u8 = 0x0F;
pub(crate) const EV_LE_META: u8 = 0x3E;

/// What a device is, as far as input goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Keyboard,
    Mouse,
    Combo,
    Other,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Keyboard => "tastiera",
            Kind::Mouse => "mouse",
            Kind::Combo => "tastiera e mouse",
            Kind::Other => "dispositivo",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Kind::Keyboard => "keyboard",
            Kind::Mouse => "mouse",
            Kind::Combo => "combo",
            Kind::Other => "other",
        }
    }

    fn from_key(k: &str) -> Kind {
        match k {
            "keyboard" => Kind::Keyboard,
            "mouse" => Kind::Mouse,
            "combo" => Kind::Combo,
            _ => Kind::Other,
        }
    }

    /// From a BR/EDR class of device (peripheral, keyboard/pointing bits).
    pub(crate) fn from_class(cod: u32) -> Kind {
        if (cod >> 8) & 0x1F != 5 {
            return Kind::Other;
        }
        match (cod >> 6) & 3 {
            1 => Kind::Keyboard,
            2 => Kind::Mouse,
            3 => Kind::Combo,
            _ => Kind::Other,
        }
    }

    /// From an LE appearance.
    pub(crate) fn from_appearance(a: u16) -> Kind {
        match a {
            0x03C1 => Kind::Keyboard,
            0x03C2 => Kind::Mouse,
            0x03C0 | 0x03C4 => Kind::Combo,
            _ => Kind::Other,
        }
    }
}

/// The key of an LE bond.
#[derive(Clone, Copy, Default)]
pub(crate) struct Ltk {
    pub key: [u8; 16],
    pub ediv: u16,
    pub rand: [u8; 8],
}

/// A device seen in a scan or paired before.
#[derive(Clone)]
pub(crate) struct Device {
    pub addr: Addr,
    pub le: bool,
    /// LE address type: 0 public, 1 random.
    pub addr_type: u8,
    pub name: String,
    pub kind: Kind,
    pub hid: bool,
    pub rssi: i8,
    pub paired: bool,
    pub link_key: Option<[u8; 16]>,
    pub ltk: Option<Ltk>,
    /// The HID report descriptor (from SDP or the GATT report map).
    pub report_map: Vec<u8>,
    /// Page scan repetition mode and clock offset, from the inquiry.
    pub psrm: u8,
    pub clock: u16,
    /// Seen in the last scan.
    pub seen: bool,
}

impl Device {
    fn new(addr: Addr, le: bool) -> Device {
        Device {
            addr,
            le,
            addr_type: 0,
            name: String::new(),
            kind: Kind::Other,
            hid: false,
            rssi: 0,
            paired: false,
            link_key: None,
            ltk: None,
            report_map: Vec::new(),
            psrm: 1,
            clock: 0,
            seen: false,
        }
    }
}

/// An L2CAP channel on a BR/EDR link.
pub(crate) struct Chan {
    pub psm: u16,
    pub local: u16,
    pub remote: u16,
    /// Our configuration was accepted / theirs was answered.
    pub out_done: bool,
    pub in_done: bool,
    pub rx: VecDeque<Vec<u8>>,
}

/// A link to a device.
pub(crate) struct Conn {
    pub handle: u16,
    pub addr: Addr,
    pub le: bool,
    pub encrypted: bool,
    /// Authentication / encryption results, when they come.
    pub auth: Option<u8>,
    pub enc: Option<u8>,
    /// A packet being put together from ACL fragments.
    pub rx_buf: Vec<u8>,
    pub chans: Vec<Chan>,
    /// Answers to our signalling requests, ATT responses, SMP packets.
    pub sig: VecDeque<Vec<u8>>,
    pub att: VecDeque<Vec<u8>>,
    pub smp: VecDeque<Vec<u8>>,
    pub next_id: u8,
    pub next_cid: u16,
    /// Keyboard and mouse reports become input once this is set.
    pub hid: Option<hid::HidIn>,
    /// LE: report characteristic value handles and their report ids.
    pub reports: Vec<(u16, u8)>,
    /// IO capability of the other side (BR/EDR pairing).
    pub peer_io: u8,
    pub peer_auth: u8,
    pub encrypt_asked: bool,
}

impl Conn {
    pub fn new(handle: u16, addr: Addr, le: bool) -> Conn {
        Conn {
            handle,
            addr,
            le,
            encrypted: false,
            auth: None,
            enc: None,
            rx_buf: Vec::new(),
            chans: Vec::new(),
            sig: VecDeque::new(),
            att: VecDeque::new(),
            smp: VecDeque::new(),
            next_id: 1,
            next_cid: 0x0040,
            hid: None,
            reports: Vec::new(),
            peer_io: 3,
            peer_auth: 0,
            encrypt_asked: false,
        }
    }
}

/// What a pairing needs the user for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    /// Both screens show this number: the same?
    Confirm(u32),
    /// Type this number on the device (and Enter).
    Display(u32),
    /// Type the number the device shows.
    Enter,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub id: u32,
    pub addr: Addr,
    pub name: String,
    pub kind: PromptKind,
    /// Answered with an HCI command (BR/EDR) rather than by SMP.
    classic: bool,
}

pub(crate) struct State {
    pub addr: Addr,
    pub powered: bool,
    pub scanning: bool,
    pub inquiry_done: bool,
    pub devices: Vec<Device>,
    pub conns: Vec<Conn>,
    /// Connection attempts that failed: address and status.
    pub failed: Vec<(Addr, u8)>,
    pub prompt: Option<Prompt>,
    pub answer: Option<(u32, bool, u32)>,
    next_prompt: u32,
    /// The adapter in use (index in ADAPTERS).
    active: Option<usize>,
    pub acl_mtu: usize,
    pub le_mtu: usize,
}

pub(crate) static ST: IrqLock<State> = IrqLock::new(State {
    addr: [0; 6],
    powered: true,
    scanning: false,
    inquiry_done: false,
    devices: Vec::new(),
    conns: Vec::new(),
    failed: Vec::new(),
    prompt: None,
    answer: None,
    next_prompt: 1,
    active: None,
    acl_mtu: 27,
    le_mtu: 27,
});

/// Notified whenever the state changes, for procedures waiting on it.
pub(crate) static WAKE: WaitQueue = WaitQueue::new();

/// Waits up to `ms` for `f` to find what it wants in the state. `f` runs
/// with interrupts off: short, and never sending anything.
pub(crate) fn wait<T>(ms: u64, mut f: impl FnMut(&mut State) -> Option<T>) -> Option<T> {
    WAKE.wait_for_timeout(ms, || f(&mut ST.lock()))
}

pub(crate) fn changed() {
    WAKE.notify_all();
}

struct Adapter {
    device: u32,
    desc: *const HciDesc,
    name: String,
    gone: bool,
}

// SAFETY: the descriptor belongs to the driver, which serialises its sends.
unsafe impl Send for Adapter {}

static ADAPTERS: IrqLock<Vec<Adapter>> = IrqLock::new(Vec::new());

/// Packets from the drivers, for `bt-rx`: (adapter, kind, packet).
static RX: IrqLock<VecDeque<(usize, u32, Vec<u8>)>> = IrqLock::new(VecDeque::new());
static RX_WAKE: Event = Event::new();

/// Events and jobs for `bt-events`.
pub(crate) enum Job {
    Event(Vec<u8>),
    /// A command to send and wait for, its answer unused.
    Command(u16, Vec<u8>),
    /// LE: encrypt the link with the stored key.
    Encrypt(u16),
}
static JOBS: IrqLock<VecDeque<Job>> = IrqLock::new(VecDeque::new());
static JOBS_WAKE: Event = Event::new();

pub(crate) fn job(j: Job) {
    let mut q = JOBS.lock();
    if q.len() < 256 {
        q.push_back(j);
    }
    drop(q);
    JOBS_WAKE.signal();
}

/// Command completions: (opcode, command status?, parameters).
static DONE: IrqLock<VecDeque<(u16, bool, Vec<u8>)>> = IrqLock::new(VecDeque::new());
static DONE_WAKE: WaitQueue = WaitQueue::new();
/// One command at a time: controllers take few at once.
static CMD: Mutex<()> = Mutex::new(());
/// One procedure (scan, pairing, connection) at a time.
static BUSY: Mutex<()> = Mutex::new(());

// ----------------------------------------------------------------------------
// From the drivers

pub fn register(device: u32, desc: *const HciDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    let len = d.name.iter().position(|&b| b == 0).unwrap_or(d.name.len());
    let name = String::from_utf8_lossy(&d.name[..len]).into_owned();
    let n = {
        let mut a = ADAPTERS.lock();
        a.push(Adapter { device, desc, name: name.clone(), gone: false });
        a.len() - 1
    };
    static STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !STARTED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        if !crypto::selftest() {
            crate::error!("bt", "AES-CMAC self-test failed: LE pairing disabled");
        }
        load();
        if crate::task::spawn("bt-rx", rx_thread).is_none() || crate::task::spawn("bt-events", events_thread).is_none() {
            crate::error!("bt", "could not start the Bluetooth threads");
        }
    }
    let started = crate::task::spawn("bt-setup", move || setup(n));
    if started.is_none() {
        return justdk::E_NOMEM;
    }
    crate::info!("bt", "hci{n}: {name}");
    n as Status
}

pub fn receive(adapter: u32, kind: u32, data: &[u8]) {
    let mut q = RX.lock();
    if q.len() < 512 {
        q.push_back((adapter as usize, kind, data.to_vec()));
    }
    drop(q);
    RX_WAKE.signal();
}

/// The driver's device went away: its controller and links with it.
/// Called with the driver manager held: does not block.
pub fn remove_device(device: u32) {
    let mut gone = Vec::new();
    {
        let mut a = ADAPTERS.lock();
        for (i, ad) in a.iter_mut().enumerate() {
            if ad.device == device && !ad.gone {
                ad.gone = true;
                gone.push(i);
            }
        }
    }
    if gone.is_empty() {
        return;
    }
    let mut st = ST.lock();
    if st.active.is_some_and(|i| gone.contains(&i)) {
        st.active = None;
        st.conns.clear();
        st.scanning = false;
        st.prompt = None;
        crate::info!("bt", "controller removed");
    }
    drop(st);
    changed();
    DONE_WAKE.notify_all();
}

// ----------------------------------------------------------------------------
// Sending

fn active_desc() -> Option<*const HciDesc> {
    let i = ST.lock().active?;
    let a = ADAPTERS.lock();
    a.get(i).filter(|a| !a.gone).map(|a| a.desc)
}

fn send_to(desc: *const HciDesc, kind: u32, data: &[u8]) -> Status {
    // SAFETY: the driver keeps the descriptor (and its context) alive even
    // after its device went; a send then fails.
    let d = unsafe { &*desc };
    // SAFETY: as above; `data` is valid for its length.
    unsafe { (d.send)(d.ctx, kind, data.as_ptr(), data.len()) }
}

pub(crate) fn send_acl_raw(data: &[u8]) -> Status {
    match active_desc() {
        Some(d) => send_to(d, HCI_ACL, data),
        None => E_GONE,
    }
}

/// Status of a command that failed, or 0xFF when there was no answer.
pub(crate) const NO_ANSWER: u8 = 0xFF;

/// Sends a command on the active controller and waits for its completion
/// (or its status, for the commands that answer with one): the return
/// parameters, status first. Err: the status.
pub(crate) fn command(op: u16, params: &[u8]) -> Result<Vec<u8>, u8> {
    let Some(desc) = active_desc() else { return Err(NO_ANSWER) };
    command_on(desc, op, params)
}

fn command_on(desc: *const HciDesc, op: u16, params: &[u8]) -> Result<Vec<u8>, u8> {
    let _one = CMD.lock();
    DONE.lock().retain(|(o, _, _)| *o != op);
    let mut cmd = Vec::with_capacity(3 + params.len());
    cmd.extend_from_slice(&op.to_le_bytes());
    cmd.push(params.len() as u8);
    cmd.extend_from_slice(params);
    if send_to(desc, HCI_COMMAND, &cmd) != OK {
        return Err(NO_ANSWER);
    }
    let r = DONE_WAKE.wait_for_timeout(5000, || {
        let mut q = DONE.lock();
        let i = q.iter().position(|(o, _, _)| *o == op)?;
        q.remove(i)
    });
    match r {
        Some((_, _, p)) if p.first() == Some(&0) => Ok(p),
        Some((_, _, p)) => Err(p.first().copied().unwrap_or(NO_ANSWER)),
        None => {
            crate::warn!("bt", "command {op:04x}: no answer");
            Err(NO_ANSWER)
        }
    }
}

// ----------------------------------------------------------------------------
// Threads

fn rx_thread() {
    loop {
        RX_WAKE.wait(Some(20));
        loop {
            let Some((adapter, kind, p)) = RX.lock().pop_front() else { break };
            let mine = ST.lock().active == Some(adapter);
            match kind {
                HCI_EVENT if p.len() >= 2 => match p[0] {
                    EV_COMMAND_COMPLETE if p.len() >= 5 => {
                        let op = u16::from_le_bytes([p[3], p[4]]);
                        done(op, false, p[5..].to_vec());
                    }
                    EV_COMMAND_STATUS if p.len() >= 6 => {
                        let op = u16::from_le_bytes([p[4], p[5]]);
                        done(op, true, alloc::vec![p[2]]);
                    }
                    _ if mine => job(Job::Event(p)),
                    _ => {}
                },
                HCI_ACL if mine => l2cap::receive(&p),
                _ => {}
            }
        }
        hid::tick();
    }
}

fn done(op: u16, status: bool, params: Vec<u8>) {
    let mut q = DONE.lock();
    if q.len() >= 32 {
        q.pop_front();
    }
    q.push_back((op, status, params));
    drop(q);
    DONE_WAKE.notify_all();
}

fn events_thread() {
    loop {
        JOBS_WAKE.wait(Some(1000));
        loop {
            let Some(j) = JOBS.lock().pop_front() else { break };
            match j {
                Job::Event(ev) => event(&ev),
                Job::Command(op, params) => {
                    let _ = command(op, &params);
                }
                Job::Encrypt(handle) => smp::encrypt_stored(handle),
            }
        }
    }
}

/// An event that is not a command's completion.
fn event(ev: &[u8]) {
    let p = &ev[2..];
    match ev[0] {
        EV_DISCONN_COMPLETE if p.len() >= 4 => {
            let handle = u16::from_le_bytes([p[1], p[2]]) & 0x0FFF;
            let mut st = ST.lock();
            if let Some(i) = st.conns.iter().position(|c| c.handle == handle) {
                let c = st.conns.remove(i);
                let a = c.addr;
                drop(st);
                hid::release(c.hid);
                crate::info!("bt", "{} disconnected (reason {:02x})", addr_text(&a), p[3]);
            }
            changed();
        }
        EV_ENCRYPT_CHANGE if p.len() >= 4 => {
            let handle = u16::from_le_bytes([p[1], p[2]]) & 0x0FFF;
            let mut st = ST.lock();
            if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
                c.enc = Some(p[0]);
                c.encrypted = p[0] == 0 && p[3] != 0;
            }
            drop(st);
            changed();
        }
        EV_LE_META if !p.is_empty() => smp::le_event(p),
        _ => classic::event(ev[0], p),
    }
}

/// Gets controller `n` ready: the commands every host sends first.
fn setup(n: usize) {
    let Some(desc) = ADAPTERS.lock().get(n).map(|a| a.desc) else { return };
    if ST.lock().active.is_some() {
        crate::info!("bt", "hci{n}: another controller is in use; this one stays idle");
        return;
    }
    let c = |op: u16, p: &[u8]| command_on(desc, op, p);
    // While setting up, completions must reach `command_on` even though no
    // controller is active yet: the receive thread passes them on for all.
    if c(0x0C03, &[]).is_err() {
        crate::warn!("bt", "hci{n}: no answer to HCI_Reset");
        return;
    }
    let Ok(a) = c(0x1009, &[]) else {
        crate::warn!("bt", "hci{n}: cannot read the address");
        return;
    };
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&a[1..7]);
    let acl_mtu = c(0x1005, &[]).ok().filter(|r| r.len() >= 3).map_or(27, |r| u16::from_le_bytes([r[1], r[2]]) as usize);
    let le_mtu = c(0x2002, &[]).ok().filter(|r| r.len() >= 3).map(|r| u16::from_le_bytes([r[1], r[2]]) as usize).filter(|&m| m > 0).unwrap_or(acl_mtu);
    // Every event but a few rarely wanted ones, LE meta included.
    let _ = c(0x0C01, &[0xff, 0xff, 0xfb, 0xff, 0x07, 0xf8, 0xbf, 0x3d]);
    let _ = c(0x2001, &[0x1f, 0, 0, 0, 0, 0, 0, 0]);
    let _ = c(0x0C56, &[1]); // Secure Simple Pairing
    let _ = c(0x0C45, &[2]); // inquiry results with RSSI and EIR
    let _ = c(0x0C6D, &[1, 0]); // LE host
    let _ = c(0x0C7A, &[1]); // Secure Connections host (if the chip has it)
    let _ = c(0x0C24, &[0x0C, 0x01, 0x00]); // computer, laptop
    let mut name = [0u8; 248];
    name[..6].copy_from_slice(b"justOS");
    let _ = c(0x0C13, &name);
    let powered = {
        let mut st = ST.lock();
        if st.active.is_some() {
            return;
        }
        st.active = Some(n);
        st.addr = addr;
        st.acl_mtu = acl_mtu.clamp(27, 1021);
        st.le_mtu = le_mtu.clamp(27, 251);
        st.powered
    };
    // Paired keyboards and mice connect by themselves: answer pages.
    let _ = c(0x0C1A, &[if powered { 2 } else { 0 }]);
    crate::info!("bt", "hci{n} ready: address {}, ACL {acl_mtu}, LE {le_mtu}", addr_text(&addr));
    changed();
    if powered {
        reconnect_le();
    }
}

/// LE devices do not page us: connect the paired ones that are near.
fn reconnect_le() {
    let le: Vec<Addr> = ST.lock().devices.iter().filter(|d| d.paired && d.le).map(|d| d.addr).collect();
    if le.is_empty() {
        return;
    }
    let _ = crate::task::spawn("bt-reconnect", move || {
        for a in le {
            let _ = connect(&a);
        }
    });
}

// ----------------------------------------------------------------------------
// For the shell, the service and the interface

pub fn addr_text(a: &Addr) -> String {
    format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", a[5], a[4], a[3], a[2], a[1], a[0])
}

pub fn parse_addr(s: &str) -> Option<Addr> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut a = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        a[5 - i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(a)
}

/// The controller in use: (name, address, on).
pub fn adapter() -> Option<(String, String, bool)> {
    let (i, addr, powered) = {
        let st = ST.lock();
        (st.active?, st.addr, st.powered)
    };
    let name = ADAPTERS.lock().get(i)?.name.clone();
    Some((name, addr_text(&addr), powered))
}

pub fn present() -> bool {
    ST.lock().active.is_some()
}

#[derive(Clone, Debug)]
pub struct DeviceView {
    pub addr: String,
    pub name: String,
    pub kind: Kind,
    pub le: bool,
    pub paired: bool,
    pub connected: bool,
    pub rssi: i8,
}

/// Paired devices first, then the ones of the last scan.
pub fn devices() -> Vec<DeviceView> {
    let st = ST.lock();
    let mut v: Vec<DeviceView> = st
        .devices
        .iter()
        .filter(|d| d.paired || d.seen)
        .map(|d| DeviceView {
            addr: addr_text(&d.addr),
            name: if d.name.is_empty() { addr_text(&d.addr) } else { d.name.clone() },
            kind: d.kind,
            le: d.le,
            paired: d.paired,
            connected: st.conns.iter().any(|c| c.addr == d.addr && c.hid.is_some()),
            rssi: d.rssi,
        })
        .collect();
    v.sort_by_key(|d| (!d.paired, !d.connected));
    v
}

pub fn scanning() -> bool {
    ST.lock().scanning
}

/// The question the current pairing asks the user, if any.
pub fn prompt() -> Option<Prompt> {
    ST.lock().prompt.clone()
}

pub(crate) fn ask(addr: Addr, kind: PromptKind, classic: bool) -> u32 {
    let mut st = ST.lock();
    let id = st.next_prompt;
    st.next_prompt += 1;
    let name = st.devices.iter().find(|d| d.addr == addr).map(|d| d.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| addr_text(&addr));
    st.prompt = Some(Prompt { id, addr, name, kind, classic });
    st.answer = None;
    drop(st);
    changed();
    crate::dev::input::wake_gui();
    id
}

pub(crate) fn clear_prompt(addr: &Addr) {
    let mut st = ST.lock();
    if st.prompt.as_ref().is_some_and(|p| p.addr == *addr) {
        st.prompt = None;
    }
    drop(st);
    changed();
    crate::dev::input::wake_gui();
}

/// Answers prompt `id`: `ok` (the numbers match, or the passkey typed) or
/// not.
pub fn answer(id: u32, ok: bool, passkey: u32) {
    let p = {
        let mut st = ST.lock();
        let Some(p) = st.prompt.take_if(|p| p.id == id) else { return };
        st.answer = Some((id, ok, passkey));
        p
    };
    changed();
    crate::dev::input::wake_gui();
    if p.classic {
        classic::answer(&p, ok, passkey);
    }
}

/// Scans for `secs` seconds, BR/EDR and LE together: how many devices
/// were found.
pub fn scan(secs: u32) -> Result<usize, String> {
    let _busy = BUSY.try_lock().ok_or_else(busy_text)?;
    ready()?;
    {
        let mut st = ST.lock();
        st.devices.retain(|d| d.paired);
        st.devices.iter_mut().for_each(|d| d.seen = false);
        st.scanning = true;
        st.inquiry_done = false;
    }
    changed();
    crate::dev::input::wake_gui();
    let secs = secs.clamp(2, 30);
    // Inquiry length is in units of 1.28 s.
    let classic = command(0x0401, &[0x33, 0x8B, 0x9E, (secs * 100 / 128).max(1) as u8, 0]).is_ok();
    // Active LE scan: 30 ms every 60 ms, our public address, every device.
    let le = command(0x200B, &[1, 0x60, 0, 0x30, 0, 0, 0]).is_ok() && command(0x200C, &[1, 0]).is_ok();
    if !classic && !le {
        ST.lock().scanning = false;
        return Err("il controller non accetta la ricerca".to_string());
    }
    let until = crate::time::uptime_ms() + secs as u64 * 1000;
    let _ = wait(secs as u64 * 1000 + 2000, |st| (st.inquiry_done || !classic).then_some(()));
    if crate::time::uptime_ms() < until && le {
        crate::task::sleep_ms(until - crate::time::uptime_ms());
    }
    if le {
        let _ = command(0x200C, &[0, 0]);
    }
    if classic && !ST.lock().inquiry_done {
        let _ = command(0x0402, &[]);
    }
    // Names of BR/EDR devices that did not say theirs.
    let nameless: Vec<(Addr, u8, u16)> = ST.lock().devices.iter().filter(|d| d.seen && !d.le && d.name.is_empty()).map(|d| (d.addr, d.psrm, d.clock)).collect();
    for (a, psrm, clock) in nameless {
        let mut p = Vec::from(a);
        p.push(psrm);
        p.push(0);
        p.extend_from_slice(&(clock | 0x8000).to_le_bytes());
        if command(0x0419, &p).is_ok() {
            let _ = wait(5000, |st| st.devices.iter().find(|d| d.addr == a).is_some_and(|d| !d.name.is_empty()).then_some(()));
        }
    }
    let n = {
        let mut st = ST.lock();
        st.scanning = false;
        st.devices.iter().filter(|d| d.seen).count()
    };
    changed();
    crate::dev::input::wake_gui();
    Ok(n)
}

fn busy_text() -> String {
    "Bluetooth è occupato con un'altra operazione".to_string()
}

fn ready() -> Result<(), String> {
    let st = ST.lock();
    if st.active.is_none() {
        return Err("nessun adattatore Bluetooth".to_string());
    }
    if !st.powered {
        return Err("Bluetooth è spento".to_string());
    }
    Ok(())
}

/// Finds a device by address or name.
pub fn find(what: &str) -> Option<String> {
    if parse_addr(what).is_some() {
        return Some(what.to_uppercase());
    }
    let st = ST.lock();
    let w = what.to_lowercase();
    st.devices.iter().find(|d| d.name.to_lowercase() == w).or_else(|| st.devices.iter().find(|d| d.name.to_lowercase().contains(&w))).map(|d| addr_text(&d.addr))
}

/// Pairs with a device of the last scan and connects it.
pub fn pair(addr: &str) -> Result<(), String> {
    let a = parse_addr(addr).ok_or("indirizzo non valido")?;
    let _busy = BUSY.try_lock().ok_or_else(busy_text)?;
    ready()?;
    let d = ST.lock().devices.iter().find(|d| d.addr == a).cloned().ok_or("dispositivo non trovato: cercalo prima")?;
    let r = if d.le { smp::pair(&d) } else { classic::pair(&d) };
    clear_prompt(&a);
    match &r {
        Ok(()) => crate::info!("bt", "paired with {} ({})", d.name, addr_text(&a)),
        Err(e) => crate::warn!("bt", "pairing with {} failed: {e}", addr_text(&a)),
    }
    r
}

/// Connects a paired device.
pub fn connect(a: &Addr) -> Result<(), String> {
    let _busy = BUSY.try_lock().ok_or_else(busy_text)?;
    ready()?;
    let d = ST.lock().devices.iter().find(|d| d.addr == *a && d.paired).cloned().ok_or("dispositivo non associato")?;
    if ST.lock().conns.iter().any(|c| c.addr == *a && c.hid.is_some()) {
        return Ok(());
    }
    if d.le {
        smp::connect(&d)
    } else {
        classic::connect(&d)
    }
}

pub fn disconnect(a: &Addr) {
    let handle = ST.lock().conns.iter().find(|c| c.addr == *a).map(|c| c.handle);
    if let Some(h) = handle {
        let mut p = Vec::from(h.to_le_bytes());
        p.push(0x13); // remote user terminated connection
        let _ = command(0x0406, &p);
        let _ = wait(3000, |st| (!st.conns.iter().any(|c| c.handle == h)).then_some(()));
    }
}

/// Disconnects and forgets a device, keys included.
pub fn forget(a: &Addr) -> bool {
    disconnect(a);
    let found = {
        let mut st = ST.lock();
        let before = st.devices.len();
        st.devices.retain(|d| d.addr != *a);
        before != st.devices.len()
    };
    if found {
        // Also the controller's copy of a link key, if it kept one.
        let mut p = Vec::from(*a);
        p.push(0);
        let _ = command(0x0C12, &p);
        save();
    }
    changed();
    crate::dev::input::wake_gui();
    found
}

/// Turns Bluetooth on or off (kept across restarts).
pub fn set_powered(on: bool) {
    let handles: Vec<u16> = {
        let mut st = ST.lock();
        st.powered = on;
        if on {
            Vec::new()
        } else {
            st.conns.iter().map(|c| c.handle).collect()
        }
    };
    for h in handles {
        let mut p = Vec::from(h.to_le_bytes());
        p.push(0x15); // power off
        let _ = command(0x0406, &p);
    }
    let _ = command(0x0C1A, &[if on { 2 } else { 0 }]);
    save();
    changed();
    crate::dev::input::wake_gui();
    if on {
        reconnect_le();
    }
}

pub fn powered() -> bool {
    ST.lock().powered
}

// ----------------------------------------------------------------------------
// The store

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Writes the paired devices and the on/off switch.
pub(crate) fn save() {
    let (powered, devs) = {
        let st = ST.lock();
        (st.powered, st.devices.iter().filter(|d| d.paired).cloned().collect::<Vec<_>>())
    };
    let list = devs
        .iter()
        .map(|d| {
            let mut o = alloc::vec![
                ("addr", Value::string(&addr_text(&d.addr))),
                ("le", Value::Bool(d.le)),
                ("type", Value::Num(d.addr_type as f64)),
                ("name", Value::string(&d.name)),
                ("kind", Value::string(d.kind.key())),
                ("report_map", Value::string(&hex(&d.report_map))),
            ];
            if let Some(k) = d.link_key {
                o.push(("link_key", Value::string(&hex(&k))));
            }
            if let Some(l) = d.ltk {
                o.push(("ltk", Value::string(&hex(&l.key))));
                o.push(("ediv", Value::Num(l.ediv as f64)));
                o.push(("rand", Value::string(&hex(&l.rand))));
            }
            Value::object(o)
        })
        .collect();
    let v = Value::object(alloc::vec![("powered", Value::Bool(powered)), ("devices", Value::Arr(list))]);
    let text = format!("{v}");
    if let Err(e) = crate::fs::abs(STORE).and_then(|p| crate::fs::write_file(&p, text.as_bytes())) {
        crate::warn!("bt", "cannot save {STORE}: {}", e.as_str());
    }
}

fn load() {
    let Some(v) = crate::fs::abs(STORE).and_then(|p| crate::fs::read_file(&p)).ok().and_then(|b| justjson::parse_bytes(&b)) else { return };
    let mut st = ST.lock();
    st.powered = v.get("powered").as_bool().unwrap_or(true);
    for d in v.get("devices").as_array() {
        let Some(addr) = parse_addr(d.get("addr").str()) else { continue };
        let mut dev = Device::new(addr, d.get("le").as_bool().unwrap_or(false));
        dev.addr_type = d.get("type").as_i64().unwrap_or(0) as u8;
        dev.name = d.get("name").str().to_string();
        dev.kind = Kind::from_key(d.get("kind").str());
        dev.hid = true;
        dev.paired = true;
        dev.report_map = unhex(d.get("report_map").str()).unwrap_or_default();
        dev.link_key = unhex(d.get("link_key").str()).and_then(|k| k.try_into().ok());
        if let Some(key) = unhex(d.get("ltk").str()).and_then(|k| <[u8; 16]>::try_from(k).ok()) {
            let rand = unhex(d.get("rand").str()).and_then(|r| r.try_into().ok()).unwrap_or([0; 8]);
            dev.ltk = Some(Ltk { key, ediv: d.get("ediv").as_i64().unwrap_or(0) as u16, rand });
        }
        st.devices.push(dev);
    }
}

/// A device's entry, made if there is none yet.
pub(crate) fn device_mut<'a>(st: &'a mut State, addr: &Addr, le: bool) -> &'a mut Device {
    if let Some(i) = st.devices.iter().position(|d| d.addr == *addr) {
        return &mut st.devices[i];
    }
    st.devices.push(Device::new(*addr, le));
    st.devices.last_mut().unwrap()
}
