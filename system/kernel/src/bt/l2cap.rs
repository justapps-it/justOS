//! L2CAP (core spec vol 3, part A): ACL packets put together, fixed
//! channels (signalling, ATT, SMP) and the dynamic channels of BR/EDR
//! (SDP, HID control and interrupt), in basic mode with the default MTU.
//!
//! What the other side asks is answered right away from the receive
//! thread; what we ask waits in the procedure's thread ([`open`]).

use super::{changed, gatt, hid, smp, wait, Chan, Job, ST};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const CID_SIGNAL: u16 = 0x0001;
pub const CID_ATT: u16 = 0x0004;
pub const CID_LE_SIGNAL: u16 = 0x0005;
pub const CID_SMP: u16 = 0x0006;

pub const PSM_SDP: u16 = 0x0001;
pub const PSM_HID_CONTROL: u16 = 0x0011;
pub const PSM_HID_INTERRUPT: u16 = 0x0013;

const CONN_REQ: u8 = 0x02;
const CONN_RSP: u8 = 0x03;
const CONF_REQ: u8 = 0x04;
const CONF_RSP: u8 = 0x05;
const DISC_REQ: u8 = 0x06;
const DISC_RSP: u8 = 0x07;
const ECHO_REQ: u8 = 0x08;
const INFO_REQ: u8 = 0x0A;
const CONN_PARAM_REQ: u8 = 0x12;

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}

/// An ACL packet from the controller.
pub fn receive(p: &[u8]) {
    if p.len() < 4 {
        return;
    }
    let hf = le16(p, 0);
    let handle = hf & 0x0FFF;
    let pb = (hf >> 12) & 3;
    let len = le16(p, 2) as usize;
    let Some(data) = p.get(4..4 + len) else { return };
    let pdu = {
        let mut st = ST.lock();
        let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) else { return };
        if pb == 1 {
            c.rx_buf.extend_from_slice(data);
        } else {
            c.rx_buf.clear();
            c.rx_buf.extend_from_slice(data);
        }
        if c.rx_buf.len() < 4 {
            return;
        }
        let want = 4 + le16(&c.rx_buf, 0) as usize;
        if c.rx_buf.len() < want {
            if c.rx_buf.len() > 65539 {
                c.rx_buf.clear();
            }
            return;
        }
        let mut pdu = core::mem::take(&mut c.rx_buf);
        pdu.truncate(want);
        pdu
    };
    let cid = le16(&pdu, 2);
    let payload = &pdu[4..];
    match cid {
        CID_SIGNAL => signal(handle, payload),
        CID_LE_SIGNAL => le_signal(handle, payload),
        CID_ATT => gatt::receive(handle, payload),
        CID_SMP => smp::receive(handle, payload),
        _ => dynamic(handle, cid, payload),
    }
}

/// Sends an L2CAP packet on channel `cid` of link `handle`, in ACL
/// packets the controller takes.
pub fn send(handle: u16, cid: u16, payload: &[u8]) -> bool {
    let (mtu, le) = {
        let st = ST.lock();
        let Some(c) = st.conns.iter().find(|c| c.handle == handle) else { return false };
        (if c.le { st.le_mtu } else { st.acl_mtu }, c.le)
    };
    let mut pdu = Vec::with_capacity(4 + payload.len());
    pdu.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    pdu.extend_from_slice(&cid.to_le_bytes());
    pdu.extend_from_slice(payload);
    for (i, chunk) in pdu.chunks(mtu.max(1)).enumerate() {
        // First packet: not flushable on LE, flushable on BR/EDR; then
        // continuations.
        let pb: u16 = if i > 0 {
            1
        } else if le {
            0
        } else {
            2
        };
        let mut acl = Vec::with_capacity(4 + chunk.len());
        acl.extend_from_slice(&(handle | pb << 12).to_le_bytes());
        acl.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        acl.extend_from_slice(chunk);
        if super::send_acl_raw(&acl) != justdk::OK {
            return false;
        }
    }
    true
}

