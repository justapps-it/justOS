//! The cryptography of IEEE 802.11 that `justtls` does not have: SHA-1 and
//! HMAC-SHA1, PBKDF2 (the passphrase of WPA2), the PRF and KDF that derive
//! the keys, AES-CMAC (RFC 4493), AES key wrap (RFC 3394) and CCMP
//! (AES-CCM, the cipher of the data frames).

use alloc::vec;
use alloc::vec::Vec;
use justtls::aes::Aes;
use justtls::hash::{self, Alg};

// ----------------------------------------------------------------------------
// SHA-1 (FIPS 180-4)

pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    fill: usize,
    total: u64,
}

impl Default for Sha1 {
    fn default() -> Sha1 {
        Sha1::new()
    }
}

impl Sha1 {
    pub fn new() -> Sha1 {
        Sha1 { h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0], buf: [0; 64], fill: 0, total: 0 }
    }

    fn block(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b2, mut c, mut d, mut e] = self.h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b2 & c) | (!b2 & d), 0x5A827999),
                20..=39 => (b2 ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b2 & c) | (b2 & d) | (c & d), 0x8F1BBCDC),
                _ => (b2 ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b2.rotate_left(30);
            b2 = a;
            a = t;
        }
        for (h, v) in self.h.iter_mut().zip([a, b2, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = (64 - self.fill).min(data.len());
            self.buf[self.fill..self.fill + n].copy_from_slice(&data[..n]);
            self.fill += n;
            data = &data[n..];
            if self.fill == 64 {
                let b = self.buf;
                self.block(&b);
                self.fill = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.total * 8;
        self.update(&[0x80]);
        while self.fill != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 20];
        for (i, h) in self.h.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&h.to_be_bytes());
        }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finish()
}

/// HMAC-SHA1 over the concatenation of `parts`.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(&k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let ih = inner.finish();
    let mut outer = Sha1::new();
    outer.update(&k.map(|b| b ^ 0x5c));
    outer.update(&ih);
    outer.finish()
}

pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    hash::hmac_parts(Alg::Sha256, key, parts).try_into().unwrap()
}

/// PBKDF2 with HMAC-SHA1 (RFC 8018).
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    for (i, chunk) in out.chunks_mut(20).enumerate() {
        let index = (i as u32 + 1).to_be_bytes();
        let mut u = hmac_sha1(password, &[salt, &index]);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(password, &[&u]);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

/// The pairwise master key of a WPA2 passphrase (IEEE 802.11 J.4).
pub fn psk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2_sha1(passphrase, ssid, 4096, &mut out);
    out
}

/// The PRF of IEEE 802.11 (12.7.1.2): HMAC-SHA1(K, A || 0 || B || i).
pub fn prf_sha1(key: &[u8], label: &[u8], data: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 20);
    let mut i = 0u8;
    while out.len() < len {
        out.extend_from_slice(&hmac_sha1(key, &[label, &[0], data, &[i]]));
        i += 1;
    }
    out.truncate(len);
    out
}

/// KDF-SHA256 of IEEE 802.11 (12.7.1.7.2), `bits` long: HMAC-SHA256(K,
/// i || label || context || length), counters and length little-endian.
pub fn kdf_sha256(key: &[u8], label: &[u8], context: &[u8], bits: usize) -> Vec<u8> {
    let len = bits.div_ceil(8);
    let mut out = Vec::with_capacity(len + 32);
    let mut i = 1u16;
    let length = (bits as u16).to_le_bytes();
    while out.len() < len {
        out.extend_from_slice(&hmac_sha256(key, &[&i.to_le_bytes(), label, context, &length]));
        i += 1;
    }
    out.truncate(len);
    // A length that is not whole bytes keeps its leftmost bits.
    if bits % 8 != 0 {
        let last = out.len() - 1;
        out[last] &= 0xFFu8 << (8 - bits % 8);
    }
    out
}

// ----------------------------------------------------------------------------
// AES-CMAC (RFC 4493)

fn dbl(b: &[u8; 16]) -> [u8; 16] {
    let v = u128::from_be_bytes(*b);
    let r = (v << 1) ^ if v >> 127 == 1 { 0x87 } else { 0 };
    r.to_be_bytes()
}

/// AES-CMAC of the concatenation of `parts`, with a 16- or 32-byte key.
pub fn cmac(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let aes = Aes::new(key);
    let mut l = [0u8; 16];
    aes.encrypt(&mut l);
    let k1 = dbl(&l);
    let k2 = dbl(&k1);
    let data: Vec<u8> = parts.concat();
    let n = data.len().div_ceil(16).max(1);
    let mut x = [0u8; 16];
    for i in 0..n {
        let mut block = [0u8; 16];
        let chunk = &data[(16 * i).min(data.len())..(16 * i + 16).min(data.len())];
        block[..chunk.len()].copy_from_slice(chunk);
        if i == n - 1 {
            if chunk.len() == 16 {
                block.iter_mut().zip(k1).for_each(|(a, b)| *a ^= b);
            } else {
                block[chunk.len()] = 0x80;
                block.iter_mut().zip(k2).for_each(|(a, b)| *a ^= b);
            }
        }
        for j in 0..16 {
            x[j] ^= block[j];
        }
        aes.encrypt(&mut x);
    }
    x
}

