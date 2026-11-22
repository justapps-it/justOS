//! The Realtek RTL8852B firmware file (rtw89/rtw8852b_fw-2.bin), taken
//! apart: the multi-firmware header picks the image for the chip's cut, the
//! image's header lists the sections the download sends to the card, and
//! after the images come the "elements": the tables of the baseband and the
//! radio, and the transmit power limits.
//!
//! The layout follows the Linux rtw89 driver (fw.c, fw.h), Copyright(c)
//! 2019-2022 Realtek Corporation, used under the BSD-3-Clause half of its
//! "GPL-2.0 OR BSD-3-Clause" licence; written again in Rust for justOS, no
//! code copied. See ../README.md.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

/// First byte of a multi-firmware file.
const MFW_SIG: u8 = 0xFF;
/// Firmware types: the ordinary Wi-Fi firmware, and its "CE" variant,
/// which rtw89 tries first on the 8852B (others: WoWLAN 3, log formats 255).
pub const TYPE_NORMAL: u8 = 1;
pub const TYPE_NORMAL_CE: u8 = 5;
/// At most this many sections in an image.
pub const MAX_SECTIONS: usize = 10;
/// The security section: signatures follow its data.
pub const SECURITY_SECTION: u8 = 9;
/// Bytes of a section sent in one download packet (802.11ax chips).
pub const PART_LEN: usize = 2020;
const CHECKSUM_LEN: u32 = 8;
const SIG_LEN: u32 = 512;
const MSS_SIGNATURE: [u8; 8] = *b"MSSKPOOL";
const MSS_HDR_LEN: u32 = 32;
const ELEMENT_ALIGN: usize = 16;
/// An element's header: id, size, version, then 20 bytes whose last 8
/// depend on the kind of table.
const ELEMENT_HDR_LEN: usize = 32;

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// An image in the multi-firmware file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Chip cut it is for (0 = CAV, 1 = CBV...).
    pub cv: u8,
    pub kind: u8,
    /// Factory test firmware.
    pub mp: bool,
    pub offset: usize,
    pub size: usize,
}

/// The images a multi-firmware file holds, and its own version.
pub fn entries(file: &[u8]) -> Result<(Vec<Entry>, [u8; 4]), &'static str> {
    if file.first() != Some(&MFW_SIG) {
        return Err("not a multi-firmware file");
    }
    let count = file[1] as usize;
    if count == 0 {
        return Err("no firmware in the file");
    }
    let ver: [u8; 4] = file.get(4..8).ok_or("file too short")?.try_into().unwrap();
    let mut out = Vec::new();
    for i in 0..count {
        let at = 16 + i * 16;
        let e = file.get(at..at + 16).ok_or("header truncated")?;
        let offset = le32(e, 4).unwrap() as usize;
        let size = le32(e, 8).unwrap() as usize;
        if offset.checked_add(size).is_none_or(|end| end > file.len()) {
            return Err("an image lies outside the file");
        }
        out.push(Entry { cv: e[0], kind: e[1], mp: e[2] != 0, offset, size });
    }
    Ok((out, ver))
}

/// The Wi-Fi firmware for a chip of cut `cv`, as rtw89 picks it: the CE
/// variant if there is one, the ordinary one otherwise; of those not newer
/// than the chip, the newest (the file does not keep them in order).
pub fn select(file: &[u8], cv: u8) -> Result<(Entry, &[u8]), &'static str> {
    let (list, _) = entries(file)?;
    let pick = |kind: u8| list.iter().filter(|e| e.kind == kind && !e.mp && e.cv <= cv).max_by_key(|e| e.cv).copied();
    let best = pick(TYPE_NORMAL_CE).or_else(|| pick(TYPE_NORMAL)).ok_or("no firmware for this chip cut")?;
    Ok((best, &file[best.offset..best.offset + best.size]))
}

/// A part of the image the download sends to an address of the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub kind: u8,
    /// Where the card's CPU wants it.
    pub addr: u32,
    /// Its bytes in the image (with the checksum, when it has one).
    pub offset: usize,
    pub len: usize,
    /// Downloaded again after a reset.
    pub redl: bool,
    /// Signatures after the data (security section), not sent on a card
    /// without secure boot.
    pub sig_len: usize,
}