fn sig_send(handle: u16, cid: u16, code: u8, id: u8, data: &[u8]) {
    let mut p = Vec::with_capacity(4 + data.len());
    p.push(code);
    p.push(id);
    p.extend_from_slice(&(data.len() as u16).to_le_bytes());
    p.extend_from_slice(data);
    send(handle, cid, &p);
}

fn next_id(handle: u16) -> u8 {
    let mut st = ST.lock();
    let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) else { return 1 };
    let id = c.next_id;
    c.next_id = if c.next_id >= 0xFF { 1 } else { c.next_id + 1 };
    id
}

/// The BR/EDR signalling channel.
fn signal(handle: u16, mut p: &[u8]) {
    while p.len() >= 4 {
        let (code, id, len) = (p[0], p[1], le16(p, 2) as usize);
        let Some(d) = p.get(4..4 + len) else { return };
        match code {
            CONN_REQ if d.len() >= 4 => conn_request(handle, id, le16(d, 0), le16(d, 2)),
            CONF_REQ if d.len() >= 4 => {
                let local = le16(d, 0);
                let remote = {
                    let mut st = ST.lock();
                    let ch = st.conns.iter_mut().find(|c| c.handle == handle).and_then(|c| c.chans.iter_mut().find(|ch| ch.local == local));
                    ch.map(|ch| {
                        ch.in_done = true;
                        ch.remote
                    })
                };
                match remote {
                    // Whatever they want (MTU, flush timeout) is fine.
                    Some(r) => sig_send(handle, CID_SIGNAL, CONF_RSP, id, &[r as u8, (r >> 8) as u8, 0, 0, 0, 0]),
                    None => sig_send(handle, CID_SIGNAL, 0x01, id, &[2, 0, d[0], d[1], 0, 0]),
                }
                changed();
            }
            DISC_REQ if d.len() >= 4 => {
                let (local, remote) = (le16(d, 0), le16(d, 2));
                let hid_gone = {
                    let mut st = ST.lock();
                    let mut gone = false;
                    if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
                        if c.chans.iter().any(|ch| ch.local == local && ch.psm == PSM_HID_INTERRUPT) {
                            gone = true;
                        }
                        c.chans.retain(|ch| ch.local != local);
                    }
                    gone
                };
                let mut r = Vec::from(local.to_le_bytes());
                r.extend_from_slice(&remote.to_le_bytes());
                sig_send(handle, CID_SIGNAL, DISC_RSP, id, &r);
                if hid_gone {
                    let h = ST.lock().conns.iter_mut().find(|c| c.handle == handle).and_then(|c| c.hid.take());
                    hid::release(h);
                }
                changed();
            }
            ECHO_REQ => sig_send(handle, CID_SIGNAL, 0x09, id, d),
            INFO_REQ if d.len() >= 2 => match le16(d, 0) {
                // Extended features: none; fixed channels: signalling.
                2 => sig_send(handle, CID_SIGNAL, 0x0B, id, &[2, 0, 0, 0, 0, 0, 0, 0]),
                3 => sig_send(handle, CID_SIGNAL, 0x0B, id, &[3, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0]),
                t => sig_send(handle, CID_SIGNAL, 0x0B, id, &[t as u8, (t >> 8) as u8, 1, 0]),
            },
            // "Pending": the real answer follows.
            CONN_RSP if d.len() >= 8 && le16(d, 4) == 1 => {}
            // Answers to what we asked.
            0x01 | CONN_RSP | CONF_RSP | DISC_RSP | 0x0B => {
                let mut st = ST.lock();
                if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
                    // The other side's channel id, at once: its own
                    // configuration request may come right after.
                    if code == CONN_RSP && d.len() >= 8 && le16(d, 4) == 0 {
                        if let Some(ch) = c.chans.iter_mut().find(|ch| ch.local == le16(d, 2)) {
                            ch.remote = le16(d, 0);
                        }
                    }
                    if c.sig.len() >= 16 {
                        c.sig.pop_front();
                    }
                    c.sig.push_back(p[..4 + len].to_vec());
                }
                drop(st);
                changed();
            }
            _ => sig_send(handle, CID_SIGNAL, 0x01, id, &[0, 0]),
        }
        p = &p[4 + len..];
    }
}

