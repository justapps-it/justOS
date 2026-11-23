//! SBC, the codec every A2DP device has (A2DP 1.3, appendix B): an
//! encoder, in fixed point, for sending sound to Bluetooth headphones.
//!
//! Eight subbands only (every sink must accept them); 4, 8, 12 or 16
//! blocks; mono, dual channel, stereo and joint stereo; loudness or SNR
//! bit allocation. [`Encoder::encode`] takes one frame's samples,
//! interleaved, and appends the frame.
//!
//! The tests decode what the encoder makes with the decoder the
//! specification describes and check that the sound comes back.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::vec::Vec;

pub const SYNCWORD: u8 = 0x9C;
pub const SUBBANDS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Mono,
    DualChannel,
    Stereo,
    JointStereo,
}

impl Mode {
    pub fn channels(self) -> usize {
        if self == Mode::Mono {
            1
        } else {
            2
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// 16000, 32000, 44100 or 48000.
    pub rate: u32,
    pub mode: Mode,
    /// 4, 8, 12 or 16.
    pub blocks: u8,
    /// SNR allocation instead of loudness.
    pub snr: bool,
    pub bitpool: u8,
}

impl Config {
    /// What most headphones get: 44.1 kHz joint stereo, 16 blocks,
    /// loudness, bitpool 53 (about 328 kbit/s).
    pub const HIGH: Config = Config { rate: 44100, mode: Mode::JointStereo, blocks: 16, snr: false, bitpool: 53 };

    fn rate_code(&self) -> u8 {
        match self.rate {
            16000 => 0,
            32000 => 1,
            44100 => 2,
            _ => 3,
        }
    }

    pub fn channels(&self) -> usize {
        self.mode.channels()
    }

    /// Samples per channel in a frame.
    pub fn samples(&self) -> usize {
        self.blocks as usize * SUBBANDS
    }

    /// The largest bitpool the mode allows.
    pub fn max_bitpool(&self) -> u8 {
        match self.mode {
            Mode::Mono | Mode::DualChannel => (16 * SUBBANDS) as u8,
            _ => 250,
        }
    }

    /// Bytes in a frame.
    pub fn frame_len(&self) -> usize {
        let nch = self.channels();
        let b = self.blocks as usize;
        let bp = self.bitpool as usize;
        let data = match self.mode {
            Mode::Mono | Mode::DualChannel => b * nch * bp,
            Mode::Stereo => b * bp,
            Mode::JointStereo => SUBBANDS + b * bp,
        };
        4 + 4 * SUBBANDS * nch / 8 + data.div_ceil(8)
    }

    /// Bits per second.
    pub fn bitrate(&self) -> u32 {
        (8 * self.frame_len() as u64 * self.rate as u64 / self.samples() as u64) as u32
    }

    fn header(&self) -> [u8; 3] {
        let blocks = (self.blocks / 4).clamp(1, 4) - 1;
        let mode = match self.mode {
            Mode::Mono => 0,
            Mode::DualChannel => 1,
            Mode::Stereo => 2,
            Mode::JointStereo => 3,
        };
        [SYNCWORD, self.rate_code() << 6 | blocks << 4 | mode << 2 | (self.snr as u8) << 1 | 1, self.bitpool]
    }

    /// Reads a frame header (sync word, then two bytes): the configuration.
    pub fn parse(h: &[u8]) -> Option<Config> {
        if h.len() < 3 || h[0] != SYNCWORD || h[1] & 1 == 0 {
            return None;
        }
        let rate = [16000, 32000, 44100, 48000][(h[1] >> 6) as usize];
        let blocks = 4 * (((h[1] >> 4) & 3) + 1);
        let mode = [Mode::Mono, Mode::DualChannel, Mode::Stereo, Mode::JointStereo][((h[1] >> 2) & 3) as usize];
        Some(Config { rate, mode, blocks, snr: h[1] & 2 != 0, bitpool: h[2] })
    }
}

/// The prototype filter for eight subbands (table B.4 of A2DP), times 2^31.
const PROTO: [i32; 80] = [
    0, 336243, 737138, 1191038, 1769354, 2447970, 3170548, 3830504, //
    4320362, 4517704, 4283254, 3471542, 1937362, -383982, -3542770, -7510125, //
    12153672, 17243030, 22459338, 27374475, 31466061, 34154783, 34834004, 32896036, //
    27782384, 19021498, 6279423, -10556558, -31440036, -56070530, -83913220, -114218864, //
    146026618, 178208410, 209541558, 238793071, 264708601, 286183152, 302265850, 312222319, //
    315583606, 312222319, 302265850, 286183152, 264708601, 238793071, 209541558, 178208410, //
    -146026618, -114218864, -83913220, -56070530, -31440036, -10556558, 6279423, 19021498, //
    27782384, 32896036, 34834004, 34154783, 31466061, 27374475, 22459338, 17243030, //
    -12153672, -7510125, -3542770, -383982, 1937362, 3471542, 4283254, 4517704, //
    4320362, 3830504, 3170548, 2447970, 1769354, 1191038, 737138, 336243,
];

/// cos((k + 0.5)(i - 4)π/8), times 32767: the analysis matrix.
const COS: [[i32; 16]; 8] = [
    [23170, 27245, 30273, 32137, 32767, 32137, 30273, 27245, 23170, 18204, 12539, 6393, 0, -6393, -12539, -18204],
    [-23170, -6393, 12539, 27245, 32767, 27245, 12539, -6393, -23170, -32137, -30273, -18204, 0, 18204, 30273, 32137],
    [-23170, -32137, -12539, 18204, 32767, 18204, -12539, -32137, -23170, 6393, 30273, 27245, 0, -27245, -30273, -6393],
    [23170, -18204, -30273, 6393, 32767, 6393, -30273, -18204, 23170, 27245, -12539, -32137, 0, 32137, 12539, -27245],
    [23170, 18204, -30273, -6393, 32767, -6393, -30273, 18204, 23170, -27245, -12539, 32137, 0, -32137, 12539, 27245],
    [-23170, 32137, -12539, -18204, 32767, -18204, -12539, 32137, -23170, -6393, 30273, -27245, 0, 27245, -30273, 6393],
    [-23170, 6393, 12539, -27245, 32767, -27245, 12539, 6393, -23170, 32137, -30273, 18204, 0, -18204, 30273, -32137],
    [23170, -27245, 30273, -32137, 32767, -32137, 30273, -27245, 23170, -18204, 12539, -6393, 0, 6393, -12539, 18204],
];

/// Loudness offsets for eight subbands, by sampling rate (table 12.17).
const OFFSET8: [[i32; 8]; 4] = [
    [-2, 0, 0, 0, 0, 0, 0, 1],
    [-3, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
    [-4, 0, 0, 0, 0, 0, 1, 2],
];

/// Subband samples carry 15 fraction bits: |s| < 2^(sf + 16) for scale
/// factor sf.
const FRAC: u32 = 15;

/// Bits for every subband of every channel, from the scale factors
/// (section 12.6.3): the same on both sides of the link.
pub fn allocate(cfg: &Config, sf: &[[u8; SUBBANDS]; 2], bits: &mut [[u8; SUBBANDS]; 2]) {
    let nch = cfg.channels();
    let mut need = [[0i32; SUBBANDS]; 2];
    for ch in 0..nch {
        for sb in 0..SUBBANDS {
            let s = sf[ch][sb] as i32;
            need[ch][sb] = if cfg.snr {
                s
            } else if s == 0 {
                -5
            } else {
                let l = s - OFFSET8[cfg.rate_code() as usize][sb];
                if l > 0 {
                    l / 2
                } else {
                    l
                }
            };
        }
    }
    let pool = cfg.bitpool as i32;
    // Mono and dual channel: each channel on its own; stereo: both at once.
    let groups: &[&[usize]] = match cfg.mode {
        Mode::Mono => &[&[0]],
        Mode::DualChannel => &[&[0], &[1]],
        _ => &[&[0, 1]],
    };
    for chs in groups {
        let max = chs.iter().flat_map(|&c| need[c].iter()).copied().max().unwrap_or(0);
        let mut count = 0;
        let mut slice_count = 0;
        let mut slice = max + 1;
        loop {
            slice -= 1;
            count += slice_count;
            slice_count = 0;
            for &c in chs.iter() {
                for &n in need[c].iter() {
                    if n > slice + 1 && n < slice + 16 {
                        slice_count += 1;
                    } else if n == slice + 1 {
                        slice_count += 2;
                    }
                }
            }
            if count + slice_count >= pool {
                break;
            }
        }
        if count + slice_count == pool {
            count += slice_count;
            slice -= 1;
        }
        for &c in chs.iter() {
            for sb in 0..SUBBANDS {
                let n = need[c][sb];
                bits[c][sb] = if n < slice + 2 { 0 } else { (n - slice).min(16) as u8 };
            }
        }
        // What is left goes to the lower subbands first, channel by channel.
        let walk = |count: &mut i32, bits: &mut [[u8; SUBBANDS]; 2], first: bool| {
            let mut sb = 0;
            let mut i = 0;
            while *count < pool && sb < SUBBANDS {
                let c = chs[i];
                let b = &mut bits[c][sb];
                if first {
                    if *b >= 2 && *b < 16 {
                        *b += 1;
                        *count += 1;
                    } else if need[c][sb] == slice + 1 && pool > *count + 1 {
                        *b = 2;
                        *count += 2;
                    }
                } else if *b < 16 {
                    *b += 1;
                    *count += 1;
                }
                i += 1;
                if i == chs.len() {
                    i = 0;
                    sb += 1;
                }
            }
        };
        walk(&mut count, bits, true);
        walk(&mut count, bits, false);
    }
}

/// The CRC-8 of the header (polynomial x^8 + x^4 + x^3 + x^2 + 1, start
/// 0x0F) over the first `nbits` bits of `data`.
pub fn crc8(data: &[u8], nbits: usize) -> u8 {
    let mut crc: u8 = 0x0F;
    for i in 0..nbits {
        let bit = (data[i / 8] >> (7 - i % 8)) & 1;
        let top = (crc >> 7) ^ bit;
        crc <<= 1;
        if top != 0 {
            crc ^= 0x1D;
        }
    }
    crc
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitWriter<'_> {
    fn put(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = self.acc << 1 | (v >> i) & 1;
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn flush(&mut self) {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

fn scale_factor(max: i64) -> u8 {
    let mut sf = 0;
    while sf < 15 && max >= 1i64 << (sf + 1 + FRAC) {
        sf += 1;
    }
    sf as u8
}

pub struct Encoder {
    cfg: Config,
    /// The last 80 input samples of each channel, newest first.
    x: [[i32; 80]; 2],
}

impl Encoder {
    pub fn new(cfg: Config) -> Encoder {
        let mut cfg = cfg;
        cfg.blocks = match cfg.blocks {
            0..=4 => 4,
            5..=8 => 8,
            9..=12 => 12,
            _ => 16,
        };
        cfg.bitpool = cfg.bitpool.clamp(2, cfg.max_bitpool());
        Encoder { cfg, x: [[0; 80]; 2] }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The analysis filter (section 12.5.1) on eight new samples.
    fn analyse(&mut self, ch: usize, input: &[i16], at: usize, step: usize, out: &mut [i64; SUBBANDS]) {
        let x = &mut self.x[ch];
        x.copy_within(0..72, 8);
        for i in 0..8 {
            x[7 - i] = input[at + i * step] as i32;
        }
        let mut y = [0i64; 16];
        for (i, yi) in y.iter_mut().enumerate() {
            let mut acc = 0i64;
            for j in 0..5 {
                acc += x[i + 16 * j] as i64 * PROTO[i + 16 * j] as i64;
            }
            // From 31 fraction bits to 15.
            *yi = acc >> 16;
        }
        for (k, o) in out.iter_mut().enumerate() {
            let mut acc = 0i64;
            for i in 0..16 {
                acc += COS[k][i] as i64 * y[i];
            }
            *o = acc >> 15;
        }
    }

    /// Encodes one frame: [`Config::samples`] samples per channel,
    /// interleaved (fewer are taken as silence). Appends it to `out`.
    pub fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) {
        let cfg = self.cfg;
        let nch = cfg.channels();
        let blocks = cfg.blocks as usize;
        let mut buf;
        let pcm = if pcm.len() < cfg.samples() * nch {
            buf = Vec::from(pcm);
            buf.resize(cfg.samples() * nch, 0);
            &buf[..]
        } else {
            pcm
        };
        let mut s = [[[0i64; SUBBANDS]; 2]; 16];
        for (b, sb) in s.iter_mut().enumerate().take(blocks) {
            for (ch, c) in sb.iter_mut().enumerate().take(nch) {
                self.analyse(ch, pcm, b * SUBBANDS * nch + ch, nch, c);
            }
        }
        // Scale factors, and joint stereo where it saves bits.
        let limit = (1i64 << (16 + FRAC)) - 1;
        let mut sf = [[0u8; SUBBANDS]; 2];
        let max_of = |s: &[[[i64; SUBBANDS]; 2]; 16], ch: usize, sb: usize| s[..blocks].iter().map(|b| b[ch][sb].abs()).max().unwrap_or(0);
        for ch in 0..nch {
            for (sb, f) in sf[ch].iter_mut().enumerate() {
                *f = scale_factor(max_of(&s, ch, sb).min(limit));
            }
        }
        let mut join = 0u8;
        if cfg.mode == Mode::JointStereo {
            for sb in 0..SUBBANDS - 1 {
                let (mut m, mut d) = (0i64, 0i64);
                for b in &s[..blocks] {
                    m = m.max(((b[0][sb] + b[1][sb]) >> 1).abs());
                    d = d.max(((b[0][sb] - b[1][sb]) >> 1).abs());
                }
                let (fm, fd) = (scale_factor(m.min(limit)), scale_factor(d.min(limit)));
                if fm + fd < sf[0][sb] + sf[1][sb] {
                    join |= 0x80 >> sb;
                    sf[0][sb] = fm;
                    sf[1][sb] = fd;
                    for b in &mut s[..blocks] {
                        let (l, r) = (b[0][sb], b[1][sb]);
                        b[0][sb] = (l + r) >> 1;
                        b[1][sb] = (l - r) >> 1;
                    }
                }
            }
        }
        let mut bits = [[0u8; SUBBANDS]; 2];
        allocate(&cfg, &sf, &mut bits);

        let start = out.len();
        out.extend_from_slice(&cfg.header());
        out.push(0); // the CRC, below
        let mut w = BitWriter { out, acc: 0, n: 0 };
        if cfg.mode == Mode::JointStereo {
            w.put(join as u32, 8);
        }
        for f in sf.iter().take(nch) {
            for &v in f {
                w.put(v as u32, 4);
            }
        }
        let crc_bits = 16 + if cfg.mode == Mode::JointStereo { 8 } else { 0 } + 4 * SUBBANDS * nch;
        for b in &s[..blocks] {
            for ch in 0..nch {
                for sb in 0..SUBBANDS {
                    let nb = bits[ch][sb] as u32;
                    if nb == 0 {
                        continue;
                    }
                    let levels = (1i64 << nb) - 1;
                    let shift = sf[ch][sb] as u32 + 1 + FRAC;
                    let v = b[ch][sb].clamp(-limit, limit);
                    let q = (((v + (1i64 << shift)) * levels) >> (shift + 1)).clamp(0, levels - 1);
                    w.put(q as u32, nb);
                }
            }
        }
        w.flush();
        // The CRC covers the header after the sync word and the scale
        // factors, skipping its own byte.
        let mut covered: Vec<u8> = Vec::with_capacity(16);
        covered.extend_from_slice(&out[start + 1..start + 3]);
        covered.extend_from_slice(&out[start + 4..start + 4 + (crc_bits - 16).div_ceil(8)]);
        out[start + 3] = crc8(&covered, crc_bits);
        debug_assert_eq!(out.len() - start, cfg.frame_len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// The decoder of section 12.6, in floating point.
    struct Decoder {
        v: [[f64; 160]; 2],
    }

    impl Decoder {
        fn new() -> Decoder {
            Decoder { v: [[0.0; 160]; 2] }
        }

        /// Decodes one frame: its length and the samples, interleaved.
        fn frame(&mut self, f: &[u8]) -> (usize, Vec<f64>) {
            let cfg = Config::parse(f).expect("header");
            let len = cfg.frame_len();
            assert!(f.len() >= len);
            let nch = cfg.channels();
            let blocks = cfg.blocks as usize;
            let mut pos = 32usize;
            let mut get = |n: usize| {
                let mut v = 0u32;
                for _ in 0..n {
                    v = v << 1 | ((f[pos / 8] >> (7 - pos % 8)) & 1) as u32;
                    pos += 1;
                }
                v
            };
            let join = if cfg.mode == Mode::JointStereo { get(8) as u8 } else { 0 };
            let mut sf = [[0u8; SUBBANDS]; 2];
            for c in sf.iter_mut().take(nch) {
                for v in c.iter_mut() {
                    *v = get(4) as u8;
                }
            }
            let crc_bits = 16 + if cfg.mode == Mode::JointStereo { 8 } else { 0 } + 4 * SUBBANDS * nch;
            let mut covered = vec![f[1], f[2]];
            covered.extend_from_slice(&f[4..4 + (crc_bits - 16).div_ceil(8)]);
            assert_eq!(crc8(&covered, crc_bits), f[3], "CRC");
            let mut bits = [[0u8; SUBBANDS]; 2];
            allocate(&cfg, &sf, &mut bits);
            let mut out = Vec::new();
            for _ in 0..blocks {
                let mut s = [[0f64; SUBBANDS]; 2];
                for ch in 0..nch {
                    for sb in 0..SUBBANDS {
                        let nb = bits[ch][sb] as usize;
                        if nb == 0 {
                            continue;
                        }
                        let q = get(nb) as f64;
                        let levels = ((1u32 << nb) - 1) as f64;
                        s[ch][sb] = 2f64.powi(sf[ch][sb] as i32 + 1) * ((q * 2.0 + 1.0) / levels - 1.0);
                    }
                }
                for sb in 0..SUBBANDS {
                    if join & (0x80 >> sb) != 0 {
                        let (m, d) = (s[0][sb], s[1][sb]);
                        s[0][sb] = m + d;
                        s[1][sb] = m - d;
                    }
                }
                let mut x = [[0f64; 8]; 2];
                for ch in 0..nch {
                    self.synth(&s[ch], &mut x[ch], ch);
                }
                for j in 0..8 {
                    for c in x.iter().take(nch) {
                        out.push(c[j]);
                    }
                }
            }
            (len, out)
        }

        fn synth(&mut self, s: &[f64; SUBBANDS], x: &mut [f64; 8], ch: usize) {
            let v = &mut self.v[ch];
            v.copy_within(0..144, 16);
            for (k, vk) in v.iter_mut().enumerate().take(16) {
                *vk = (0..8).map(|i| (((i as f64 + 0.5) * (k as f64 + 4.0)) * core::f64::consts::PI / 8.0).cos() * s[i]).sum();
            }
            let mut u = [0f64; 80];
            for i in 0..5 {
                for j in 0..8 {
                    u[i * 16 + j] = v[i * 32 + j];
                    u[i * 16 + j + 8] = v[i * 32 + j + 24];
                }
            }
            for (j, xj) in x.iter_mut().enumerate() {
                *xj = (0..10).map(|i| u[j + 8 * i] * PROTO[j + 8 * i] as f64 / 2f64.powi(31) * -8.0).sum();
            }
        }
    }

    fn tone(rate: u32, freqs: &[f64], amp: f64, n: usize) -> Vec<i16> {
        let mut v = Vec::with_capacity(n * freqs.len());
        for i in 0..n {
            for &f in freqs {
                v.push((amp * (2.0 * core::f64::consts::PI * f * i as f64 / rate as f64).sin()).round() as i16);
            }
        }
        v
    }

    fn encode_all(cfg: Config, pcm: &[i16]) -> Vec<u8> {
        let mut e = Encoder::new(cfg);
        let step = cfg.samples() * cfg.channels();
        let mut out = Vec::new();
        for chunk in pcm.chunks(step) {
            e.encode(chunk, &mut out);
        }
        out
    }

    fn decode_all(sbc: &[u8]) -> Vec<f64> {
        let mut d = Decoder::new();
        let mut pos = 0;
        let mut out = Vec::new();
        while pos < sbc.len() {
            let (len, mut s) = d.frame(&sbc[pos..]);
            out.append(&mut s);
            pos += len;
        }
        out
    }

    /// Signal to noise ratio of `got` against `want`, channel `ch` of
    /// `nch`, at the best delay: (dB, delay).
    fn snr(want: &[i16], got: &[f64], nch: usize, ch: usize) -> (f64, usize) {
        let n = want.len() / nch;
        let mut best = (f64::MIN, 0);
        for delay in 0..100 {
            let (mut sig, mut err) = (0.0, 0.0);
            for i in 1000..n - 300 {
                let w = want[i * nch + ch] as f64;
                let g = got[(i + delay) * nch + ch];
                sig += w * w;
                err += (w - g) * (w - g);
            }
            let db = 10.0 * (sig / err.max(1e-9)).log10();
            if db > best.0 {
                best = (db, delay);
            }
        }
        best
    }

    #[test]
    fn lengths() {
        assert_eq!(Config::HIGH.frame_len(), 119);
        assert_eq!(Config::HIGH.bitrate(), 327_993);
        let mono = Config { rate: 48000, mode: Mode::Mono, blocks: 16, snr: false, bitpool: 31 };
        assert_eq!(mono.frame_len(), 4 + 4 + 62);
        assert_eq!(Config::parse(&Config::HIGH.header()), Some(Config::HIGH));
    }

    #[test]
    fn crc_of_the_spec() {
        // Every frame carries its CRC: a frame of silence checks it.
        let mut e = Encoder::new(Config::HIGH);
        let mut out = Vec::new();
        e.encode(&[], &mut out);
        assert_eq!(out.len(), 119);
        assert_eq!(&out[..3], &[0x9C, 0xBD, 53]);
        let mut d = Decoder::new();
        let (_, s) = d.frame(&out);
        assert!(s.iter().all(|v| v.abs() < 1.0));
    }

    #[test]
    fn tones_come_back() {
        for cfg in [
            Config::HIGH,
            Config { mode: Mode::Stereo, ..Config::HIGH },
            Config { mode: Mode::DualChannel, bitpool: 32, ..Config::HIGH },
            Config { rate: 48000, snr: true, blocks: 8, bitpool: 51, ..Config::HIGH },
        ] {
            let pcm = tone(cfg.rate, &[440.0, 1000.0], 12000.0, 8192);
            let got = decode_all(&encode_all(cfg, &pcm));
            for ch in 0..2 {
                let (db, delay) = snr(&pcm, &got, 2, ch);
                std::println!("{cfg:?} channel {ch}: {db:.1} dB at delay {delay}");
                assert!(db > 30.0, "{cfg:?}: {db} dB");
                // the tones repeat every 48 samples at 48 kHz, so 25 and 121 look alike
                assert_eq!(delay % 48, 73 % 48);
            }
        }
        let mono = Config { rate: 32000, mode: Mode::Mono, blocks: 12, snr: false, bitpool: 30 };
        let pcm = tone(32000, &[660.0], 20000.0, 6000);
        let (db, _) = snr(&pcm, &decode_all(&encode_all(mono, &pcm)), 1, 0);
        assert!(db > 25.0, "mono: {db} dB");
    }

    #[test]
    fn loud_input_does_not_wrap() {
        let pcm: Vec<i16> = (0..4096).map(|i| if (i / 37) % 2 == 0 { i16::MAX } else { i16::MIN }).collect();
        let got = decode_all(&encode_all(Config::HIGH, &pcm));
        assert!(got.iter().all(|v| v.abs() < 50000.0));
    }

    /// With SBC_DUMP=dir, writes a tone and its frames for a check with
    /// another decoder (BlueZ's sbcdec).
    #[test]
    fn dump() {
        let Ok(dir) = std::env::var("SBC_DUMP") else { return };
        let pcm = tone(44100, &[440.0, 1000.0], 12000.0, 44100);
        let raw: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(std::format!("{dir}/tone.raw"), raw).unwrap();
        std::fs::write(std::format!("{dir}/tone.sbc"), encode_all(Config::HIGH, &pcm)).unwrap();
    }
}
