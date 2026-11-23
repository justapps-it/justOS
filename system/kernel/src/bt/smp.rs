//! LE: advertising reports, connections and the Security Manager
//! (core spec vol 3, part H). We are always the central and the
//! initiator: Secure Connections when the device has it (Just Works,
//! numeric comparison, passkey entry), legacy pairing otherwise (Just
//! Works, passkey). The device types the passkey we show; we never type.

use super::crypto;
use super::l2cap::{self, CID_SMP};
use super::{addr_text, ask, changed, command, device_mut, gatt, wait, Addr, Conn, Device, Job, Kind, Ltk, PromptKind, ST};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const PAIRING_REQUEST: u8 = 0x01;
const PAIRING_RESPONSE: u8 = 0x02;
const PAIRING_CONFIRM: u8 = 0x03;
const PAIRING_RANDOM: u8 = 0x04;
const PAIRING_FAILED: u8 = 0x05;
const ENCRYPTION_INFO: u8 = 0x06;
const CENTRAL_IDENT: u8 = 0x07;
const SECURITY_REQUEST: u8 = 0x0B;
const PUBLIC_KEY: u8 = 0x0C;
const DHKEY_CHECK: u8 = 0x0D;

const IO_DISPLAY_YES_NO: u8 = 1;
/// Bonding, MITM protection, Secure Connections.
const AUTH: u8 = 0x01 | 0x04 | 0x08;

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}

/// An LE meta event.
pub fn le_event(p: &[u8]) {
    match p[0] {
        // Connection complete, and its enhanced form.
        0x01 | 0x0A if p.len() >= 12 => {
            let Ok(a) = <Addr>::try_from(&p[6..12]) else { return };
            let mut st = ST.lock();
            if p[1] == 0 {
                let handle = le16(p, 2) & 0x0FFF;
                st.conns.push(Conn::new(handle, a, true));
                crate::info!("bt", "{} connected (LE, link {handle:#x})", addr_text(&a));
            } else {
                st.failed.push((a, p[1]));
            }
            drop(st);
            changed();
        }
        0x02 if p.len() >= 2 => {
            let mut r = &p[2..];
            for _ in 0..p[1] {
                if r.len() < 9 {
                    break;
                }
                let len = r[8] as usize;
                let Some(data) = r.get(9..9 + len) else { break };
                let rssi = r.get(9 + len).copied().unwrap_or(0) as i8;
                let Ok(a) = <Addr>::try_from(&r[2..8]) else { break };
                advertised(a, r[1], data, rssi);
                r = r.get(10 + len..).unwrap_or(&[]);
            }
        }
        // A key asked for as peripheral: we are not one.
        0x05 if p.len() >= 3 => super::job(Job::Command(0x201B, p[1..3].to_vec())),
        _ => {}
    }
}

fn advertised(a: Addr, addr_type: u8, mut ad: &[u8], rssi: i8) {
    let mut st = ST.lock();
    if !st.scanning {
        return;
    }
    let d = device_mut(&mut st, &a, true);
    d.le = true;
    d.addr_type = addr_type & 1;
    d.rssi = rssi;
    d.seen = true;
    while ad.len() >= 2 && ad[0] != 0 {
        let len = ad[0] as usize;
        let Some(f) = ad.get(1..1 + len) else { break };
        match f[0] {
            0x08 if d.name.is_empty() => d.name = String::from_utf8_lossy(&f[1..]).into_owned(),
            0x09 => d.name = String::from_utf8_lossy(&f[1..]).into_owned(),
            0x19 if f.len() >= 3 => {
                let k = Kind::from_appearance(le16(f, 1));
                if k != Kind::Other {
                    d.kind = k;
                    d.hid = true;
                }
            }
            0x02 | 0x03 => {
                if f[1..].chunks_exact(2).any(|u| u == [0x12, 0x18]) {
                    d.hid = true;
                }
            }
            _ => {}
        }
        ad = &ad[1 + len..];
    }
    drop(st);
    changed();
}