// ----------------------------------------------------------------------------
// AES key wrap (RFC 3394)

const WRAP_IV: [u8; 8] = [0xA6; 8];

pub fn key_wrap(kek: &[u8], plain: &[u8]) -> Vec<u8> {
    let aes = Aes::new(kek);
    let n = plain.len() / 8;
    let mut a = WRAP_IV;
    let mut r: Vec<[u8; 8]> = plain.chunks(8).map(|c| c.try_into().unwrap()).collect();
    for j in 0..6 {
        for i in 0..n {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(&r[i]);
            aes.encrypt(&mut b);
            let t = (n * j + i + 1) as u64;
            a = (u64::from_be_bytes(b[..8].try_into().unwrap()) ^ t).to_be_bytes();
            r[i].copy_from_slice(&b[8..]);
        }
    }
    let mut out = a.to_vec();
    for c in r {
        out.extend_from_slice(&c);
    }
    out
}

/// The unwrapped key, or None if the integrity check fails.
pub fn key_unwrap(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    if wrapped.len() % 8 != 0 || wrapped.len() < 16 {
        return None;
    }
    let aes = Aes::new(kek);
    let n = wrapped.len() / 8 - 1;
    let mut a: [u8; 8] = wrapped[..8].try_into().unwrap();
    let mut r: Vec<[u8; 8]> = wrapped[8..].chunks(8).map(|c| c.try_into().unwrap()).collect();
    for j in (0..6).rev() {
        for i in (0..n).rev() {
            let t = (n * j + i + 1) as u64;
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&(u64::from_be_bytes(a) ^ t).to_be_bytes());
            b[8..].copy_from_slice(&r[i]);
            aes.decrypt(&mut b);
            a.copy_from_slice(&b[..8]);
            r[i].copy_from_slice(&b[8..]);
        }
    }
    if !justtls::ct_eq(&a, &WRAP_IV) {
        return None;
    }
    Some(r.concat())
}

// ----------------------------------------------------------------------------
// CCMP (IEEE 802.11 12.5.3): AES-CCM with an 8-byte MIC and a 2-byte length.

/// The additional authenticated data and nonce of a data or management
/// frame `hdr` (its MAC header, 24 to 32 bytes) sent with packet number `pn`.
fn ccmp_aad_nonce(hdr: &[u8], pn: u64) -> (Vec<u8>, [u8; 13]) {
    let fc0 = hdr[0];
    let fc1 = hdr[1];
    let is_data = (fc0 >> 2) & 3 == 2;
    // Masked frame control: subtype bits 4-6 of data frames, retry, power
    // management and more data are zero; protected stays one.
    let m0 = if is_data { fc0 & 0x8F } else { fc0 };
    let qos_data = is_data && fc0 & 0x80 != 0;
    let m1 = (fc1 & !0x38 & if qos_data { 0x7F } else { 0xFF }) | 0x40;
    let mut aad = vec![m0, m1];
    aad.extend_from_slice(&hdr[4..22]); // A1, A2, A3
    aad.extend_from_slice(&[hdr[22] & 0x0F, 0]); // sequence control: fragment number only
    let to_from_ds = fc1 & 3;
    let mut at = 24;
    if to_from_ds == 3 {
        aad.extend_from_slice(&hdr[24..30]);
        at = 30;
    }
    let qos = is_data && fc0 & 0x80 != 0;
    let mut priority = 0;
    if qos {
        let tid = hdr[at] & 0x0F;
        priority = tid;
        aad.extend_from_slice(&[tid, 0]);
    }
    let mut nonce = [0u8; 13];
    // Management frames have the management flag in the nonce flags.
    nonce[0] = priority | if (fc0 >> 2) & 3 == 0 { 0x10 } else { 0 };
    nonce[1..7].copy_from_slice(&hdr[10..16]); // A2
    for i in 0..6 {
        nonce[7 + i] = (pn >> (8 * (5 - i))) as u8;
    }
    (aad, nonce)
}

fn ccm_ctr(aes: &Aes, nonce: &[u8; 13], i: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = 1; // L - 1 = 1
    a[1..14].copy_from_slice(nonce);
    a[14..16].copy_from_slice(&i.to_be_bytes());
    aes.encrypt(&mut a);
    a
}

