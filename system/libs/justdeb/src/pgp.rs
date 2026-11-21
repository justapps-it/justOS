//! OpenPGP (RFC 4880, RFC 9580), only as far as Debian's archive needs it:
//! reading a keyring (debian-archive-keyring's `.pgp` files, or `.asc`
//! ones) and checking the signatures of a cleartext-signed file
//! (`InRelease`). Debian signs with RSA-4096 subkeys of its archive keys and
//! with the Ed25519 key of the stable release, over SHA-256 or SHA-512.
//!
//! What is checked: that one of the signatures over the text is a good one
//! by one of the keys. What is not: the keys' own self-signatures, their
//! expiry and revocations; the keyring is trusted as it is, as apt trusts
//! the files in `/etc/apt/trusted.gpg.d` (the image carries Debian's own
//! package of them, and Debian takes old keys out of it).

use alloc::string::String;
use alloc::vec::Vec;
use justtls::hash::{self, Alg};

/// A key that can make signatures.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Key {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ed25519([u8; 32]),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The text is not a cleartext-signed message.
    NotSigned,
    /// The signature block cannot be read.
    BadArmor,
    /// No signature is a good one by a known key.
    NoGoodSignature,
}

// ----------------------------------------------------------------------------
// ASCII armor

fn b64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

/// Base64, ignoring white space; None on any other stray character.
pub fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | b64_value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The CRC-24 of the armor's checksum line.
fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xB7_04CE;
    for &b in data {
        crc ^= (b as u32) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x100_0000 != 0 {
                crc ^= 0x186_4CFB;
            }
        }
    }
    crc & 0xFF_FFFF
}

/// The data of the armored blocks in a text (`-----BEGIN PGP …-----`),
/// one after the other; a block whose checksum is wrong is refused.
pub fn dearmor(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut lines = text.lines().map(str::trim_end);
    let mut found = false;
    while let Some(line) = lines.next() {
        if !(line.starts_with("-----BEGIN PGP ") && line.ends_with("-----")) || line.contains("SIGNED MESSAGE") {
            continue;
        }
        // Header lines ("Comment: …") up to an empty line.
        let mut body = String::new();
        let mut in_headers = true;
        let mut crc = None;
        for l in lines.by_ref() {
            if l.starts_with("-----END PGP ") {
                break;
            }
            if in_headers {
                if l.is_empty() {
                    in_headers = false;
                } else if !l.contains(':') {
                    // No headers at all: this is already data.
                    in_headers = false;
                    body.push_str(l);
                }
                continue;
            }
            match l.strip_prefix('=') {
                Some(c) if c.len() == 4 => crc = base64(c),
                _ => body.push_str(l),
            }
        }
        let data = base64(&body)?;
        if let Some(c) = crc {
            if c.len() != 3 || u32::from_be_bytes([0, c[0], c[1], c[2]]) != crc24(&data) {
                return None;
            }
        }
        out.extend(data);
        found = true;
    }
    found.then_some(out)
}

// ----------------------------------------------------------------------------
// Packets

/// The packets of binary OpenPGP data: (tag, body). Partial lengths are
/// only used for streamed data, never in keys or signatures: they end the
/// list.
pub fn packets(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let head = data[i];
        if head & 0x80 == 0 {
            break;
        }
        i += 1;
        let (tag, len) = if head & 0x40 != 0 {
            let Some(&first) = data.get(i) else { break };
            i += 1;
            let len = match first {
                0..=191 => first as usize,
                192..=223 => {
                    let Some(&second) = data.get(i) else { break };
                    i += 1;
                    ((first as usize - 192) << 8) + second as usize + 192
                }
                255 => {
                    let Some(b) = data.get(i..i + 4) else { break };
                    i += 4;
                    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
                }
                _ => break,
            };
            (head & 0x3f, len)
        } else {
            let n = match head & 3 {
                0 => 1,
                1 => 2,
                2 => 4,
                _ => break,
            };
            let Some(b) = data.get(i..i + n) else { break };
            i += n;
            ((head >> 2) & 0xf, b.iter().fold(0usize, |a, &x| (a << 8) | x as usize))
        };
        let Some(body) = data.get(i..i + len) else { break };
        out.push((tag, body));
        i += len;
    }
    out
}