/// An SMP packet.
pub fn receive(handle: u16, p: &[u8]) {
    if p.is_empty() {
        return;
    }
    let mut encrypt = false;
    {
        let mut st = ST.lock();
        let Some(ci) = st.conns.iter().position(|c| c.handle == handle) else { return };
        let addr = st.conns[ci].addr;
        let bonded = st.devices.iter().any(|d| d.addr == addr && d.ltk.is_some());
        let c = &mut st.conns[ci];
        if p[0] == SECURITY_REQUEST {
            // The device wants encryption: with the key it has, if bonded.
            encrypt = bonded && !c.encrypted && !c.encrypt_asked;
            c.encrypt_asked |= encrypt;
        } else if c.smp.len() < 8 {
            c.smp.push_back(p.to_vec());
        }
    }
    if encrypt {
        super::job(Job::Encrypt(handle));
    }
    changed();
}

/// The next SMP packet, which must be `code`.
fn recv(handle: u16, code: u8, ms: u64) -> Result<Vec<u8>, String> {
    let p = wait(ms, |st| match st.conns.iter_mut().find(|c| c.handle == handle) {
        None => Some(None),
        Some(c) => c.smp.pop_front().map(Some),
    });
    match p {
        None => Err("il dispositivo non risponde".to_string()),
        Some(None) => Err("il dispositivo si è scollegato".to_string()),
        Some(Some(p)) if p[0] == PAIRING_FAILED => Err(failed_text(p.get(1).copied().unwrap_or(0))),
        Some(Some(p)) if p[0] == code && p.len() >= expected_len(code) => Ok(p),
        Some(Some(p)) => Err(format!("risposta inattesa ({:02x})", p[0])),
    }
}

fn expected_len(code: u8) -> usize {
    match code {
        PAIRING_RESPONSE => 7,
        PAIRING_CONFIRM | PAIRING_RANDOM | DHKEY_CHECK | ENCRYPTION_INFO => 17,
        CENTRAL_IDENT => 11,
        PUBLIC_KEY => 65,
        _ => 1,
    }
}

fn failed_text(reason: u8) -> String {
    match reason {
        0x01 => "codice sbagliato".to_string(),
        0x03 | 0x0B => "il dispositivo non vuole associarsi".to_string(),
        0x04 => "conferma non valida".to_string(),
        0x05 => "il dispositivo non regge la sicurezza richiesta".to_string(),
        0x08 => "il dispositivo ha già troppe associazioni".to_string(),
        0x0C => "i numeri non coincidono".to_string(),
        r => format!("associazione rifiutata (codice {r:02x})"),
    }
}

fn send(handle: u16, code: u8, data: &[u8]) {
    let mut p = Vec::with_capacity(1 + data.len());
    p.push(code);
    p.extend_from_slice(data);
    l2cap::send(handle, CID_SMP, &p);
}

fn fail(handle: u16, reason: u8, text: &str) -> String {
    send(handle, PAIRING_FAILED, &[reason]);
    text.to_string()
}

fn random16() -> [u8; 16] {
    let mut r = [0u8; 16];
    crate::rand::fill(&mut r);
    r
}

fn arr<const N: usize>(p: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&p[..N]);
    a
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    JustWorks,
    Compare,
    /// We show the passkey, the device types it.
    Passkey,
}

/// The association model for us (display + yes/no, initiator) and a
/// device with IO capability `io`.
fn method(io: u8, sc: bool) -> Method {
    match io {
        1 if sc => Method::Compare,
        2 => Method::Passkey,
        4 if sc => Method::Compare,
        4 => Method::Passkey,
        _ => Method::JustWorks,
    }
}

/// Waits for the user's answer to prompt `id`.
fn user(id: u32, ms: u64) -> bool {
    wait(ms, |st| st.answer.filter(|a| a.0 == id).map(|a| a.1)).unwrap_or(false)
}

