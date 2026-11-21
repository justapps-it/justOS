//! A PulseAudio server, in the kernel: how Linux programs (Firefox, Wine,
//! SDL, GStreamer...) make sound on justOS.
//!
//! It listens at `/home/tmp/pulse/native` ([`SOCKET`]; `PULSE_SERVER` in
//! the environment of Linux programs points there), a local socket of the
//! kernel's own, and serves each client from a thread of its own, like the
//! Wayland compositor. It speaks PulseAudio's native protocol, version 35
//! (PulseAudio 17), with the libpulse the programs load: packets of
//! commands and replies, and blocks of sound in between, all on the socket
//! (shared memory is never agreed on).
//!
//! Each playback stream becomes a stream of the kernel's mixer
//! ([`crate::dev::audio`]), its samples turned into 16-bit stereo, so that
//! programs play together and with the system's sounds, at the system
//! volume. The server has one sink, the mixer, whose volume is the system
//! volume; each stream has its own on top. Clients are asked for sound as
//! the mixer takes it, by PulseAudio's rules (`tlength`, `minreq`,
//! prebuffering), and told how late what they wrote is heard, so that a
//! video keeps in step with its sound.
//!
//! Each record stream becomes a recording of the kernel
//! ([`crate::dev::capture`]) from one of two sources: the microphone
//! (`justos-mic`, the default) or what the sink plays (`justos.monitor`).
//! Its sound goes to the client as it comes, in the client's format. The
//! microphone needs the microphone permission ([`may_record`]).
//!
//! What is not there: cached samples, modules, cards and events
//! (SUBSCRIBE is accepted, nothing is sent); asked for, they are not
//! supported or not found.

mod format;
mod info;
mod record;
mod stream;
mod tag;

use super::fd::Desc;
use super::unix::{self, Conn};
use crate::dev::audio;
use crate::dev::capture::{self, Source};
use crate::task::WaitQueue;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use format::VOLUME_NORM;
use info::{ClientEntry, Input, Output, CLIENTS, INPUTS, OUTPUTS, SINK, SINK_INDEX};
use record::{RecAttr, Record, Timing};
use stream::{Attr, Playback, SINK_LATENCY_US};
use tag::{Frame, Props, Reader, Writer, INVALID};

/// The server's socket, for `PULSE_SERVER`.
pub const SOCKET: &str = "/home/tmp/pulse/native";
/// The protocol version spoken (PulseAudio 17's).
const VERSION: u32 = 35;
/// A client that leaves this much unread is stuck: it is let go.
const MAX_UNREAD: usize = 8 << 20;

// Commands (and replies), by number.
const ERROR: u32 = 0;
const TIMEOUT: u32 = 1;
const REPLY: u32 = 2;
const CREATE_PLAYBACK_STREAM: u32 = 3;
const DELETE_PLAYBACK_STREAM: u32 = 4;
const CREATE_RECORD_STREAM: u32 = 5;
const DELETE_RECORD_STREAM: u32 = 6;
const EXIT: u32 = 7;
const AUTH: u32 = 8;
const SET_CLIENT_NAME: u32 = 9;
const LOOKUP_SINK: u32 = 10;
const LOOKUP_SOURCE: u32 = 11;
const DRAIN_PLAYBACK_STREAM: u32 = 12;
const STAT: u32 = 13;
const GET_PLAYBACK_LATENCY: u32 = 14;
const CREATE_UPLOAD_STREAM: u32 = 15;
const DELETE_UPLOAD_STREAM: u32 = 16;
const FINISH_UPLOAD_STREAM: u32 = 17;
const PLAY_SAMPLE: u32 = 18;
const REMOVE_SAMPLE: u32 = 19;
const GET_SERVER_INFO: u32 = 20;
const GET_SINK_INFO: u32 = 21;
const GET_SINK_INFO_LIST: u32 = 22;
const GET_SOURCE_INFO: u32 = 23;
const GET_SOURCE_INFO_LIST: u32 = 24;
const GET_MODULE_INFO: u32 = 25;
const GET_MODULE_INFO_LIST: u32 = 26;
const GET_CLIENT_INFO: u32 = 27;
const GET_CLIENT_INFO_LIST: u32 = 28;
const GET_SINK_INPUT_INFO: u32 = 29;
const GET_SINK_INPUT_INFO_LIST: u32 = 30;
const GET_SOURCE_OUTPUT_INFO: u32 = 31;
const GET_SOURCE_OUTPUT_INFO_LIST: u32 = 32;
const GET_SAMPLE_INFO: u32 = 33;
const GET_SAMPLE_INFO_LIST: u32 = 34;
const SUBSCRIBE: u32 = 35;
const SET_SINK_VOLUME: u32 = 36;
const SET_SINK_INPUT_VOLUME: u32 = 37;
const SET_SOURCE_VOLUME: u32 = 38;
const SET_SINK_MUTE: u32 = 39;
const SET_SOURCE_MUTE: u32 = 40;
const CORK_PLAYBACK_STREAM: u32 = 41;
const FLUSH_PLAYBACK_STREAM: u32 = 42;
const TRIGGER_PLAYBACK_STREAM: u32 = 43;
const SET_DEFAULT_SINK: u32 = 44;
const SET_DEFAULT_SOURCE: u32 = 45;
const SET_PLAYBACK_STREAM_NAME: u32 = 46;
const SET_RECORD_STREAM_NAME: u32 = 47;
const KILL_CLIENT: u32 = 48;
const KILL_SINK_INPUT: u32 = 49;
const KILL_SOURCE_OUTPUT: u32 = 50;
const LOAD_MODULE: u32 = 51;
const UNLOAD_MODULE: u32 = 52;
// 53 to 56: autoload, gone.
const GET_RECORD_LATENCY: u32 = 57;
const CORK_RECORD_STREAM: u32 = 58;
const FLUSH_RECORD_STREAM: u32 = 59;
const PREBUF_PLAYBACK_STREAM: u32 = 60;
const REQUEST: u32 = 61;
const OVERFLOW: u32 = 62;
const UNDERFLOW: u32 = 63;
const MOVE_SINK_INPUT: u32 = 67;
const MOVE_SOURCE_OUTPUT: u32 = 68;
const SET_SINK_INPUT_MUTE: u32 = 69;
const SUSPEND_SINK: u32 = 70;
const SUSPEND_SOURCE: u32 = 71;
const SET_PLAYBACK_STREAM_BUFFER_ATTR: u32 = 72;
const SET_RECORD_STREAM_BUFFER_ATTR: u32 = 73;
const UPDATE_PLAYBACK_STREAM_SAMPLE_RATE: u32 = 74;
const UPDATE_RECORD_STREAM_SAMPLE_RATE: u32 = 75;
const UPDATE_RECORD_STREAM_PROPLIST: u32 = 80;
const UPDATE_PLAYBACK_STREAM_PROPLIST: u32 = 81;
const UPDATE_CLIENT_PROPLIST: u32 = 82;
const REMOVE_RECORD_STREAM_PROPLIST: u32 = 83;
const REMOVE_PLAYBACK_STREAM_PROPLIST: u32 = 84;
const REMOVE_CLIENT_PROPLIST: u32 = 85;
const STARTED: u32 = 86;
const EXTENSION: u32 = 87;
const GET_CARD_INFO: u32 = 88;
const GET_CARD_INFO_LIST: u32 = 89;
const SET_CARD_PROFILE: u32 = 90;
const SET_SINK_PORT: u32 = 96;
const SET_SOURCE_PORT: u32 = 97;
const SET_SOURCE_OUTPUT_VOLUME: u32 = 98;
const SET_SOURCE_OUTPUT_MUTE: u32 = 99;
const SET_PORT_LATENCY_OFFSET: u32 = 100;
const ENABLE_SRBCHANNEL: u32 = 101;
const DISABLE_SRBCHANNEL: u32 = 102;
const REGISTER_MEMFD_SHMID: u32 = 103;
const SEND_OBJECT_MESSAGE: u32 = 104;

