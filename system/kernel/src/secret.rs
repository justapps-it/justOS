//! Secrets the system keeps for itself (the Wi-Fi passwords): encrypted
//! with AES-256-GCM under a key made from a random machine secret,
//! `sys:/system/machine.key`, created the first time one is needed.
//!
//! What this protects against: the file holding the secrets read on its own
//! (copied, backed up, shown) is useless without the machine secret, and the
//! sandbox keeps apps away from both. What it does not: justOS has no TPM
//! support and no keyring unlocked by the user's password (the Wi-Fi has to
//! join before anyone logs in), so whoever can read the whole disk can read
//! the machine secret too and decrypt.

use crate::fs;
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use justtls::aes::Gcm;

const MACHINE_KEY: &str = "sys:/system/machine.key";

static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| alloc::format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The machine secret, read or made.
fn machine() -> Option<[u8; 32]> {
    let mut g = KEY.lock();
    if let Some(k) = *g {
        return Some(k);
    }
    let path = fs::abs(MACHINE_KEY).ok()?;
    let read = fs::read_file(&path).ok().and_then(|t| unhex(core::str::from_utf8(&t).ok()?)).filter(|k| k.len() == 32);
    let key: [u8; 32] = match read {
        Some(k) => k.try_into().ok()?,
        None => {
            let mut k = [0u8; 32];
            crate::rand::fill(&mut k);
            if let Err(e) = fs::write_file(&path, alloc::format!("{}\n", hex(&k)).as_bytes()) {
                crate::warn!("secret", "cannot save the machine key: {e:?}");
                return None;
            }
            k
        }
    };
    *g = Some(key);
    Some(key)
}

/// The key for one purpose ("wifi"), so that secrets of one kind cannot be
/// passed off as another's.
fn key(purpose: &str) -> Option<Gcm> {
    let m = machine()?;
    let mut h = justtls::sha2::Sha256::new();
    h.update(b"justOS secret v1\0");
    h.update(purpose.as_bytes());
    h.update(b"\0");
    h.update(&m);
    Some(Gcm::new(&h.finish()))
}

/// `data` encrypted, as hex text (nonce, then ciphertext and tag); `aad`
/// is bound to it (the network's name). None if the machine key cannot be
/// made.
pub fn seal(purpose: &str, aad: &[u8], data: &[u8]) -> Option<String> {
    let g = key(purpose)?;
    let mut nonce = [0u8; 12];
    crate::rand::fill(&mut nonce);
    let mut buf = data.to_vec();
    g.seal(&nonce, aad, &mut buf);
    let mut out = nonce.to_vec();
    out.extend_from_slice(&buf);
    Some(hex(&out))
}

/// What [`seal`] made, decrypted; None if it was altered, belongs to
/// another `aad` or another machine key.
pub fn open(purpose: &str, aad: &[u8], text: &str) -> Option<Vec<u8>> {
    let raw = unhex(text)?;
    if raw.len() < 12 + 16 {
        return None;
    }
    let nonce: [u8; 12] = raw[..12].try_into().ok()?;
    let mut buf = raw[12..].to_vec();
    key(purpose)?.open(&nonce, aad, &mut buf).then_some(buf)
}
