//! Network interfaces registered by drivers, or by the kernel itself (a
//! Wi-Fi station's, [`crate::net::wifi`]). Frames are counted and kept in a
//! small receive queue; protocols live in [`crate::net`].
//!
//! An interface has a link: a cable's is always up (the drivers do not say),
//! a Wi-Fi station's is up while it is joined to a network. The IP stack
//! asks DHCP only on a link that is up, and starts over when it comes back.
//! An interface can also be *claimed* by Wi-Fi (the test radio runs over an
//! Ethernet card): its frames are then not IP's.

use crate::sync::IrqLock;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use justdk::{NetDesc, Status, E_INVAL, OK};

/// Who sends an interface's frames.
#[derive(Clone, Copy)]
enum Sender {
    Driver(*const NetDesc),
    /// The kernel itself, given the interface's number.
    Kernel(fn(u32, &[u8]) -> Status),
}

pub struct Iface {
    pub device: u32,
    pub mac: [u8; 6],
    pub mtu: u32,
    desc: Sender,
    link: bool,
    wifi: bool,
    claimed: bool,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    rx: VecDeque<Vec<u8>>,
}

// SAFETY: the descriptor belongs to the driver, which serialises sends itself.
unsafe impl Send for Iface {}

/// Interfaces by number; a removed interface leaves its slot empty, so the
/// numbers of the others never change.
static IFACES: IrqLock<Vec<Option<Iface>>> = IrqLock::new(Vec::new());

pub fn register(device: u32, desc: *const NetDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    let mut ifaces = IFACES.lock();
    let n = ifaces.len();
    ifaces.push(Some(Iface {
        device,
        mac: d.mac,
        mtu: d.mtu,
        desc: Sender::Driver(desc),
        link: true,
        wifi: false,
        claimed: false,
        rx_packets: 0,
        rx_bytes: 0,
        tx_packets: 0,
        tx_bytes: 0,
        rx: VecDeque::new(),
    }));
    drop(ifaces);
    let m = d.mac;
    crate::info!("net", "net{n}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]);
    crate::net::kick();
    n as Status
}

/// An interface the kernel drives itself (a Wi-Fi station's): `send` gets
/// its number and the frame. Its link starts down.
pub fn register_kernel(device: u32, mac: [u8; 6], mtu: u32, send: fn(u32, &[u8]) -> Status) -> u32 {
    let mut ifaces = IFACES.lock();
    let n = ifaces.len();
    ifaces.push(Some(Iface {
        device,
        mac,
        mtu,
        desc: Sender::Kernel(send),
        link: false,
        wifi: true,
        claimed: false,
        rx_packets: 0,
        rx_bytes: 0,
        tx_packets: 0,
        tx_bytes: 0,
        rx: VecDeque::new(),
    }));
    drop(ifaces);
    crate::info!("net", "net{n}: Wi-Fi, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    crate::net::kick();
    n as u32
}

/// The link of interface `iface` came up or went down.
pub fn set_link(iface: u32, up: bool) {
    if let Some(Some(i)) = IFACES.lock().get_mut(iface as usize) {
        i.link = up;
    }
    crate::net::kick();
}

/// Wi-Fi takes interface `iface` for its own frames (or gives it back).
pub fn claim(iface: u32, claimed: bool) -> bool {
    let ok = match IFACES.lock().get_mut(iface as usize) {
        Some(Some(i)) if !i.wifi => {
            i.claimed = claimed;
            i.rx.clear();
            true
        }
        _ => false,
    };
    crate::net::kick();
    ok
}

/// A frame from the wire (the driver's interrupt handler or poller): queued
/// for the network thread, which is woken.
pub fn receive(iface: u32, frame: &[u8]) {
    let mut ifaces = IFACES.lock();
    let mut claimed = false;
    if let Some(Some(i)) = ifaces.get_mut(iface as usize) {
        i.rx_packets += 1;
        i.rx_bytes += frame.len() as u64;
        if i.rx.len() < 64 {
            i.rx.push_back(frame.to_vec());
        }
        claimed = i.claimed;
    }
    drop(ifaces);
    if claimed {
        crate::net::wifi::kick();
    } else {
        crate::net::kick();
    }
}

/// Next received frame on an interface.
pub fn poll(iface: u32) -> Option<Vec<u8>> {
    IFACES.lock().get_mut(iface as usize)?.as_mut()?.rx.pop_front()
}

pub fn send(iface: u32, frame: &[u8]) -> Status {
    if !super::pm::awake() {
        return OK; // the card sleeps: the frame is lost, as on a busy wire
    }
    let desc = {
        let mut ifaces = IFACES.lock();
        let Some(Some(i)) = ifaces.get_mut(iface as usize) else { return E_INVAL };
        i.tx_packets += 1;
        i.tx_bytes += frame.len() as u64;
        i.desc
    };
    match desc {
        // SAFETY: the descriptor stays valid while the interface exists.
        Sender::Driver(desc) => unsafe { ((*desc).send)((*desc).ctx, frame.as_ptr(), frame.len()) },
        Sender::Kernel(send) => send(iface, frame),
    }
}

pub struct IfaceInfo {
    pub index: u32,
    pub device: u32,
    pub mac: [u8; 6],
    pub mtu: u32,
    pub rx: (u64, u64),
    pub tx: (u64, u64),
    /// Up: a cable, or a Wi-Fi network joined.
    pub link: bool,
    /// A Wi-Fi station's interface.
    pub wifi: bool,
    /// Taken by Wi-Fi (the test radio): not for IP.
    pub claimed: bool,
}

pub fn list() -> Vec<IfaceInfo> {
    IFACES
        .lock()
        .iter()
        .enumerate()
        .filter_map(|(n, i)| Some((n, i.as_ref()?)))
        .map(|(n, i)| IfaceInfo {
            index: n as u32,
            device: i.device,
            mac: i.mac,
            mtu: i.mtu,
            rx: (i.rx_packets, i.rx_bytes),
            tx: (i.tx_packets, i.tx_bytes),
            link: i.link,
            wifi: i.wifi,
            claimed: i.claimed,
        })
        .collect()
}

pub fn remove_device(device: u32) {
    for slot in IFACES.lock().iter_mut() {
        if slot.as_ref().is_some_and(|i| i.device == device) {
            *slot = None;
        }
    }
}

#[allow(dead_code)]
pub const SEND_OK: Status = OK;
