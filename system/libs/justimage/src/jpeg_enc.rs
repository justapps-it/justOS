//! A baseline JPEG encoder: YCbCr with 4:2:0 chroma, the standard
//! quantisation tables scaled by a quality, the standard Huffman tables of
//! the specification (ITU T.81 annex K). Small and quick rather than the
//! smallest file: it is what JustOS Connect sends a phone, frame after
//! frame, while it shows the screen.
//!
//! The DCT is integer (a fixed-point matrix with 12 fraction bits, rows then
//! columns): the kernel draws without the FPU.

use alloc::vec::Vec;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54,
    47, 55, 62, 63,
];

const Q_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51,
    87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];

const Q_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14,
    0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09,
    0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a,
    0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65,
    0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88,
    0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9,
    0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca,
    0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32,
    0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16,
    0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
    0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86,
    0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
    0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// cos((2x+1)uπ/16) · C(u)/2 · 4096, for u (row) and x (column).
const DCT: [[i32; 8]; 8] = {
    // Precomputed: C(0) = 1/√2, C(u>0) = 1.
    [
        [1448, 1448, 1448, 1448, 1448, 1448, 1448, 1448],
        [2009, 1703, 1138, 400, -400, -1138, -1703, -2009],
        [1892, 784, -784, -1892, -1892, -784, 784, 1892],
        [1703, -400, -2009, -1138, 1138, 2009, 400, -1703],
        [1448, -1448, -1448, 1448, 1448, -1448, -1448, 1448],
        [1138, -2009, 400, 1703, -1703, -400, 2009, -1138],
        [784, -1892, 1892, -784, -784, 1892, -1892, 784],
        [400, -1138, 1703, -2009, 2009, -1703, 1138, -400],
    ]
};

/// A Huffman code table: code and length for each symbol.
struct Huff {
    code: [u16; 256],
    len: [u8; 256],
}

impl Huff {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Huff {
        let mut h = Huff { code: [0; 256], len: [0; 256] };
        let mut code = 0u16;
        let mut k = 0;
        for (i, &n) in bits.iter().enumerate() {
            for _ in 0..n {
                let v = vals[k] as usize;
                h.code[v] = code;
                h.len[v] = i as u8 + 1;
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        h
    }
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code & ((1 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0); // byte stuffing
            }
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }

    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

/// The bit count and bits of a coefficient, as JPEG writes them.
fn magnitude(v: i32) -> (u32, u32) {
    let a = v.unsigned_abs();
    let size = 32 - a.leading_zeros();
    let bits = if v < 0 { (v - 1) as u32 } else { v as u32 };
    (size, bits & ((1u32 << size) - 1))
}

/// Forward DCT and quantisation of one 8x8 block (samples level shifted),
/// in zigzag order.
fn fdct_quant(block: &[i32; 64], q: &[i32; 64], out: &mut [i32; 64]) {
    let mut tmp = [0i32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0i32;
            for x in 0..8 {
                s += DCT[u][x] * block[y * 8 + x];
            }
            tmp[y * 8 + u] = (s + 2048) >> 12;
        }
    }
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0i32;
            for y in 0..8 {
                s += DCT[v][y] * tmp[y * 8 + u];
            }
            let f = (s + 2048) >> 12;
            let i = v * 8 + u;
            let d = q[i];
            out[i] = if f >= 0 { (f + d / 2) / d } else { -((-f + d / 2) / d) };
        }
    }
}

fn scaled(base: &[u16; 64], quality: u32) -> [u8; 64] {
    let quality = quality.clamp(1, 100);
    let s = if quality < 50 { 5000 / quality } else { 200 - quality * 2 };
    let mut t = [0u8; 64];
    for i in 0..64 {
        t[i] = ((base[i] as u32 * s + 50) / 100).clamp(1, 255) as u8;
    }
    t
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
}

fn encode_block(bits: &mut Bits, coef: &[i32; 64], prev_dc: &mut i32, dc: &Huff, ac: &Huff) {
    let diff = coef[0] - *prev_dc;
    *prev_dc = coef[0];
    let (size, v) = magnitude(diff);
    bits.put(dc.code[size as usize] as u32, dc.len[size as usize] as u32);
    bits.put(v, size);
    let mut run = 0;
    for &z in &ZIGZAG[1..] {
        let c = coef[z];
        if c == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            bits.put(ac.code[0xF0] as u32, ac.len[0xF0] as u32);
            run -= 16;
        }
        let (size, v) = magnitude(c);
        let sym = (run << 4) | size as usize;
        bits.put(ac.code[sym] as u32, ac.len[sym] as u32);
        bits.put(v, size);
        run = 0;
    }
    if run > 0 {
        bits.put(ac.code[0] as u32, ac.len[0] as u32); // end of block
    }
}

