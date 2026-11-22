//! ATT and a GATT client (core spec vol 3, parts F and G), enough for HID
//! over GATT: the HID service, its report map and its input reports, whose
//! notifications become input. With the default MTU of 23.
//!
//! We have no GATT server: a device asking us gets "not found".

use super::hid::{self, HidIn};
use super::l2cap::{self, CID_ATT};
use super::{changed, wait, ST};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MTU: usize = 23;
const ERROR_RSP: u8 = 0x01;
const MTU_REQ: u8 = 0x02;
const FIND_INFO_REQ: u8 = 0x04;
const FIND_BY_TYPE_REQ: u8 = 0x06;
const READ_BY_TYPE_REQ: u8 = 0x08;
const READ_REQ: u8 = 0x0A;
const READ_BLOB_REQ: u8 = 0x0C;
const READ_BY_GROUP_REQ: u8 = 0x10;
const WRITE_REQ: u8 = 0x12;
const NOTIFY: u8 = 0x1B;
const INDICATE: u8 = 0x1D;
const CONFIRM: u8 = 0x1E;

const UUID_PRIMARY: u16 = 0x2800;
const UUID_CHARACTERISTIC: u16 = 0x2803;
const UUID_HID: u16 = 0x1812;
const UUID_REPORT_MAP: u16 = 0x2A4B;
const UUID_REPORT: u16 = 0x2A4D;
const UUID_CCCD: u16 = 0x2902;
const UUID_REPORT_REF: u16 = 0x2908;

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}

/// An ATT packet from the device.
pub fn receive(handle: u16, p: &[u8]) {
    let Some(&op) = p.first() else { return };
    match op {
        NOTIFY | INDICATE if p.len() >= 3 => {
            if op == INDICATE {
                l2cap::send(handle, CID_ATT, &[CONFIRM]);
            }
            let attr = le16(p, 1);
            let report = {
                let st = ST.lock();
                let Some(c) = st.conns.iter().find(|c| c.handle == handle) else { return };
                let Some(&(_, id)) = c.reports.iter().find(|r| r.0 == attr) else { return };
                let ids = c.hid.as_ref().is_some_and(HidIn::uses_ids);
                let mut r = Vec::with_capacity(p.len());
                if ids && id != 0 {
                    r.push(id);
                }
                r.extend_from_slice(&p[3..]);
                r
            };
            hid::report(handle, &report);
        }
        MTU_REQ => {
            l2cap::send(handle, CID_ATT, &[0x03, MTU as u8, 0]);
        }
        // Responses: for the request waiting.
        _ if op & 1 == 1 && op < 0x20 => {
            let mut st = ST.lock();
            if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
                if c.att.len() < 8 {
                    c.att.push_back(p.to_vec());
                }
            }
            drop(st);
            changed();
        }
        // Commands (bit 6) need no answer; requests get an error.
        _ if op & 0x40 == 0 => {
            let code = match op {
                FIND_INFO_REQ | FIND_BY_TYPE_REQ | READ_BY_TYPE_REQ | READ_BY_GROUP_REQ => 0x0A,
                _ => 0x06,
            };
            let h = le16(p, 1);
            l2cap::send(handle, CID_ATT, &[ERROR_RSP, op, h as u8, (h >> 8) as u8, code]);
        }
        _ => {}
    }
}

/// Sends a request and waits for its response. Err: the ATT error code
/// (0 when there was no answer).
fn request(handle: u16, pdu: &[u8]) -> Result<Vec<u8>, u8> {
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.att.clear();
    }
    if !l2cap::send(handle, CID_ATT, pdu) {
        return Err(0);
    }
    let r = wait(10_000, |st| match st.conns.iter_mut().find(|c| c.handle == handle) {
        None => Some(None),
        Some(c) => c.att.pop_front().map(Some),
    });
    match r {
        Some(Some(p)) if p[0] == ERROR_RSP => Err(p.get(4).copied().unwrap_or(0)),
        Some(Some(p)) if p[0] == pdu[0] + 1 => Ok(p),
        _ => Err(0),
    }
}

fn range_req(op: u16, start: u16, end: u16, uuid: u16) -> Vec<u8> {
    let mut v = alloc::vec![op as u8];
    v.extend_from_slice(&start.to_le_bytes());
    v.extend_from_slice(&end.to_le_bytes());
    v.extend_from_slice(&uuid.to_le_bytes());
    v
}

/// A characteristic: declaration handle, properties, value handle, UUID
/// (0 for 128-bit ones).
struct Chr {
    decl: u16,
    props: u8,
    value: u16,
    uuid: u16,
}