/// A multiprecision integer (two bytes of bit count, then the bytes) at the
/// start of `d`: its bytes and what follows.
fn mpi(d: &[u8]) -> Option<(&[u8], &[u8])> {
    let bits = u16::from_be_bytes([*d.first()?, *d.get(1)?]) as usize;
    let n = bits.div_ceil(8);
    Some((d.get(2..2 + n)?, &d[2 + n..]))
}

/// The OID of Ed25519 in OpenPGP's legacy EdDSA keys (1.3.6.1.4.1.11591.15.1).
const ED25519_OID: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

/// A public key or subkey packet's key, if it is one this module verifies.
fn key_of(body: &[u8]) -> Option<Key> {
    let (version, rest) = (*body.first()?, body.get(1..)?);
    // Version 4: created (4), algorithm, the key. Version 6 adds the
    // length of the key material (4) after the algorithm.
    let (algo, material) = match version {
        4 => (*rest.get(4)?, rest.get(5..)?),
        6 => (*rest.get(4)?, rest.get(9..)?),
        _ => return None,
    };
    match algo {
        1..=3 => {
            let (n, more) = mpi(material)?;
            let (e, _) = mpi(more)?;
            Some(Key::Rsa { n: n.to_vec(), e: e.to_vec() })
        }
        22 => {
            let len = *material.first()? as usize;
            if material.get(1..1 + len)? != ED25519_OID {
                return None;
            }
            let (point, _) = mpi(&material[1 + len..])?;
            // The point is prefixed with 0x40 (its "native" encoding).
            let raw = point.strip_prefix(&[0x40])?;
            Some(Key::Ed25519(raw.try_into().ok()?))
        }
        27 => Some(Key::Ed25519(material.get(..32)?.try_into().ok()?)),
        _ => None,
    }
}

/// The keys of a keyring, binary or armored: primary keys and subkeys.
pub fn keyring(data: &[u8]) -> Vec<Key> {
    let armored;
    let data = if data.starts_with(b"-----BEGIN") || data.windows(15).take(4096).any(|w| w == b"-----BEGIN PGP ") {
        match dearmor(&String::from_utf8_lossy(data)) {
            Some(d) => {
                armored = d;
                &armored[..]
            }
            None => return Vec::new(),
        }
    } else {
        data
    };
    packets(data).into_iter().filter(|(tag, _)| matches!(tag, 6 | 14)).filter_map(|(_, body)| key_of(body)).collect()
}

// ----------------------------------------------------------------------------
// Signatures

/// A signature packet, as far as checking it goes.
struct Signature<'a> {
    kind: u8,
    algo: u8,
    hash: Alg,
    /// What is hashed after the data (the trailer of RFC 4880 5.2.4).
    trailer: Vec<u8>,
    /// The signature's numbers (RSA: one; EdDSA: r and s).
    values: Vec<&'a [u8]>,
}

fn hash_alg(id: u8) -> Option<Alg> {
    Some(match id {
        8 => Alg::Sha256,
        9 => Alg::Sha384,
        10 => Alg::Sha512,
        _ => return None,
    })
}

fn signature(body: &[u8]) -> Option<Signature<'_>> {
    if *body.first()? != 4 {
        return None;
    }
    let (kind, algo, hash) = (*body.get(1)?, *body.get(2)?, hash_alg(*body.get(3)?)?);
    let hashed_len = u16::from_be_bytes([*body.get(4)?, *body.get(5)?]) as usize;
    let hashed_end = 6 + hashed_len;
    let mut trailer = body.get(..hashed_end)?.to_vec();
    trailer.extend_from_slice(&[4, 0xFF]);
    trailer.extend_from_slice(&(hashed_end as u32).to_be_bytes());
    let unhashed_len = u16::from_be_bytes([*body.get(hashed_end)?, *body.get(hashed_end + 1)?]) as usize;
    // Then the hash's first two bytes (a quick check), then the numbers.
    let mut rest = body.get(hashed_end + 2 + unhashed_len + 2..)?;
    let mut values = Vec::new();
    while !rest.is_empty() {
        let (v, more) = mpi(rest)?;
        values.push(v);
        rest = more;
    }
    Some(Signature { kind, algo, hash, trailer, values })
}