// Errors.
const ERR_ACCESS: u32 = 1;
const ERR_COMMAND: u32 = 2;
const ERR_INVALID: u32 = 3;
const ERR_NOENTITY: u32 = 5;
const ERR_PROTOCOL: u32 = 7;
const ERR_VERSION: u32 = 17;
const ERR_NOTSUPPORTED: u32 = 19;
const ERR_NOEXTENSION: u32 = 21;
const ERR_OBSOLETE: u32 = 22;

/// How a client's properties are updated: the new set in place of the
/// old, the new ones added (keeping those there), or added over them.
const UPDATE_SET: u32 = 0;
const UPDATE_MERGE: u32 = 1;
const UPDATE_REPLACE: u32 = 2;

static NEXT_CLIENT: AtomicU32 = AtomicU32::new(0);
static NEXT_INPUT: AtomicU32 = AtomicU32::new(0);
static NEXT_OUTPUT: AtomicU32 = AtomicU32::new(0);
/// The system volume before a client muted the sink.
static UNMUTED: AtomicU32 = AtomicU32::new(0);

/// Starts the server (at boot: programs without windows play sound too).
pub fn start() {
    let tmp = crate::fs::Path::root("sys").join("home").join("tmp");
    for dir in [tmp.clone(), tmp.join("pulse")] {
        if !crate::fs::exists(&dir) {
            let _ = crate::fs::make_dir(&dir);
        }
    }
    crate::task::spawn("pulse", serve);
}

fn serve() {
    let l = match unix::kernel_listen(SOCKET) {
        Ok(l) => l,
        Err(e) => {
            crate::warn!("pulse", "cannot listen at {SOCKET}: {e}");
            return;
        }
    };
    loop {
        if let Some(conn) = l.accept(60_000) {
            if crate::task::spawn("pulse-client", move || Client::new(conn).run()).is_none() {
                crate::warn!("pulse", "no thread for a client");
            }
        }
    }
}

/// The parse went wrong: a protocol error for the client.
fn parsed(r: &Reader<'_>) -> Result<(), u32> {
    if r.bad { Err(ERR_PROTOCOL) } else { Ok(()) }
}

/// The realtime clock, as Linux programs read it (seconds, µs).
fn now_timeval() -> (u32, u32) {
    let ns = crate::time::uptime_ns();
    ((crate::time::boot_unix() + ns / 1_000_000_000) as u32, (ns % 1_000_000_000 / 1000) as u32)
}

/// Properties updated as `mode` says.
fn update_props(props: &mut Props, mode: u32, new: Props) {
    if mode == UPDATE_SET {
        *props = new;
        return;
    }
    for (k, v) in new {
        match props.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) if mode != UPDATE_MERGE => slot.1 = v,
            Some(_) => {}
            None => props.push((k, v)),
        }
    }
}

/// May the client's program record from the microphone? It needs the
/// microphone permission of justOS's sandbox, asked in the usual popup the
/// first time ([`crate::sandbox::device_allowed`]): an app under its own
/// name, a Linux program as `linux.<file>` and a Windows one as
/// `windows.<program>`, with the name and icon of its .desktop file, so that
/// Impostazioni lists it with the apps. Linux programs read the user's files
/// freely, but the microphone is only theirs through this server.
fn may_record(pid: u32) -> bool {
    let allowed = crate::sandbox::device_allowed(pid, "microphone");
    if !allowed {
        let who = crate::sandbox::who(pid).map_or_else(|| alloc::string::String::from("?"), |a| a.id);
        crate::debug!("pulse", "client {pid}: {who} may not use the microphone");
    }
    allowed
}

/// The keys of a REMOVE_*_PROPLIST command (up to a null string).
fn read_keys(r: &mut Reader<'_>) -> Vec<alloc::string::String> {
    let mut keys = Vec::new();
    while let Some(k) = r.string() {
        keys.push(k);
    }
    keys
}

// ----------------------------------------------------------------------------
// A client

struct Client {
    conn: Conn,
    pid: u32,
    index: u32,
    /// The protocol version agreed on (AUTH), and whether it was.
    version: u32,
    authorized: bool,
    inbuf: Vec<u8>,
    infds: VecDeque<Arc<Desc>>,
    out: Vec<u8>,
    /// Its playback and record streams, by channel.
    streams: BTreeMap<u32, Playback>,
    records: BTreeMap<u32, Record>,
    next_channel: u32,
    /// A stream came or went: the thread looks again before sleeping.
    changed: bool,
    dead: bool,
}

impl Client {
    fn new(conn: Conn) -> Client {
        let pid = conn.peer_pid();
        let index = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        CLIENTS.lock().push(ClientEntry { index, props: Vec::new() });
        Client {
            conn,
            pid,
            index,
            version: VERSION,
            authorized: false,
            inbuf: Vec::new(),
            infds: VecDeque::new(),
            out: Vec::new(),
            streams: BTreeMap::new(),
            records: BTreeMap::new(),
            next_channel: 0,
            changed: false,
            dead: false,
        }
    }

