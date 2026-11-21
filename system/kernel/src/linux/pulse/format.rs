//! Sample formats and channel layouts of PulseAudio streams, and how their
//! sound becomes the mixer's (16-bit stereo, at the stream's own rate) and
//! how a recording's 16-bit stereo becomes theirs.

use alloc::vec::Vec;

/// A stream's samples: format, channels and frames per second.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Spec {
    pub format: u8,
    pub channels: u8,
    pub rate: u32,
}

// The sample formats, by number.
const U8: u8 = 0;
const ALAW: u8 = 1;
const ULAW: u8 = 2;
pub const S16LE: u8 = 3;
const S16BE: u8 = 4;
const F32LE: u8 = 5;
const F32BE: u8 = 6;
const S32LE: u8 = 7;
const S32BE: u8 = 8;
const S24LE: u8 = 9;
const S24BE: u8 = 10;
const S24_32LE: u8 = 11;
const S24_32BE: u8 = 12;

/// The highest rate PulseAudio takes.
const RATE_MAX: u32 = 384_000;
/// What PulseAudio calls full volume (0 dB).
pub const VOLUME_NORM: u32 = 0x10000;

impl Spec {
    pub fn valid(&self) -> bool {
        self.format <= S24_32BE && (1..=super::tag::CHANNELS_MAX as u8).contains(&self.channels) && (1..=RATE_MAX).contains(&self.rate)
    }

    fn sample_size(&self) -> usize {
        match self.format {
            U8 | ALAW | ULAW => 1,
            S16LE | S16BE => 2,
            S24LE | S24BE => 3,
            _ => 4,
        }
    }

    pub fn frame_size(&self) -> usize {
        self.sample_size() * self.channels as usize
    }

    pub fn bytes_to_usec(&self, bytes: u64) -> u64 {
        bytes / self.frame_size() as u64 * 1_000_000 / self.rate as u64
    }
}

