//! BR/EDR ("classic"): the inquiry's results, links, Secure Simple
//! Pairing (the controller does the cryptography; the host answers its
//! questions), SDP for the HID descriptor and the two HID channels.

use super::hid::{HidIn, BOOT_KEYBOARD, BOOT_MOUSE};
use super::l2cap::{self, PSM_HID_CONTROL, PSM_HID_INTERRUPT, PSM_SDP};
use super::{addr_text, ask, changed, clear_prompt, command, device_mut, wait, Addr, Conn, Device, Kind, Prompt, PromptKind, ST};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const EV_INQUIRY_RESULT_RSSI: u8 = 0x22;
const EV_EXT_INQUIRY_RESULT: u8 = 0x2F;
const EV_LINK_KEY_REQUEST: u8 = 0x17;
const EV_LINK_KEY_NOTIFY: u8 = 0x18;
const EV_IO_CAP_REQUEST: u8 = 0x31;
const EV_IO_CAP_RESPONSE: u8 = 0x32;
const EV_USER_CONFIRM: u8 = 0x33;
const EV_PASSKEY_REQUEST: u8 = 0x34;
const EV_SSP_COMPLETE: u8 = 0x36;
const EV_PASSKEY_NOTIFY: u8 = 0x3B;

/// Our IO capability: a screen and a yes/no answer.
const IO_DISPLAY_YES_NO: u8 = 1;
const IO_NONE: u8 = 3;

fn addr_at(p: &[u8], at: usize) -> Option<Addr> {
    p.get(at..at + 6)?.try_into().ok()
}

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}

/// A device of the inquiry.
fn found(addr: Addr, psrm: u8, cod: u32, clock: u16, rssi: i8, eir: &[u8]) {
    let mut st = ST.lock();
    let d = device_mut(&mut st, &addr, false);
    d.psrm = psrm;
    d.clock = clock;
    d.rssi = rssi;
    d.seen = true;
    let kind = Kind::from_class(cod);
    if kind != Kind::Other {
        d.kind = kind;
        d.hid = true;
    }
    if let Some(name) = eir_name(eir) {
        d.name = name;
    }
    drop(st);
    changed();
}

/// The name in extended inquiry (or LE advertising) data.
pub(super) fn eir_name(mut eir: &[u8]) -> Option<String> {
    let mut short = None;
    while eir.len() >= 2 && eir[0] != 0 {
        let len = eir[0] as usize;
        let Some(field) = eir.get(1..1 + len) else { break };
        match field[0] {
            0x09 => return Some(String::from_utf8_lossy(&field[1..]).into_owned()),
            0x08 => short = Some(String::from_utf8_lossy(&field[1..]).into_owned()),
            _ => {}
        }
        eir = &eir[1 + len..];
    }
    short
}