/// A big-endian number left-padded to `n` bytes (MPIs drop leading zeros).
fn pad(v: &[u8], n: usize) -> Option<Vec<u8>> {
    if v.len() > n {
        return None;
    }
    let mut out = alloc::vec![0u8; n - v.len()];
    out.extend_from_slice(v);
    Some(out)
}

/// Whether `sig` is `key`'s signature of `data`.
fn good(sig: &Signature<'_>, key: &Key, data: &[u8]) -> bool {
    let mut msg = Vec::with_capacity(data.len() + sig.trailer.len());
    msg.extend_from_slice(data);
    msg.extend_from_slice(&sig.trailer);
    match (key, sig.algo) {
        (Key::Rsa { n, e }, 1 | 3) => {
            let Some(s) = sig.values.first().and_then(|s| pad(s, n.len())) else { return false };
            justtls::rsa::verify_pkcs1(n, e, sig.hash, &msg, &s)
        }
        (Key::Ed25519(public), 22 | 27) => {
            // EdDSA signs the digest, not the data.
            let digest = hash::digest(sig.hash, &msg);
            let rs = match (sig.algo, &sig.values[..]) {
                (22, [r, s]) => match (pad(r, 32), pad(s, 32)) {
                    (Some(mut r), Some(s)) => {
                        r.extend(s);
                        r
                    }
                    _ => return false,
                },
                _ => return false,
            };
            justtls::ed25519::verify(public, &digest, &rs)
        }
        _ => false,
    }
}

/// The text of a cleartext-signed message (dash-escapes taken away, lines
/// apart by "\n") and its signature block.
pub fn cleartext(text: &str) -> Result<(String, &str), Error> {
    let start = text.find("-----BEGIN PGP SIGNED MESSAGE-----").ok_or(Error::NotSigned)?;
    let after = &text[start..];
    let sig_at = after.find("\n-----BEGIN PGP SIGNATURE-----").ok_or(Error::NotSigned)?;
    let (head, sig) = (&after[..sig_at], &after[sig_at + 1..]);
    // The armor headers ("Hash: SHA256") end at the first empty line.
    let body_at = head.find("\n\n").or_else(|| head.find("\n\r\n")).ok_or(Error::NotSigned)?;
    let body = head[body_at..].trim_start_matches(['\r', '\n']);
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        out.push_str(line.strip_prefix("- ").unwrap_or(line));
    }
    Ok((out, sig))
}

/// The form that is signed: each line without trailing spaces, lines apart
/// by "\r\n", no line end after the last.
fn canonical(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + text.len() / 32);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(line.trim_end_matches([' ', '\t', '\r']).as_bytes());
    }
    out
}

/// Checks a cleartext-signed message (an `InRelease` file) against the
/// keys: the signed text, once one of its signatures is a good one.
pub fn verify_cleartext(text: &str, keys: &[Key]) -> Result<String, Error> {
    let (body, block) = cleartext(text)?;
    let data = dearmor(block).ok_or(Error::BadArmor)?;
    let signed = canonical(&body);
    for (tag, p) in packets(&data) {
        let Some(sig) = signature(p).filter(|_| tag == 2) else { continue };
        // 0x01: a signature of a canonical text document.
        if sig.kind == 0x01 && keys.iter().any(|k| good(&sig, k, &signed)) {
            return Ok(body);
        }
    }
    Err(Error::NoGoodSignature)
}
