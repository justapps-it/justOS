//! What the server says about itself: the server, its one sink and its
//! two sources (the microphone and the sink's monitor), and the clients and
//! streams it has (all clients see all of them, as with PulseAudio).

use super::format::{Spec, S16LE, VOLUME_NORM};
use super::stream::SINK_LATENCY_US;
use super::tag::{prop, Props, Writer, INVALID};
use crate::dev::audio::{self, Stream};
use crate::dev::capture::{self, Recording, Source};
use crate::sync::Mutex;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

/// The sink everything plays on: the mixer.
pub const SINK: &str = "justos";
pub const SINK_INDEX: u32 = 0;
/// What clients may call the sink by.
const DEFAULT_SINK: &str = "@DEFAULT_SINK@";
/// The sources: the microphone (the default one) and what the sink plays.
pub const MIC: &str = "justos-mic";
pub const MIC_INDEX: u32 = 0;
pub const MONITOR: &str = "justos.monitor";
pub const MONITOR_INDEX: u32 = 1;
const DEFAULT_SOURCE: &str = "@DEFAULT_SOURCE@";
const DEFAULT_MONITOR: &str = "@DEFAULT_MONITOR@";
/// The microphone's volume, as PulseAudio's (the kernel applies it).
static MIC_VOLUME: AtomicU32 = AtomicU32::new(VOLUME_NORM);
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

/// A record stream (a source output), as everyone sees it.
pub struct Output {
    pub index: u32,
    pub client: u32,
    pub source: Source,
    pub spec: Spec,
    pub map: Vec<u8>,
    pub volumes: Vec<u32>,
    pub muted: bool,
    pub corked: bool,
    pub props: Props,
    pub rec: Arc<Recording>,
}

impl Output {
    /// Its volume on the kernel's recording.
    pub fn apply_volume(&self) {
        self.rec.set_gain(super::format::stream_gain(&self.volumes, self.muted));
    }
}

pub static CLIENTS: Mutex<Vec<ClientEntry>> = Mutex::new(Vec::new());
pub static INPUTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());
pub static OUTPUTS: Mutex<Vec<Output>> = Mutex::new(Vec::new());

/// The sink by number or by name (the default's names too).
pub fn is_sink(index: u32, name: Option<&str>) -> bool {
    match name {
        Some(n) => n == SINK || n == DEFAULT_SINK,
        None => index == SINK_INDEX,
    }
}

/// The source by number or by name (the default's names too).
pub fn source(index: u32, name: Option<&str>) -> Option<Source> {
    match name {
        Some(MIC | DEFAULT_SOURCE) => Some(Source::Mic),
        Some(MONITOR | DEFAULT_MONITOR) => Some(Source::Monitor),
        Some(_) => None,
        None if index == MIC_INDEX => Some(Source::Mic),
        None if index == MONITOR_INDEX => Some(Source::Monitor),
        None => None,
    }
}

pub fn source_index(s: Source) -> u32 {
    match s {
        Source::Mic => MIC_INDEX,
        Source::Monitor => MONITOR_INDEX,
    }
}

pub fn source_name(s: Source) -> &'static str {
    match s {
        Source::Mic => MIC,
        Source::Monitor => MONITOR,
    }
}

/// The microphone's volume (PulseAudio's, one for all channels), applied
/// by the kernel to everything recorded from it.
pub fn set_mic_volume(v: u32) {
    MIC_VOLUME.store(v, Ordering::Relaxed);
    capture::set_gain(super::format::gain(v));
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
    w.sample_spec(sink_spec()).str(SINK).str(MIC).u32(0x6A75_7374);
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
    w.u32(MONITOR_INDEX).str(MONITOR).usec(audio::latency_us()).str("justos").u32(flags);
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

/// What the microphone records at: 16-bit stereo at the input's rate.
fn mic_spec() -> Spec {
    Spec { format: S16LE, channels: 2, rate: capture::rate() }
}

/// Is anyone recording from `s` (not corked)?
fn source_busy(s: Source) -> bool {
    OUTPUTS.lock().iter().any(|o| o.source == s && !o.corked)
}

pub fn source_info(w: &mut Writer, version: u32, s: Source) {
    let input = capture::list().into_iter().next();
    let (desc, spec, volume, muted, hardware, monitor_of) = match s {
        Source::Mic => {
            let desc = input.as_ref().map_or_else(|| String::from("justOS (no sound input)"), |i| i.name.clone());
            (desc, mic_spec(), MIC_VOLUME.load(Ordering::Relaxed), capture::muted(), input.is_some(), None)
        }
        Source::Monitor => (alloc::format!("Monitor of {}", description()), sink_spec(), VOLUME_NORM, false, false, Some(SINK)),
    };
    let flags = SINK_HW_VOLUME | SINK_LATENCY | if hardware { SINK_HARDWARE } else { 0 };
    let latency = capture::STEP_MS * 1000;
    w.u32(source_index(s)).str(source_name(s)).str(&desc).sample_spec(spec).channel_map(&STEREO);
    w.u32(INVALID).cvolume(2, volume).bool(muted);
    w.u32(if monitor_of.is_some() { SINK_INDEX } else { INVALID }).string(monitor_of).usec(latency).str("justos").u32(flags);
    if version >= 13 {
        let class = if monitor_of.is_some() { "monitor" } else { "sound" };
        w.proplist(&[("device.description", &desc), ("device.class", class), ("device.api", "justos")]);
        w.usec(latency);
    }
    if version >= 15 {
        let state = if source_busy(s) { RUNNING } else { IDLE };
        w.volume(VOLUME_NORM).u32(state).u32(101).u32(INVALID);
    }
    if version >= 16 {
        // No ports, so no active one.
        w.u32(0).string(None);
    }
    if version >= 22 {
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

pub fn output_info(w: &mut Writer, version: u32, o: &Output) {
    let name = prop(&o.props, "media.name").unwrap_or("record stream");
    let buffered = o.spec.bytes_to_usec((o.rec.queued() * o.spec.frame_size()) as u64);
    w.u32(o.index).str(name).u32(INVALID).u32(o.client).u32(source_index(o.source));
    w.sample_spec(o.spec).channel_map(&o.map);
    w.usec(buffered).usec(capture::STEP_MS * 1000).str("linear").str(DRIVER);
    if version >= 13 {
        w.props(&o.props);
    }
    if version >= 19 {
        w.bool(o.corked);
    }
    if version >= 22 {
        // It has a volume, and clients may change it.
        w.cvolume_of(&o.volumes).bool(o.muted).bool(true).bool(true).pcm_format();
    }
}
