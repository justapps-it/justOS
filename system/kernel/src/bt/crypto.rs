//! The cryptographic functions of LE pairing (core spec vol 3, part H,
//! 2.2): AES-CMAC (RFC 4493) and f4, f5, f6, g2 of Secure Connections,
//! c1 and s1 of legacy pairing, and the P-256 key exchange.
//!
//! Every value here is in the byte order of the SMP packets, least
//! significant byte first, as Linux's smp.c keeps them; the spec writes
//! them most significant first, so AES gets them reversed.

use alloc::vec::Vec;
use justtls::aes::Aes;
use justtls::ec::{self, Curve};

fn rev16(b: &[u8; 16]) -> [u8; 16] {
    let mut r = *b;
    r.reverse();
    r
}

/// AES-128 in the spec's order: key and block most significant first.
fn aes_be(key: &[u8; 16], block: &mut [u8; 16]) {
    Aes::new(key).encrypt(block);
}

/// The spec's e(): AES-128 on little-endian values.
pub fn e(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut b = rev16(data);
    aes_be(&rev16(key), &mut b);
    b.reverse();
    b
}

fn shift_left(b: &[u8; 16]) -> [u8; 16] {
    let mut r = [0u8; 16];
    for i in 0..16 {
        r[i] = b[i] << 1 | b.get(i + 1).map_or(0, |n| n >> 7);
    }
    r
}

/// AES-CMAC with key and message most significant first (RFC 4493).
fn cmac_be(key: &[u8; 16], msg: &[u8]) -> [u8; 16] {
    let aes = Aes::new(key);
    let mut l = [0u8; 16];
    aes.encrypt(&mut l);
    let sub = |b: &[u8; 16]| {
        let mut k = shift_left(b);
        if b[0] & 0x80 != 0 {
            k[15] ^= 0x87;
        }
        k
    };
    let k1 = sub(&l);
    let k2 = sub(&k1);
    let n = msg.len().div_ceil(16).max(1);
    let complete = !msg.is_empty() && msg.len() % 16 == 0;
    let mut x = [0u8; 16];
    for i in 0..n {
        let mut block = [0u8; 16];
        let chunk = &msg[i * 16..msg.len().min(i * 16 + 16)];
        block[..chunk.len()].copy_from_slice(chunk);
        if i == n - 1 {
            if complete {
                block.iter_mut().zip(k1).for_each(|(b, k)| *b ^= k);
            } else {
                block[chunk.len()] = 0x80;
                block.iter_mut().zip(k2).for_each(|(b, k)| *b ^= k);
            }
        }
        x.iter_mut().zip(block).for_each(|(a, b)| *a ^= b);
        aes.encrypt(&mut x);
    }
    x
}

/// AES-CMAC on little-endian key and message.
fn cmac(key: &[u8; 16], msg_le: &[u8]) -> [u8; 16] {
    let m: Vec<u8> = msg_le.iter().rev().copied().collect();
    let mut r = cmac_be(&rev16(key), &m);
    r.reverse();
    r
}

/// f4(U, V, X, Z): the confirm values of Secure Connections.
pub fn f4(u: &[u8; 32], v: &[u8; 32], x: &[u8; 16], z: u8) -> [u8; 16] {
    let mut m = Vec::with_capacity(65);
    m.push(z);
    m.extend_from_slice(v);
    m.extend_from_slice(u);
    cmac(x, &m)
}

/// f5(W, N1, N2, A1, A2): (MacKey, LTK). A1 and A2 are 7 bytes: the
/// address, then its type.
pub fn f5(w: &[u8; 32], n1: &[u8; 16], n2: &[u8; 16], a1: &[u8; 7], a2: &[u8; 7]) -> ([u8; 16], [u8; 16]) {
    const SALT: [u8; 16] = [0xbe, 0x83, 0x60, 0x5a, 0xdb, 0x0b, 0x37, 0x60, 0x38, 0xa5, 0xf5, 0xaa, 0x91, 0x83, 0x88, 0x6c];
    let t = cmac(&SALT, w);
    let mut m = [0u8; 53];
    m[0..2].copy_from_slice(&[0x00, 0x01]); // length: 256 bits
    m[2..9].copy_from_slice(a2);
    m[9..16].copy_from_slice(a1);
    m[16..32].copy_from_slice(n2);
    m[32..48].copy_from_slice(n1);
    m[48..52].copy_from_slice(&[0x65, 0x6c, 0x74, 0x62]); // "btle"
    m[52] = 0;
    let mackey = cmac(&t, &m);
    m[52] = 1;
    let ltk = cmac(&t, &m);
    (mackey, ltk)
}

