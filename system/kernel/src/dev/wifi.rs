//! Wi-Fi radios: cards that send and receive raw 802.11 frames. The station
//! on top of each (scanning, WPA2/WPA3, the data path) is the kernel's own,
//! [`crate::net::wifi`].
//!
//! A radio is either a driver's ([`register`], through the driver API), or
//! the test radio: 802.11 frames carried in Ethernet frames of type 0x88B5
//! over an ordinary network card ([`add_sim`], `wifi sim net<N>`), which
//! `make test-wifi` connects to an access point simulator. There is no
//! emulated Wi-Fi card in QEMU; the test radio stands in for one.

use crate::sync::IrqLock;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use justdk::{Status, WifiDesc, WifiLink, E_INVAL, OK};
use justwifi::sim;
use justwifi::sta::{Link, RxInfo};

type Mac = [u8; 6];

#[derive(Clone, Copy)]
enum Backend {
    Driver(*const WifiDesc),
    /// The test radio, over Ethernet interface `net<N>`.
    Sim(u32),
}

struct Radio {
    device: u32,
    mac: Mac,
    backend: Backend,
    channel: u8,
    channels: Vec<u8>,
    rx: VecDeque<(Vec<u8>, RxInfo)>,
    rx_frames: u64,
    tx_frames: u64,
}

// SAFETY: the descriptor belongs to the driver, which serialises calls.
unsafe impl Send for Radio {}

/// Radios by number (`wifi<N>`); a removed one leaves its slot empty.
static RADIOS: IrqLock<Vec<Option<Radio>>> = IrqLock::new(Vec::new());

fn add(device: u32, mac: Mac, backend: Backend, channels: Vec<u8>) -> u32 {
    let mut r = RADIOS.lock();
    let n = r.len();
    r.push(Some(Radio { device, mac, backend, channel: 0, channels, rx: VecDeque::new(), rx_frames: 0, tx_frames: 0 }));
    drop(r);
    crate::info!("wifi", "wifi{n}: radio, MAC {}", justwifi::sta::mac_text(&mac));
    crate::net::wifi::kick();
    n as u32
}

/// A driver's radio.
pub fn register(device: u32, desc: *const WifiDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    let channels = if d.channels.is_null() || d.channel_count == 0 {
        Vec::new()
    } else {
        // SAFETY: the driver passes `channel_count` channels.
        unsafe { core::slice::from_raw_parts(d.channels, d.channel_count.min(64) as usize) }.to_vec()
    };
    add(device, d.mac, Backend::Driver(desc), channels) as Status
}

/// The test radio over Ethernet interface `iface`. Its address is the
/// card's with the locally administered bit, so that it differs from it.
pub fn add_sim(iface: u32) -> Result<u32, &'static str> {
    let info = super::net::list().into_iter().find(|i| i.index == iface).ok_or("no such interface")?;
    if info.wifi {
        return Err("that is a Wi-Fi interface");
    }
    if RADIOS.lock().iter().flatten().any(|r| matches!(r.backend, Backend::Sim(i) if i == iface)) {
        return Err("already a test radio");
    }
    if !super::net::claim(iface, true) {
        return Err("cannot take the interface");
    }
    let mut mac = info.mac;
    mac[0] = (mac[0] | 0x02) & !0x01;
    mac[5] ^= 0x80;
    Ok(add(info.device, mac, Backend::Sim(iface), Vec::new()))
}

/// A frame from a driver (safe in interrupt handlers).
pub fn receive(radio: u32, frame: &[u8], channel: u8, rssi: i8) {
    if let Some(Some(r)) = RADIOS.lock().get_mut(radio as usize) {
        r.rx_frames += 1;
        if r.rx.len() < 256 {
            r.rx.push_back((frame.to_vec(), RxInfo { channel, rssi }));
        }
    }
    crate::net::wifi::kick();
}