pub fn event(code: u8, p: &[u8]) {
    match code {
        super::EV_INQUIRY_COMPLETE => {
            ST.lock().inquiry_done = true;
            changed();
        }
        EV_INQUIRY_RESULT_RSSI if !p.is_empty() => {
            for r in p[1..].chunks_exact(14).take(p[0] as usize) {
                let cod = u32::from_le_bytes([r[8], r[9], r[10], 0]);
                found(addr_at(r, 0).unwrap_or_default(), r[6], cod, le16(r, 11), r[13] as i8, &[]);
            }
        }
        EV_EXT_INQUIRY_RESULT if p.len() >= 15 => {
            let cod = u32::from_le_bytes([p[9], p[10], p[11], 0]);
            found(addr_at(p, 1).unwrap_or_default(), p[7], cod, le16(p, 12), p[14] as i8, &p[15..]);
        }
        super::EV_REMOTE_NAME if p.len() >= 7 => {
            let Some(a) = addr_at(p, 1) else { return };
            let raw = &p[7..];
            let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            let name = if p[0] == 0 { String::from_utf8_lossy(&raw[..len]).into_owned() } else { String::new() };
            let mut st = ST.lock();
            if let Some(d) = st.devices.iter_mut().find(|d| d.addr == a) {
                // An empty name ends the wait too.
                d.name = if name.is_empty() { addr_text(&a) } else { name };
            }
            drop(st);
            changed();
        }
        super::EV_CONN_COMPLETE if p.len() >= 11 => {
            let Some(a) = addr_at(p, 3) else { return };
            let mut st = ST.lock();
            if p[0] == 0 && p[9] == 1 {
                let handle = le16(p, 1) & 0x0FFF;
                st.conns.push(Conn::new(handle, a, false));
                crate::info!("bt", "{} connected (BR/EDR, link {handle:#x})", addr_text(&a));
            } else if p[0] != 0 {
                st.failed.push((a, p[0]));
            }
            drop(st);
            changed();
        }
        super::EV_CONN_REQUEST if p.len() >= 10 => {
            let Some(a) = addr_at(p, 0) else { return };
            let known = ST.lock().devices.iter().any(|d| d.addr == a && d.paired);
            let mut c = Vec::from(a);
            if known && p[9] == 1 {
                c.push(0x00); // become the central
                let _ = command(0x0409, &c);
            } else {
                c.push(0x0F); // unacceptable address
                let _ = command(0x040A, &c);
            }
        }
        super::EV_AUTH_COMPLETE if p.len() >= 3 => {
            let handle = le16(p, 1) & 0x0FFF;
            if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
                c.auth = Some(p[0]);
            }
            changed();
        }
        EV_LINK_KEY_REQUEST if p.len() >= 6 => {
            let Some(a) = addr_at(p, 0) else { return };
            let key = ST.lock().devices.iter().find(|d| d.addr == a).and_then(|d| d.link_key);
            let mut c = Vec::from(a);
            match key {
                Some(k) => {
                    c.extend_from_slice(&k);
                    let _ = command(0x040B, &c);
                }
                None => {
                    let _ = command(0x040C, &c);
                }
            }
        }
        EV_LINK_KEY_NOTIFY if p.len() >= 22 => {
            let Some(a) = addr_at(p, 0) else { return };
            let mut key = [0u8; 16];
            key.copy_from_slice(&p[6..22]);
            let mut st = ST.lock();
            let paired = {
                let d = device_mut(&mut st, &a, false);
                d.link_key = Some(key);
                d.paired
            };
            drop(st);
            if paired {
                super::save(); // a new key for a known device
            }
        }
        EV_IO_CAP_REQUEST if p.len() >= 6 => {
            let mut c = Vec::from(&p[..6]);
            // Display and yes/no, no OOB data, general bonding (MITM when
            // the device's capabilities allow it).
            c.extend_from_slice(&[IO_DISPLAY_YES_NO, 0, 0x04]);
            let _ = command(0x042B, &c);
        }
        EV_IO_CAP_RESPONSE if p.len() >= 9 => {
            let Some(a) = addr_at(p, 0) else { return };
            if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.addr == a && !c.le) {
                c.peer_io = p[6];
                c.peer_auth = p[8];
            }
        }
        EV_USER_CONFIRM if p.len() >= 10 => {
            let Some(a) = addr_at(p, 0) else { return };
            let value = u32::from_le_bytes([p[6], p[7], p[8], p[9]]);
            let peer_io = ST.lock().conns.iter().find(|c| c.addr == a && !c.le).map_or(IO_NONE, |c| c.peer_io);
            // Nothing to compare on a device without a screen: Just Works.
            if peer_io == IO_NONE || peer_io == 0 {
                let _ = command(0x042C, &a);
            } else {
                ask(a, PromptKind::Confirm(value), true);
            }
        }
        EV_PASSKEY_REQUEST if p.len() >= 6 => {
            if let Some(a) = addr_at(p, 0) {
                ask(a, PromptKind::Enter, true);
            }
        }
        EV_PASSKEY_NOTIFY if p.len() >= 10 => {
            if let Some(a) = addr_at(p, 0) {
                let passkey = u32::from_le_bytes([p[6], p[7], p[8], p[9]]);
                crate::info!("bt", "pairing {}: type {passkey:06} on the device", addr_text(&a));
                ask(a, PromptKind::Display(passkey), true);
            }
        }
        EV_SSP_COMPLETE if p.len() >= 7 => {
            if let Some(a) = addr_at(p, 1) {
                clear_prompt(&a);
            }
        }
        _ => {}
    }
}

/// The user's answer to a BR/EDR pairing question.
pub fn answer(p: &Prompt, ok: bool, passkey: u32) {
    let mut c = Vec::from(p.addr);
    match (p.kind, ok) {
        (PromptKind::Confirm(_), true) => {
            let _ = command(0x042C, &c);
        }
        (PromptKind::Confirm(_), false) => {
            let _ = command(0x042D, &c);
        }
        (PromptKind::Enter, true) => {
            c.extend_from_slice(&passkey.to_le_bytes());
            let _ = command(0x042E, &c);
        }
        (PromptKind::Enter, false) => {
            let _ = command(0x042F, &c);
        }
        (PromptKind::Display(_), _) => {}
    }
}

