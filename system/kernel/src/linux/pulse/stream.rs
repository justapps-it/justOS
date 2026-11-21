//! A client's playback stream: its buffer as PulseAudio sizes it, the
//! accounting that says how much the client may write next, and the sound
//! it writes going to the mixer.

use super::format::{Layout, Spec};
use crate::dev::audio::{self, Stream};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The largest buffer a stream can have (PulseAudio's).
const MAX_LENGTH: u32 = 4 << 20;
/// A stream that does not say how much to buffer gets 2 s; it is asked for
/// more 20 ms at a time at most.
const DEFAULT_TLENGTH_US: u64 = 2_000_000;
const DEFAULT_PROCESS_US: u64 = 20_000;
/// How late what a stream hands over is heard: what the mixer keeps in the
/// device, about.
pub const SINK_LATENCY_US: u64 = (audio::AHEAD_MS + audio::STEP_MS) * 1000;

/// The buffer's sizes, in bytes: largest, target, before playing, and the
/// least the client is asked for.
#[derive(Clone, Copy, Debug)]
pub struct Attr {
    pub maxlength: u32,
    pub tlength: u32,
    pub prebuf: u32,
    pub minreq: u32,
}

/// Seek modes of a block: from where the last block ended, from the start,
/// from what was played, from the end of what is queued.
const SEEK_RELATIVE: u32 = 0;
const SEEK_ABSOLUTE: u32 = 1;
const SEEK_ON_READ: u32 = 2;

pub struct Playback {
    /// The sink input it is, for everyone (the info lists, volumes).
    pub index: u32,
    /// When it was created (uptime, ms).
    pub created: u64,
    pub spec: Spec,
    pub map: Vec<u8>,
    layout: Layout,
    pub stream: Arc<Stream>,
    pub attr: Attr,
    /// Bytes there is room for and the client was not asked for yet
    /// (PulseAudio's `missing`): it grows as the mixer takes sound.
    missing: i64,
    /// Bytes the client wrote (its write index) and, of what the mixer
    /// took, how much `missing` counts already (frames).
    write_index: i64,
    credited: u64,
    /// The start of a frame the client has not finished writing.
    partial: Vec<u8>,
    /// Drain requests waiting for the stream to play out (their tags).
    pub drains: Vec<u32>,
    /// The stream's counts of running out and starting, as told so far.
    told: (u64, u64),
    frames: Vec<[i16; 2]>,
}

impl Playback {
    pub fn new(index: u32, spec: Spec, map: Vec<u8>, req: Attr, adjust_latency: bool, early_requests: bool, corked: bool) -> Playback {
        let attr = fix_attr(spec, req, adjust_latency, early_requests);
        let frame = spec.frame_size() as u32;
        let stream = audio::open_stream(spec.rate, (attr.prebuf / frame) as usize, corked);
        let layout = Layout::new(spec, &map);
        Playback {
            index,
            created: crate::time::uptime_ms(),
            spec,
            map,
            layout,
            stream,
            attr,
            missing: attr.tlength as i64,
            write_index: 0,
            credited: 0,
            partial: Vec::new(),
            drains: Vec::new(),
            told: (0, 0),
            frames: Vec::new(),
        }
    }

    /// New sizes asked for (SET_PLAYBACK_STREAM_BUFFER_ATTR).
    pub fn set_attr(&mut self, req: Attr, adjust_latency: bool, early_requests: bool) {
        let old = self.attr.tlength as i64;
        self.attr = fix_attr(self.spec, req, adjust_latency, early_requests);
        // A longer target leaves room for more.
        self.missing += self.attr.tlength as i64 - old;
        self.stream.set_prebuf((self.attr.prebuf / self.frame() as u32) as usize);
    }

    fn frame(&self) -> usize {
        self.spec.frame_size()
    }

    /// The read index: bytes the mixer took.
    pub fn read_index(&self) -> i64 {
        (self.stream.taken() * self.frame() as u64) as i64
    }

    pub fn write_index(&self) -> i64 {
        self.write_index
    }

    /// The bytes asked for by the creation's reply (all of the target).
    pub fn first_request(&mut self) -> u32 {
        core::mem::take(&mut self.missing).max(0) as u32
    }

    /// A block of sound from the client, at `offset` by `seek`. False if
    /// there was no room for all of it.
    pub fn write(&mut self, offset: i64, seek: u32, data: &[u8]) -> bool {
        let target = match seek {
            SEEK_RELATIVE => self.write_index + offset,
            SEEK_ABSOLUTE => offset,
            SEEK_ON_READ => self.read_index() + offset,
            // From the end of what is queued: where the client wrote last.
            _ => self.write_index + offset,
        };
        if seek != SEEK_RELATIVE {
            // As PulseAudio: a seek the client cannot count itself changes
            // what it is asked for.
            self.missing -= target - self.write_index;
        }
        if target != self.write_index {
            self.seek(target);
        }
        self.partial.extend_from_slice(data);
        let whole = self.partial.len() / self.frame() * self.frame();
        self.frames.clear();
        self.layout.convert(&self.partial[..whole], &mut self.frames);
        self.partial.drain(..whole);
        let room = self.room();
        let fits = self.frames.len().min(room);
        self.stream.push(&self.frames[..fits]);
        self.write_index += (fits * self.frame()) as i64;
        fits == self.frames.len()
    }

    /// Frames there is room for, up to the largest buffer.
    fn room(&self) -> usize {
        (self.attr.maxlength as usize / self.frame()).saturating_sub(self.stream.queued())
    }

    /// Moves the write index: a gap is silence; going back drops what is
    /// still queued after the new place.
    fn seek(&mut self, target: i64) {
        let frame = self.frame() as i64;
        self.partial.clear();
        if target > self.write_index {
            let n = (((target - self.write_index) / frame) as usize).min(self.room());
            self.stream.push(&alloc::vec![[0i16; 2]; n]);
            self.write_index += n as i64 * frame;
        } else {
            let dropped = self.stream.rewind(((self.write_index - target) / frame) as usize);
            self.write_index -= dropped as i64 * frame;
        }
    }

    /// Drops what is queued (FLUSH): the write index goes back to where
    /// the mixer is, and the client may write it all again.
    pub fn flush(&mut self) {
        let dropped = (self.stream.flush() * self.frame()) as i64;
        self.partial.clear();
        self.write_index -= dropped;
        self.missing += dropped;
    }

    /// How much to ask the client for now, if anything: what the mixer
    /// took since the last request, once it is at least `minreq` (or the
    /// stream waits to fill up).
    pub fn request(&mut self) -> Option<u32> {
        let taken = self.stream.taken();
        self.missing += ((taken - self.credited) * self.frame() as u64) as i64;
        self.credited = taken;
        if self.missing <= 0 {
            return None;
        }
        let queued = self.stream.queued() * self.frame();
        let filling = self.attr.prebuf > 0 && (queued == 0 || (self.stream.filling() && queued < self.attr.prebuf as usize));
        if self.missing < self.attr.minreq as i64 && !filling {
            return None;
        }
        Some(core::mem::take(&mut self.missing).min(u32::MAX as i64) as u32)
    }

    /// Whether the stream ran out and whether it started, since last told.
    pub fn events(&mut self) -> (bool, bool) {
        let now = self.stream.events();
        let changed = (now.0 != self.told.0, now.1 != self.told.1);
        self.told = now;
        changed
    }
}

/// The sizes PulseAudio gives a stream asking for `req` (-1: "choose"), as
/// its server works them out (`fix_playback_buffer_attr`), with the
/// sink's latency fixed at what the mixer keeps queued.
pub fn fix_attr(spec: Spec, req: Attr, adjust_latency: bool, early_requests: bool) -> Attr {
    let frame = spec.frame_size() as u32;
    let round_up = |us: u64| {
        let frames = (us * spec.rate as u64).div_ceil(1_000_000);
        (frames * frame as u64).min(u32::MAX as u64) as u32
    };
    let usec = |b: u32| spec.bytes_to_usec(b as u64);
    let mut a = req;
    if a.maxlength == u32::MAX || a.maxlength > MAX_LENGTH {
        a.maxlength = MAX_LENGTH;
    }
    a.maxlength = a.maxlength.max(frame);
    if a.tlength == u32::MAX {
        a.tlength = round_up(DEFAULT_TLENGTH_US);
    }
    a.tlength = a.tlength.max(frame).min(a.maxlength);
    if a.minreq == u32::MAX {
        let m = a.tlength / 4;
        a.minreq = round_up(DEFAULT_PROCESS_US).min(m - m % frame);
    }
    a.minreq = a.minreq.max(frame);
    if a.tlength < a.minreq + frame {
        a.tlength = a.minreq + frame;
    }
    let (orig_tlength_us, minreq_us) = (usec(a.tlength), usec(a.minreq));
    let mut tlength_us = orig_tlength_us;
    if !early_requests && adjust_latency {
        // Half of the latency asked for is the sink's: what the sink
        // really has comes off the stream's buffer.
        tlength_us = tlength_us.saturating_sub(SINK_LATENCY_US);
    }
    tlength_us = tlength_us.max(SINK_LATENCY_US + 2 * minreq_us);
    if round_up(orig_tlength_us) != round_up(tlength_us) {
        a.tlength = round_up(tlength_us);
    }
    if a.minreq == 0 {
        a.minreq = frame;
        a.tlength += 2 * frame;
    }
    if a.tlength <= a.minreq {
        a.tlength = 2 * a.minreq + frame;
    }
    let max_prebuf = a.tlength + frame - a.minreq;
    if a.prebuf == u32::MAX || a.prebuf > max_prebuf {
        a.prebuf = max_prebuf;
    }
    a
}
