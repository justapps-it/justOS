//! SAE, the password authentication of WPA3 (IEEE 802.11-2020 12.4), with
//! the elliptic curve group 19 (NIST P-256), the one every WPA3 network
//! offers. The password element comes from either method of the standard:
//! hunting and pecking (12.4.4.2.2) or hash-to-element (12.4.4.2.3, needed
//! by networks that say so in their RSNXE).
//!
//! Not constant time: the password element search and the scalar
//! multiplications take times that depend on the password. That leaks
//! information to someone who measures them closely on this computer; over
//! the air an attacker sees only the frames.

use crate::crypto::{hmac_sha256, kdf_sha256};
use alloc::vec::Vec;
use core::cmp::Ordering;
use justtls::bigint::{self, Limbs};
use justtls::ec::{Curve, Group, Point};

/// The only group justOS offers.
pub const GROUP: u16 = 19;

/// a mod m for a number of any length (big-endian bytes).
fn rem(a: &[u8], m: &[u64]) -> Limbs {
    let n = m.len();
    let mut r = alloc::vec![0u64; n];
    for byte in a {
        for bit in (0..8).rev() {
            // r = 2r + bit
            let d = bigint::add(&r, &r);
            let mut d = if (byte >> bit) & 1 == 1 { bigint::add(&d, &[1]) } else { d };
            if bigint::cmp(&d, m) != Ordering::Less {
                d = bigint::sub(&d, m);
            }
            r = bigint::resize(&d, n);
        }
    }
    r
}

/// Field arithmetic modulo p with plain numbers.
struct Fp<'a> {
    g: &'a Group,
}

impl Fp<'_> {
    fn p(&self) -> &[u64] {
        &self.g.field().m
    }
    fn n(&self) -> usize {
        self.p().len()
    }
    fn mul(&self, a: &[u64], b: &[u64]) -> Limbs {
        let f = self.g.field();
        f.from(&f.mul(&f.to(a), &f.to(b)))
    }
    fn add(&self, a: &[u64], b: &[u64]) -> Limbs {
        self.g.field().add(&bigint::resize(a, self.n()), &bigint::resize(b, self.n()))
    }
    fn sub(&self, a: &[u64], b: &[u64]) -> Limbs {
        self.g.field().sub(&bigint::resize(a, self.n()), &bigint::resize(b, self.n()))
    }
    fn neg(&self, a: &[u64]) -> Limbs {
        self.sub(&[0], a)
    }
    fn pow(&self, a: &[u64], e: &[u64]) -> Limbs {
        self.g.field().pow(&bigint::resize(a, self.n()), e)
    }
    fn inv(&self, a: &[u64]) -> Limbs {
        self.pow(a, &bigint::sub(self.p(), &[2]))
    }
    /// x^3 + a x + b with a = -3.
    fn curve(&self, x: &[u64]) -> Limbs {
        let x3 = self.mul(&self.mul(x, x), x);
        let three_x = self.add(&self.add(x, x), x);
        self.add(&self.sub(&x3, &three_x), &self.g.b())
    }
    /// Is `v` a square (Euler's criterion)? Zero counts as one.
    fn is_square(&self, v: &[u64]) -> bool {
        let e = shr1(&bigint::sub(self.p(), &[1]));
        let r = self.pow(v, &e);
        bigint::is_zero(&r) || bigint::cmp(&r, &[1]) == Ordering::Equal
    }
    /// A square root (p = 3 mod 4).
    fn sqrt(&self, v: &[u64]) -> Limbs {
        let e = shr1(&shr1(&bigint::add(self.p(), &[1])));
        self.pow(v, &e)
    }
}

fn shr1(a: &[u64]) -> Limbs {
    let mut out = a.to_vec();
    for i in 0..out.len() {
        let hi = out.get(i + 1).copied().unwrap_or(0);
        out[i] = (out[i] >> 1) | (hi << 63);
    }
    out
}

fn odd(a: &[u64]) -> bool {
    a[0] & 1 == 1
}

/// The two addresses, larger first (as bytes).
fn max_min(a: &[u8; 6], b: &[u8; 6]) -> [u8; 12] {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    let mut out = [0u8; 12];
    out[..6].copy_from_slice(hi);
    out[6..].copy_from_slice(lo);
    out
}