/// The next frame radio `radio` received on its channel.
pub fn poll(radio: u32) -> Option<(Vec<u8>, RxInfo)> {
    let (backend, channel) = {
        let mut r = RADIOS.lock();
        let r = r.get_mut(radio as usize)?.as_mut()?;
        if let Some(f) = r.rx.pop_front() {
            return Some(f);
        }
        (r.backend, r.channel)
    };
    let Backend::Sim(iface) = backend else { return None };
    // The test radio: what the card received, unwrapped; a frame sent on
    // another channel is not heard, as on the air.
    while let Some(e) = super::net::poll(iface) {
        if let Some((ch, rssi, f)) = sim::unwrap(&e) {
            if ch == channel {
                if let Some(Some(r)) = RADIOS.lock().get_mut(radio as usize) {
                    r.rx_frames += 1;
                }
                return Some((f.to_vec(), RxInfo { channel: ch, rssi }));
            }
        }
    }
    None
}

pub fn send(radio: u32, frame: &[u8]) -> Status {
    let (backend, mac, channel) = {
        let mut r = RADIOS.lock();
        let Some(Some(r)) = r.get_mut(radio as usize) else { return E_INVAL };
        r.tx_frames += 1;
        (r.backend, r.mac, r.channel)
    };
    match backend {
        // SAFETY: the descriptor stays valid while the radio exists.
        Backend::Driver(d) => unsafe { ((*d).send)((*d).ctx, frame.as_ptr(), frame.len()) },
        Backend::Sim(iface) => super::net::send(iface, &sim::wrap(&mac, channel, 0, frame)),
    }
}

pub fn set_channel(radio: u32, channel: u8) -> Status {
    let backend = {
        let mut r = RADIOS.lock();
        let Some(Some(r)) = r.get_mut(radio as usize) else { return E_INVAL };
        r.channel = channel;
        r.backend
    };
    match backend {
        // SAFETY: as in `send`.
        Backend::Driver(d) => unsafe { ((*d).set_channel)((*d).ctx, channel as u32) },
        Backend::Sim(_) => OK,
    }
}

pub fn set_link(radio: u32, link: Option<&Link>) -> Status {
    let backend = match RADIOS.lock().get(radio as usize) {
        Some(Some(r)) => r.backend,
        _ => return E_INVAL,
    };
    let Backend::Driver(d) = backend else { return OK };
    let l = link.map(|l| {
        let mut ssid = [0u8; 32];
        let n = l.ssid.len().min(32);
        ssid[..n].copy_from_slice(&l.ssid[..n]);
        WifiLink { bssid: l.bssid, channel: l.channel, ssid_len: n as u8, ssid, aid: l.aid, _pad: [0; 6] }
    });
    let p = l.as_ref().map_or(core::ptr::null(), |l| l as *const WifiLink);
    // SAFETY: as in `send`; `p` lives for the call.
    unsafe { ((*d).set_link)((*d).ctx, p) }
}

pub struct RadioInfo {
    pub index: u32,
    pub device: u32,
    pub mac: Mac,
    pub channels: Vec<u8>,
    /// The test radio's Ethernet interface.
    pub sim: Option<u32>,
    pub rx: u64,
    pub tx: u64,
}

pub fn list() -> Vec<RadioInfo> {
    RADIOS
        .lock()
        .iter()
        .enumerate()
        .filter_map(|(n, r)| Some((n, r.as_ref()?)))
        .map(|(n, r)| RadioInfo {
            index: n as u32,
            device: r.device,
            mac: r.mac,
            channels: r.channels.clone(),
            sim: match r.backend {
                Backend::Sim(i) => Some(i),
                Backend::Driver(_) => None,
            },
            rx: r.rx_frames,
            tx: r.tx_frames,
        })
        .collect()
}

/// A driver let go of its device: its radios go (the test radio stays with
/// its card's interface, which goes the same way).
pub fn remove_device(device: u32) {
    let mut gone = Vec::new();
    for (n, slot) in RADIOS.lock().iter_mut().enumerate() {
        if slot.as_ref().is_some_and(|r| r.device == device) {
            *slot = None;
            gone.push(n as u32);
        }
    }
    for n in gone {
        crate::net::wifi::radio_gone(n);
    }
}