    fn run(mut self) {
        crate::debug!("pulse", "client {} connected", self.pid);
        loop {
            // How far the streams' queues were told, before looking at the
            // streams: what they do after this wakes the thread.
            let seen: Vec<u64> = self.queues().iter().map(|q| q.notified()).collect();
            self.changed = false;
            let open = self.conn.take(&mut self.inbuf, &mut self.infds);
            // Descriptors (shared memory offered) are of no use here.
            self.infds.clear();
            self.frames();
            self.streams_moved();
            self.records_moved();
            self.flush();
            if !open || self.dead {
                break;
            }
            if self.changed {
                continue;
            }
            // Asleep until the client writes or a stream moves on.
            let streams = self.queues();
            let conn = &self.conn;
            let mut queues: Vec<&WaitQueue> = Vec::with_capacity(streams.len() + 1);
            queues.push(conn.queue());
            queues.extend(&streams);
            let until = crate::time::uptime_ms() + 1000;
            crate::task::wait_any(&queues, Some(until), || {
                (conn.ready() || streams.iter().zip(&seen).any(|(q, &n)| q.notified() != n)).then_some(())
            });
        }
        // Its streams go with it.
        for (channel, p) in core::mem::take(&mut self.streams) {
            self.close_stream(channel, &p);
        }
        for (channel, r) in core::mem::take(&mut self.records) {
            self.close_record(channel, &r);
        }
        CLIENTS.lock().retain(|c| c.index != self.index);
        crate::debug!("pulse", "client {} gone", self.pid);
    }

    /// A stream ends: it leaves the mixer and the lists.
    fn close_stream(&self, channel: u32, p: &Playback) {
        p.stream.close();
        INPUTS.lock().retain(|i| i.index != p.index);
        let ms = crate::time::uptime_ms() - p.created;
        crate::debug!("pulse", "client {}: stream {channel} closed after {ms} ms", self.pid);
    }

    /// The queues of its streams, told when they move on: playback streams
    /// when the mixer takes sound, record streams when sound arrives.
    fn queues(&self) -> Vec<&WaitQueue> {
        self.streams.values().map(|p| &p.stream.wq).chain(self.records.values().map(|r| &r.rec.wq)).collect()
    }

    /// A record stream ends: it leaves the kernel's recordings and the
    /// lists.
    fn close_record(&self, channel: u32, r: &Record) {
        r.rec.close();
        OUTPUTS.lock().retain(|o| o.index != r.index);
        let ms = crate::time::uptime_ms() - r.created;
        let frames = r.sent / r.spec.frame_size() as u64;
        // Captured and dropped by the kernel count frames at the source's
        // rate; sent ones are at the stream's.
        let (captured, dropped) = (r.rec.captured(), r.rec.dropped());
        crate::debug!(
            "pulse",
            "client {}: record stream {channel} closed after {ms} ms, {frames} frames sent ({captured} captured, {dropped} dropped)",
            self.pid
        );
    }

    fn send(&mut self, w: &Writer) {
        tag::put_packet(&mut self.out, w);
    }

    fn ack(&mut self, tag: u32) {
        self.send(&Writer::new(REPLY, tag));
    }

    fn flush(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let out = core::mem::take(&mut self.out);
        if !self.conn.send(out, Vec::new()) || self.conn.unread() > MAX_UNREAD {
            self.dead = true;
        }
    }

    /// Handles the whole frames that arrived; keeps the start of the next.
    fn frames(&mut self) {
        let mut buf = core::mem::take(&mut self.inbuf);
        let mut at = 0;
        while !self.dead {
            match tag::next_frame(&buf[at..]) {
                Ok(Some((frame, len))) => {
                    match frame {
                        Frame::Packet(p) => self.packet(p),
                        Frame::Block { channel, offset, seek, data } => self.block(channel, offset, seek, data),
                        Frame::Other => {}
                    }
                    at += len;
                }
                Ok(None) => break,
                Err(()) => {
                    crate::warn!("pulse", "client {}: a frame too large", self.pid);
                    self.dead = true;
                }
            }
        }
        buf.drain(..at);
        self.inbuf = buf;
    }

    /// Sound for a stream.
    fn block(&mut self, channel: u32, offset: i64, seek: u32, data: &[u8]) {
        // A stream deleted meanwhile: its sound goes nowhere.
        let Some(p) = self.streams.get_mut(&channel) else { return };
        if !p.write(offset, seek, data) {
            let mut w = Writer::new(OVERFLOW, INVALID);
            w.u32(channel);
            self.send(&w);
        }
    }

    /// Tells the client what its streams did: drained, ran out, started,
    /// and how much more sound they want.
    fn streams_moved(&mut self) {
        let mut out = Vec::new();
        for (&channel, p) in self.streams.iter_mut() {
            let (ran_out, started) = p.events();
            // While a drain waits, running out is the drain's end.
            if ran_out && p.drains.is_empty() {
                let mut w = Writer::new(UNDERFLOW, INVALID);
                w.u32(channel);
                if self.version >= 23 {
                    w.s64(p.read_index());
                }
                out.push(w);
            }
            if !p.drains.is_empty() && p.stream.queued() == 0 {
                out.extend(p.drains.drain(..).map(|tag| Writer::new(REPLY, tag)));
            }
            if started {
                let mut w = Writer::new(STARTED, INVALID);
                w.u32(channel);
                out.push(w);
            }
            if let Some(n) = p.request() {
                let mut w = Writer::new(REQUEST, INVALID);
                w.u32(channel).u32(n);
                out.push(w);
            }
        }
        for w in &out {
            self.send(w);
        }
    }

    /// Sends the record streams' sound that came, as much as the client
    /// has room for in each stream's buffer (what it is slow to read stays
    /// in the socket and counts).
    fn records_moved(&mut self) {
        let unread = self.conn.unread() + self.out.len();
        let mut out = core::mem::take(&mut self.out);
        for (&channel, r) in self.records.iter_mut() {
            let room = (r.attr.maxlength as usize).saturating_sub(unread);
            for block in r.blocks(room) {
                tag::put_block(&mut out, channel, &block);
            }
        }
        self.out = out;
    }