/// The other side opens a channel: HID channels of a paired device (it
/// reconnects); nothing else.
fn conn_request(handle: u16, id: u8, psm: u16, scid: u16) {
    let local = {
        let mut st = ST.lock();
        let Some(ci) = st.conns.iter().position(|c| c.handle == handle) else { return };
        let addr = st.conns[ci].addr;
        let dev = st.devices.iter().find(|d| d.addr == addr && d.paired).map(|d| (d.report_map.clone(), d.kind));
        match dev {
            Some((map, kind)) if psm == PSM_HID_CONTROL || psm == PSM_HID_INTERRUPT => {
                let c = &mut st.conns[ci];
                let local = c.next_cid;
                c.next_cid += 1;
                c.chans.push(Chan { psm, local, remote: scid, out_done: false, in_done: false, rx: Default::default() });
                if psm == PSM_HID_INTERRUPT && c.hid.is_none() {
                    c.hid = Some(super::classic::hid_input(&map, kind));
                }
                Some(local)
            }
            _ => None,
        }
    };
    let mut r = Vec::with_capacity(8);
    r.extend_from_slice(&local.unwrap_or(0).to_le_bytes());
    r.extend_from_slice(&scid.to_le_bytes());
    // Result: success, or PSM not supported.
    r.extend_from_slice(&(if local.is_some() { 0u16 } else { 2 }).to_le_bytes());
    r.extend_from_slice(&[0, 0]);
    sig_send(handle, CID_SIGNAL, CONN_RSP, id, &r);
    if local.is_some() {
        let cid = next_id(handle);
        sig_send(handle, CID_SIGNAL, CONF_REQ, cid, &[scid as u8, (scid >> 8) as u8, 0, 0]);
        crate::info!("bt", "channel {psm:#06x} opened by the device");
    }
    changed();
}

/// The LE signalling channel: connection parameter updates.
fn le_signal(handle: u16, p: &[u8]) {
    if p.len() < 4 {
        return;
    }
    let (code, id) = (p[0], p[1]);
    let d = &p[4..];
    match code {
        CONN_PARAM_REQ if d.len() >= 8 => {
            sig_send(handle, CID_LE_SIGNAL, 0x13, id, &[0, 0]);
            let mut c = Vec::from(handle.to_le_bytes());
            c.extend_from_slice(&d[..8]);
            c.extend_from_slice(&[0, 0, 0, 0]);
            super::job(Job::Command(0x2013, c));
        }
        0x01 | 0x13 => {}
        _ => sig_send(handle, CID_LE_SIGNAL, 0x01, id, &[0, 0]),
    }
}

/// Data on a dynamic channel: HID reports become input, the rest waits
/// for its reader.
fn dynamic(handle: u16, cid: u16, p: &[u8]) {
    let psm = {
        let mut st = ST.lock();
        let Some(ch) = st.conns.iter_mut().find(|c| c.handle == handle).and_then(|c| c.chans.iter_mut().find(|ch| ch.local == cid)) else { return };
        if ch.psm != PSM_HID_INTERRUPT && ch.rx.len() < 16 {
            ch.rx.push_back(p.to_vec());
        }
        ch.psm
    };
    if psm == PSM_HID_INTERRUPT {
        // DATA | Input.
        if p.first() == Some(&0xA1) {
            hid::report(handle, &p[1..]);
        }
    } else {
        changed();
    }
}

/// Waits for the answer to our request `id`.
fn answer(handle: u16, id: u8, ms: u64, mut want: impl FnMut(&[u8]) -> bool) -> Option<Vec<u8>> {
    wait(ms, |st| {
        let c = st.conns.iter_mut().find(|c| c.handle == handle)?;
        let i = c.sig.iter().position(|s| s[1] == id && want(s))?;
        c.sig.remove(i)
    })
}