/// A firmware image, its header read.
#[derive(Debug)]
pub struct Image<'a> {
    pub data: &'a [u8],
    /// major, minor, sub version, sub index
    pub version: [u8; 4],
    pub commit: u32,
    /// year, month, day, hour, minute
    pub built: (u32, u8, u8, u8, u8),
    /// The header the download sends first.
    pub header_len: usize,
    pub sections: Vec<Section>,
}

/// Reads an image's header (format 0, the one the 802.11ax chips use).
pub fn parse(fw: &[u8]) -> Result<Image<'_>, &'static str> {
    let w = |i: usize| le32(fw, i * 4).ok_or("header truncated");
    let (w1, w2, w3, w4, w5, w6, w7) = (w(1)?, w(2)?, w(3)?, w(4)?, w(5)?, w(6)?, w(7)?);
    if w3 >> 24 != 0 {
        return Err("unknown firmware header format");
    }
    let count = ((w6 >> 8) & 0xFF) as usize;
    if count == 0 || count > MAX_SECTIONS {
        return Err("bad number of sections");
    }
    let base = 32 + count * 16;
    if base > fw.len() {
        return Err("header truncated");
    }
    let header_len = if w7 & (1 << 16) != 0 {
        // A dynamic header follows the sections; its first word is its size.
        let len = ((w3 >> 16) & 0xFF) as usize;
        let dynamic = le32(fw, base).ok_or("header truncated")? as usize;
        if len < base || len - base != dynamic {
            return Err("bad dynamic header");
        }
        len
    } else {
        base
    };
    let mut at = header_len;
    let mut sections = Vec::new();
    for i in 0..count {
        let h = 32 + i * 16;
        let (s0, s1, s2) = (le32(fw, h).unwrap(), le32(fw, h + 4).unwrap(), le32(fw, h + 8).unwrap());
        let kind = ((s1 >> 24) & 0xF) as u8;
        let mut len = s1 & 0xFF_FFFF;
        if s1 & (1 << 28) != 0 {
            len += CHECKSUM_LEN;
        }
        let sig_len = if kind == SECURITY_SECTION { mssc_len(fw, at, at + len as usize, s2)? } else { 0 };
        if at + len as usize + sig_len as usize > fw.len() {
            return Err("a section lies outside the image");
        }
        sections.push(Section {
            kind,
            addr: s0 & 0x1FFF_FFFF,
            offset: at,
            len: len as usize,
            redl: s1 & (1 << 29) != 0,
            sig_len: sig_len as usize,
        });
        at += len as usize + sig_len as usize;
    }
    if at != fw.len() {
        return Err("the sections do not fill the image");
    }
    Ok(Image {
        data: fw,
        version: w1.to_le_bytes(),
        commit: w2,
        built: (w5, w4 as u8, (w4 >> 8) as u8, (w4 >> 16) as u8, (w4 >> 24) as u8),
        header_len,
        sections,
    })
}

/// Size of the signatures after a security section's data (at `data` in
/// the image), which start at `sig` (`mssc`: the section's third word).
fn mssc_len(fw: &[u8], data: usize, sig: usize, mssc: u32) -> Result<u32, &'static str> {
    let mssc = mssc & 0xFF;
    if mssc != 0xFF {
        return Ok(mssc * SIG_LEN);
    }
    // A key pool: its header, a remap table, then the key pairs.
    let h = fw.get(sig..sig + MSS_HDR_LEN as usize).ok_or("key pool truncated")?;
    if h[..8] != MSS_SIGNATURE {
        return Err("bad key pool signature");
    }
    let key_raw = le32(h, 12).unwrap();
    let defen = h[16];
    let rmpfmt = h[20];
    let dev_max = h[21] as u32;
    let pairs = le16(h, 22).unwrap() as u32;
    let cust_max = le16(h, 24).unwrap() as u32;
    let key_max = le16(h, 26).unwrap() as u32;
    if rmpfmt != 0 {
        return Err("unknown key pool table");
    }
    let mut table = (key_max * cust_max * dev_max) >> 3;
    if defen != 0 {
        table += 8;
    }
    if table + MSS_HDR_LEN != key_raw {
        return Err("bad key pool");
    }
    // The length of a signature is in the section's own data (bytes 60-61,
    // shifted right by two as rtw89 reads it), 512 when zero.
    let sign = match le16(fw, data + 60).ok_or("security section truncated")? as u32 >> 2 {
        0 => SIG_LEN,
        n => n,
    };
    Ok(MSS_HDR_LEN + table + pairs * sign)
}