    fn packet(&mut self, data: &[u8]) {
        let mut r = Reader::new(data);
        let (cmd, tag) = (r.u32(), r.u32());
        if r.bad {
            self.dead = true;
            return;
        }
        let result = if !self.authorized && cmd != AUTH { Err(ERR_ACCESS) } else { self.command(cmd, tag, &mut r) };
        if let Err(e) = result {
            if e == ERR_PROTOCOL {
                crate::warn!("pulse", "client {}: command {cmd} not understood", self.pid);
            }
            let mut w = Writer::new(ERROR, tag);
            w.u32(e);
            self.send(&w);
        }
    }

    /// Carries out a command; on success the reply is sent, on failure
    /// the caller sends the error.
    fn command(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        match cmd {
            AUTH => self.auth(tag, r),
            SET_CLIENT_NAME => self.set_client_name(tag, r),
            CREATE_PLAYBACK_STREAM => self.create_playback(tag, r),
            DELETE_PLAYBACK_STREAM => {
                let channel = r.u32();
                parsed(r)?;
                let p = self.streams.remove(&channel).ok_or(ERR_NOENTITY)?;
                self.close_stream(channel, &p);
                self.changed = true;
                self.ack(tag);
                Ok(())
            }
            DRAIN_PLAYBACK_STREAM
            | CORK_PLAYBACK_STREAM
            | FLUSH_PLAYBACK_STREAM
            | TRIGGER_PLAYBACK_STREAM
            | PREBUF_PLAYBACK_STREAM
            | GET_PLAYBACK_LATENCY
            | SET_PLAYBACK_STREAM_BUFFER_ATTR
            | UPDATE_PLAYBACK_STREAM_SAMPLE_RATE
            | SET_PLAYBACK_STREAM_NAME
            | UPDATE_PLAYBACK_STREAM_PROPLIST
            | REMOVE_PLAYBACK_STREAM_PROPLIST => self.stream_command(cmd, tag, r),
            CREATE_RECORD_STREAM => self.create_record(tag, r),
            DELETE_RECORD_STREAM => {
                let channel = r.u32();
                parsed(r)?;
                let rec = self.records.remove(&channel).ok_or(ERR_NOENTITY)?;
                self.close_record(channel, &rec);
                self.changed = true;
                self.ack(tag);
                Ok(())
            }
            CORK_RECORD_STREAM
            | FLUSH_RECORD_STREAM
            | GET_RECORD_LATENCY
            | SET_RECORD_STREAM_BUFFER_ATTR
            | UPDATE_RECORD_STREAM_SAMPLE_RATE
            | SET_RECORD_STREAM_NAME
            | UPDATE_RECORD_STREAM_PROPLIST
            | REMOVE_RECORD_STREAM_PROPLIST => self.record_command(cmd, tag, r),
            GET_SERVER_INFO..=GET_SAMPLE_INFO_LIST | GET_CARD_INFO | GET_CARD_INFO_LIST => self.info(cmd, tag, r),
            SUBSCRIBE | SET_SINK_VOLUME | SET_SINK_MUTE | SET_SINK_INPUT_VOLUME | SET_SINK_INPUT_MUTE | LOOKUP_SINK | SET_DEFAULT_SINK
            | MOVE_SINK_INPUT | SUSPEND_SINK | STAT | UPDATE_CLIENT_PROPLIST | REMOVE_CLIENT_PROPLIST => self.control(cmd, tag, r),
            LOOKUP_SOURCE | SET_SOURCE_VOLUME | SET_SOURCE_MUTE | SET_DEFAULT_SOURCE | SUSPEND_SOURCE | MOVE_SOURCE_OUTPUT
            | SET_SOURCE_OUTPUT_VOLUME | SET_SOURCE_OUTPUT_MUTE => self.source_control(cmd, tag, r),
            // What a server with only a mixer does not do.
            CREATE_UPLOAD_STREAM | LOAD_MODULE | UNLOAD_MODULE | KILL_CLIENT | KILL_SINK_INPUT | KILL_SOURCE_OUTPUT
            | SEND_OBJECT_MESSAGE | EXIT => Err(ERR_NOTSUPPORTED),
            EXTENSION => Err(ERR_NOEXTENSION),
            53..=56 => Err(ERR_OBSOLETE),
            // Samples, cards and ports: there are none.
            DELETE_UPLOAD_STREAM | FINISH_UPLOAD_STREAM | PLAY_SAMPLE | REMOVE_SAMPLE | SET_CARD_PROFILE | SET_SINK_PORT
            | SET_SOURCE_PORT | SET_PORT_LATENCY_OFFSET => Err(ERR_NOENTITY),
            // Replies to the server, and shared memory: nothing to do.
            REPLY | ERROR | TIMEOUT | ENABLE_SRBCHANNEL | DISABLE_SRBCHANNEL | REGISTER_MEMFD_SHMID => Ok(()),
            _ => Err(ERR_COMMAND),
        }
    }

    /// The first command: the version the client speaks (in the high bits,
    /// the shared memory it could use: we say none) and its cookie (any
    /// will do: the socket is the user's own).
    fn auth(&mut self, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let version = r.u32() & 0xFFFF;
        let _cookie = r.arbitrary();
        parsed(r)?;
        if version < 8 {
            return Err(ERR_VERSION);
        }
        self.version = version.min(VERSION);
        self.authorized = true;
        let mut w = Writer::new(REPLY, tag);
        w.u32(VERSION);
        self.send(&w);
        Ok(())
    }

    fn set_client_name(&mut self, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let props = if self.version >= 13 {
            r.proplist()
        } else {
            let mut name = r.string().unwrap_or_default().into_bytes();
            name.push(0);
            alloc::vec![(alloc::string::String::from("application.name"), name)]
        };
        parsed(r)?;
        if let Some(name) = tag::prop(&props, "application.name") {
            crate::debug!("pulse", "client {}: {name}", self.pid);
        }
        if let Some(c) = CLIENTS.lock().iter_mut().find(|c| c.index == self.index) {
            update_props(&mut c.props, UPDATE_REPLACE, props);
        }
        let mut w = Writer::new(REPLY, tag);
        if self.version >= 13 {
            w.u32(self.index);
        }
        self.send(&w);
        Ok(())
    }