/// Opens a channel to `psm` and configures it both ways: our channel id.
pub fn open(handle: u16, psm: u16) -> Result<u16, String> {
    let local = {
        let mut st = ST.lock();
        let c = st.conns.iter_mut().find(|c| c.handle == handle).ok_or("collegamento perso")?;
        let local = c.next_cid;
        c.next_cid += 1;
        c.chans.push(Chan { psm, local, remote: 0, out_done: false, in_done: false, rx: Default::default() });
        local
    };
    let fail = |e: &str| {
        let mut st = ST.lock();
        if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
            c.chans.retain(|ch| ch.local != local);
        }
        e.to_string()
    };
    let id = next_id(handle);
    let mut r = Vec::from(psm.to_le_bytes());
    r.extend_from_slice(&local.to_le_bytes());
    sig_send(handle, CID_SIGNAL, CONN_REQ, id, &r);
    // "Pending" answers (left out by `signal`) come first while the other
    // side checks security: a longer wait.
    let Some(rsp) = answer(handle, id, 10_000, |s| s[0] == 0x01 || s[0] == CONN_RSP) else { return Err(fail("il dispositivo non apre il canale")) };
    if rsp[0] != CONN_RSP || le16(&rsp, 8) != 0 {
        return Err(fail("il dispositivo rifiuta il canale"));
    }
    let remote = le16(&rsp, 4);
    {
        let mut st = ST.lock();
        if let Some(ch) = st.conns.iter_mut().find(|c| c.handle == handle).and_then(|c| c.chans.iter_mut().find(|ch| ch.local == local)) {
            ch.remote = remote;
        }
    }
    let id = next_id(handle);
    sig_send(handle, CID_SIGNAL, CONF_REQ, id, &[remote as u8, (remote >> 8) as u8, 0, 0]);
    let Some(rsp) = answer(handle, id, 5000, |s| s[0] == CONF_RSP || s[0] == 0x01) else { return Err(fail("il dispositivo non configura il canale")) };
    if rsp[0] != CONF_RSP || le16(&rsp, 8) != 0 {
        return Err(fail("configurazione del canale rifiutata"));
    }
    let ok = wait(5000, |st| {
        let ch = st.conns.iter_mut().find(|c| c.handle == handle)?.chans.iter_mut().find(|ch| ch.local == local)?;
        ch.out_done = true;
        ch.in_done.then_some(())
    });
    if ok.is_none() {
        return Err(fail("il dispositivo non configura il canale"));
    }
    Ok(local)
}

/// Closes our channel `local`.
pub fn close(handle: u16, local: u16) {
    let remote = {
        let st = ST.lock();
        st.conns.iter().find(|c| c.handle == handle).and_then(|c| c.chans.iter().find(|ch| ch.local == local)).map(|ch| ch.remote)
    };
    if let Some(remote) = remote {
        let id = next_id(handle);
        let mut r = Vec::from(remote.to_le_bytes());
        r.extend_from_slice(&local.to_le_bytes());
        sig_send(handle, CID_SIGNAL, DISC_REQ, id, &r);
        let _ = answer(handle, id, 2000, |s| s[0] == DISC_RSP || s[0] == 0x01);
    }
    let mut st = ST.lock();
    if let Some(c) = st.conns.iter_mut().find(|c| c.handle == handle) {
        c.chans.retain(|ch| ch.local != local);
    }
}

/// Sends on our channel `local`.
pub fn chan_send(handle: u16, local: u16, data: &[u8]) -> bool {
    let remote = {
        let st = ST.lock();
        st.conns.iter().find(|c| c.handle == handle).and_then(|c| c.chans.iter().find(|ch| ch.local == local)).map(|ch| ch.remote)
    };
    remote.is_some_and(|r| send(handle, r, data))
}

/// The next packet on our channel `local`.
pub fn chan_recv(handle: u16, local: u16, ms: u64) -> Option<Vec<u8>> {
    wait(ms, |st| st.conns.iter_mut().find(|c| c.handle == handle)?.chans.iter_mut().find(|ch| ch.local == local)?.rx.pop_front())
}
