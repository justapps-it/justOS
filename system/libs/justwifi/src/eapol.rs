//! EAPOL-Key frames and the keys of the 4-way and group key handshakes
//! (IEEE 802.11-2020 12.7), for the AKMs a home network uses: PSK (WPA2),
//! PSK-SHA256 and SAE (WPA3), all with CCMP.

use crate::crypto::{cmac, hmac_sha1, kdf_sha256, key_unwrap, key_wrap, prf_sha1};
use crate::frame::{Mac, AKM_PSK, AKM_PSK_SHA256, AKM_SAE, OUI_IEEE};
use alloc::vec::Vec;

pub const ETHERTYPE: u16 = 0x888E;

// Key information bits.
pub const KI_VERSION: u16 = 0x0007;
pub const KI_PAIRWISE: u16 = 1 << 3;
pub const KI_INSTALL: u16 = 1 << 6;
pub const KI_ACK: u16 = 1 << 7;
pub const KI_MIC: u16 = 1 << 8;
pub const KI_SECURE: u16 = 1 << 9;
pub const KI_ERROR: u16 = 1 << 10;
pub const KI_REQUEST: u16 = 1 << 11;
pub const KI_ENCRYPTED: u16 = 1 << 12;

/// How the keys are derived and checked for an AKM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Akm {
    Psk,
    PskSha256,
    Sae,
}

impl Akm {
    pub fn from_suite(s: u8) -> Option<Akm> {
        match s {
            AKM_PSK => Some(Akm::Psk),
            AKM_PSK_SHA256 => Some(Akm::PskSha256),
            AKM_SAE => Some(Akm::Sae),
            _ => None,
        }
    }

    pub fn suite(self) -> u8 {
        match self {
            Akm::Psk => AKM_PSK,
            Akm::PskSha256 => AKM_PSK_SHA256,
            Akm::Sae => AKM_SAE,
        }
    }

    /// The key descriptor version in the key information.
    pub fn version(self) -> u16 {
        match self {
            Akm::Psk => 2,       // HMAC-SHA1-128, AES key wrap
            Akm::PskSha256 => 3, // AES-128-CMAC, AES key wrap
            Akm::Sae => 0,       // as the AKM says: AES-128-CMAC
        }
    }
}

/// The pairwise keys: KCK (MIC), KEK (key wrap), TK (the data's CCMP key).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ptk {
    pub kck: [u8; 16],
    pub kek: [u8; 16],
    pub tk: [u8; 16],
}

fn min_max<'a>(a: &'a [u8], b: &'a [u8]) -> (&'a [u8], &'a [u8]) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

/// PTK = PRF-384(PMK, "Pairwise key expansion", min(AA,SPA) || max(AA,SPA)
/// || min(ANonce,SNonce) || max(ANonce,SNonce)).
pub fn derive_ptk(akm: Akm, pmk: &[u8], aa: &Mac, spa: &Mac, anonce: &[u8; 32], snonce: &[u8; 32]) -> Ptk {
    let (a1, a2) = min_max(aa, spa);
    let (n1, n2) = min_max(anonce, snonce);
    let data = [a1, a2, n1, n2].concat();
    let label = b"Pairwise key expansion";
    let k = match akm {
        Akm::Psk => prf_sha1(pmk, label, &data, 48),
        Akm::PskSha256 | Akm::Sae => kdf_sha256(pmk, label, &data, 384),
    };
    Ptk { kck: k[..16].try_into().unwrap(), kek: k[16..32].try_into().unwrap(), tk: k[32..48].try_into().unwrap() }
}

/// The MIC of an EAPOL frame (with its MIC field zero).
pub fn mic(akm: Akm, kck: &[u8], frame: &[u8]) -> [u8; 16] {
    match akm {
        Akm::Psk => hmac_sha1(kck, &[frame])[..16].try_into().unwrap(),
        Akm::PskSha256 | Akm::Sae => cmac(kck, &[frame]),
    }
}

/// An EAPOL-Key frame (the body after the LLC header).
#[derive(Clone, Debug, Default)]
pub struct Key {
    pub eapol_version: u8,
    pub info: u16,
    pub key_len: u16,
    pub replay: u64,
    pub nonce: [u8; 32],
    pub rsc: [u8; 8],
    pub mic: [u8; 16],
    pub data: Vec<u8>,
}

/// Where the MIC is in an EAPOL-Key frame.
pub const MIC_AT: usize = 4 + 77;