fn characteristics(handle: u16, start: u16, end: u16) -> Vec<Chr> {
    let mut out: Vec<Chr> = Vec::new();
    let mut at = start;
    while at <= end && out.len() < 64 {
        let Ok(r) = request(handle, &range_req(READ_BY_TYPE_REQ as u16, at, end, UUID_CHARACTERISTIC)) else { break };
        let len = r[1] as usize;
        if len < 7 {
            break;
        }
        let before = out.len();
        for e in r[2..].chunks_exact(len) {
            out.push(Chr { decl: le16(e, 0), props: e[2], value: le16(e, 3), uuid: if len == 7 { le16(e, 5) } else { 0 } });
        }
        let Some(last) = out.last().map(|c| c.decl) else { break };
        if out.len() == before || last >= end {
            break;
        }
        at = last + 1;
    }
    out
}

/// Descriptors (handle, UUID) between two handles.
fn descriptors(handle: u16, start: u16, end: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut at = start;
    while at <= end && out.len() < 16 {
        let mut req = alloc::vec![FIND_INFO_REQ];
        req.extend_from_slice(&at.to_le_bytes());
        req.extend_from_slice(&end.to_le_bytes());
        let Ok(r) = request(handle, &req) else { break };
        if r[1] != 1 {
            break; // 128-bit UUIDs: not ours
        }
        let mut last = at;
        for e in r[2..].chunks_exact(4) {
            last = le16(e, 0);
            out.push((last, le16(e, 2)));
        }
        if last >= end {
            break;
        }
        at = last + 1;
    }
    out
}

/// A value of any length (Read, then Read Blob).
fn read_long(handle: u16, attr: u16) -> Result<Vec<u8>, u8> {
    let mut v = request(handle, &[READ_REQ, attr as u8, (attr >> 8) as u8])?[1..].to_vec();
    while v.len() < 4096 {
        let n = v.len() as u16;
        if v.is_empty() || v.len() % (MTU - 1) != 0 {
            break;
        }
        match request(handle, &[READ_BLOB_REQ, attr as u8, (attr >> 8) as u8, n as u8, (n >> 8) as u8]) {
            Ok(r) if r.len() > 1 => v.extend_from_slice(&r[1..]),
            _ => break,
        }
        if v.len() as u16 - n < (MTU - 1) as u16 {
            break;
        }
    }
    Ok(v)
}

/// Finds the HID service, reads its report map, turns input on and
/// subscribes to its input reports: the report map.
pub fn hogp(handle: u16) -> Result<Vec<u8>, String> {
    let mut req = range_req(FIND_BY_TYPE_REQ as u16, 1, 0xFFFF, UUID_PRIMARY);
    req.extend_from_slice(&UUID_HID.to_le_bytes());
    let svc = request(handle, &req).map_err(|_| "il dispositivo non ha il servizio HID".to_string())?;
    if svc.len() < 5 {
        return Err("il dispositivo non ha il servizio HID".to_string());
    }
    let (start, end) = (le16(&svc, 1), le16(&svc, 3));
    let chars = characteristics(handle, start, end);
    let map_chr = chars.iter().find(|c| c.uuid == UUID_REPORT_MAP).ok_or("manca la mappa dei report HID")?;
    let map = read_long(handle, map_chr.value).map_err(|e| alloc::format!("mappa dei report illeggibile (codice {e:02x})"))?;
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.hid = Some(HidIn::new(&map, false));
        c.reports.clear();
    }
    let mut inputs = 0;
    for (i, c) in chars.iter().enumerate() {
        if c.uuid != UUID_REPORT || c.props & 0x10 == 0 {
            continue;
        }
        let last = chars.get(i + 1).map_or(end, |n| n.decl - 1);
        let descs = descriptors(handle, c.value + 1, last);
        let Some(&(cccd, _)) = descs.iter().find(|d| d.1 == UUID_CCCD) else { continue };
        let (id, kind) = match descs.iter().find(|d| d.1 == UUID_REPORT_REF) {
            Some(&(r, _)) => match request(handle, &[READ_REQ, r as u8, (r >> 8) as u8]) {
                Ok(v) if v.len() >= 3 => (v[1], v[2]),
                _ => (0, 1),
            },
            None => (0, 1),
        };
        if kind != 1 {
            continue; // output and feature reports
        }
        if let Some(cn) = ST.lock().conns.iter_mut().find(|cn| cn.handle == handle) {
            cn.reports.push((c.value, id));
        }
        if request(handle, &[WRITE_REQ, cccd as u8, (cccd >> 8) as u8, 0x01, 0x00]).is_ok() {
            inputs += 1;
        }
    }
    if inputs == 0 {
        return Err("il dispositivo non manda input".to_string());
    }
    crate::info!("bt", "HID over GATT: report map of {} bytes, {inputs} input reports", map.len());
    Ok(map)
}
