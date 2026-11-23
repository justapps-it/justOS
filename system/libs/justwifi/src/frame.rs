//! IEEE 802.11 frames: the MAC header, the information elements of beacons
//! and probe responses, the management frames a station sends, and the
//! conversion of data frames to and from Ethernet.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub type Mac = [u8; 6];
pub const BROADCAST: Mac = [0xFF; 6];

// Frame types and subtypes (type << 4 | subtype).
pub const ASSOC_REQ: u8 = 0x00;
pub const ASSOC_RESP: u8 = 0x01;
pub const REASSOC_RESP: u8 = 0x03;
pub const PROBE_REQ: u8 = 0x04;
pub const PROBE_RESP: u8 = 0x05;
pub const BEACON: u8 = 0x08;
pub const DISASSOC: u8 = 0x0A;
pub const AUTH: u8 = 0x0B;
pub const DEAUTH: u8 = 0x0C;
pub const ACTION: u8 = 0x0D;
pub const DATA: u8 = 0x20;
pub const QOS_DATA: u8 = 0x28;

// Element ids.
pub const IE_SSID: u8 = 0;
pub const IE_RATES: u8 = 1;
pub const IE_DS: u8 = 3;
pub const IE_HT_CAP: u8 = 45;
pub const IE_RSN: u8 = 48;
pub const IE_EXT_RATES: u8 = 50;
pub const IE_HT_OP: u8 = 61;
pub const IE_RSNX: u8 = 244;
pub const IE_VENDOR: u8 = 221;

/// A parsed MAC header.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    /// Type and subtype (`type << 4 | subtype`, as the constants above).
    pub kind: u8,
    pub flags: u8,
    pub addr1: Mac,
    pub addr2: Mac,
    pub addr3: Mac,
    pub seq: u16,
    /// Bytes of the header (24 to 34).
    pub len: usize,
    /// The QoS control's TID and A-MSDU bit, for QoS data.
    pub qos: Option<u16>,
}

pub const FLAG_TO_DS: u8 = 0x01;
pub const FLAG_FROM_DS: u8 = 0x02;
pub const FLAG_RETRY: u8 = 0x08;
pub const FLAG_PROTECTED: u8 = 0x40;
pub const FLAG_ORDER: u8 = 0x80;

fn mac_at(b: &[u8], o: usize) -> Mac {
    b[o..o + 6].try_into().unwrap()
}

pub fn parse(f: &[u8]) -> Option<Header> {
    if f.len() < 24 {
        return None;
    }
    let fc0 = f[0];
    if fc0 & 3 != 0 {
        return None; // protocol version 0 only
    }
    let ty = (fc0 >> 2) & 3;
    let kind = (ty << 4) | (fc0 >> 4);
    let flags = f[1];
    let mut len = 24;
    if ty == 2 && flags & 3 == 3 {
        len += 6;
    }
    let mut qos = None;
    if ty == 2 && fc0 & 0x80 != 0 {
        if f.len() < len + 2 {
            return None;
        }
        qos = Some(u16::from_le_bytes([f[len], f[len + 1]]));
        len += 2;
        if flags & FLAG_ORDER != 0 {
            len += 4; // HT control
        }
    } else if ty == 0 && flags & FLAG_ORDER != 0 {
        len += 4;
    }
    if f.len() < len || ty == 1 || ty == 3 {
        return None; // control frames are the card's business
    }
    Some(Header {
        kind,
        flags,
        addr1: mac_at(f, 4),
        addr2: mac_at(f, 10),
        addr3: mac_at(f, 16),
        seq: u16::from_le_bytes([f[22], f[23]]) >> 4,
        len,
        qos,
    })
}

/// A management frame's header.
pub fn mgmt_header(kind: u8, da: &Mac, sa: &Mac, bssid: &Mac, seq: u16) -> Vec<u8> {
    let mut h = vec![(kind & 0x0F) << 4, 0, 0, 0];
    h.extend_from_slice(da);
    h.extend_from_slice(sa);
    h.extend_from_slice(bssid);
    h.extend_from_slice(&(seq << 4).to_le_bytes());
    h
}

/// The information elements of a body: (id, data).
pub fn elements(mut b: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        if b.len() < 2 || b.len() < 2 + b[1] as usize {
            return None;
        }
        let (id, len) = (b[0], b[1] as usize);
        let data = &b[2..2 + len];
        b = &b[2 + len..];
        Some((id, data))
    })
}

pub fn push_element(out: &mut Vec<u8>, id: u8, data: &[u8]) {
    out.push(id);
    out.push(data.len() as u8);
    out.extend_from_slice(data);
}

// ----------------------------------------------------------------------------
// RSN

