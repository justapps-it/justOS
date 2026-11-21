//! A client's record stream: the kernel's recording it reads from, its
//! buffer as PulseAudio sizes it, and the sound going to the client in the
//! client's format.

use super::format::{Spec, Unlayout};
use crate::dev::capture::{self, Recording, Source};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The largest buffer a stream can have (PulseAudio's).
const MAX_LENGTH: u32 = 4 << 20;
/// A stream that does not say how often it wants sound gets it 2 s at a
/// time at most (PulseAudio's default fragment).
const DEFAULT_FRAGSIZE_US: u64 = 2_000_000;
/// The latencies the sources can keep to: from a step of the capture
/// thread to the default fragment.
const MIN_LATENCY_US: u64 = capture::STEP_MS * 1000;
const MAX_LATENCY_US: u64 = DEFAULT_FRAGSIZE_US;

/// How the client wants its latency kept: to the fragment it asked for
/// (`adjust_latency`), or with fragments sent as soon as the source has
/// them (`early_requests`).
#[derive(Clone, Copy, Default)]
pub struct Timing {
    pub adjust_latency: bool,
    pub early_requests: bool,
}

/// The buffer's sizes, in bytes: largest, and the most sent at a time.
#[derive(Clone, Copy, Debug)]
pub struct RecAttr {
    pub maxlength: u32,
    pub fragsize: u32,
}

pub struct Record {
    /// The source output it is, for everyone (the info lists, volumes).
    pub index: u32,
    /// When it was created (uptime, ms).
    pub created: u64,
    pub spec: Spec,
    pub map: Vec<u8>,
    layout: Unlayout,
    pub rec: Arc<Recording>,
    pub attr: RecAttr,
    /// The source's latency the client agreed to (µs; 0: none asked for).
    pub latency_us: u64,
    /// Bytes sent to the client: its read index (the write index is this
    /// and what is still in the recording).
    pub sent: u64,
    /// Bytes dropped because the client did not read them.
    pub lost: u64,
    corked: bool,
}

impl Record {
    pub fn new(index: u32, source: Source, spec: Spec, map: Vec<u8>, req: (RecAttr, Timing), corked: bool) -> Record {
        let (attr, latency_us) = fix_attr(spec, req.0, req.1);
        let rec = capture::open_recording(source, spec.rate, corked);
        let layout = Unlayout::new(spec, &map);
        Record { index, created: crate::time::uptime_ms(), spec, map, layout, rec, attr, latency_us, sent: 0, lost: 0, corked }
    }

    /// New sizes asked for (SET_RECORD_STREAM_BUFFER_ATTR).
    pub fn set_attr(&mut self, req: RecAttr, t: Timing) {
        (self.attr, self.latency_us) = fix_attr(self.spec, req, t);
    }

    pub fn cork(&mut self, corked: bool) {
        self.corked = corked;
        self.rec.cork(corked);
    }

    pub fn corked(&self) -> bool {
        self.corked
    }

    /// The write index: bytes the stream has, sent or not.
    pub fn write_index(&self) -> i64 {
        (self.sent + (self.rec.queued() * self.spec.frame_size()) as u64) as i64
    }

    /// The sound there is for the client, in blocks of at most `fragsize`;
    /// `room` bytes of it at most (what the client is slow to read beyond
    /// that is dropped, as PulseAudio's queue drops what overflows it).
    pub fn blocks(&mut self, room: usize) -> Vec<Vec<u8>> {
        let frame = self.spec.frame_size();
        let frames = self.rec.take(usize::MAX);
        if frames.is_empty() {
            return Vec::new();
        }
        let keep = (room / frame).min(frames.len());
        self.lost += ((frames.len() - keep) * frame) as u64;
        let mut data = Vec::new();
        self.layout.convert(&frames[frames.len() - keep..], &mut data);
        self.sent += data.len() as u64;
        let block = (self.attr.fragsize as usize).clamp(frame, 64 * 1024) / frame * frame;
        data.chunks(block).map(<[u8]>::to_vec).collect()
    }

    /// Drops what is waiting (FLUSH_RECORD_STREAM).
    pub fn flush(&mut self) {
        self.rec.flush();
    }
}

/// The sizes PulseAudio gives a stream asking for `req` (-1: "choose"), as
/// its server works them out (`fix_record_buffer_attr_pre` and `_post`),
/// and the source latency it set for it.
pub fn fix_attr(spec: Spec, req: RecAttr, t: Timing) -> (RecAttr, u64) {
    let Timing { adjust_latency, early_requests } = t;
    let frame = spec.frame_size() as u32;
    let bytes = |us: u64| ((us * spec.rate as u64).div_ceil(1_000_000) * frame as u64).min(u32::MAX as u64) as u32;
    let mut a = req;
    if a.maxlength == u32::MAX || a.maxlength > MAX_LENGTH {
        a.maxlength = MAX_LENGTH;
    }
    a.maxlength = a.maxlength.max(frame);
    if a.fragsize == u32::MAX {
        a.fragsize = bytes(DEFAULT_FRAGSIZE_US);
    }
    a.fragsize = a.fragsize.max(frame);
    let mut latency = 0;
    if adjust_latency || early_requests {
        // The source keeps to the fragment, as far as it can; the client
        // then gets a fragment each time the source has one.
        latency = spec.bytes_to_usec(a.fragsize as u64).clamp(MIN_LATENCY_US, MAX_LATENCY_US);
        if adjust_latency && !early_requests {
            a.fragsize = bytes(latency);
        }
    }
    a.fragsize = (a.fragsize / frame * frame).max(frame).min(a.maxlength);
    (a, latency)
}
