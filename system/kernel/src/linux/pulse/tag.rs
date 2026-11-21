//! The PulseAudio wire format: packets are "tagstructs", a row of values
//! each led by a byte saying its type (big-endian numbers), and they travel
//! in frames with a 20-byte descriptor, as do the blocks of sound.

use super::format::Spec;
use alloc::string::String;
use alloc::vec::Vec;

/// "No index": no sink, no module, a request the server makes itself.
pub const INVALID: u32 = u32::MAX;

const STRING: u8 = b't';
const STRING_NULL: u8 = b'N';
const U32: u8 = b'L';
const U8: u8 = b'B';
const U64: u8 = b'R';
const S64: u8 = b'r';
const SAMPLE_SPEC: u8 = b'a';
const ARBITRARY: u8 = b'x';
const TRUE: u8 = b'1';
const FALSE: u8 = b'0';
const TIMEVAL: u8 = b'T';
const USEC: u8 = b'U';
const CHANNEL_MAP: u8 = b'm';
const CVOLUME: u8 = b'v';
const PROPLIST: u8 = b'P';
const VOLUME: u8 = b'V';
const FORMAT_INFO: u8 = b'f';

/// Channels a sample spec, map or volume may have.
pub const CHANNELS_MAX: usize = 32;
/// The largest value of a property, as libpulse checks it.
const MAX_PROP: usize = 64 * 1024;

/// Properties: names with bytes (text ends in a NUL).
pub type Props = Vec<(String, Vec<u8>)>;

/// Reads the values of a packet in order. A value that is not there or
/// not of the type asked for sets `bad` (and reads as zero): the caller
/// looks once, after reading everything.
pub struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
    pub bad: bool,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, at: 0, bad: false }
    }

    fn take(&mut self, n: usize) -> &'a [u8] {
        if self.bad || self.buf.len() - self.at < n {
            self.bad = true;
            return &[];
        }
        let b = &self.buf[self.at..self.at + n];
        self.at += n;
        b
    }

    fn tag(&mut self, t: u8) -> bool {
        let ok = self.take(1) == [t];
        self.bad |= !ok;
        !self.bad
    }

    fn raw_u32(&mut self) -> u32 {
        self.take(4).try_into().map_or(0, u32::from_be_bytes)
    }

    fn raw_u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }

    pub fn u32(&mut self) -> u32 {
        if self.tag(U32) { self.raw_u32() } else { 0 }
    }

    pub fn u8(&mut self) -> u8 {
        if self.tag(U8) { self.raw_u8() } else { 0 }
    }

    pub fn bool(&mut self) -> bool {
        match self.take(1) {
            [TRUE] => true,
            [FALSE] => false,
            _ => {
                self.bad = true;
                false
            }
        }
    }

    /// A string, or None for the null string.
    pub fn string(&mut self) -> Option<String> {
        if self.buf.get(self.at) == Some(&STRING_NULL) {
            self.at += 1;
            return None;
        }
        if !self.tag(STRING) {
            return None;
        }
        let rest = &self.buf[self.at..];
        let Some(n) = rest.iter().position(|&b| b == 0) else {
            self.bad = true;
            return None;
        };
        self.at += n + 1;
        Some(String::from_utf8_lossy(&rest[..n]).into_owned())
    }

    pub fn arbitrary(&mut self) -> &'a [u8] {
        if !self.tag(ARBITRARY) {
            return &[];
        }
        let n = self.raw_u32() as usize;
        self.take(n)
    }

    pub fn timeval(&mut self) -> (u32, u32) {
        if self.tag(TIMEVAL) { (self.raw_u32(), self.raw_u32()) } else { (0, 0) }
    }

    pub fn sample_spec(&mut self) -> Spec {
        if !self.tag(SAMPLE_SPEC) {
            return Spec::default();
        }
        let format = self.raw_u8();
        let channels = self.raw_u8();
        Spec { format, channels, rate: self.raw_u32() }
    }

    pub fn channel_map(&mut self) -> Vec<u8> {
        if !self.tag(CHANNEL_MAP) {
            return Vec::new();
        }
        let n = self.raw_u8() as usize;
        self.bad |= n > CHANNELS_MAX;
        self.take(n.min(CHANNELS_MAX)).to_vec()
    }

    pub fn cvolume(&mut self) -> Vec<u32> {
        if !self.tag(CVOLUME) {
            return Vec::new();
        }
        let n = self.raw_u8() as usize;
        self.bad |= n > CHANNELS_MAX;
        (0..n.min(CHANNELS_MAX)).map(|_| self.raw_u32()).collect()
    }

    pub fn proplist(&mut self) -> Props {
        let mut props = Vec::new();
        if !self.tag(PROPLIST) {
            return props;
        }
        while let Some(key) = self.string() {
            let n = self.u32() as usize;
            let value = self.arbitrary();
            if self.bad || n > MAX_PROP || value.len() != n {
                self.bad = true;
                break;
            }
            props.push((key, value.to_vec()));
        }
        props
    }

    /// A format: its encoding (1: PCM) and its properties.
    pub fn format_info(&mut self) -> (u8, Props) {
        if !self.tag(FORMAT_INFO) {
            return (0, Vec::new());
        }
        let encoding = self.u8();
        (encoding, self.proplist())
    }
}

/// The text of property `key`, if it has one.
pub fn prop<'p>(props: &'p Props, key: &str) -> Option<&'p str> {
    let (_, v) = props.iter().find(|(k, _)| k == key)?;
    core::str::from_utf8(v.strip_suffix(&[0]).unwrap_or(v)).ok()
}

/// Writes a packet.
pub struct Writer(pub Vec<u8>);