pub const OUI_IEEE: [u8; 3] = [0x00, 0x0F, 0xAC];
pub const CIPHER_TKIP: u8 = 2;
pub const CIPHER_CCMP: u8 = 4;
pub const CIPHER_BIP_CMAC: u8 = 6;
pub const AKM_8021X: u8 = 1;
pub const AKM_PSK: u8 = 2;
pub const AKM_PSK_SHA256: u8 = 6;
pub const AKM_SAE: u8 = 8;

/// What an RSN element says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rsn {
    pub group: u8,
    pub pairwise: Vec<u8>,
    pub akms: Vec<u8>,
    pub caps: u16,
    /// The group management cipher (BIP), when given.
    pub group_mgmt: Option<u8>,
}

pub const RSN_CAP_MFPR: u16 = 1 << 6;
pub const RSN_CAP_MFPC: u16 = 1 << 7;

impl Rsn {
    pub fn parse(d: &[u8]) -> Option<Rsn> {
        if d.len() < 2 || u16::from_le_bytes([d[0], d[1]]) != 1 {
            return None;
        }
        let mut r = Rsn { group: CIPHER_CCMP, pairwise: vec![CIPHER_CCMP], akms: vec![AKM_8021X], ..Rsn::default() };
        let mut i = 2;
        // Suites of other organisations count as "unknown" (0xFF).
        let suite = |s: &[u8]| if s[..3] == OUI_IEEE { s[3] } else { 0xFF };
        if d.len() >= i + 4 {
            r.group = suite(&d[i..i + 4]);
            i += 4;
        }
        for list in 0..2 {
            if d.len() < i + 2 {
                return Some(r);
            }
            let n = u16::from_le_bytes([d[i], d[i + 1]]) as usize;
            i += 2;
            if d.len() < i + 4 * n {
                return None;
            }
            let v: Vec<u8> = (0..n).map(|k| suite(&d[i + 4 * k..i + 4 * k + 4])).collect();
            i += 4 * n;
            if list == 0 {
                r.pairwise = v;
            } else {
                r.akms = v;
            }
        }
        if d.len() >= i + 2 {
            r.caps = u16::from_le_bytes([d[i], d[i + 1]]);
            i += 2;
        }
        if d.len() >= i + 2 {
            let n = u16::from_le_bytes([d[i], d[i + 1]]) as usize;
            i += 2 + 16 * n; // PMKIDs
        }
        if d.len() >= i + 4 {
            r.group_mgmt = Some(suite(&d[i..i + 4]));
        }
        Some(r)
    }

    /// The element's data (without id and length).
    pub fn build(&self) -> Vec<u8> {
        let mut d = vec![1, 0];
        d.extend_from_slice(&OUI_IEEE);
        d.push(self.group);
        for list in [&self.pairwise, &self.akms] {
            d.extend_from_slice(&(list.len() as u16).to_le_bytes());
            for s in list.iter() {
                d.extend_from_slice(&OUI_IEEE);
                d.push(*s);
            }
        }
        d.extend_from_slice(&self.caps.to_le_bytes());
        if let Some(g) = self.group_mgmt {
            d.extend_from_slice(&[0, 0]);
            d.extend_from_slice(&OUI_IEEE);
            d.push(g);
        }
        d
    }
}

// ----------------------------------------------------------------------------
// Networks seen

/// The security of a network, as a station sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WEP or WPA (TKIP only): too old, justOS does not join them.
    Legacy,
    Wpa2,
    /// WPA2 and WPA3 together (transition mode).
    Wpa2Wpa3,
    Wpa3,
    /// 802.1X (company networks): not supported yet.
    Enterprise,
}

impl Security {
    pub fn name(self) -> &'static str {
        match self {
            Security::Open => "aperta",
            Security::Legacy => "WEP/WPA",
            Security::Wpa2 => "WPA2",
            Security::Wpa2Wpa3 => "WPA2/WPA3",
            Security::Wpa3 => "WPA3",
            Security::Enterprise => "WPA2 Enterprise",
        }
    }

    pub fn needs_password(self) -> bool {
        matches!(self, Security::Wpa2 | Security::Wpa2Wpa3 | Security::Wpa3)
    }
}

/// A network (BSS) heard in a beacon or probe response.
#[derive(Clone, Debug)]
pub struct Bss {
    pub bssid: Mac,
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub rssi: i8,
    pub capability: u16,
    pub beacon_interval: u16,
    pub rsn: Option<Rsn>,
    /// Hash-to-element is offered (RSNXE), or required for SAE.
    pub h2e: bool,
    pub wpa1: bool,
    pub ht: bool,
    pub wmm: bool,
    pub rates: Vec<u8>,
    /// When it was last heard (ms).
    pub seen: u64,
}