/// A table after the images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: u32,
    pub version: [u8; 4],
    pub offset: usize,
    pub size: usize,
}

/// The tables after the images (empty for a file that has none).
pub fn elements(file: &[u8]) -> Vec<Element> {
    let Ok((list, _)) = entries(file) else { return Vec::new() };
    let Some(last) = list.last() else { return Vec::new() };
    let mut at = (last.offset + last.size).next_multiple_of(ELEMENT_ALIGN);
    let mut out = Vec::new();
    while at + ELEMENT_HDR_LEN < file.len() {
        let (Some(id), Some(size)) = (le32(file, at), le32(file, at + 4)) else { break };
        let size = size as usize;
        if at + size >= file.len() {
            break;
        }
        out.push(Element {
            id,
            version: file[at + 8..at + 12].try_into().unwrap(),
            offset: at + ELEMENT_HDR_LEN,
            size,
        });
        at = (at + ELEMENT_HDR_LEN + size).next_multiple_of(ELEMENT_ALIGN);
    }
    out
}

/// The pieces (offset in the image, length) a section is downloaded in.
pub fn parts(s: &Section) -> impl Iterator<Item = (usize, usize)> + '_ {
    (0..s.len).step_by(PART_LEN).map(move |o| (s.offset + o, (s.len - o).min(PART_LEN)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(name: &str) -> Option<Vec<u8>> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
        std::fs::read(p).ok()
    }

    #[test]
    fn shipped_firmware() {
        let f = file("firmware/rtw8852b_fw-2.bin").expect("firmware/rtw8852b_fw-2.bin");
        let (list, ver) = entries(&f).unwrap();
        std::println!("file version {ver:?}");
        for e in &list {
            std::println!("{e:?}");
        }
        // Nothing for the first cut (CAV), as in rtw89; every image for the
        // others parses and its sections fill it exactly.
        assert!(select(&f, 0).is_err());
        for cv in 1..4 {
            let (e, data) = select(&f, cv).unwrap();
            assert!(e.cv <= cv);
            let img = parse(data).unwrap();
            std::println!(
                "cv {cv}: image cv {} v{}.{}.{}.{} built {:?}, header {} bytes, {} sections",
                e.cv,
                img.version[0],
                img.version[1],
                img.version[2],
                img.version[3],
                img.built,
                img.header_len,
                img.sections.len()
            );
            for s in &img.sections {
                std::println!("  {s:?}");
                assert!(s.len > 0);
                assert_eq!(parts(s).map(|(_, n)| n).sum::<usize>(), s.len);
            }
            // The 8852B firmware is version 0.29.x or newer.
            assert_eq!(img.version[0], 0);
            assert!(img.version[1] >= 27, "{:?}", img.version);
        }
        let els = elements(&f);
        std::println!("{} elements: {:?}", els.len(), els.iter().map(|e| e.id).collect::<Vec<_>>());
        // The BB and RF tables of the 8852B come in the file: BB_REG 2,
        // BB_GAIN 3, RADIO_A 4, RADIO_B 5, RF_NCTL 8, then the power tables.
        for id in [2, 3, 4, 5, 8, 9, 10, 11, 18, 20] {
            assert!(els.iter().any(|e| e.id == id), "element {id}");
        }
        for e in &els {
            assert!(e.id < 64, "{e:?}");
        }
        // They fill the file to its end.
        let last = els.last().unwrap();
        assert_eq!((last.offset + last.size).next_multiple_of(16), f.len().next_multiple_of(16));
    }

    /// The older files of linux-firmware, when they were downloaded into
    /// build/rtw89-fw (not part of the repository).
    #[test]
    fn older_firmware_files() {
        for name in ["rtw8852b_fw.bin", "rtw8852b_fw-1.bin"] {
            let Some(f) = file(&std::format!("../../../build/rtw89-fw/{name}")) else { continue };
            let (_, data) = select(&f, 1).unwrap();
            let img = parse(data).unwrap();
            std::println!("{name}: v{:?}, {} sections", img.version, img.sections.len());
        }
    }

    #[test]
    fn rejects_damage() {
        let f = file("firmware/rtw8852b_fw-2.bin").unwrap();
        let (_, data) = select(&f, 1).unwrap();
        assert!(parse(&data[..data.len() - 1]).is_err());
        assert!(entries(&f[1..]).is_err());
        let mut bad = f.clone();
        bad[16 + 4..16 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(entries(&bad).is_err());
    }
}
