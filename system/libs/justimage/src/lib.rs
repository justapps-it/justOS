//! Pictures: PNG, JPEG (baseline and progressive) and GIF into canvases.
//! The desktop draws the icons of Linux programs (PNG files named by their
//! .desktop files) with them.

#![no_std]

extern crate alloc;

mod gif;
mod jpeg;
mod jpeg_enc;
mod png;

pub use jpeg_enc::encode as encode_jpeg;

use justgfx::Canvas;

/// The largest picture we decode, in pixels (larger ones would take too
/// much memory for what a window shows).
pub const MAX_PIXELS: usize = 24 << 20;

/// Decodes a picture by its signature.
pub fn decode(data: &[u8]) -> Option<Canvas> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        png::decode(data)
    } else if data.starts_with(&[0xFF, 0xD8]) {
        jpeg::decode(data)
    } else if data.starts_with(b"GIF8") {
        gif::decode(data)
    } else {
        None
    }
}

/// The size of a picture without decoding it, when its header says.
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    if data.starts_with(b"\x89PNG") && data.len() >= 24 {
        let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        return Some((be(16), be(20)));
    }
    if data.starts_with(b"GIF8") && data.len() >= 10 {
        return Some((u16::from_le_bytes([data[6], data[7]]) as u32, u16::from_le_bytes([data[8], data[9]]) as u32));
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return jpeg::size(data);
    }
    None
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec::Vec;

    /// A picture with flat areas, edges and a gradient survives encoding:
    /// the decoder reads it back at its size, close to the original.
    #[test]
    fn jpeg_round_trip() {
        let (w, h) = (37usize, 21usize);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = if x < 12 {
                    0x00FF_5A1F
                } else if y < 8 {
                    0x0010_1010
                } else {
                    ((x * 6) as u32) << 16 | ((y * 12) as u32) << 8 | 0x80
                };
                px.push(p);
            }
        }
        let file = encode_jpeg(&px, w, h, w, 85);
        assert_eq!(&file[..2], &[0xFF, 0xD8]);
        assert_eq!(size(&file), Some((w as u32, h as u32)));
        let c = decode(&file).expect("decodes");
        assert_eq!((c.w as usize, c.h as usize), (w, h));
        let mut worst = 0i32;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if x == 11 || x == 12 || y == 7 || y == 8 {
                    continue; // the edges ring a little
                }
                let (a, b) = (px[y * w + x], c.px[y * c.w as usize + x]);
                for s in [16, 8, 0] {
                    let d = ((a >> s) & 0xFF) as i32 - ((b >> s) & 0xFF) as i32;
                    worst = worst.max(d.abs());
                }
            }
        }
        assert!(worst < 40, "worst channel error {worst}");
    }
}