impl Bss {
    /// The SSID as text (invalid UTF-8 shown with replacement characters).
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.ssid).into_owned()
    }

    pub fn security(&self) -> Security {
        match &self.rsn {
            Some(r) => {
                let psk = r.akms.iter().any(|&a| a == AKM_PSK || a == AKM_PSK_SHA256);
                let sae = r.akms.contains(&AKM_SAE);
                if !r.pairwise.contains(&CIPHER_CCMP) {
                    Security::Legacy
                } else if psk && sae {
                    Security::Wpa2Wpa3
                } else if sae {
                    Security::Wpa3
                } else if psk {
                    Security::Wpa2
                } else if r.akms.contains(&AKM_8021X) {
                    Security::Enterprise
                } else {
                    Security::Legacy
                }
            }
            None if self.wpa1 || self.capability & 0x10 != 0 => Security::Legacy,
            None => Security::Open,
        }
    }

    /// Signal strength in four steps (0 to 4).
    pub fn bars(&self) -> u8 {
        bars(self.rssi)
    }
}

pub fn bars(rssi: i8) -> u8 {
    match rssi {
        r if r >= -55 => 4,
        r if r >= -67 => 3,
        r if r >= -78 => 2,
        r if r >= -88 => 1,
        _ => 0,
    }
}

/// A beacon or probe response from `bssid`, heard on `channel`.
pub fn parse_bss(h: &Header, body: &[u8], channel: u8, rssi: i8, now: u64) -> Option<Bss> {
    if body.len() < 12 {
        return None;
    }
    let mut b = Bss {
        bssid: h.addr3,
        ssid: Vec::new(),
        channel,
        rssi,
        capability: u16::from_le_bytes([body[10], body[11]]),
        beacon_interval: u16::from_le_bytes([body[8], body[9]]),
        rsn: None,
        h2e: false,
        wpa1: false,
        ht: false,
        wmm: false,
        rates: Vec::new(),
        seen: now,
    };
    for (id, d) in elements(&body[12..]) {
        match id {
            IE_SSID => b.ssid = d.to_vec(),
            IE_RATES | IE_EXT_RATES => b.rates.extend_from_slice(d),
            IE_DS if !d.is_empty() => b.channel = d[0],
            IE_HT_OP if !d.is_empty() => b.channel = d[0],
            IE_HT_CAP => b.ht = true,
            IE_RSN => b.rsn = Rsn::parse(d),
            // Extended RSN capabilities: bit 5, SAE hash-to-element.
            IE_RSNX if !d.is_empty() => b.h2e = d[0] & 0x20 != 0,
            IE_VENDOR if d.len() >= 4 && d[..4] == [0x00, 0x50, 0xF2, 0x01] => b.wpa1 = true,
            IE_VENDOR if d.len() >= 5 && d[..5] == [0x00, 0x50, 0xF2, 0x02, 0x00] => b.wmm = true,
            _ => {}
        }
    }
    Some(b)
}

// ----------------------------------------------------------------------------
// What a station sends

/// The rates justOS offers: 802.11b/g on 2.4 GHz, 802.11a on 5 GHz.
pub fn rates(channel: u8) -> (&'static [u8], &'static [u8]) {
    if channel <= 14 {
        (&[0x82, 0x84, 0x8B, 0x96, 0x0C, 0x12, 0x18, 0x24], &[0x30, 0x48, 0x60, 0x6C])
    } else {
        (&[0x8C, 0x12, 0x98, 0x24, 0xB0, 0x48, 0x60, 0x6C], &[])
    }
}

/// HT capabilities: 20 MHz, short guard interval, MCS 0-15 (two streams).
pub fn ht_capabilities() -> [u8; 26] {
    let mut d = [0u8; 26];
    // Capability info: SM power save disabled (bits 2-3 = 3), short GI 20
    // MHz (bit 5), RX STBC one stream (bit 8).
    d[0..2].copy_from_slice(&(0x000C_u16 | 1 << 5 | 1 << 8).to_le_bytes());
    d[2] = 0x17; // A-MPDU: 64 KiB, 8 us spacing
    d[3] = 0xFF; // MCS 0-7
    d[4] = 0xFF; // MCS 8-15
    d
}

/// The WMM information element of a station (no U-APSD).
pub const WMM_INFO: [u8; 7] = [0x00, 0x50, 0xF2, 0x02, 0x00, 0x01, 0x00];

pub fn probe_request(sa: &Mac, seq: u16, ssid: &[u8], channel: u8) -> Vec<u8> {
    let mut f = mgmt_header(PROBE_REQ, &BROADCAST, sa, &BROADCAST, seq);
    push_element(&mut f, IE_SSID, ssid);
    let (r, x) = rates(channel);
    push_element(&mut f, IE_RATES, r);
    if !x.is_empty() {
        push_element(&mut f, IE_EXT_RATES, x);
    }
    push_element(&mut f, IE_HT_CAP, &ht_capabilities());
    f
}