/// The password element by hunting and pecking: up to 40 tries, each a
/// candidate x from the password, the addresses and a counter.
pub fn pwe_hunt_and_peck(own: &[u8; 6], peer: &[u8; 6], password: &[u8]) -> Option<(Limbs, Limbs)> {
    let g = Group::new(Curve::P256);
    let f = Fp { g: &g };
    let p_bytes = bigint::to_be(f.p(), 32);
    let key = max_min(own, peer);
    let mut found = None;
    for counter in 1..=40u8 {
        let seed = hmac_sha256(&key, &[password, &[counter]]);
        let value = kdf_sha256(&seed, b"SAE Hunting and Pecking", &p_bytes, 256);
        let x = bigint::from_be(&value);
        if bigint::cmp(&x, f.p()) != Ordering::Less {
            continue;
        }
        let x = bigint::resize(&x, f.n());
        let v = f.curve(&x);
        if !f.is_square(&v) {
            continue;
        }
        if found.is_none() {
            let y = f.sqrt(&v);
            let y = if odd(&y) == (seed[31] & 1 == 1) { y } else { f.neg(&y) };
            found = Some((x, y));
        }
    }
    found
}

/// Simplified SWU (12.4.4.2.3): a point from a field element.
fn sswu(f: &Fp<'_>, u: &[u64]) -> (Limbs, Limbs) {
    let z = f.neg(&[10]);
    let a = f.neg(&[3]);
    let b = f.g.b();
    let u2 = f.mul(u, u);
    let zu2 = f.mul(&z, &u2);
    let m = f.add(&f.mul(&zu2, &zu2), &zu2); // z^2 u^4 + z u^2
    let x1 = if bigint::is_zero(&m) {
        f.mul(&b, &f.inv(&f.mul(&z, &a)))
    } else {
        let t = f.inv(&m);
        f.mul(&f.mul(&f.neg(&b), &f.inv(&a)), &f.add(&[1], &t))
    };
    let gx1 = f.curve(&x1);
    let (x, v) = if f.is_square(&gx1) {
        (x1, gx1)
    } else {
        let x2 = f.mul(&zu2, &x1);
        let gx2 = f.curve(&x2);
        (x2, gx2)
    };
    let y = f.sqrt(&v);
    let y = if odd(&y) == odd(u) { y } else { f.neg(&y) };
    (x, y)
}

/// The password's point for hash-to-element (PT): depends only on the
/// network's name, the password and its identifier, not on the addresses.
pub fn pt(ssid: &[u8], password: &[u8], identifier: &[u8]) -> (Limbs, Limbs) {
    use justtls::hash::{hkdf_expand, hkdf_extract, Alg};
    let g = Group::new(Curve::P256);
    let f = Fp { g: &g };
    let ikm = [password, identifier].concat();
    let seed = hkdf_extract(Alg::Sha256, ssid, &ikm);
    let mut points = Vec::new();
    for label in [&b"SAE Hash to Element u1 P1"[..], b"SAE Hash to Element u2 P2"] {
        let value = hkdf_expand(Alg::Sha256, &seed, label, 48);
        let u = rem(&value, f.p());
        let (x, y) = sswu(&f, &u);
        points.push(g.point(&x, &y).expect("SSWU gives points on the curve"));
    }
    g.affine(&g.add(&points[0], &points[1])).expect("not the point at infinity")
}

/// The password element from PT and the two addresses (hash-to-element).
pub fn pwe_from_pt(pt: &(Limbs, Limbs), own: &[u8; 6], peer: &[u8; 6]) -> Option<(Limbs, Limbs)> {
    let g = Group::new(Curve::P256);
    let val = hmac_sha256(&[0u8; 32], &[&max_min(own, peer)]);
    let q1 = bigint::sub(g.order(), &[1]);
    let val = bigint::add(&rem(&val, &q1), &[1]);
    let p = g.point(&pt.0, &pt.1)?;
    g.affine(&g.mul(&val, &p))
}

/// One side of an SAE exchange.
pub struct Sae {
    g: Group,
    pwe: Point,
    rand: Limbs,
    pub scalar: Limbs,
    /// Our element, x || y.
    pub element: Vec<u8>,
    peer_scalar: Limbs,
    peer_element: Vec<u8>,
    pub kck: [u8; 32],
    pub pmk: [u8; 32],
    pub pmkid: [u8; 16],
}

/// Why a peer's commit was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum SaeError {
    /// Not group 19.
    Group,
    /// A malformed commit, or a scalar or element out of range.
    Bad,
    /// Our own commit sent back.
    Reflected,
}

impl Sae {
    /// Our commit from the password element and two random numbers (each
    /// at least 32 bytes): rand and mask, both in [2, r).
    pub fn new(pwe: &(Limbs, Limbs), rand: &[u8], mask: &[u8]) -> Option<Sae> {
        let g = Group::new(Curve::P256);
        let r = g.order().to_vec();
        let pwe = g.point(&pwe.0, &pwe.1)?;
        let into_range = |b: &[u8]| {
            let v = rem(b, &bigint::sub(&r, &[2]));
            bigint::add(&v, &[2])
        };
        let rand = bigint::resize(&into_range(rand), r.len());
        let mask = bigint::resize(&into_range(mask), r.len());
        Self::with(g, pwe, rand, mask)
    }