    fn create_playback(&mut self, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        if self.version < 13 {
            let _name = r.string();
        }
        let spec = r.sample_spec();
        let map = r.channel_map();
        let (sink_index, sink_name) = (r.u32(), r.string());
        let maxlength = r.u32();
        let corked = r.bool();
        let (tlength, prebuf, minreq, _sync_id) = (r.u32(), r.u32(), r.u32(), r.u32());
        let volumes = r.cvolume();
        if self.version >= 12 {
            // No remapping or remixing, fixed format/rate/channels, no
            // moving, variable rate: all the same to the mixer.
            for _ in 0..7 {
                r.bool();
            }
        }
        let (mut muted, mut adjust_latency, mut props) = (false, false, Vec::new());
        if self.version >= 13 {
            muted = r.bool();
            adjust_latency = r.bool();
            props = r.proplist();
        }
        let (mut volume_set, mut early_requests, mut muted_set) = (true, false, muted);
        if self.version >= 14 {
            volume_set = r.bool();
            early_requests = r.bool();
        }
        if self.version >= 15 {
            muted_set = r.bool();
            // Not keeping the sink awake, failing if it sleeps: it never does.
            r.bool();
            r.bool();
        }
        if self.version >= 17 {
            // A volume relative to the sink's: they always are here.
            r.bool();
        }
        if self.version >= 18 {
            // Passthrough (compressed audio to a receiver): not here.
            r.bool();
        }
        let mut formats = 0;
        if self.version >= 21 {
            formats = r.u8();
            for _ in 0..formats {
                r.format_info();
            }
        }
        parsed(r)?;
        if !spec.valid() {
            // Only PCM, given as a sample spec.
            return Err(if formats > 0 { ERR_NOTSUPPORTED } else { ERR_INVALID });
        }
        if (sink_index != INVALID || sink_name.is_some()) && !info::is_sink(sink_index, sink_name.as_deref()) {
            return Err(ERR_NOENTITY);
        }
        let channels = spec.channels as usize;
        let map = if map.len() == channels { map } else { format::default_map(spec.channels) };
        let volumes = if volume_set && volumes.len() == channels { volumes } else { alloc::vec![VOLUME_NORM; channels] };
        let req = Attr { maxlength, tlength, prebuf, minreq };
        let index = NEXT_INPUT.fetch_add(1, Ordering::Relaxed);
        let mut p = Playback::new(index, spec, map.clone(), req, adjust_latency, early_requests, corked);
        let input = Input { index, client: self.index, spec, map, volumes, muted: muted && muted_set, corked, props, stream: p.stream.clone() };
        input.apply_volume();
        INPUTS.lock().push(input);
        let channel = self.next_channel;
        self.next_channel += 1;
        let a = p.attr;
        crate::debug!(
            "pulse",
            "client {}: stream {channel}: format {} x{} at {} Hz, buffer {} prebuf {} minreq {}",
            self.pid,
            spec.format,
            spec.channels,
            spec.rate,
            a.tlength,
            a.prebuf,
            a.minreq
        );
        let mut w = Writer::new(REPLY, tag);
        w.u32(channel).u32(index).u32(p.first_request());
        w.u32(a.maxlength).u32(a.tlength).u32(a.prebuf).u32(a.minreq);
        if self.version >= 12 {
            w.sample_spec(spec).channel_map(&p.map).u32(SINK_INDEX).str(SINK).bool(false);
        }
        if self.version >= 13 {
            w.usec(SINK_LATENCY_US);
        }
        if self.version >= 21 {
            w.pcm_format();
        }
        self.send(&w);
        self.streams.insert(channel, p);
        self.changed = true;
        Ok(())
    }

    /// Commands for one of the client's streams (by channel).
    fn stream_command(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let channel = r.u32();
        let version = self.version;
        let p = self.streams.get_mut(&channel);
        let mut w = Writer::new(REPLY, tag);
        match cmd {
            DRAIN_PLAYBACK_STREAM => {
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                // It plays what it has, however little, and the reply comes
                // once all of it is taken.
                p.stream.trigger();
                p.drains.push(tag);
                self.changed = true;
                return Ok(());
            }
            CORK_PLAYBACK_STREAM => {
                let corked = r.bool();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                p.stream.cork(corked);
                with_input(p.index, |i| i.corked = corked);
            }
            FLUSH_PLAYBACK_STREAM => {
                parsed(r)?;
                p.ok_or(ERR_NOENTITY)?.flush();
            }
            TRIGGER_PLAYBACK_STREAM => {
                parsed(r)?;
                p.ok_or(ERR_NOENTITY)?.stream.trigger();
            }
            PREBUF_PLAYBACK_STREAM => {
                parsed(r)?;
                p.ok_or(ERR_NOENTITY)?.stream.prebuf_force();
            }
            GET_PLAYBACK_LATENCY => {
                let (sec, usec) = r.timeval();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                latency_reply(&mut w, version, p, (sec, usec));
            }
            SET_PLAYBACK_STREAM_BUFFER_ATTR => {
                let req = Attr { maxlength: r.u32(), tlength: r.u32(), prebuf: r.u32(), minreq: r.u32() };
                let adjust_latency = version >= 13 && r.bool();
                let early_requests = version >= 14 && r.bool();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                p.set_attr(req, adjust_latency, early_requests);
                let a = p.attr;
                w.u32(a.maxlength).u32(a.tlength).u32(a.prebuf).u32(a.minreq);
                if version >= 13 {
                    w.usec(SINK_LATENCY_US);
                }
            }
            UPDATE_PLAYBACK_STREAM_SAMPLE_RATE => {
                let rate = r.u32();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                if rate == 0 || rate > 384_000 {
                    return Err(ERR_INVALID);
                }
                p.spec.rate = rate;
                p.stream.set_rate(rate);
                with_input(p.index, |i| i.spec.rate = rate);
            }
            SET_PLAYBACK_STREAM_NAME => {
                let name = r.string();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                let mut value = name.unwrap_or_default().into_bytes();
                value.push(0);
                let new = alloc::vec![(alloc::string::String::from("media.name"), value)];
                with_input(p.index, |i| update_props(&mut i.props, UPDATE_REPLACE, new));
            }
            UPDATE_PLAYBACK_STREAM_PROPLIST => {
                let mode = r.u32();
                let new = r.proplist();
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                with_input(p.index, |i| update_props(&mut i.props, mode, new));
            }
            _ => {
                let keys = read_keys(r);
                parsed(r)?;
                let p = p.ok_or(ERR_NOENTITY)?;
                with_input(p.index, |i| i.props.retain(|(k, _)| !keys.contains(k)));
            }
        }
        self.send(&w);
        Ok(())
    }