/// A key cut to the agreed size.
fn sized(mut key: [u8; 16], size: u8) -> [u8; 16] {
    for b in key.iter_mut().skip(size.clamp(7, 16) as usize) {
        *b = 0;
    }
    key
}

/// Pairs on link `handle`: the key, with the link encrypted.
fn pair_link(handle: u16, d: &Device) -> Result<Ltk, String> {
    if !crypto::selftest() {
        return Err("la crittografia non funziona su questo sistema".to_string());
    }
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.smp.clear();
    }
    let preq: [u8; 7] = [PAIRING_REQUEST, IO_DISPLAY_YES_NO, 0, AUTH, 16, 0x00, 0x01];
    l2cap::send(handle, CID_SMP, &preq);
    let pres: [u8; 7] = arr(&recv(handle, PAIRING_RESPONSE, 30_000)?);
    let sc = pres[3] & 0x08 != 0;
    let size = pres[4].min(16);
    if size < 7 {
        return Err(fail(handle, 0x06, "chiave troppo corta"));
    }
    let m = method(pres[1], sc);
    let ia = ST.lock().addr;
    let ra = d.addr;
    let rat = d.addr_type;
    crate::info!("bt", "pairing {}: {}, {}", addr_text(&ra), if sc { "Secure Connections" } else { "legacy" }, match m {
        Method::JustWorks => "Just Works",
        Method::Compare => "numeric comparison",
        Method::Passkey => "passkey",
    });
    if sc {
        let ltk = sc_pair(handle, &preq, &pres, m, ia, ra, rat)?;
        let ltk = Ltk { key: sized(ltk, size), ediv: 0, rand: [0; 8] };
        encrypt(handle, &ltk)?;
        Ok(ltk)
    } else {
        legacy_pair(handle, &preq, &pres, m, ia, ra, rat, size)
    }
}

#[allow(clippy::too_many_arguments)]
fn sc_pair(handle: u16, preq: &[u8; 7], pres: &[u8; 7], m: Method, ia: Addr, ra: Addr, rat: u8) -> Result<[u8; 16], String> {
    let kp = crypto::keypair();
    send(handle, PUBLIC_KEY, &kp.public);
    let peer: [u8; 64] = arr(&recv(handle, PUBLIC_KEY, 10_000)?[1..]);
    let dh = crypto::dhkey(&kp, &peer).ok_or_else(|| fail(handle, 0x0B, "chiave pubblica non valida"))?;
    let pka: [u8; 32] = arr(&kp.public[..32]);
    let pkb: [u8; 32] = arr(&peer[..32]);
    let (na, nb, r) = if m == Method::Passkey {
        let passkey = (crate::rand::u64() % 1_000_000) as u32;
        ask(ra, PromptKind::Display(passkey), false);
        let (mut na, mut nb) = ([0u8; 16], [0u8; 16]);
        for i in 0..20 {
            let ri = 0x80 | ((passkey >> i) & 1) as u8;
            na = random16();
            send(handle, PAIRING_CONFIRM, &crypto::f4(&pka, &pkb, &na, ri));
            // The first answer waits for the person typing.
            let cb: [u8; 16] = arr(&recv(handle, PAIRING_CONFIRM, if i == 0 { 90_000 } else { 10_000 })?[1..]);
            send(handle, PAIRING_RANDOM, &na);
            nb = arr(&recv(handle, PAIRING_RANDOM, 10_000)?[1..]);
            if crypto::f4(&pkb, &pka, &nb, ri) != cb {
                return Err(fail(handle, 0x04, "codice sbagliato"));
            }
        }
        let mut r = [0u8; 16];
        r[..4].copy_from_slice(&passkey.to_le_bytes());
        (na, nb, r)
    } else {
        let cb: [u8; 16] = arr(&recv(handle, PAIRING_CONFIRM, 10_000)?[1..]);
        let na = random16();
        send(handle, PAIRING_RANDOM, &na);
        let nb: [u8; 16] = arr(&recv(handle, PAIRING_RANDOM, 10_000)?[1..]);
        if crypto::f4(&pkb, &pka, &nb, 0) != cb {
            return Err(fail(handle, 0x04, "conferma non valida"));
        }
        if m == Method::Compare {
            let id = ask(ra, PromptKind::Confirm(crypto::g2(&pka, &pkb, &na, &nb)), false);
            if !user(id, 60_000) {
                return Err(fail(handle, 0x0C, "associazione annullata"));
            }
        }
        (na, nb, [0u8; 16])
    };
    let mut a = [0u8; 7];
    a[..6].copy_from_slice(&ia);
    let mut b = [0u8; 7];
    b[..6].copy_from_slice(&ra);
    b[6] = rat;
    let (mackey, ltk) = crypto::f5(&dh, &na, &nb, &a, &b);
    let ea = crypto::f6(&mackey, &na, &nb, &r, &[preq[1], preq[2], preq[3]], &a, &b);
    send(handle, DHKEY_CHECK, &ea);
    let eb: [u8; 16] = arr(&recv(handle, DHKEY_CHECK, 10_000)?[1..]);
    if crypto::f6(&mackey, &nb, &na, &r, &[pres[1], pres[2], pres[3]], &b, &a) != eb {
        return Err(fail(handle, 0x0B, "verifica della chiave non riuscita"));
    }
    Ok(ltk)
}