fn ccm_mic(aes: &Aes, nonce: &[u8; 13], aad: &[u8], data: &[u8]) -> [u8; 8] {
    let mut b0 = [0u8; 16];
    b0[0] = 0x40 | (((8 - 2) / 2) << 3) | 1; // Adata, M = 8, L = 2
    b0[1..14].copy_from_slice(nonce);
    b0[14..16].copy_from_slice(&(data.len() as u16).to_be_bytes());
    let mut x = b0;
    aes.encrypt(&mut x);
    let mut a = (aad.len() as u16).to_be_bytes().to_vec();
    a.extend_from_slice(aad);
    for stream in [&a[..], data] {
        for chunk in stream.chunks(16) {
            for (j, b) in chunk.iter().enumerate() {
                x[j] ^= b;
            }
            aes.encrypt(&mut x);
        }
    }
    x[..8].try_into().unwrap()
}

/// The CCMP header of packet number `pn` with key `key_id`.
pub fn ccmp_header(pn: u64, key_id: u8) -> [u8; 8] {
    let p = pn.to_le_bytes();
    [p[0], p[1], 0, 0x20 | (key_id << 6), p[2], p[3], p[4], p[5]]
}

/// The packet number in a CCMP header.
pub fn ccmp_pn(h: &[u8]) -> u64 {
    u64::from_le_bytes([h[0], h[1], h[4], h[5], h[6], h[7], 0, 0])
}

/// Encrypts the body of a frame: returns CCMP header || ciphertext || MIC.
/// `hdr` is the frame's MAC header, with the protected bit already set.
pub fn ccmp_encrypt(tk: &[u8], hdr: &[u8], pn: u64, key_id: u8, body: &[u8]) -> Vec<u8> {
    let aes = Aes::new(tk);
    let (aad, nonce) = ccmp_aad_nonce(hdr, pn);
    let mic = ccm_mic(&aes, &nonce, &aad, body);
    let mut out = ccmp_header(pn, key_id).to_vec();
    for (i, chunk) in body.chunks(16).enumerate() {
        let s = ccm_ctr(&aes, &nonce, i as u16 + 1);
        out.extend(chunk.iter().zip(s).map(|(a, b)| a ^ b));
    }
    let s0 = ccm_ctr(&aes, &nonce, 0);
    out.extend(mic.iter().zip(s0).map(|(a, b)| a ^ b));
    out
}

/// Decrypts a protected body (CCMP header || ciphertext || MIC): the
/// packet number and the plain body, or None if the MIC is wrong.
pub fn ccmp_decrypt(tk: &[u8], hdr: &[u8], body: &[u8]) -> Option<(u64, Vec<u8>)> {
    if body.len() < 16 || body[3] & 0x20 == 0 {
        return None;
    }
    let pn = ccmp_pn(body);
    let aes = Aes::new(tk);
    let (aad, nonce) = ccmp_aad_nonce(hdr, pn);
    let data = &body[8..body.len() - 8];
    let mut plain = Vec::with_capacity(data.len());
    for (i, chunk) in data.chunks(16).enumerate() {
        let s = ccm_ctr(&aes, &nonce, i as u16 + 1);
        plain.extend(chunk.iter().zip(s).map(|(a, b)| a ^ b));
    }
    let s0 = ccm_ctr(&aes, &nonce, 0);
    let got: Vec<u8> = body[body.len() - 8..].iter().zip(s0).map(|(a, b)| a ^ b).collect();
    let mic = ccm_mic(&aes, &nonce, &aad, &plain);
    justtls::ct_eq(&got, &mic).then_some((pn, plain))
}

// ----------------------------------------------------------------------------
// BIP-CMAC-128 (IEEE 802.11 12.5.4): the integrity of group-addressed
// management frames (a broadcast deauthentication) under protected
// management frames.

/// Checks the MMIE at the end of `body` against IGTK `igtk`: the key id and
/// packet number if the MIC is right.
pub fn bip_check(igtk: &[u8], hdr: &[u8], body: &[u8]) -> Option<(u16, u64)> {
    if body.len() < 18 || body[body.len() - 18] != 76 || body[body.len() - 17] != 16 {
        return None;
    }
    let mmie = &body[body.len() - 18..];
    let key_id = u16::from_le_bytes([mmie[2], mmie[3]]);
    let pn = u64::from_le_bytes([mmie[4], mmie[5], mmie[6], mmie[7], mmie[8], mmie[9], 0, 0]);
    let mut aad = vec![hdr[0], hdr[1] & !0x38];
    aad.extend_from_slice(&hdr[4..22]);
    let mut zeroed = body.to_vec();
    let n = zeroed.len();
    zeroed[n - 8..].fill(0);
    let mic = cmac(igtk, &[&aad, &zeroed]);
    justtls::ct_eq(&mic[..8], &mmie[10..18]).then_some((key_id, pn))
}