impl Writer {
    /// A packet: a command (or a reply) and the tag that pairs requests
    /// with their replies.
    pub fn new(command: u32, tag: u32) -> Writer {
        let mut w = Writer(Vec::new());
        w.u32(command).u32(tag);
        w
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.push(U32);
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.extend_from_slice(&[U8, v]);
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.push(U64);
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn s64(&mut self, v: i64) -> &mut Self {
        self.0.push(S64);
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn usec(&mut self, v: u64) -> &mut Self {
        self.0.push(USEC);
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.0.push(if v { TRUE } else { FALSE });
        self
    }

    pub fn string(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            Some(s) => {
                self.0.push(STRING);
                self.0.extend_from_slice(s.as_bytes());
                self.0.push(0);
            }
            None => self.0.push(STRING_NULL),
        }
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        self.string(Some(s))
    }

    fn arbitrary(&mut self, b: &[u8]) -> &mut Self {
        self.0.push(ARBITRARY);
        self.0.extend_from_slice(&(b.len() as u32).to_be_bytes());
        self.0.extend_from_slice(b);
        self
    }

    pub fn timeval(&mut self, sec: u32, usec: u32) -> &mut Self {
        self.0.push(TIMEVAL);
        self.0.extend_from_slice(&sec.to_be_bytes());
        self.0.extend_from_slice(&usec.to_be_bytes());
        self
    }

    pub fn sample_spec(&mut self, s: Spec) -> &mut Self {
        self.0.extend_from_slice(&[SAMPLE_SPEC, s.format, s.channels]);
        self.0.extend_from_slice(&s.rate.to_be_bytes());
        self
    }

    pub fn channel_map(&mut self, map: &[u8]) -> &mut Self {
        self.0.extend_from_slice(&[CHANNEL_MAP, map.len() as u8]);
        self.0.extend_from_slice(map);
        self
    }

    /// The same volume on each of `channels` channels.
    pub fn cvolume(&mut self, channels: u8, v: u32) -> &mut Self {
        self.0.extend_from_slice(&[CVOLUME, channels]);
        for _ in 0..channels {
            self.0.extend_from_slice(&v.to_be_bytes());
        }
        self
    }

    /// A volume for each channel.
    pub fn cvolume_of(&mut self, volumes: &[u32]) -> &mut Self {
        self.0.extend_from_slice(&[CVOLUME, volumes.len() as u8]);
        for v in volumes {
            self.0.extend_from_slice(&v.to_be_bytes());
        }
        self
    }

    pub fn volume(&mut self, v: u32) -> &mut Self {
        self.0.push(VOLUME);
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Properties, given as text.
    pub fn proplist(&mut self, props: &[(&str, &str)]) -> &mut Self {
        self.0.push(PROPLIST);
        for (k, v) in props {
            let mut value = v.as_bytes().to_vec();
            value.push(0);
            self.str(k).u32(value.len() as u32).arbitrary(&value);
        }
        self.string(None)
    }

    /// Properties as the client gave them.
    pub fn props(&mut self, props: &Props) -> &mut Self {
        self.0.push(PROPLIST);
        for (k, v) in props {
            self.str(k).u32(v.len() as u32).arbitrary(v);
        }
        self.string(None)
    }

    /// The PCM format, the only one there is.
    pub fn pcm_format(&mut self) -> &mut Self {
        self.0.push(FORMAT_INFO);
        self.u8(ENCODING_PCM).proplist(&[])
    }
}

pub const ENCODING_PCM: u8 = 1;

// ----------------------------------------------------------------------------
// Frames

/// The descriptor before each frame: length, channel, offset (high and low
/// half), flags.
pub const DESCRIPTOR: usize = 20;
/// The channel of packets; other channels are playback streams.
pub const PACKET: u32 = u32::MAX;
/// The block is in shared memory, not in the frame (we never ask for that).
pub const FLAG_SHM: u32 = 0xFF00_0000;
pub const SEEK_MASK: u32 = 0xFF;
/// Frames larger than this end the connection, as for PulseAudio.
pub const FRAME_MAX: usize = 16 << 20;

/// What came in one frame.
pub enum Frame<'a> {
    Packet(&'a [u8]),
    /// Sound for a stream: its channel, the offset and how to seek with it.
    Block { channel: u32, offset: i64, seek: u32, data: &'a [u8] },
    /// A frame we cannot read (shared memory); skipped.
    Other,
}

/// The frame at the start of `buf` and its length, if all of it is there.
/// Err: the client sends nonsense.
pub fn next_frame(buf: &[u8]) -> Result<Option<(Frame<'_>, usize)>, ()> {
    if buf.len() < DESCRIPTOR {
        return Ok(None);
    }
    let word = |i: usize| u32::from_be_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
    let len = word(0) as usize;
    if len > FRAME_MAX {
        return Err(());
    }
    if buf.len() < DESCRIPTOR + len {
        return Ok(None);
    }
    let (channel, flags) = (word(1), word(4));
    let data = &buf[DESCRIPTOR..DESCRIPTOR + len];
    let frame = if flags & FLAG_SHM != 0 {
        Frame::Other
    } else if channel == PACKET {
        Frame::Packet(data)
    } else {
        let offset = ((word(2) as u64) << 32 | word(3) as u64) as i64;
        Frame::Block { channel, offset, seek: flags & SEEK_MASK, data }
    };
    Ok(Some((frame, DESCRIPTOR + len)))
}

/// Adds a packet, in its frame, to `out`.
pub fn put_packet(out: &mut Vec<u8>, packet: &Writer) {
    let len = packet.0.len() as u32;
    for w in [len, PACKET, 0, 0, 0] {
        out.extend_from_slice(&w.to_be_bytes());
    }
    out.extend_from_slice(&packet.0);
}