/// An authentication frame: algorithm (0 open system, 3 SAE), sequence,
/// status and what follows.
pub fn auth(da: &Mac, sa: &Mac, seq: u16, alg: u16, tseq: u16, status: u16, rest: &[u8]) -> Vec<u8> {
    let mut f = mgmt_header(AUTH, da, sa, da, seq);
    f.extend_from_slice(&alg.to_le_bytes());
    f.extend_from_slice(&tseq.to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    f.extend_from_slice(rest);
    f
}

/// An association request for `bss`, with our RSN element if secured.
pub fn assoc_request(bss: &Bss, sa: &Mac, seq: u16, rsn: Option<&[u8]>, rsnx: bool) -> Vec<u8> {
    let mut f = mgmt_header(ASSOC_REQ, &bss.bssid, sa, &bss.bssid, seq);
    // Capability: ESS, short preamble, short slot time (and privacy).
    let mut cap: u16 = 0x0001 | 0x0020 | 0x0400;
    if rsn.is_some() {
        cap |= 0x0010;
    }
    f.extend_from_slice(&cap.to_le_bytes());
    f.extend_from_slice(&10u16.to_le_bytes()); // listen interval
    push_element(&mut f, IE_SSID, &bss.ssid);
    let (r, x) = rates(bss.channel);
    push_element(&mut f, IE_RATES, r);
    if !x.is_empty() {
        push_element(&mut f, IE_EXT_RATES, x);
    }
    if let Some(rsn) = rsn {
        push_element(&mut f, IE_RSN, rsn);
    }
    if bss.ht {
        push_element(&mut f, IE_HT_CAP, &ht_capabilities());
    }
    if rsnx {
        push_element(&mut f, IE_RSNX, &[0x20]);
    }
    if bss.wmm {
        push_element(&mut f, IE_VENDOR, &WMM_INFO);
    }
    f
}

/// A deauthentication (or disassociation) with `reason`.
pub fn deauth(kind: u8, da: &Mac, sa: &Mac, bssid: &Mac, seq: u16, reason: u16) -> Vec<u8> {
    let mut f = mgmt_header(kind, da, sa, bssid, seq);
    f.extend_from_slice(&reason.to_le_bytes());
    f
}

// ----------------------------------------------------------------------------
// Data frames and Ethernet

const LLC_SNAP: [u8; 6] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00];

/// The header of a data frame a station sends to its access point (to DS):
/// A1 the BSSID, A2 our address, A3 the destination.
pub fn data_header(bssid: &Mac, sa: &Mac, da: &Mac, seq: u16, qos: bool) -> Vec<u8> {
    let kind = if qos { QOS_DATA } else { DATA };
    let mut h = vec![((kind & 0x0F) << 4) | 0x08, FLAG_TO_DS, 0, 0];
    h.extend_from_slice(bssid);
    h.extend_from_slice(sa);
    h.extend_from_slice(da);
    h.extend_from_slice(&(seq << 4).to_le_bytes());
    if qos {
        h.extend_from_slice(&[0, 0]); // TID 0, normal ack
    }
    h
}

/// The body of a data frame carrying an Ethernet payload of `ethertype`.
pub fn llc(ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + payload.len());
    b.extend_from_slice(&LLC_SNAP);
    b.extend_from_slice(&ethertype.to_be_bytes());
    b.extend_from_slice(payload);
    b
}

fn ethernet(da: &Mac, sa: &Mac, body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < 8 || body[..6] != LLC_SNAP {
        return None;
    }
    let mut e = Vec::with_capacity(6 + body.len());
    e.extend_from_slice(da);
    e.extend_from_slice(sa);
    e.extend_from_slice(&body[6..]);
    Some(e)
}

/// The Ethernet frames in the (decrypted) body of a data frame from the
/// access point: one, or several for an A-MSDU.
pub fn to_ethernet(h: &Header, body: &[u8]) -> Vec<Vec<u8>> {
    // From DS: A1 destination, A3 source.
    let (da, sa) = (h.addr1, if h.flags & 3 == FLAG_FROM_DS { h.addr3 } else { h.addr2 });
    if h.kind & 0x04 != 0 {
        return Vec::new(); // null data
    }
    if h.qos.is_some_and(|q| q & 0x80 != 0) {
        // A-MSDU: subframes of DA, SA, length, LLC body, padded to 4 bytes.
        let mut out = Vec::new();
        let mut i = 0;
        while i + 14 <= body.len() {
            let len = u16::from_be_bytes([body[i + 12], body[i + 13]]) as usize;
            let end = i + 14 + len;
            if end > body.len() {
                break;
            }
            if let Some(e) = ethernet(&mac_at(body, i), &mac_at(body, i + 6), &body[i + 14..end]) {
                out.push(e);
            }
            i = (end + 3) & !3;
        }
        return out;
    }
    ethernet(&da, &sa, body).into_iter().collect()
}