    /// Our commit from exactly these rand and mask (the test vectors).
    pub fn with_values(pwe: &(Limbs, Limbs), rand: &[u8], mask: &[u8]) -> Option<Sae> {
        let g = Group::new(Curve::P256);
        let pwe = g.point(&pwe.0, &pwe.1)?;
        Self::with(g, pwe, bigint::from_be(rand), bigint::from_be(mask))
    }

    fn with(g: Group, pwe: Point, rand: Limbs, mask: Limbs) -> Option<Sae> {
        let r = g.order().to_vec();
        let scalar = rem(&bigint::to_be(&bigint::add(&rand, &mask), 40), &r);
        if bigint::cmp(&scalar, &[1]) != Ordering::Greater {
            return None;
        }
        let e = g.neg(&g.mul(&mask, &pwe));
        let (x, y) = g.affine(&e)?;
        let mut element = bigint::to_be(&x, 32);
        element.extend(bigint::to_be(&y, 32));
        Some(Sae {
            g,
            pwe,
            rand,
            scalar,
            element,
            peer_scalar: Vec::new(),
            peer_element: Vec::new(),
            kck: [0; 32],
            pmk: [0; 32],
            pmkid: [0; 16],
        })
    }

    /// The commit fields of an authentication frame: group, scalar, element.
    pub fn commit(&self) -> Vec<u8> {
        let mut out = GROUP.to_le_bytes().to_vec();
        out.extend(bigint::to_be(&self.scalar, 32));
        out.extend_from_slice(&self.element);
        out
    }

    /// The peer's commit fields (group, [anti-clogging token,] scalar,
    /// element): derives KCK, PMK and PMKID.
    pub fn process_commit(&mut self, body: &[u8]) -> Result<(), SaeError> {
        if body.len() < 2 {
            return Err(SaeError::Bad);
        }
        if u16::from_le_bytes([body[0], body[1]]) != GROUP {
            return Err(SaeError::Group);
        }
        if body.len() < 2 + 96 {
            return Err(SaeError::Bad);
        }
        // Scalar and element are the last 96 bytes, unless elements follow
        // (password identifier, rejected groups): take them in order.
        let fields = &body[2..2 + 96];
        let r = self.g.order().to_vec();
        let ps = bigint::from_be(&fields[..32]);
        if bigint::cmp(&ps, &[1]) != Ordering::Greater || bigint::cmp(&ps, &r) != Ordering::Less {
            return Err(SaeError::Bad);
        }
        let pe = &fields[32..96];
        let peer = self.g.point(&bigint::from_be(&pe[..32]), &bigint::from_be(&pe[32..])).ok_or(SaeError::Bad)?;
        let ps = bigint::resize(&ps, r.len());
        if ps == bigint::resize(&self.scalar, r.len()) && pe == &self.element[..] {
            return Err(SaeError::Reflected);
        }
        // K = F(rand * (peer_scalar * PWE + peer_element))
        let t = self.g.add(&self.g.mul(&ps, &self.pwe), &peer);
        let k = self.g.affine(&self.g.mul(&self.rand, &t)).ok_or(SaeError::Bad)?;
        let k = bigint::to_be(&k.0, 32);
        let keyseed = hmac_sha256(&[0u8; 32], &[&k]);
        let sum = rem(&bigint::to_be(&bigint::add(&self.scalar, &ps), 40), &r);
        let context = bigint::to_be(&sum, 32);
        let keys = kdf_sha256(&keyseed, b"SAE KCK and PMK", &context, 512);
        self.kck.copy_from_slice(&keys[..32]);
        self.pmk.copy_from_slice(&keys[32..]);
        self.pmkid.copy_from_slice(&context[..16]);
        self.peer_scalar = ps;
        self.peer_element = pe.to_vec();
        Ok(())
    }

    fn confirm_of(&self, send_confirm: u16, first: (&[u64], &[u8]), second: (&[u64], &[u8])) -> [u8; 32] {
        hmac_sha256(
            &self.kck,
            &[&send_confirm.to_le_bytes(), &bigint::to_be(first.0, 32), first.1, &bigint::to_be(second.0, 32), second.1],
        )
    }

    /// Our confirm.
    pub fn confirm(&self, send_confirm: u16) -> [u8; 32] {
        self.confirm_of(send_confirm, (&self.scalar, &self.element), (&self.peer_scalar, &self.peer_element))
    }

    /// Is the peer's confirm (send-confirm || confirm) right?
    pub fn check_confirm(&self, body: &[u8]) -> bool {
        if body.len() < 34 {
            return false;
        }
        let sc = u16::from_le_bytes([body[0], body[1]]);
        let want = self.confirm_of(sc, (&self.peer_scalar, &self.peer_element), (&self.scalar, &self.element));
        justtls::ct_eq(&want, &body[2..34])
    }
}