/// The input of a HID device with report descriptor `map` (the boot
/// protocol's when there is none).
pub fn hid_input(map: &[u8], kind: Kind) -> HidIn {
    if map.is_empty() {
        HidIn::new(if kind == Kind::Mouse { BOOT_MOUSE } else { BOOT_KEYBOARD }, true)
    } else {
        HidIn::new(map, false)
    }
}

fn link_of(st: &super::State, a: &Addr) -> Option<u16> {
    st.conns.iter().find(|c| c.addr == *a && !c.le).map(|c| c.handle)
}

/// A link to the device, made if there is none.
fn link(d: &Device) -> Result<u16, String> {
    if let Some(h) = link_of(&ST.lock(), &d.addr) {
        return Ok(h);
    }
    ST.lock().failed.retain(|f| f.0 != d.addr);
    let mut c = Vec::from(d.addr);
    c.extend_from_slice(&0xCC18u16.to_le_bytes()); // DM1..DH5
    c.push(d.psrm);
    c.push(0);
    c.extend_from_slice(&(if d.clock != 0 { d.clock | 0x8000 } else { 0 }).to_le_bytes());
    c.push(1); // the device may become the central
    command(0x0405, &c).map_err(|e| format!("il controller non si collega (codice {e:02x})"))?;
    let r = wait(20_000, |st| {
        if let Some(h) = link_of(st, &d.addr) {
            return Some(Ok(h));
        }
        st.failed.iter().find(|f| f.0 == d.addr).map(|f| Err(f.1))
    });
    match r {
        Some(Ok(h)) => Ok(h),
        Some(Err(0x04)) | None => Err("il dispositivo non risponde: è acceso e vicino?".to_string()),
        Some(Err(e)) => Err(format!("collegamento non riuscito (codice {e:02x})")),
    }
}

fn drop_link(handle: u16) {
    let mut p = Vec::from(handle.to_le_bytes());
    p.push(0x13);
    let _ = command(0x0406, &p);
}

/// Authenticates the link (pairing if there is no key) and encrypts it.
fn secure(handle: u16, pairing: bool) -> Result<(), String> {
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.auth = None;
        c.enc = None;
    }
    command(0x0411, &handle.to_le_bytes()).map_err(|e| format!("autenticazione rifiutata (codice {e:02x})"))?;
    // Long enough for someone to type a passkey.
    let r = wait(if pairing { 90_000 } else { 15_000 }, |st| match st.conns.iter().find(|c| c.handle == handle) {
        None => Some(Err(())),
        Some(c) => c.auth.map(Ok),
    });
    match r {
        Some(Ok(0)) => {}
        Some(Ok(0x05)) | Some(Ok(0x06)) => return Err("chiave rifiutata: il dispositivo va associato di nuovo".to_string()),
        Some(Ok(e)) => return Err(format!("associazione non riuscita (codice {e:02x})")),
        Some(Err(())) => return Err("il dispositivo si è scollegato".to_string()),
        None => return Err("tempo scaduto".to_string()),
    }
    let already = ST.lock().conns.iter().any(|c| c.handle == handle && c.encrypted);
    if !already {
        let mut p = Vec::from(handle.to_le_bytes());
        p.push(1);
        command(0x0413, &p).map_err(|e| format!("cifratura rifiutata (codice {e:02x})"))?;
        let ok = wait(10_000, |st| st.conns.iter().find(|c| c.handle == handle).and_then(|c| c.enc.map(|_| c.encrypted)));
        if ok != Some(true) {
            return Err("cifratura non riuscita".to_string());
        }
    }
    Ok(())
}

/// Opens the HID channels, input on.
fn start_hid(handle: u16, map: &[u8], kind: Kind) -> Result<(), String> {
    let ctrl = l2cap::open(handle, PSM_HID_CONTROL)?;
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.hid = Some(hid_input(map, kind));
    }
    l2cap::open(handle, PSM_HID_INTERRUPT)?;
    if map.is_empty() {
        // SET_PROTOCOL: boot.
        l2cap::chan_send(handle, ctrl, &[0x70]);
    }
    Ok(())
}