    fn create_record(&mut self, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        if self.version < 13 {
            let _name = r.string();
        }
        let spec = r.sample_spec();
        let map = r.channel_map();
        let (source_index, source_name) = (r.u32(), r.string());
        let maxlength = r.u32();
        let corked = r.bool();
        let fragsize = r.u32();
        if self.version >= 12 {
            // No remapping or remixing, fixed format/rate/channels, no
            // moving, variable rate: all the same here.
            for _ in 0..7 {
                r.bool();
            }
        }
        let (mut timing, mut props) = (Timing::default(), Vec::new());
        if self.version >= 13 {
            // Peak detection (the client wants levels): it gets the sound.
            r.bool();
            timing.adjust_latency = r.bool();
            props = r.proplist();
            // Recording one sink input alone: not here.
            if r.u32() != INVALID {
                return Err(ERR_NOTSUPPORTED);
            }
        }
        timing.early_requests = self.version >= 14 && r.bool();
        if self.version >= 15 {
            // Not keeping the source awake, failing if it sleeps: it never
            // does.
            r.bool();
            r.bool();
        }
        let (mut volumes, mut muted, mut volume_set, mut formats) = (Vec::new(), false, false, 0);
        if self.version >= 22 {
            formats = r.u8();
            for _ in 0..formats {
                r.format_info();
            }
            volumes = r.cvolume();
            muted = r.bool();
            volume_set = r.bool();
            let muted_set = r.bool();
            muted &= muted_set;
            // A volume relative to the source's, and passthrough: no.
            r.bool();
            r.bool();
        }
        parsed(r)?;
        if !spec.valid() {
            return Err(if formats > 0 { ERR_NOTSUPPORTED } else { ERR_INVALID });
        }
        let source = if source_index == INVALID && source_name.is_none() {
            Source::Mic
        } else {
            info::source(source_index, source_name.as_deref()).ok_or(ERR_NOENTITY)?
        };
        if source == Source::Mic && !may_record(self.pid) {
            return Err(ERR_ACCESS);
        }
        let channels = spec.channels as usize;
        let map = if map.len() == channels { map } else { format::default_map(spec.channels) };
        let volumes = if volume_set && volumes.len() == channels { volumes } else { alloc::vec![VOLUME_NORM; channels] };
        let index = NEXT_OUTPUT.fetch_add(1, Ordering::Relaxed);
        let req = RecAttr { maxlength, fragsize };
        let rec = Record::new(index, source, spec, map.clone(), (req, timing), corked);
        let output = Output { index, client: self.index, source, spec, map, volumes, muted, corked, props, rec: rec.rec.clone() };
        output.apply_volume();
        OUTPUTS.lock().push(output);
        let channel = self.next_channel;
        self.next_channel += 1;
        let a = rec.attr;
        crate::debug!(
            "pulse",
            "client {}: record stream {channel} from {}: format {} x{} at {} Hz, fragment {}",
            self.pid,
            info::source_name(source),
            spec.format,
            spec.channels,
            spec.rate,
            a.fragsize
        );
        let mut w = Writer::new(REPLY, tag);
        w.u32(channel).u32(index);
        if self.version >= 9 {
            w.u32(a.maxlength).u32(a.fragsize);
        }
        if self.version >= 12 {
            w.sample_spec(spec).channel_map(&rec.map).u32(info::source_index(source)).str(info::source_name(source)).bool(false);
        }
        if self.version >= 13 {
            w.usec(rec.latency_us);
        }
        if self.version >= 22 {
            w.pcm_format();
        }
        self.send(&w);
        self.records.insert(channel, rec);
        self.changed = true;
        Ok(())
    }

    /// Commands for one of the client's record streams (by channel).
    fn record_command(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let channel = r.u32();
        let version = self.version;
        let rec = self.records.get_mut(&channel);
        let mut w = Writer::new(REPLY, tag);
        match cmd {
            CORK_RECORD_STREAM => {
                let corked = r.bool();
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                rec.cork(corked);
                with_output(rec.index, |o| o.corked = corked);
            }
            FLUSH_RECORD_STREAM => {
                parsed(r)?;
                rec.ok_or(ERR_NOENTITY)?.flush();
            }
            GET_RECORD_LATENCY => {
                let (sec, usec) = r.timeval();
                parsed(r)?;
                record_latency_reply(&mut w, rec.ok_or(ERR_NOENTITY)?, (sec, usec));
            }
            SET_RECORD_STREAM_BUFFER_ATTR => {
                let req = RecAttr { maxlength: r.u32(), fragsize: r.u32() };
                let adjust_latency = version >= 13 && r.bool();
                let early_requests = version >= 14 && r.bool();
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                rec.set_attr(req, Timing { adjust_latency, early_requests });
                w.u32(rec.attr.maxlength).u32(rec.attr.fragsize);
                if version >= 13 {
                    w.usec(rec.latency_us);
                }
            }
            UPDATE_RECORD_STREAM_SAMPLE_RATE => {
                let rate = r.u32();
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                if rate == 0 || rate > 384_000 {
                    return Err(ERR_INVALID);
                }
                rec.spec.rate = rate;
                rec.rec.set_rate(rate);
                with_output(rec.index, |o| o.spec.rate = rate);
            }
            SET_RECORD_STREAM_NAME => {
                let name = r.string();
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                let mut value = name.unwrap_or_default().into_bytes();
                value.push(0);
                let new = alloc::vec![(alloc::string::String::from("media.name"), value)];
                with_output(rec.index, |o| update_props(&mut o.props, UPDATE_REPLACE, new));
            }
            UPDATE_RECORD_STREAM_PROPLIST => {
                let mode = r.u32();
                let new = r.proplist();
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                with_output(rec.index, |o| update_props(&mut o.props, mode, new));
            }
            _ => {
                let keys = read_keys(r);
                parsed(r)?;
                let rec = rec.ok_or(ERR_NOENTITY)?;
                with_output(rec.index, |o| o.props.retain(|(k, _)| !keys.contains(k)));
            }
        }
        self.send(&w);
        Ok(())
    }