#[allow(clippy::too_many_arguments)]
fn legacy_pair(handle: u16, preq: &[u8; 7], pres: &[u8; 7], m: Method, ia: Addr, ra: Addr, rat: u8, size: u8) -> Result<Ltk, String> {
    let mut tk = [0u8; 16];
    if m == Method::Passkey {
        let passkey = (crate::rand::u64() % 1_000_000) as u32;
        tk[..4].copy_from_slice(&passkey.to_le_bytes());
        ask(ra, PromptKind::Display(passkey), false);
    }
    let mrand = random16();
    send(handle, PAIRING_CONFIRM, &crypto::c1(&tk, &mrand, preq, pres, 0, &ia, rat, &ra));
    let sconfirm: [u8; 16] = arr(&recv(handle, PAIRING_CONFIRM, 90_000)?[1..]);
    send(handle, PAIRING_RANDOM, &mrand);
    let srand: [u8; 16] = arr(&recv(handle, PAIRING_RANDOM, 10_000)?[1..]);
    if crypto::c1(&tk, &srand, preq, pres, 0, &ia, rat, &ra) != sconfirm {
        return Err(fail(handle, 0x04, "conferma non valida"));
    }
    let stk = Ltk { key: sized(crypto::s1(&tk, &srand, &mrand), size), ediv: 0, rand: [0; 8] };
    encrypt(handle, &stk)?;
    // On the encrypted link the device hands its long-term key.
    if pres[6] & 1 == 0 {
        return Err("il dispositivo non dà una chiave".to_string());
    }
    let info = recv(handle, ENCRYPTION_INFO, 5000)?;
    let ident = recv(handle, CENTRAL_IDENT, 5000)?;
    Ok(Ltk { key: arr(&info[1..]), ediv: le16(&ident, 1), rand: arr(&ident[3..]) })
}

/// Encrypts link `handle` with `key` and waits for it.
fn encrypt(handle: u16, key: &Ltk) -> Result<(), String> {
    if let Some(c) = ST.lock().conns.iter_mut().find(|c| c.handle == handle) {
        c.enc = None;
    }
    let mut p = Vec::from(handle.to_le_bytes());
    p.extend_from_slice(&key.rand);
    p.extend_from_slice(&key.ediv.to_le_bytes());
    p.extend_from_slice(&key.key);
    command(0x2019, &p).map_err(|e| format!("cifratura rifiutata (codice {e:02x})"))?;
    match wait(10_000, |st| match st.conns.iter().find(|c| c.handle == handle) {
        None => Some(None),
        Some(c) => c.enc.map(|e| Some((e, c.encrypted))),
    }) {
        Some(Some((0, true))) => Ok(()),
        Some(Some((0x06, _))) => Err("chiave rifiutata: il dispositivo va associato di nuovo".to_string()),
        Some(Some((e, _))) => Err(format!("cifratura non riuscita (codice {e:02x})")),
        Some(None) => Err("il dispositivo si è scollegato".to_string()),
        None => Err("cifratura: tempo scaduto".to_string()),
    }
}