/// One sample as a signed 16-bit one.
fn sample(format: u8, b: &[u8]) -> i16 {
    match format {
        U8 => ((b[0] as i16) - 128) << 8,
        ALAW => alaw(b[0]),
        ULAW => ulaw(b[0]),
        S16LE => i16::from_le_bytes([b[0], b[1]]),
        S16BE => i16::from_be_bytes([b[0], b[1]]),
        F32LE => float(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        F32BE => float(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
        // The top 16 bits of the wider ones (24 bits in 32: the low three
        // bytes).
        S32LE => i16::from_le_bytes([b[2], b[3]]),
        S24LE | S24_32LE => i16::from_le_bytes([b[1], b[2]]),
        S32BE | S24BE => i16::from_be_bytes([b[0], b[1]]),
        S24_32BE => i16::from_be_bytes([b[1], b[2]]),
        _ => 0,
    }
}

/// A 32-bit float sample (-1 to 1) as 16 bits, without floating point: the
/// kernel does not use it. The mantissa, shifted by the exponent, is the
/// value in 1/32768.
fn float(bits: u32) -> i16 {
    let exp = (bits >> 23) & 0xFF;
    let v = if exp < 112 {
        // Below 1/32768: silence.
        0
    } else if exp >= 127 {
        // 1 and more (and infinities, NaN): as loud as it goes.
        32767
    } else {
        let mantissa = (bits & 0x7F_FFFF) | 0x80_0000;
        ((mantissa >> (135 - exp)) as i32).min(32767)
    };
    if bits >> 31 != 0 { -v as i16 } else { v as i16 }
}

/// G.711 A-law, as in the standard.
fn alaw(a: u8) -> i16 {
    let a = a ^ 0x55;
    let seg = (a >> 4) & 7;
    let mut t = ((a & 0x0F) as i16) << 4;
    t = match seg {
        0 => t + 8,
        1 => t + 0x108,
        _ => (t + 0x108) << (seg - 1),
    };
    if a & 0x80 != 0 { t } else { -t }
}

/// G.711 µ-law, as in the standard.
fn ulaw(u: u8) -> i16 {
    let u = !u;
    let t = ((((u & 0x0F) as i16) << 3) + 0x84) << ((u >> 4) & 7);
    if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }
}

/// How a stream's channels go to the left and the right: a weight for each
/// side, by channel position (PulseAudio's numbers). The low-frequency
/// channel is left out; the sides are averaged by their weights, so that a
/// stream with many channels is no louder than one with two.
pub struct Layout {
    spec: Spec,
    weights: Vec<(i32, i32)>,
    total: (i32, i32),
}

/// Channel positions: mono, front left/right/centre, rear centre/left/
/// right, LFE, front left/right of centre, side left/right, the auxiliary
/// ones (12 to 43), then the top ones (44 to 50).
fn side(position: u8) -> (i32, i32) {
    match position {
        1 | 5 | 8 | 10 | 45 | 48 => (2, 0),
        2 | 6 | 9 | 11 | 46 | 49 => (0, 2),
        0 | 3 | 4 | 44 | 47 | 50 => (1, 1),
        7 => (0, 0),
        12..=43 if position.is_multiple_of(2) => (2, 0),
        12..=43 => (0, 2),
        _ => (1, 1),
    }
}

/// The map PulseAudio gives a stream that says none.
pub fn default_map(channels: u8) -> Vec<u8> {
    match channels {
        1 => alloc::vec![0],
        2 => alloc::vec![1, 2],
        n => (0..n).map(|i| [1, 2, 5, 6, 3, 7, 10, 11].get(i as usize).copied().unwrap_or(12 + i)).collect(),
    }
}

impl Layout {
    pub fn new(spec: Spec, map: &[u8]) -> Layout {
        let map = if map.len() == spec.channels as usize { map.to_vec() } else { default_map(spec.channels) };
        let weights: Vec<(i32, i32)> = map.iter().map(|&p| side(p)).collect();
        let total = weights.iter().fold((0, 0), |(l, r), w| (l + w.0, r + w.1));
        Layout { spec, weights, total: (total.0.max(1), total.1.max(1)) }
    }

    /// The whole frames of `data` as stereo frames, added to `out`.
    pub fn convert(&self, data: &[u8], out: &mut Vec<[i16; 2]>) {
        let size = self.spec.sample_size();
        let frame = self.spec.frame_size();
        out.reserve(data.len() / frame);
        for f in data.chunks_exact(frame) {
            if self.spec.channels == 2 && self.weights == [(2, 0), (0, 2)] {
                out.push([sample(self.spec.format, &f[..size]), sample(self.spec.format, &f[size..])]);
                continue;
            }
            let (mut l, mut r) = (0i32, 0i32);
            for (s, w) in f.chunks_exact(size).zip(&self.weights) {
                let v = sample(self.spec.format, s) as i32;
                l += v * w.0;
                r += v * w.1;
            }
            out.push([(l / self.total.0) as i16, (r / self.total.1) as i16]);
        }
    }
}

/// A signed 16-bit sample in `format`, added to `out`.
fn put_sample(format: u8, v: i16, out: &mut Vec<u8>) {
    let wide = (v as i32) << 16;
    match format {
        U8 => out.push(((v >> 8) + 128) as u8),
        ALAW => out.push(to_alaw(v)),
        ULAW => out.push(to_ulaw(v)),
        S16LE => out.extend_from_slice(&v.to_le_bytes()),
        S16BE => out.extend_from_slice(&v.to_be_bytes()),
        F32LE => out.extend_from_slice(&to_float(v).to_le_bytes()),
        F32BE => out.extend_from_slice(&to_float(v).to_be_bytes()),
        S32LE => out.extend_from_slice(&wide.to_le_bytes()),
        S32BE => out.extend_from_slice(&wide.to_be_bytes()),
        S24LE => out.extend_from_slice(&wide.to_le_bytes()[1..]),
        S24BE => out.extend_from_slice(&wide.to_be_bytes()[..3]),
        S24_32LE => out.extend_from_slice(&(wide >> 8).to_le_bytes()),
        _ => out.extend_from_slice(&(wide >> 8).to_be_bytes()),
    }
}

/// A 16-bit sample as a 32-bit float (-1 to 1), built bit by bit: the
/// kernel does not use floating point. The highest bit set gives the
/// exponent, the bits below it the mantissa.
fn to_float(v: i16) -> u32 {
    if v == 0 {
        return 0;
    }
    let m = v.unsigned_abs() as u32;
    let e = 31 - m.leading_zeros();
    let bits = (112 + e) << 23 | ((m << (23 - e)) & 0x7F_FFFF);
    if v < 0 { bits | 1 << 31 } else { bits }
}

/// G.711 A-law, as in the standard.
fn to_alaw(v: i16) -> u8 {
    let mut s = (v >> 3) as i32;
    let mask = if s >= 0 {
        0xD5
    } else {
        s = -s - 1;
        0x55
    };
    let seg = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF].iter().position(|&end| s <= end);
    let Some(seg) = seg else { return 0x7F ^ mask };
    let low = if seg < 2 { (s >> 1) & 0x0F } else { (s >> seg) & 0x0F };
    ((seg as i32) << 4 | low) as u8 ^ mask
}

