//! What the server says about itself: the server, its one sink, and the
//! clients and streams it has (all clients see all of them, as with
//! PulseAudio).

use super::format::{Spec, S16LE, VOLUME_NORM};
use super::stream::SINK_LATENCY_US;
use super::tag::{prop, Props, Writer, INVALID};
use crate::dev::audio::{self, Stream};
use crate::sync::Mutex;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The sink everything plays on: the mixer.
pub const SINK: &str = "justos";
pub const SINK_INDEX: u32 = 0;
/// What clients may call the sink by.
const DEFAULT_SINK: &str = "@DEFAULT_SINK@";
const DRIVER: &str = "protocol-native.c";

// Sink flags: its volume is the system's, it says its latency, it is a
// device.
const SINK_HW_VOLUME: u32 = 0x1;
const SINK_LATENCY: u32 = 0x2;
const SINK_HARDWARE: u32 = 0x4;
// Sink states.
const RUNNING: u32 = 0;
const IDLE: u32 = 1;

/// A client, as the others see it.
pub struct ClientEntry {
    pub index: u32,
    pub props: Props,
}

/// A playback stream (a sink input), as everyone sees it: the volume
/// commands of any client reach it through here.
pub struct Input {
    pub index: u32,
    pub client: u32,
    pub spec: Spec,
    pub map: Vec<u8>,
    pub volumes: Vec<u32>,
    pub muted: bool,
    pub corked: bool,
    pub props: Props,
    pub stream: Arc<Stream>,
}

impl Input {
    /// Its volume on the mixer's stream.
    pub fn apply_volume(&self) {
        self.stream.set_gain(super::format::stream_gain(&self.volumes, self.muted));
    }
}

pub static CLIENTS: Mutex<Vec<ClientEntry>> = Mutex::new(Vec::new());
pub static INPUTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());

/// The sink by number or by name (the default's names too).
pub fn is_sink(index: u32, name: Option<&str>) -> bool {
    match name {
        Some(n) => n == SINK || n == DEFAULT_SINK,
        None => index == SINK_INDEX,
    }
}

/// The mix: 16-bit stereo at the output's rate.
pub fn sink_spec() -> Spec {
    Spec { format: S16LE, channels: 2, rate: audio::rate() }
}

const STEREO: [u8; 2] = [1, 2];

/// The system volume as PulseAudio's (0 to 100 % of 0x10000).
fn sink_volume() -> u32 {
    audio::volume() * VOLUME_NORM / 100
}

pub fn server_info(w: &mut Writer, version: u32) {
    let host = crate::linux::ns::names().0;
    w.str("pulseaudio").str("17.0").str("root").str(&host);
    w.sample_spec(sink_spec()).str(SINK).string(None).u32(0x6A75_7374);
    if version >= 15 {
        w.channel_map(&STEREO);
    }
}

/// The name of the output the mix goes to.
fn description() -> String {
    audio::list().into_iter().next().map_or_else(|| String::from("justOS (no sound output)"), |o| o.name)
}

pub fn sink_info(w: &mut Writer, version: u32) {
    let desc = description();
    let flags = SINK_HW_VOLUME | SINK_LATENCY | if audio::available() { SINK_HARDWARE } else { 0 };
    w.u32(SINK_INDEX).str(SINK).str(&desc).sample_spec(sink_spec()).channel_map(&STEREO);
    w.u32(INVALID).cvolume(2, sink_volume()).bool(audio::volume() == 0);
    // No monitor source: nothing records here.
    w.u32(INVALID).string(None).usec(audio::latency_us()).str("justos").u32(flags);
    if version >= 13 {
        w.proplist(&[("device.description", &desc), ("device.class", "sound"), ("device.api", "justos")]);
        w.usec(SINK_LATENCY_US);
    }
    if version >= 15 {
        let state = if audio::latency_us() > 0 { RUNNING } else { IDLE };
        w.volume(VOLUME_NORM).u32(state).u32(101).u32(INVALID);
    }
    if version >= 16 {
        // No ports, so no active one.
        w.u32(0).string(None);
    }
    if version >= 21 {
        w.u8(1).pcm_format();
    }
}

pub fn client_info(w: &mut Writer, version: u32, c: &ClientEntry) {
    w.u32(c.index).str(prop(&c.props, "application.name").unwrap_or("client")).u32(INVALID).str(DRIVER);
    if version >= 13 {
        w.props(&c.props);
    }
}

pub fn input_info(w: &mut Writer, version: u32, i: &Input) {
    let name = prop(&i.props, "media.name").unwrap_or("stream");
    let buffered = i.spec.bytes_to_usec((i.stream.queued() * i.spec.frame_size()) as u64);
    w.u32(i.index).str(name).u32(INVALID).u32(i.client).u32(SINK_INDEX);
    w.sample_spec(i.spec).channel_map(&i.map).cvolume_of(&i.volumes);
    w.usec(buffered).usec(audio::latency_us()).str("linear").str(DRIVER);
    if version >= 11 {
        w.bool(i.muted);
    }
    if version >= 13 {
        w.props(&i.props);
    }
    if version >= 19 {
        w.bool(i.corked);
    }
    if version >= 20 {
        // It has a volume, and clients may change it.
        w.bool(true).bool(true);
    }
    if version >= 21 {
        w.pcm_format();
    }
}