impl Key {
    pub fn parse(b: &[u8]) -> Option<Key> {
        // version, type 3 (key), length; descriptor type 2 (RSN)
        if b.len() < 4 + 95 || b[1] != 3 || b[4] != 2 {
            return None;
        }
        let len = u16::from_be_bytes([b[2], b[3]]) as usize;
        let b = &b[..(4 + len).min(b.len())];
        if b.len() < 4 + 95 {
            return None;
        }
        let kd = u16::from_be_bytes([b[97], b[98]]) as usize;
        let data = b.get(99..99 + kd)?.to_vec();
        Some(Key {
            eapol_version: b[0],
            info: u16::from_be_bytes([b[5], b[6]]),
            key_len: u16::from_be_bytes([b[7], b[8]]),
            replay: u64::from_be_bytes(b[9..17].try_into().unwrap()),
            nonce: b[17..49].try_into().unwrap(),
            rsc: b[65..73].try_into().unwrap(),
            mic: b[MIC_AT..MIC_AT + 16].try_into().unwrap(),
            data,
        })
    }

    /// The frame, with the MIC field as it is in `self`.
    pub fn build(&self) -> Vec<u8> {
        let body_len = 95 + self.data.len();
        let mut b = Vec::with_capacity(4 + body_len);
        b.push(self.eapol_version);
        b.push(3);
        b.extend_from_slice(&(body_len as u16).to_be_bytes());
        b.push(2);
        b.extend_from_slice(&self.info.to_be_bytes());
        b.extend_from_slice(&self.key_len.to_be_bytes());
        b.extend_from_slice(&self.replay.to_be_bytes());
        b.extend_from_slice(&self.nonce);
        b.extend_from_slice(&[0; 16]); // IV
        b.extend_from_slice(&self.rsc);
        b.extend_from_slice(&[0; 8]); // reserved
        b.extend_from_slice(&self.mic);
        b.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        b.extend_from_slice(&self.data);
        b
    }

    /// The frame with its MIC computed.
    pub fn build_signed(&mut self, akm: Akm, kck: &[u8]) -> Vec<u8> {
        self.mic = [0; 16];
        let unsigned = self.build();
        self.mic = mic(akm, kck, &unsigned);
        self.build()
    }

    /// Is the MIC of the frame `raw` (as received) right?
    pub fn check_mic(raw: &[u8], akm: Akm, kck: &[u8]) -> bool {
        if raw.len() < MIC_AT + 16 {
            return false;
        }
        let len = (4 + u16::from_be_bytes([raw[2], raw[3]]) as usize).min(raw.len());
        let mut zeroed = raw[..len].to_vec();
        let got: [u8; 16] = zeroed[MIC_AT..MIC_AT + 16].try_into().unwrap();
        zeroed[MIC_AT..MIC_AT + 16].fill(0);
        justtls::ct_eq(&mic(akm, kck, &zeroed), &got)
    }
}

/// Decrypts the key data of a message (AES key unwrap with the KEK).
pub fn decrypt_data(kek: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    key_unwrap(kek, data)
}

/// Encrypts key data, padded as 12.7.2 says (for the authenticator side,
/// in tests).
pub fn encrypt_data(kek: &[u8], data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    if d.len() < 16 || d.len() % 8 != 0 {
        d.push(0xDD);
        while d.len() < 16 || d.len() % 8 != 0 {
            d.push(0);
        }
    }
    key_wrap(kek, &d)
}

/// A group key from the key data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupKey {
    pub id: u8,
    pub key: Vec<u8>,
}

/// What the key data of message 3 (or a group key message) carries.
#[derive(Clone, Debug, Default)]
pub struct KeyData {
    pub rsn: Option<Vec<u8>>,
    pub gtk: Option<GroupKey>,
    /// The integrity group key and its packet number (IPN).
    pub igtk: Option<(GroupKey, u64)>,
}

pub fn parse_key_data(d: &[u8]) -> KeyData {
    let mut out = KeyData::default();
    let mut i = 0;
    while i + 2 <= d.len() {
        let (id, len) = (d[i], d[i + 1] as usize);
        if id == 0xDD && len == 0 {
            break; // padding
        }
        let Some(v) = d.get(i + 2..i + 2 + len) else { break };
        match id {
            0x30 => out.rsn = Some(d[i..i + 2 + len].to_vec()),
            0xDD if v.len() >= 4 && v[..3] == OUI_IEEE => {
                let body = &v[4..];
                match v[3] {
                    1 if body.len() >= 2 => out.gtk = Some(GroupKey { id: body[0] & 3, key: body[2..].to_vec() }),
                    9 if body.len() >= 8 => {
                        let id = u16::from_le_bytes([body[0], body[1]]) as u8;
                        let ipn = u64::from_le_bytes([body[2], body[3], body[4], body[5], body[6], body[7], 0, 0]);
                        out.igtk = Some((GroupKey { id, key: body[8..].to_vec() }, ipn));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 2 + len;
    }
    out
}