    /// The sources' volumes and the record streams' sources and volumes.
    fn source_control(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let mut w = Writer::new(REPLY, tag);
        match cmd {
            LOOKUP_SOURCE => {
                let name = r.string();
                parsed(r)?;
                let s = info::source(INVALID, Some(name.as_deref().unwrap_or(""))).ok_or(ERR_NOENTITY)?;
                w.u32(info::source_index(s));
            }
            SET_SOURCE_VOLUME => {
                let (index, name, volumes) = (r.u32(), r.string(), r.cvolume());
                parsed(r)?;
                if volumes.is_empty() {
                    return Err(ERR_INVALID);
                }
                // The monitor's volume is the sink's: it stays.
                if info::source(index, name.as_deref()).ok_or(ERR_NOENTITY)? == Source::Mic {
                    let avg = volumes.iter().map(|&v| v as u64).sum::<u64>() / volumes.len() as u64;
                    info::set_mic_volume(avg.min(u32::MAX as u64) as u32);
                }
            }
            SET_SOURCE_MUTE => {
                let (index, name, mute) = (r.u32(), r.string(), r.bool());
                parsed(r)?;
                if info::source(index, name.as_deref()).ok_or(ERR_NOENTITY)? == Source::Mic {
                    capture::set_muted(mute);
                }
            }
            SET_DEFAULT_SOURCE => {
                let name = r.string();
                parsed(r)?;
                if name.as_deref().is_some_and(|n| info::source(INVALID, Some(n)).is_none()) {
                    return Err(ERR_NOENTITY);
                }
            }
            SUSPEND_SOURCE => {
                // The microphone stops by itself when nobody records.
                let (index, name, _suspend) = (r.u32(), r.string(), r.bool());
                parsed(r)?;
                if !(index == INVALID && name.is_none()) && info::source(index, name.as_deref()).is_none() {
                    return Err(ERR_NOENTITY);
                }
            }
            MOVE_SOURCE_OUTPUT => {
                let (index, source, name) = (r.u32(), r.u32(), r.string());
                parsed(r)?;
                let s = info::source(source, name.as_deref()).ok_or(ERR_NOENTITY)?;
                let mut outputs = OUTPUTS.lock();
                let o = outputs.iter_mut().find(|o| o.index == index).ok_or(ERR_NOENTITY)?;
                if s == Source::Mic && o.source != Source::Mic {
                    // To the microphone only with the permission, asked of
                    // the program that records (not while holding the list).
                    let (client, rec) = (o.client, o.rec.clone());
                    drop(outputs);
                    if client != self.index || !may_record(self.pid) {
                        return Err(ERR_ACCESS);
                    }
                    rec.set_source(s);
                    with_output(index, |o| o.source = s);
                } else {
                    o.rec.set_source(s);
                    o.source = s;
                }
            }
            SET_SOURCE_OUTPUT_VOLUME => {
                let (index, volumes) = (r.u32(), r.cvolume());
                parsed(r)?;
                let mut outputs = OUTPUTS.lock();
                let o = outputs.iter_mut().find(|o| o.index == index).ok_or(ERR_NOENTITY)?;
                o.volumes = match volumes.len() {
                    n if n == o.spec.channels as usize => volumes,
                    1 => alloc::vec![volumes[0]; o.spec.channels as usize],
                    _ => return Err(ERR_INVALID),
                };
                o.apply_volume();
            }
            _ => {
                let (index, mute) = (r.u32(), r.bool());
                parsed(r)?;
                let mut outputs = OUTPUTS.lock();
                let o = outputs.iter_mut().find(|o| o.index == index).ok_or(ERR_NOENTITY)?;
                o.muted = mute;
                o.apply_volume();
            }
        }
        self.send(&w);
        Ok(())
    }

    /// What there is: the server, the sink, clients and streams; lists of
    /// what there is not are empty.
    fn info(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let v = self.version;
        let mut w = Writer::new(REPLY, tag);
        match cmd {
            GET_SERVER_INFO => info::server_info(&mut w, v),
            GET_SINK_INFO => {
                let (index, name) = (r.u32(), r.string());
                parsed(r)?;
                // Neither number nor name: the default sink.
                if !(index == INVALID && name.is_none()) && !info::is_sink(index, name.as_deref()) {
                    return Err(ERR_NOENTITY);
                }
                info::sink_info(&mut w, v);
            }
            GET_SINK_INFO_LIST => info::sink_info(&mut w, v),
            GET_CLIENT_INFO => {
                let index = r.u32();
                parsed(r)?;
                let clients = CLIENTS.lock();
                let c = clients.iter().find(|c| c.index == index).ok_or(ERR_NOENTITY)?;
                info::client_info(&mut w, v, c);
            }
            GET_CLIENT_INFO_LIST => {
                for c in CLIENTS.lock().iter() {
                    info::client_info(&mut w, v, c);
                }
            }
            GET_SINK_INPUT_INFO => {
                let index = r.u32();
                parsed(r)?;
                let inputs = INPUTS.lock();
                let i = inputs.iter().find(|i| i.index == index).ok_or(ERR_NOENTITY)?;
                info::input_info(&mut w, v, i);
            }
            GET_SINK_INPUT_INFO_LIST => {
                for i in INPUTS.lock().iter() {
                    info::input_info(&mut w, v, i);
                }
            }
            GET_SOURCE_INFO => {
                let (index, name) = (r.u32(), r.string());
                parsed(r)?;
                // Neither number nor name: the default source.
                let s = if index == INVALID && name.is_none() { Some(Source::Mic) } else { info::source(index, name.as_deref()) };
                info::source_info(&mut w, v, s.ok_or(ERR_NOENTITY)?);
            }
            GET_SOURCE_INFO_LIST => {
                info::source_info(&mut w, v, Source::Mic);
                info::source_info(&mut w, v, Source::Monitor);
            }
            GET_SOURCE_OUTPUT_INFO => {
                let index = r.u32();
                parsed(r)?;
                let outputs = OUTPUTS.lock();
                let o = outputs.iter().find(|o| o.index == index).ok_or(ERR_NOENTITY)?;
                info::output_info(&mut w, v, o);
            }
            GET_SOURCE_OUTPUT_INFO_LIST => {
                for o in OUTPUTS.lock().iter() {
                    info::output_info(&mut w, v, o);
                }
            }
            GET_MODULE_INFO_LIST | GET_SAMPLE_INFO_LIST | GET_CARD_INFO_LIST => {}
            GET_MODULE_INFO | GET_SAMPLE_INFO | GET_CARD_INFO => return Err(ERR_NOENTITY),
            _ => return Err(ERR_COMMAND),
        }
        self.send(&w);
        Ok(())
    }