/// From the events thread, when a bonded device asks for security: the
/// command only (the change comes as an event to that same thread).
pub fn encrypt_stored(handle: u16) {
    let key = {
        let st = ST.lock();
        let Some(c) = st.conns.iter().find(|c| c.handle == handle) else { return };
        st.devices.iter().find(|d| d.addr == c.addr).and_then(|d| d.ltk)
    };
    if let Some(k) = key {
        let mut p = Vec::from(handle.to_le_bytes());
        p.extend_from_slice(&k.rand);
        p.extend_from_slice(&k.ediv.to_le_bytes());
        p.extend_from_slice(&k.key);
        let _ = command(0x2019, &p);
    }
}

fn link_of(st: &super::State, a: &Addr) -> Option<u16> {
    st.conns.iter().find(|c| c.addr == *a && c.le).map(|c| c.handle)
}

/// An LE link to the device, made if there is none.
fn link(d: &Device) -> Result<u16, String> {
    if let Some(h) = link_of(&ST.lock(), &d.addr) {
        return Ok(h);
    }
    ST.lock().failed.retain(|f| f.0 != d.addr);
    let mut c = alloc::vec![0x60, 0x00, 0x30, 0x00, 0x00, d.addr_type];
    c.extend_from_slice(&d.addr);
    c.push(0); // our public address
    // Interval 30-50 ms, no latency, 5 s supervision timeout.
    c.extend_from_slice(&[0x18, 0x00, 0x28, 0x00, 0x00, 0x00, 0xF4, 0x01, 0x00, 0x00, 0x00, 0x00]);
    command(0x200D, &c).map_err(|e| format!("il controller non si collega (codice {e:02x})"))?;
    let r = wait(10_000, |st| {
        if let Some(h) = link_of(st, &d.addr) {
            return Some(Ok(h));
        }
        st.failed.iter().find(|f| f.0 == d.addr).map(|f| Err(f.1))
    });
    match r {
        Some(Ok(h)) => Ok(h),
        Some(Err(e)) => Err(format!("collegamento non riuscito (codice {e:02x})")),
        None => {
            let _ = command(0x200E, &[]);
            Err("il dispositivo non risponde: è acceso e vicino?".to_string())
        }
    }
}

fn drop_link(handle: u16) {
    let mut p = Vec::from(handle.to_le_bytes());
    p.push(0x13);
    let _ = command(0x0406, &p);
}

/// Pairs with an LE device and starts its input.
pub fn pair(d: &Device) -> Result<(), String> {
    let handle = link(d)?;
    let r = (|| {
        let ltk = pair_link(handle, d)?;
        let map = gatt::hogp(handle)?;
        let mut st = ST.lock();
        let dev = device_mut(&mut st, &d.addr, true);
        dev.paired = true;
        dev.hid = true;
        dev.ltk = Some(ltk);
        if dev.kind == Kind::Other {
            dev.kind = super::classic::kind_of_map(&map);
        }
        dev.report_map = map;
        Ok(())
    })();
    super::clear_prompt(&d.addr);
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

/// Connects a bonded LE device again.
pub fn connect(d: &Device) -> Result<(), String> {
    let key = d.ltk.ok_or("nessuna chiave: il dispositivo va associato di nuovo")?;
    let handle = link(d)?;
    let r = encrypt(handle, &key).and_then(|_| gatt::hogp(handle).map(|_| ()));
    if r.is_err() {
        drop_link(handle);
    }
    changed();
    r
}