/// G.711 µ-law, as in the standard.
fn to_ulaw(v: i16) -> u8 {
    let mut s = v as i32;
    let sign = if s < 0 {
        s = -s;
        0x80
    } else {
        0
    };
    s = s.min(32635) + 0x84;
    let exp = 31 - (s as u32).leading_zeros() as i32 - 7;
    let mantissa = (s >> (exp + 3)) & 0x0F;
    !((sign | exp << 4 | mantissa) as u8)
}

/// How a recording's stereo becomes a stream's channels: each channel
/// takes the left, the right or (in the middle, and for mono) the average
/// of both, by its position.
pub struct Unlayout {
    spec: Spec,
    /// For each channel: 0 the average, 1 the left, 2 the right.
    sides: Vec<u8>,
}

impl Unlayout {
    pub fn new(spec: Spec, map: &[u8]) -> Unlayout {
        let map = if map.len() == spec.channels as usize { map.to_vec() } else { default_map(spec.channels) };
        let sides = map
            .iter()
            .map(|&p| match side(p) {
                (2, 0) => 1,
                (0, 2) => 2,
                _ => 0,
            })
            .collect();
        Unlayout { spec, sides }
    }

    /// `frames` in the stream's format, added to `out`.
    pub fn convert(&self, frames: &[[i16; 2]], out: &mut Vec<u8>) {
        out.reserve(frames.len() * self.spec.frame_size());
        for f in frames {
            for &s in &self.sides {
                let v = match s {
                    1 => f[0],
                    2 => f[1],
                    _ => ((f[0] as i32 + f[1] as i32) / 2) as i16,
                };
                put_sample(self.spec.format, v, out);
            }
        }
    }
}

/// A PulseAudio volume as a gain in 1/65536. PulseAudio's volumes are on a
/// cubic curve (full volume: 0x10000); above twice that it stops.
pub fn gain(volume: u32) -> u32 {
    let v = volume.min(2 * VOLUME_NORM) as u64;
    ((v * v * v) >> 32) as u32
}

/// The gain of a stream from its volume on each channel: their average.
pub fn stream_gain(volumes: &[u32], muted: bool) -> u32 {
    if muted || volumes.is_empty() {
        return if muted { 0 } else { 65536 };
    }
    let avg = volumes.iter().map(|&v| v as u64).sum::<u64>() / volumes.len() as u64;
    gain(avg.min(u32::MAX as u64) as u32)
}