    /// The sink's and the streams' volumes, and the rest of the controls.
    fn control(&mut self, cmd: u32, tag: u32, r: &mut Reader<'_>) -> Result<(), u32> {
        let mut w = Writer::new(REPLY, tag);
        match cmd {
            SUBSCRIBE => {
                // Nothing changes behind the clients' backs that they need
                // to hear of: no events are sent.
                let _mask = r.u32();
                parsed(r)?;
            }
            SET_SINK_VOLUME => {
                let (index, name, volumes) = (r.u32(), r.string(), r.cvolume());
                parsed(r)?;
                if !info::is_sink(index, name.as_deref()) || volumes.is_empty() {
                    return Err(ERR_NOENTITY);
                }
                let avg = volumes.iter().map(|&v| v as u64).sum::<u64>() / volumes.len() as u64;
                audio::set_volume(((avg * 100 + VOLUME_NORM as u64 / 2) / VOLUME_NORM as u64) as u32);
            }
            SET_SINK_MUTE => {
                let (index, name, mute) = (r.u32(), r.string(), r.bool());
                parsed(r)?;
                if !info::is_sink(index, name.as_deref()) {
                    return Err(ERR_NOENTITY);
                }
                set_sink_mute(mute);
            }
            SET_SINK_INPUT_VOLUME => {
                let (index, volumes) = (r.u32(), r.cvolume());
                parsed(r)?;
                let mut inputs = INPUTS.lock();
                let i = inputs.iter_mut().find(|i| i.index == index).ok_or(ERR_NOENTITY)?;
                i.volumes = match volumes.len() {
                    n if n == i.spec.channels as usize => volumes,
                    1 => alloc::vec![volumes[0]; i.spec.channels as usize],
                    _ => return Err(ERR_INVALID),
                };
                i.apply_volume();
            }
            SET_SINK_INPUT_MUTE => {
                let (index, mute) = (r.u32(), r.bool());
                parsed(r)?;
                let mut inputs = INPUTS.lock();
                let i = inputs.iter_mut().find(|i| i.index == index).ok_or(ERR_NOENTITY)?;
                i.muted = mute;
                i.apply_volume();
            }
            LOOKUP_SINK => {
                let name = r.string();
                parsed(r)?;
                if !info::is_sink(INVALID, Some(name.as_deref().unwrap_or(""))) {
                    return Err(ERR_NOENTITY);
                }
                w.u32(SINK_INDEX);
            }
            SET_DEFAULT_SINK => {
                let name = r.string();
                parsed(r)?;
                if name.as_deref().is_some_and(|n| !info::is_sink(INVALID, Some(n))) {
                    return Err(ERR_NOENTITY);
                }
            }
            MOVE_SINK_INPUT => {
                let (index, sink, name) = (r.u32(), r.u32(), r.string());
                parsed(r)?;
                if !INPUTS.lock().iter().any(|i| i.index == index) || !info::is_sink(sink, name.as_deref()) {
                    return Err(ERR_NOENTITY);
                }
            }
            SUSPEND_SINK => {
                // The mixer stops the device by itself when nothing plays.
                let (index, name, _suspend) = (r.u32(), r.string(), r.bool());
                parsed(r)?;
                if !(index == INVALID && name.is_none()) && !info::is_sink(index, name.as_deref()) {
                    return Err(ERR_NOENTITY);
                }
            }
            STAT => {
                // Memory blocks and cached samples: none.
                for _ in 0..5 {
                    w.u32(0);
                }
            }
            UPDATE_CLIENT_PROPLIST => {
                let mode = r.u32();
                let new = r.proplist();
                parsed(r)?;
                if let Some(c) = CLIENTS.lock().iter_mut().find(|c| c.index == self.index) {
                    update_props(&mut c.props, mode, new);
                }
            }
            _ => {
                let keys = read_keys(r);
                parsed(r)?;
                if let Some(c) = CLIENTS.lock().iter_mut().find(|c| c.index == self.index) {
                    c.props.retain(|(k, _)| !keys.contains(k));
                }
            }
        }
        self.send(&w);
        Ok(())
    }
}

/// Changes the entry of source output `index`.
fn with_output(index: u32, f: impl FnOnce(&mut Output)) {
    if let Some(o) = OUTPUTS.lock().iter_mut().find(|o| o.index == index) {
        f(o);
    }
}

/// Where a record stream is in time (GET_RECORD_LATENCY): how late what
/// the sink played reaches a monitor, how late what is captured reaches
/// the client (a step of the capture thread, and what still waits in the
/// kernel), whether it records, the client's time and ours, and the write
/// and read indexes (bytes recorded and bytes sent).
fn record_latency_reply(w: &mut Writer, rec: &Record, local: (u32, u32)) {
    let monitor = if rec.rec.source() == Source::Monitor { audio::latency_us() } else { 0 };
    let waiting = rec.spec.bytes_to_usec((rec.rec.queued() * rec.spec.frame_size()) as u64);
    let (sec, usec) = now_timeval();
    w.usec(monitor).usec(capture::STEP_MS * 1000 + waiting).bool(!rec.corked());
    w.timeval(local.0, local.1).timeval(sec, usec);
    w.s64(rec.write_index()).s64(rec.sent as i64);
}

/// Changes the entry of sink input `index`.
fn with_input(index: u32, f: impl FnOnce(&mut Input)) {
    if let Some(i) = INPUTS.lock().iter_mut().find(|i| i.index == index) {
        f(i);
    }
}

/// Muting the sink is the system volume at 0, as the desktop's mute; it
/// comes back to where it was.
fn set_sink_mute(mute: bool) {
    if mute {
        if audio::volume() > 0 {
            UNMUTED.store(audio::volume(), Ordering::Relaxed);
            audio::set_volume(0);
        }
    } else if audio::volume() == 0 {
        let v = UNMUTED.load(Ordering::Relaxed);
        audio::set_volume(if v == 0 { 60 } else { v });
    }
}

/// Where a stream is in time (GET_PLAYBACK_LATENCY): how late the last
/// sound the mixer took from it is heard, whether it plays, the client's
/// time and ours (the same clock: libpulse sees they agree), and the
/// write and read indexes, from which libpulse works out what is heard
/// now.
fn latency_reply(w: &mut Writer, version: u32, p: &Playback, local: (u32, u32)) {
    let t = p.stream.timing();
    let frame = p.spec.frame_size() as u64;
    let (sec, usec) = now_timeval();
    w.usec(t.latency_us).usec(0).bool(t.playing);
    w.timeval(local.0, local.1).timeval(sec, usec);
    w.s64(p.write_index()).s64((t.taken * frame) as i64);
    if version >= 13 {
        w.u64(t.underrun_for * frame).u64(t.playing_for * frame);
    }
}