/// f6(W, N1, N2, R, IOcap, A1, A2): the DHKey checks. `io_cap` is the IO
/// capability, the OOB flag and AuthReq, in this order.
pub fn f6(w: &[u8; 16], n1: &[u8; 16], n2: &[u8; 16], r: &[u8; 16], io_cap: &[u8; 3], a1: &[u8; 7], a2: &[u8; 7]) -> [u8; 16] {
    let mut m = Vec::with_capacity(65);
    m.extend_from_slice(a2);
    m.extend_from_slice(a1);
    m.extend_from_slice(io_cap);
    m.extend_from_slice(r);
    m.extend_from_slice(n2);
    m.extend_from_slice(n1);
    cmac(w, &m)
}

/// g2(U, V, X, Y): the six digits of numeric comparison.
pub fn g2(u: &[u8; 32], v: &[u8; 32], x: &[u8; 16], y: &[u8; 16]) -> u32 {
    let mut m = Vec::with_capacity(80);
    m.extend_from_slice(y);
    m.extend_from_slice(v);
    m.extend_from_slice(u);
    let r = cmac(x, &m);
    u32::from_le_bytes([r[0], r[1], r[2], r[3]]) % 1_000_000
}

/// c1 of legacy pairing: the confirm value. `preq` and `pres` are the
/// two 7-byte pairing packets; `ia`/`ra` the initiator's and responder's
/// addresses, `iat`/`rat` their types.
#[allow(clippy::too_many_arguments)]
pub fn c1(k: &[u8; 16], r: &[u8; 16], preq: &[u8; 7], pres: &[u8; 7], iat: u8, ia: &[u8; 6], rat: u8, ra: &[u8; 6]) -> [u8; 16] {
    let mut p1 = [0u8; 16];
    p1[0] = iat;
    p1[1] = rat;
    p1[2..9].copy_from_slice(preq);
    p1[9..16].copy_from_slice(pres);
    let mut p2 = [0u8; 16];
    p2[0..6].copy_from_slice(ra);
    p2[6..12].copy_from_slice(ia);
    let mut x = *r;
    x.iter_mut().zip(p1).for_each(|(a, b)| *a ^= b);
    let mut y = e(k, &x);
    y.iter_mut().zip(p2).for_each(|(a, b)| *a ^= b);
    e(k, &y)
}

/// s1 of legacy pairing: the short-term key from the responder's and the
/// initiator's random values.
pub fn s1(k: &[u8; 16], r1: &[u8; 16], r2: &[u8; 16]) -> [u8; 16] {
    let mut r = [0u8; 16];
    r[0..8].copy_from_slice(&r2[0..8]);
    r[8..16].copy_from_slice(&r1[0..8]);
    e(k, &r)
}

/// A P-256 key pair: the private key and the public key as SMP sends it
/// (X then Y, each little-endian).
pub struct KeyPair {
    private: justtls::bigint::Limbs,
    pub public: [u8; 64],
}

pub fn keypair() -> KeyPair {
    let mut random = [0u8; 32];
    crate::rand::fill(&mut random);
    let (private, point) = ec::keypair(Curve::P256, &random);
    let mut public = [0u8; 64];
    for i in 0..32 {
        public[i] = point[32 - i];
        public[32 + i] = point[64 - i];
    }
    KeyPair { private, public }
}

/// The DHKey with the peer's public key (as SMP sends it), little-endian;
/// None if the key is not a point of the curve.
pub fn dhkey(k: &KeyPair, peer: &[u8; 64]) -> Option<[u8; 32]> {
    let mut point = [0u8; 65];
    point[0] = 4;
    for i in 0..32 {
        point[32 - i] = peer[i];
        point[64 - i] = peer[32 + i];
    }
    let x = ec::shared(Curve::P256, &k.private, &point)?;
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = x[31 - i];
    }
    Some(out)
}

/// RFC 4493's examples, checked once when the first adapter comes.
pub fn selftest() -> bool {
    const KEY: [u8; 16] = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
    const M16: [u8; 16] = [0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a];
    const EMPTY: [u8; 16] = [0xbb, 0x1d, 0x69, 0x29, 0xe9, 0x59, 0x37, 0x28, 0x7f, 0xa3, 0x7d, 0x12, 0x9b, 0x75, 0x67, 0x46];
    const ONE: [u8; 16] = [0x07, 0x0a, 0x16, 0xb4, 0x6b, 0x4d, 0x41, 0x44, 0xf7, 0x9b, 0xdd, 0x9d, 0xd0, 0x4a, 0x28, 0x7c];
    cmac_be(&KEY, &[]) == EMPTY && cmac_be(&KEY, &M16) == ONE
}