pub fn pair(d: &Device) -> Result<(), String> {
    if let Some(dev) = ST.lock().devices.iter_mut().find(|x| x.addr == d.addr) {
        dev.link_key = None;
    }
    let handle = link(d)?;
    let r = (|| {
        secure(handle, true)?;
        let map = sdp_hid(handle).unwrap_or_default();
        if map.is_empty() {
            crate::warn!("bt", "{}: no HID descriptor from SDP, boot protocol", addr_text(&d.addr));
        }
        let mut kind = d.kind;
        if kind == Kind::Other {
            kind = kind_of_map(&map);
        }
        start_hid(handle, &map, kind)?;
        let mut st = ST.lock();
        let dev = device_mut(&mut st, &d.addr, false);
        dev.paired = true;
        dev.hid = true;
        dev.kind = kind;
        dev.report_map = map;
        Ok(())
    })();
    match r {
        Ok(()) => {
            super::save();
            changed();
            Ok(())
        }
        Err(e) => {
            drop_link(handle);
            Err(e)
        }
    }
}

pub fn connect(d: &Device) -> Result<(), String> {
    let handle = link(d)?;
    let r = secure(handle, false).and_then(|_| start_hid(handle, &d.report_map, d.kind));
    if r.is_err() {
        drop_link(handle);
    }
    changed();
    r
}

/// What a report descriptor's top collection is.
pub(super) fn kind_of_map(map: &[u8]) -> Kind {
    let fields = justdk::hid::parse(map);
    let keys = fields.iter().any(|f| f.page == 0x07);
    let pointer = fields.iter().any(|f| f.page == 0x09 || (f.page == 0x01 && f.usage(0).1 == 0x30));
    match (keys, pointer) {
        (true, true) => Kind::Combo,
        (true, false) => Kind::Keyboard,
        (false, true) => Kind::Mouse,
        _ => Kind::Other,
    }
}

/// The HID descriptor list (attribute 0x0206) of the device's HID record.
fn sdp_hid(handle: u16) -> Option<Vec<u8>> {
    let ch = l2cap::open(handle, PSM_SDP).ok()?;
    let mut all = Vec::new();
    let mut cont: Vec<u8> = alloc::vec![0];
    let mut tid: u16 = 1;
    for _ in 0..16 {
        let mut params = alloc::vec![0x35, 0x03, 0x19, 0x11, 0x24, 0x02, 0x00, 0x35, 0x03, 0x09, 0x02, 0x06];
        params.extend_from_slice(&cont);
        let mut req = alloc::vec![0x06];
        req.extend_from_slice(&tid.to_be_bytes());
        req.extend_from_slice(&(params.len() as u16).to_be_bytes());
        req.extend_from_slice(&params);
        if !l2cap::chan_send(handle, ch, &req) {
            break;
        }
        let Some(rsp) = l2cap::chan_recv(handle, ch, 5000) else { break };
        if rsp.len() < 8 || rsp[0] != 0x07 {
            break;
        }
        let count = u16::from_be_bytes([rsp[5], rsp[6]]) as usize;
        let Some(lists) = rsp.get(7..7 + count) else { break };
        all.extend_from_slice(lists);
        let Some(&clen) = rsp.get(7 + count) else { break };
        if clen == 0 {
            break;
        }
        cont = rsp.get(7 + count..8 + count + clen as usize)?.to_vec();
        tid = tid.wrapping_add(1);
    }
    l2cap::close(handle, ch);
    descriptor_in(&all)
}

/// The first descriptor (type 0x22, a text string) after attribute id
/// 0x0206 in SDP data elements.
fn descriptor_in(b: &[u8]) -> Option<Vec<u8>> {
    let start = b.windows(3).position(|w| w == [0x09, 0x02, 0x06])? + 3;
    let b = &b[start..];
    let i = b.windows(3).position(|w| w[0] == 0x08 && w[1] == 0x22 && (0x25..=0x27).contains(&w[2]))?;
    let at = i + 2;
    let (len, data) = match b[at] {
        0x25 => (*b.get(at + 1)? as usize, at + 2),
        0x26 => (u16::from_be_bytes([*b.get(at + 1)?, *b.get(at + 2)?]) as usize, at + 3),
        _ => (u32::from_be_bytes(b.get(at + 1..at + 5)?.try_into().ok()?) as usize, at + 5),
    };
    Some(b.get(data..data + len)?.to_vec())
}