/// Encodes `w` x `h` pixels (0x__RRGGBB, `stride` pixels a row) as a JPEG
/// file at `quality` (1-100).
pub fn encode(px: &[u32], w: usize, h: usize, stride: usize, quality: u32) -> Vec<u8> {
    if w == 0 || h == 0 || w > 65535 || h > 65535 {
        return Vec::new();
    }
    let ql = scaled(&Q_LUMA, quality);
    let qc = scaled(&Q_CHROMA, quality);
    let mut out = Vec::with_capacity(w * h / 4 + 1024);
    out.extend_from_slice(&[0xFF, 0xD8]);
    segment(&mut out, 0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    let mut dqt = Vec::with_capacity(130);
    for (id, t) in [(0u8, &ql), (1, &qc)] {
        dqt.push(id);
        for &z in &ZIGZAG {
            dqt.push(t[z]);
        }
    }
    segment(&mut out, 0xDB, &dqt);
    let (hw, hh) = ((w as u16).to_be_bytes(), (h as u16).to_be_bytes());
    segment(&mut out, 0xC0, &[8, hh[0], hh[1], hw[0], hw[1], 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    let mut dht = Vec::with_capacity(432);
    for (class_id, b, v) in [
        (0x00u8, &DC_LUMA_BITS, &DC_VALS[..]),
        (0x10, &AC_LUMA_BITS, &AC_LUMA_VALS[..]),
        (0x01, &DC_CHROMA_BITS, &DC_VALS[..]),
        (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS[..]),
    ] {
        dht.push(class_id);
        dht.extend_from_slice(b);
        dht.extend_from_slice(v);
    }
    segment(&mut out, 0xC4, &dht);
    segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);

    let (dc_l, ac_l) = (Huff::new(&DC_LUMA_BITS, &DC_VALS), Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS));
    let (dc_c, ac_c) = (Huff::new(&DC_CHROMA_BITS, &DC_VALS), Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS));
    // Quantisers in natural order: the transform computes F as T.81 defines
    // it, so the tables apply as they are.
    let mut qlum = [0i32; 64];
    let mut qchr = [0i32; 64];
    for i in 0..64 {
        qlum[i] = ql[i] as i32;
        qchr[i] = qc[i] as i32;
    }
    let mut bits = Bits { out, acc: 0, n: 0 };
    let (mut py, mut pcb, mut pcr) = (0i32, 0i32, 0i32);
    let mut yb = [[0i32; 64]; 4];
    let mut cb = [0i32; 64];
    let mut cr = [0i32; 64];
    let mut coef = [0i32; 64];
    let at = |x: usize, y: usize| -> u32 { px[y.min(h - 1) * stride + x.min(w - 1)] };
    for my in (0..h).step_by(16) {
        for mx in (0..w).step_by(16) {
            // Four luma blocks, and the chroma of the 16x16 averaged 2x2.
            for by in 0..16 {
                for bx in 0..16 {
                    let p = at(mx + bx, my + by);
                    let (r, g, b) = (((p >> 16) & 0xFF) as i32, ((p >> 8) & 0xFF) as i32, (p & 0xFF) as i32);
                    let yy = (19595 * r + 38470 * g + 7471 * b + 32768) >> 16;
                    yb[(by / 8) * 2 + bx / 8][(by % 8) * 8 + bx % 8] = yy - 128;
                }
            }
            for cy in 0..8 {
                for cx in 0..8 {
                    let (mut sr, mut sg, mut sb) = (0i32, 0i32, 0i32);
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let p = at(mx + cx * 2 + dx, my + cy * 2 + dy);
                        sr += ((p >> 16) & 0xFF) as i32;
                        sg += ((p >> 8) & 0xFF) as i32;
                        sb += (p & 0xFF) as i32;
                    }
                    let (r, g, b) = (sr / 4, sg / 4, sb / 4);
                    cb[cy * 8 + cx] = (-11059 * r - 21709 * g + 32768 * b + 32768) >> 16;
                    cr[cy * 8 + cx] = (32768 * r - 27439 * g - 5329 * b + 32768) >> 16;
                }
            }
            for blk in &yb {
                fdct_quant(blk, &qlum, &mut coef);
                encode_block(&mut bits, &coef, &mut py, &dc_l, &ac_l);
            }
            fdct_quant(&cb, &qchr, &mut coef);
            encode_block(&mut bits, &coef, &mut pcb, &dc_c, &ac_c);
            fdct_quant(&cr, &qchr, &mut coef);
            encode_block(&mut bits, &coef, &mut pcr, &dc_c, &ac_c);
        }
    }
    bits.flush();
    let mut out = bits.out;
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}
