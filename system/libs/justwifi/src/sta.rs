//! A Wi-Fi station: scanning, joining a network (open, WPA2-PSK, WPA3-SAE),
//! the supplicant's handshakes, and the data path with CCMP, independent of
//! the card. The card is told what to do through [`Out`]: send this frame,
//! tune to that channel; the station is told what happens with
//! [`Station::input`] and the clock with [`Station::poll`].
//!
//! Everything runs in software: the card sends and receives plain 802.11
//! frames (it adds and checks the FCS, acknowledges, retries).

use crate::crypto::{bip_check, ccmp_decrypt, ccmp_encrypt, psk};
use crate::eapol::{self, Akm, GroupKey, Key, Ptk};
use crate::frame::{self, Bss, Header, Mac, Rsn, Security};
use crate::sae::{self, Sae};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use justtls::Drbg;

/// What the card says about a frame it received.
#[derive(Clone, Copy, Debug, Default)]
pub struct RxInfo {
    pub channel: u8,
    /// Signal strength in dBm.
    pub rssi: i8,
}

/// The network a station joined, for the card (some need to know it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub bssid: Mac,
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub aid: u16,
}

/// What the station asks of the card and of the system.
#[derive(Clone, Debug)]
pub enum Out {
    /// Send this 802.11 frame (without FCS).
    Tx(Vec<u8>),
    /// Tune to this channel.
    Channel(u8),
    /// An Ethernet frame from the network for the IP stack.
    Ether(Vec<u8>),
    /// Joined a network (the IP stack may ask for an address) or left it.
    Link(Option<Link>),
    /// A line for the system log.
    Log(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Looking for the network to join.
    Searching,
    Authenticating,
    Associating,
    /// Associated, the keys are being agreed.
    Handshake,
    Connected,
}

/// Why joining failed or the connection ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    NotFound,
    NeedPassword,
    WrongPassword,
    /// A kind of network justOS cannot join (WEP, WPA, 802.1X...).
    Unsupported,
    /// The access point refused with this status code.
    Rejected(u16),
    /// The access point stopped answering.
    Timeout,
    /// The access point ended the connection (reason code).
    Deauthenticated(u16),
    /// The access point is no longer heard.
    Lost,
}

impl Failure {
    pub fn text(self) -> String {
        match self {
            Failure::NotFound => String::from("rete non trovata"),
            Failure::NeedPassword => String::from("serve la password"),
            Failure::WrongPassword => String::from("password errata"),
            Failure::Unsupported => String::from("tipo di rete non supportato"),
            Failure::Rejected(s) => format!("rifiutato dal router (stato {s})"),
            Failure::Timeout => String::from("il router non risponde"),
            Failure::Deauthenticated(r) => format!("disconnesso dal router (motivo {r})"),
            Failure::Lost => String::from("segnale perso"),
        }
    }
}

struct Scan {
    channels: Vec<u8>,
    index: usize,
    until: u64,
    /// Join the target when done.
    join: bool,
    /// The channel to go back to (connected while scanning).
    home: Option<u8>,
}

struct Target {
    ssid: Vec<u8>,
    password: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Auth,
    SaeCommit,
    SaeConfirm,
    Assoc,
    Keys,
    Up,
}

struct Conn {
    bss: Bss,
    akm: Option<Akm>,
    pmk: Option<[u8; 32]>,
    sae: Option<Sae>,
    sae_status: u16,
    token: Vec<u8>,
    /// Our RSN element (id, length, data), as sent in the association.
    rsn_ie: Vec<u8>,
    rsnx: bool,
    pmf: bool,
    step: Step,
    deadline: u64,
    tries: u32,
    aid: u16,
    anonce: Option<[u8; 32]>,
    snonce: [u8; 32],
    replay: Option<u64>,
    ptk: Option<Ptk>,
    /// A PTK being agreed while one is in use (rekeying).
    new_ptk: Option<Ptk>,
    gtk: Option<GroupKey>,
    igtk: Option<GroupKey>,
    tx_pn: u64,
    rx_pn: [u64; 17],
    gtk_pn: u64,
    igtk_pn: u64,
    got_msg1: bool,
    last_beacon: u64,
    last_seq: Option<(u8, u16)>,
}

/// How long to listen on a channel while scanning (ms).
const DWELL_ACTIVE: u64 = 60;
const DWELL_PASSIVE: u64 = 120;
/// No beacon for this long: the network is gone.
const BEACON_LOSS_MS: u64 = 8000;
/// Networks heard longer ago than this are forgotten.
const BSS_AGE_MS: u64 = 60_000;

/// 2.4 GHz channels 1-13 and the 5 GHz channels of Europe and the US.
pub const CHANNELS: [u8; 38] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140,
    144, 149, 153, 157, 161, 165,
];

/// Channels where a station must not transmit before it heard the network
/// (radar, 5 GHz DFS): passive scanning there.
pub fn passive(channel: u8) -> bool {
    (52..=144).contains(&channel) || channel >= 12 && channel <= 14
}

pub fn frequency(channel: u8) -> u32 {
    match channel {
        14 => 2484,
        1..=13 => 2407 + 5 * channel as u32,
        _ => 5000 + 5 * channel as u32,
    }
}

pub struct Station {
    pub mac: Mac,
    rng: Drbg,
    seq: u16,
    channels: Vec<u8>,
    channel: u8,
    scan: Option<Scan>,
    bss: BTreeMap<Mac, Bss>,
    target: Option<Target>,
    conn: Option<Conn>,
    state: State,
    failure: Option<Failure>,
    reconnects: u32,
    reconnect_at: Option<u64>,
    out: Vec<Out>,
}

/// What [`Station::status`] reports.
#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    pub ssid: Vec<u8>,
    pub bssid: Option<Mac>,
    pub channel: u8,
    pub rssi: i8,
    pub security: Option<Security>,
    pub failure: Option<Failure>,
    pub scanning: bool,
}

impl Station {
    /// A station with address `mac`, randomness from `seed`, on the
    /// channels the card has (empty: all of [`CHANNELS`]).
    pub fn new(mac: Mac, seed: &[u8; 32], channels: &[u8]) -> Station {
        Station {
            mac,
            rng: Drbg::new(seed),
            seq: 0,
            channels: if channels.is_empty() { CHANNELS.to_vec() } else { channels.to_vec() },
            channel: 0,
            scan: None,
            bss: BTreeMap::new(),
            target: None,
            conn: None,
            state: State::Idle,
            failure: None,
            reconnects: 0,
            reconnect_at: None,
            out: Vec::new(),
        }
    }

    /// Mixes more randomness in.
    pub fn reseed(&mut self, data: &[u8]) {
        self.rng.reseed(data);
    }

    /// What the card has to do (in order).
    pub fn take(&mut self) -> Vec<Out> {
        core::mem::take(&mut self.out)
    }

    fn log(&mut self, s: String) {
        self.out.push(Out::Log(s));
    }

    fn next_seq(&mut self) -> u16 {
        self.seq = (self.seq + 1) & 0x0FFF;
        self.seq
    }

    fn tune(&mut self, ch: u8) {
        if self.channel != ch {
            self.channel = ch;
            self.out.push(Out::Channel(ch));
        }
    }

    pub fn status(&self) -> Status {
        let c = self.conn.as_ref();
        Status {
            state: self.state,
            ssid: c.map(|c| c.bss.ssid.clone()).or_else(|| self.target.as_ref().map(|t| t.ssid.clone())).unwrap_or_default(),
            bssid: c.map(|c| c.bss.bssid),
            channel: c.map(|c| c.bss.channel).unwrap_or(self.channel),
            rssi: c.and_then(|c| self.bss.get(&c.bss.bssid).map(|b| b.rssi)).unwrap_or(0),
            security: c.map(|c| c.bss.security()),
            failure: self.failure,
            scanning: self.scan.is_some(),
        }
    }

    /// The networks heard lately, strongest first, one per name (the
    /// strongest access point of each).
    pub fn networks(&self) -> Vec<Bss> {
        let mut best: BTreeMap<Vec<u8>, Bss> = BTreeMap::new();
        for b in self.bss.values() {
            if b.ssid.is_empty() || b.ssid.iter().all(|&c| c == 0) {
                continue; // hidden
            }
            match best.get(&b.ssid) {
                Some(o) if o.rssi >= b.rssi => {}
                _ => {
                    best.insert(b.ssid.clone(), b.clone());
                }
            }
        }
        let mut v: Vec<Bss> = best.into_values().collect();
        v.sort_by(|a, b| b.rssi.cmp(&a.rssi).then(a.ssid.cmp(&b.ssid)));
        v
    }

    // ------------------------------------------------------------------------
    // Scanning

    /// Listens on every channel for networks (asking on those where it may).
    pub fn scan(&mut self, now: u64) {
        self.start_scan(now, false);
    }

    fn start_scan(&mut self, now: u64, join: bool) {
        if let Some(s) = &mut self.scan {
            s.join |= join;
            return;
        }
        let home = (self.state == State::Connected).then(|| self.channel);
        if home.is_some() {
            self.power_save(true);
        }
        self.bss.retain(|_, b| now.saturating_sub(b.seen) < BSS_AGE_MS);
        self.scan = Some(Scan { channels: self.channels.clone(), index: 0, until: 0, join, home });
        self.scan_step(now);
    }

    fn scan_step(&mut self, now: u64) {
        let Some(s) = &self.scan else { return };
        if s.index >= s.channels.len() {
            let s = self.scan.take().unwrap();
            let n = self.networks().len();
            self.log(format!("scan: {n} networks"));
            if let Some(h) = s.home {
                self.tune(h);
                self.power_save(false);
            }
            if s.join {
                self.join_best(now);
            }
            return;
        }
        let ch = s.channels[s.index];
        let probe = !passive(ch);
        let target = self.target.as_ref().map(|t| t.ssid.clone());
        self.tune(ch);
        if probe {
            let seq = self.next_seq();
            let f = frame::probe_request(&self.mac, seq, &[], ch);
            self.out.push(Out::Tx(f));
            // Hidden networks answer only to their name.
            if let Some(ssid) = target {
                let seq = self.next_seq();
                self.out.push(Out::Tx(frame::probe_request(&self.mac, seq, &ssid, ch)));
            }
        }
        let s = self.scan.as_mut().unwrap();
        s.until = now + if probe { DWELL_ACTIVE } else { DWELL_PASSIVE };
    }

    /// Tells the access point we sleep (before leaving its channel) or are
    /// awake again: a null data frame with the power management bit.
    fn power_save(&mut self, on: bool) {
        let Some(c) = &self.conn else { return };
        let bssid = c.bss.bssid;
        let seq = self.next_seq();
        let mut h = frame::data_header(&bssid, &self.mac, &bssid, seq, false);
        h[0] = 0x48; // null data
        if on {
            h[1] |= 0x10;
        }
        self.out.push(Out::Tx(h));
    }

    // ------------------------------------------------------------------------
    // Joining

    /// Joins the network `ssid` with `password` (None for an open network).
    pub fn connect(&mut self, ssid: &[u8], password: Option<&[u8]>, now: u64) {
        self.leave(3, false);
        self.target = Some(Target { ssid: ssid.to_vec(), password: password.map(|p| p.to_vec()) });
        self.failure = None;
        self.reconnects = 0;
        self.reconnect_at = None;
        self.state = State::Searching;
        let fresh = self.bss.values().any(|b| b.ssid == ssid && now.saturating_sub(b.seen) < 10_000);
        if fresh {
            self.join_best(now);
        } else {
            self.start_scan(now, true);
        }
    }

    /// Leaves the network and forgets the target.
    pub fn disconnect(&mut self) {
        self.leave(3, true);
        self.target = None;
        self.reconnect_at = None;
        self.state = State::Idle;
    }

    /// Ends a connection (telling the access point if one was made).
    fn leave(&mut self, reason: u16, tell: bool) {
        let Some(c) = self.conn.take() else { return };
        if tell && c.step as u8 >= Step::Assoc as u8 {
            let seq = self.next_seq();
            let mut f = frame::deauth(frame::DEAUTH, &c.bss.bssid, &self.mac, &c.bss.bssid, seq, reason);
            if let (true, Some(ptk)) = (c.pmf, &c.ptk) {
                f[1] |= frame::FLAG_PROTECTED;
                let body = ccmp_encrypt(&ptk.tk, &f[..24], c.tx_pn + 1, 0, &f[24..]);
                f.truncate(24);
                f.extend(body);
            }
            self.out.push(Out::Tx(f));
        }
        if c.step == Step::Up {
            self.out.push(Out::Link(None));
        }
    }

    fn fail(&mut self, f: Failure) {
        self.fail_with(f, true);
    }

    /// Joining failed or the connection ended with `f`; `tell`: say so to
    /// the access point. A connection that worked is tried again, a few
    /// times, unless the password is the problem.
    fn fail_with(&mut self, f: Failure, tell: bool) {
        self.log(format!("wifi: {}", f.text()));
        let was_up = self.conn.as_ref().is_some_and(|c| c.step == Step::Up);
        if tell {
            self.leave(1, true);
        } else if let Some(c) = self.conn.take() {
            if c.step == Step::Up {
                self.out.push(Out::Link(None));
            }
        }
        self.failure = Some(f);
        let hopeless = matches!(f, Failure::WrongPassword | Failure::NeedPassword | Failure::Unsupported);
        if (was_up || self.reconnects > 0) && !hopeless && self.reconnects < 5 && self.target.is_some() {
            self.state = State::Searching;
            self.reconnect_at = Some(0);
        } else {
            self.state = State::Idle;
        }
    }

    fn join_best(&mut self, now: u64) {
        let Some(t) = &self.target else { return };
        let best = self.bss.values().filter(|b| b.ssid == t.ssid).max_by_key(|b| b.rssi).cloned();
        let Some(bss) = best else { return self.fail(Failure::NotFound) };
        self.join(bss, now);
    }

    fn join(&mut self, bss: Bss, now: u64) {
        let password = self.target.as_ref().and_then(|t| t.password.clone());
        let sec = bss.security();
        let rsn = bss.rsn.clone();
        let (akm, pmk) = match sec {
            Security::Open => (None, None),
            Security::Legacy | Security::Enterprise => return self.fail(Failure::Unsupported),
            _ => {
                let Some(pw) = password.clone() else { return self.fail(Failure::NeedPassword) };
                let akms = &rsn.as_ref().unwrap().akms;
                let pick = if akms.contains(&frame::AKM_SAE) {
                    Akm::Sae
                } else if akms.contains(&frame::AKM_PSK_SHA256) {
                    Akm::PskSha256
                } else {
                    Akm::Psk
                };
                let pmk = match pick {
                    Akm::Sae => None,
                    _ => {
                        if pw.len() == 64 && pw.iter().all(u8::is_ascii_hexdigit) {
                            let mut k = [0u8; 32];
                            for (i, b) in k.iter_mut().enumerate() {
                                *b = u8::from_str_radix(core::str::from_utf8(&pw[2 * i..2 * i + 2]).unwrap(), 16).unwrap();
                            }
                            Some(k)
                        } else if (8..=63).contains(&pw.len()) {
                            Some(psk(&pw, &bss.ssid))
                        } else {
                            return self.fail(Failure::WrongPassword);
                        }
                    }
                };
                (Some(pick), pmk)
            }
        };
        let mut rsn_ie = Vec::new();
        let mut pmf = false;
        if let (Some(akm), Some(ap)) = (akm, &rsn) {
            let ap_mfpc = ap.caps & frame::RSN_CAP_MFPC != 0;
            let ap_mfpr = ap.caps & frame::RSN_CAP_MFPR != 0;
            let bip_ok = ap.group_mgmt.is_none_or(|g| g == frame::CIPHER_BIP_CMAC);
            if ap.group != frame::CIPHER_CCMP {
                return self.fail(Failure::Unsupported);
            }
            if ap_mfpr && !bip_ok {
                return self.fail(Failure::Unsupported);
            }
            pmf = ap_mfpc && bip_ok;
            let mut caps = 0;
            if pmf || akm == Akm::Sae && bip_ok {
                caps |= frame::RSN_CAP_MFPC;
            }
            if pmf && ap_mfpr {
                caps |= frame::RSN_CAP_MFPR;
            }
            let ours = Rsn { group: ap.group, pairwise: vec![frame::CIPHER_CCMP], akms: vec![akm.suite()], caps, group_mgmt: None };
            let d = ours.build();
            rsn_ie.push(frame::IE_RSN);
            rsn_ie.push(d.len() as u8);
            rsn_ie.extend_from_slice(&d);
        }
        let mut snonce = [0u8; 32];
        self.rng.fill(&mut snonce);
        let name = bss.name();
        self.log(format!(
            "joining \"{name}\" ({}) on channel {}, {} dBm, {}{}",
            mac_text(&bss.bssid),
            bss.channel,
            bss.rssi,
            sec.name(),
            if pmf { ", PMF" } else { "" }
        ));
        self.tune(bss.channel);
        let rsnx = akm == Some(Akm::Sae) && bss.h2e;
        self.conn = Some(Conn {
            bss,
            akm,
            pmk,
            sae: None,
            sae_status: 0,
            token: Vec::new(),
            rsn_ie,
            rsnx,
            pmf,
            step: Step::Auth,
            deadline: 0,
            tries: 0,
            aid: 0,
            anonce: None,
            snonce,
            replay: None,
            ptk: None,
            new_ptk: None,
            gtk: None,
            igtk: None,
            tx_pn: 0,
            rx_pn: [0; 17],
            gtk_pn: 0,
            igtk_pn: 0,
            got_msg1: false,
            last_beacon: now,
            last_seq: None,
        });
        self.state = State::Authenticating;
        if akm == Some(Akm::Sae) {
            if !self.sae_start() {
                return self.fail(Failure::WrongPassword);
            }
        }
        self.send_auth(now);
    }

    /// Computes the password element and our SAE commit.
    fn sae_start(&mut self) -> bool {
        let password = self.target.as_ref().and_then(|t| t.password.clone()).unwrap_or_default();
        let mac = self.mac;
        let mut rand = [0u8; 40];
        let mut mask = [0u8; 40];
        self.rng.fill(&mut rand);
        self.rng.fill(&mut mask);
        let c = self.conn.as_mut().unwrap();
        let pwe = if c.rsnx {
            c.sae_status = 126; // hash-to-element
            sae::pwe_from_pt(&sae::pt(&c.bss.ssid, &password, &[]), &mac, &c.bss.bssid)
        } else {
            c.sae_status = 0;
            sae::pwe_hunt_and_peck(&mac, &c.bss.bssid, &password)
        };
        c.sae = pwe.and_then(|p| Sae::new(&p, &rand, &mask));
        c.step = Step::SaeCommit;
        c.sae.is_some()
    }

    fn send_auth(&mut self, now: u64) {
        let seq = self.next_seq();
        let mac = self.mac;
        let c = self.conn.as_mut().unwrap();
        let f = match (&c.sae, c.step) {
            (Some(s), Step::SaeCommit) => {
                let commit = s.commit();
                let mut body = commit[..2].to_vec();
                body.extend_from_slice(&c.token);
                body.extend_from_slice(&commit[2..]);
                frame::auth(&c.bss.bssid, &mac, seq, 3, 1, c.sae_status, &body)
            }
            (Some(s), Step::SaeConfirm) => {
                let mut body = 1u16.to_le_bytes().to_vec();
                body.extend_from_slice(&s.confirm(1));
                frame::auth(&c.bss.bssid, &mac, seq, 3, 2, 0, &body)
            }
            _ => frame::auth(&c.bss.bssid, &mac, seq, 0, 1, 0, &[]),
        };
        c.deadline = now + if c.sae.is_some() { 1000 } else { 300 };
        self.out.push(Out::Tx(f));
    }

    fn send_assoc(&mut self, now: u64) {
        let seq = self.next_seq();
        let mac = self.mac;
        let c = self.conn.as_mut().unwrap();
        c.step = Step::Assoc;
        c.deadline = now + 300;
        let rsn = (!c.rsn_ie.is_empty()).then(|| c.rsn_ie[2..].to_vec());
        let f = frame::assoc_request(&c.bss, &mac, seq, rsn.as_deref(), c.rsnx);
        self.state = State::Associating;
        self.out.push(Out::Tx(f));
    }

    // ------------------------------------------------------------------------
    // Time

    /// When [`Station::poll`] next has something to do.
    pub fn deadline(&self) -> Option<u64> {
        let mut d = self.scan.as_ref().map(|s| s.until);
        if let Some(c) = &self.conn {
            let t = if c.step == Step::Up { c.last_beacon + BEACON_LOSS_MS } else { c.deadline };
            d = Some(d.map_or(t, |x| x.min(t)));
        }
        if let Some(r) = self.reconnect_at {
            d = Some(d.map_or(r, |x| x.min(r)));
        }
        d
    }

    pub fn poll(&mut self, now: u64) {
        if self.scan.as_ref().is_some_and(|s| now >= s.until) {
            self.scan.as_mut().unwrap().index += 1;
            self.scan_step(now);
        }
        if self.scan.is_some() {
            return; // off the network's channel: its timers wait
        }
        if let Some(at) = self.reconnect_at {
            if at == 0 {
                self.reconnect_at = Some(now + 1000 * (1 + self.reconnects as u64));
            } else if now >= at {
                self.reconnect_at = None;
                self.reconnects += 1;
                self.log(format!("reconnecting (attempt {})", self.reconnects));
                self.state = State::Searching;
                self.start_scan(now, true);
                return;
            }
        }
        let Some(c) = &mut self.conn else { return };
        if c.step == Step::Up {
            if now > c.last_beacon + BEACON_LOSS_MS {
                self.fail(Failure::Lost);
            }
            return;
        }
        if now < c.deadline {
            return;
        }
        c.tries += 1;
        let step = c.step;
        let tries = c.tries;
        match step {
            Step::Auth | Step::SaeCommit | Step::SaeConfirm if tries < 4 => self.send_auth(now),
            Step::SaeConfirm => self.fail(Failure::WrongPassword),
            Step::Assoc if tries < 4 => self.send_assoc(now),
            Step::Keys => {
                let f = if c.got_msg1 { Failure::WrongPassword } else { Failure::Timeout };
                self.fail(f)
            }
            _ => self.fail(Failure::Timeout),
        }
    }

    // ------------------------------------------------------------------------
    // Frames from the air

    pub fn input(&mut self, f: &[u8], info: RxInfo, now: u64) {
        let Some(h) = frame::parse(f) else { return };
        let body = &f[h.len..];
        match h.kind {
            frame::BEACON | frame::PROBE_RESP => {
                // A frame of another channel heard through the filter
                // (adjacent channels on 2.4 GHz): trust the element.
                if let Some(b) = frame::parse_bss(&h, body, info.channel, info.rssi, now) {
                    if let Some(c) = &mut self.conn {
                        if c.bss.bssid == b.bssid {
                            c.last_beacon = now;
                        }
                    }
                    self.bss.insert(b.bssid, b);
                }
            }
            _ if h.addr1 != self.mac && !(h.addr1[0] & 1 == 1) => {}
            frame::AUTH => self.on_auth(&h, body, now),
            frame::ASSOC_RESP | frame::REASSOC_RESP => self.on_assoc(&h, body, now),
            frame::DEAUTH | frame::DISASSOC => self.on_deauth(&h, f, now),
            frame::ACTION => self.on_action(&h, f),
            frame::DATA | frame::QOS_DATA => self.on_data(&h, f, now),
            _ => {}
        }
    }

    fn from_ap(&self, h: &Header) -> bool {
        self.conn.as_ref().is_some_and(|c| c.bss.bssid == h.addr2)
    }

    fn on_auth(&mut self, h: &Header, body: &[u8], now: u64) {
        if !self.from_ap(h) || body.len() < 6 {
            return;
        }
        let alg = u16::from_le_bytes([body[0], body[1]]);
        let tseq = u16::from_le_bytes([body[2], body[3]]);
        let status = u16::from_le_bytes([body[4], body[5]]);
        let rest = &body[6..];
        let c = self.conn.as_mut().unwrap();
        match (alg, tseq, c.step) {
            (0, 2, Step::Auth) => {
                if status != 0 {
                    return self.fail(Failure::Rejected(status));
                }
                c.tries = 0;
                self.send_assoc(now);
            }
            (3, 1, Step::SaeCommit | Step::SaeConfirm) => match status {
                0 | 126 => {
                    if c.step == Step::SaeConfirm {
                        return; // a repeated commit
                    }
                    let r = c.sae.as_mut().unwrap().process_commit(rest);
                    match r {
                        Ok(()) => {
                            c.step = Step::SaeConfirm;
                            c.tries = 0;
                            self.send_auth(now);
                        }
                        Err(sae::SaeError::Group) => self.fail(Failure::Unsupported),
                        Err(_) => {}
                    }
                }
                76 if rest.len() > 2 => {
                    // Anti-clogging: the commit again, with the token.
                    c.token = rest[2..].to_vec();
                    self.send_auth(now);
                }
                77 => self.fail(Failure::Unsupported),
                s => self.fail(Failure::Rejected(s)),
            },
            (3, 2, Step::SaeConfirm) => {
                if status != 0 {
                    return self.fail(Failure::Rejected(status));
                }
                let s = c.sae.as_ref().unwrap();
                if s.check_confirm(rest) {
                    c.pmk = Some(s.pmk);
                    c.tries = 0;
                    self.log(String::from("SAE: authenticated"));
                    self.send_assoc(now);
                } else {
                    self.log(String::from("SAE: wrong confirm from the access point"));
                }
            }
            _ => {}
        }
    }

    fn on_assoc(&mut self, h: &Header, body: &[u8], now: u64) {
        if !self.from_ap(h) || body.len() < 6 {
            return;
        }
        let c = self.conn.as_mut().unwrap();
        if c.step != Step::Assoc {
            return;
        }
        let status = u16::from_le_bytes([body[2], body[3]]);
        if status == 30 {
            // Try again later (the access point checks an old association).
            c.deadline = now + 1000;
            return;
        }
        if status != 0 {
            return self.fail(Failure::Rejected(status));
        }
        c.aid = u16::from_le_bytes([body[4], body[5]]) & 0x3FFF;
        c.tries = 0;
        let (aid, open) = (c.aid, c.akm.is_none());
        if !open {
            c.step = Step::Keys;
            c.deadline = now + 5000;
            self.state = State::Handshake;
        }
        self.log(format!("associated, AID {aid}"));
        if open {
            self.up();
        }
    }

    fn up(&mut self) {
        let c = self.conn.as_mut().unwrap();
        c.step = Step::Up;
        let link = Link { bssid: c.bss.bssid, ssid: c.bss.ssid.clone(), channel: c.bss.channel, aid: c.aid };
        self.state = State::Connected;
        self.failure = None;
        self.reconnects = 0;
        self.log(format!("connected to \"{}\"", String::from_utf8_lossy(&link.ssid)));
        self.out.push(Out::Link(Some(link)));
    }

    /// Is a robust management frame acceptable: protected (unicast) or
    /// signed (group) when management frames are protected. Returns its
    /// plain body.
    fn robust_body(&mut self, h: &Header, f: &[u8]) -> Option<Vec<u8>> {
        let c = self.conn.as_mut()?;
        let body = &f[h.len..];
        if !c.pmf || c.ptk.is_none() {
            return (h.flags & frame::FLAG_PROTECTED == 0).then(|| body.to_vec());
        }
        if h.addr1[0] & 1 == 1 {
            let igtk = c.igtk.as_ref()?;
            let (_, pn) = bip_check(&igtk.key, &f[..h.len], body)?;
            if pn <= c.igtk_pn {
                return None;
            }
            c.igtk_pn = pn;
            return Some(body[..body.len() - 18].to_vec());
        }
        if h.flags & frame::FLAG_PROTECTED == 0 {
            return None;
        }
        let (pn, plain) = ccmp_decrypt(&c.ptk.as_ref()?.tk, &f[..h.len], body)?;
        if pn <= c.rx_pn[16] {
            return None;
        }
        c.rx_pn[16] = pn;
        Some(plain)
    }

    fn on_deauth(&mut self, h: &Header, f: &[u8], _now: u64) {
        if !self.from_ap(h) {
            return;
        }
        let Some(body) = self.robust_body(h, f) else { return };
        let reason = if body.len() >= 2 { u16::from_le_bytes([body[0], body[1]]) } else { 0 };
        let c = self.conn.as_ref().unwrap();
        let failure = if c.step == Step::Keys && c.got_msg1 { Failure::WrongPassword } else { Failure::Deauthenticated(reason) };
        // The access point already dropped us: nothing to tell it.
        self.fail_with(failure, false);
    }

    fn on_action(&mut self, h: &Header, f: &[u8]) {
        if !self.from_ap(h) {
            return;
        }
        let Some(body) = self.robust_body(h, f) else { return };
        // SA Query request (category 8, action 0): answer it.
        if body.len() >= 4 && body[0] == 8 && body[1] == 0 {
            let seq = self.next_seq();
            let c = self.conn.as_mut().unwrap();
            let Some(ptk) = &c.ptk else { return };
            let mut out = frame::mgmt_header(frame::ACTION, &c.bss.bssid, &self.mac, &c.bss.bssid, seq);
            out[1] |= frame::FLAG_PROTECTED;
            c.tx_pn += 1;
            let enc = ccmp_encrypt(&ptk.tk, &out, c.tx_pn, 0, &[8, 1, body[2], body[3]]);
            out.extend(enc);
            self.out.push(Out::Tx(out));
        }
    }

    fn on_data(&mut self, h: &Header, f: &[u8], now: u64) {
        let Some(c) = &mut self.conn else { return };
        if h.addr2 != c.bss.bssid || h.flags & 3 != frame::FLAG_FROM_DS {
            return;
        }
        if c.step == Step::Up {
            c.last_beacon = c.last_beacon.max(now);
        }
        // Retransmissions of a frame we already have.
        let tid = h.qos.map_or(16, |q| (q & 0x0F) as u8);
        if h.flags & frame::FLAG_RETRY != 0 && c.last_seq == Some((tid, h.seq)) {
            return;
        }
        c.last_seq = Some((tid, h.seq));
        let group = h.addr1[0] & 1 == 1;
        if group && h.addr3 == self.mac {
            return; // our own broadcast, sent back by the access point
        }
        let body = &f[h.len..];
        let plain = if h.flags & frame::FLAG_PROTECTED != 0 {
            if group {
                let Some(gtk) = &c.gtk else { return };
                if body.len() < 8 || (body[3] >> 6) != gtk.id {
                    return;
                }
                let Some((pn, p)) = ccmp_decrypt(&gtk.key, &f[..h.len], body) else { return };
                if pn <= c.gtk_pn {
                    return;
                }
                c.gtk_pn = pn;
                p
            } else {
                let Some(ptk) = &c.ptk else { return };
                let Some((pn, p)) = ccmp_decrypt(&ptk.tk, &f[..h.len], body) else { return };
                let slot = tid.min(16) as usize;
                if pn <= c.rx_pn[slot] {
                    return;
                }
                c.rx_pn[slot] = pn;
                p
            }
        } else {
            body.to_vec()
        };
        let protected = h.flags & frame::FLAG_PROTECTED != 0;
        for e in frame::to_ethernet(h, &plain) {
            if e.len() < 14 {
                continue;
            }
            let ethertype = u16::from_be_bytes([e[12], e[13]]);
            if ethertype == eapol::ETHERTYPE {
                self.on_eapol(&e[14..], now);
            } else if protected || self.conn.as_ref().is_some_and(|c| c.akm.is_none() && c.step == Step::Up) {
                self.out.push(Out::Ether(e));
            }
        }
    }

    // ------------------------------------------------------------------------
    // The supplicant

    fn send_eapol(&mut self, frame_body: Vec<u8>) {
        let bssid = self.conn.as_ref().unwrap().bss.bssid;
        let mut eth = Vec::with_capacity(14 + frame_body.len());
        eth.extend_from_slice(&bssid);
        eth.extend_from_slice(&self.mac);
        eth.extend_from_slice(&eapol::ETHERTYPE.to_be_bytes());
        eth.extend_from_slice(&frame_body);
        self.send_frame(&eth, true);
    }

    fn on_eapol(&mut self, raw: &[u8], now: u64) {
        let Some(k) = Key::parse(raw) else { return };
        let Some(c) = &mut self.conn else { return };
        let Some(akm) = c.akm else { return };
        let Some(pmk) = c.pmk else { return };
        if k.info & eapol::KI_ACK == 0 {
            return;
        }
        if c.replay.is_some_and(|r| k.replay <= r) {
            return;
        }
        let version = akm.version();
        if k.info & eapol::KI_PAIRWISE != 0 {
            if k.info & eapol::KI_MIC == 0 {
                // Message 1: our nonce, the PTK, message 2.
                if c.anonce != Some(k.nonce) {
                    self.rng.fill(&mut c.snonce);
                }
                c.anonce = Some(k.nonce);
                c.got_msg1 = true;
                let ptk = eapol::derive_ptk(akm, &pmk, &c.bss.bssid, &self.mac, &k.nonce, &c.snonce);
                let mut m2 = Key {
                    eapol_version: k.eapol_version,
                    info: version | eapol::KI_PAIRWISE | eapol::KI_MIC,
                    replay: k.replay,
                    nonce: c.snonce,
                    data: c.rsn_ie.clone(),
                    ..Key::default()
                };
                let frame = m2.build_signed(akm, &ptk.kck);
                c.new_ptk = Some(ptk);
                if c.step == Step::Keys {
                    c.deadline = now + 3000;
                }
                self.send_eapol(frame);
                return;
            }
            // Message 3.
            let Some(ptk) = c.new_ptk.clone() else { return };
            if c.anonce != Some(k.nonce) || k.info & eapol::KI_INSTALL == 0 || !Key::check_mic(raw, akm, &ptk.kck) {
                self.log(String::from("4-way: message 3 refused"));
                return;
            }
            c.replay = Some(k.replay);
            let data = if k.info & eapol::KI_ENCRYPTED != 0 {
                match eapol::decrypt_data(&ptk.kek, &k.data) {
                    Some(d) => d,
                    None => return,
                }
            } else {
                k.data.clone()
            };
            let kd = eapol::parse_key_data(&data);
            // The RSN element must be the one of the beacon (a downgrade
            // otherwise).
            let ap_rsn = kd.rsn.as_ref().and_then(|r| Rsn::parse(&r[2..]));
            if ap_rsn.as_ref().map(|r| &r.akms) != c.bss.rsn.as_ref().map(|r| &r.akms) {
                self.log(String::from("4-way: the access point's RSN element differs from its beacon"));
                return self.fail(Failure::Rejected(0));
            }
            let mut m4 =
                Key { eapol_version: k.eapol_version, info: version | eapol::KI_PAIRWISE | eapol::KI_MIC | eapol::KI_SECURE, replay: k.replay, ..Key::default() };
            let frame = m4.build_signed(akm, &ptk.kck);
            self.send_eapol(frame);
            let c = self.conn.as_mut().unwrap();
            c.ptk = Some(ptk);
            c.tx_pn = 0;
            c.rx_pn = [0; 17];
            if let Some(g) = kd.gtk {
                c.gtk_pn = u64::from_le_bytes([k.rsc[0], k.rsc[1], k.rsc[2], k.rsc[3], k.rsc[4], k.rsc[5], 0, 0]);
                c.gtk = Some(g);
            }
            if let Some((ig, ipn)) = kd.igtk {
                c.igtk = Some(ig);
                c.igtk_pn = ipn;
            }
            if c.step != Step::Up {
                self.log(format!("4-way handshake done ({akm:?})"));
                self.up();
            } else {
                self.log(String::from("pairwise key renewed"));
            }
        } else {
            // The group key handshake.
            let Some(ptk) = c.ptk.clone() else { return };
            if k.info & eapol::KI_MIC == 0 || !Key::check_mic(raw, akm, &ptk.kck) {
                return;
            }
            c.replay = Some(k.replay);
            let Some(data) = eapol::decrypt_data(&ptk.kek, &k.data) else { return };
            let kd = eapol::parse_key_data(&data);
            if let Some(g) = kd.gtk {
                c.gtk_pn = u64::from_le_bytes([k.rsc[0], k.rsc[1], k.rsc[2], k.rsc[3], k.rsc[4], k.rsc[5], 0, 0]);
                let id = g.id;
                c.gtk = Some(g);
                self.log(format!("group key {id} installed"));
            }
            let c = self.conn.as_mut().unwrap();
            if let Some((ig, ipn)) = kd.igtk {
                c.igtk = Some(ig);
                c.igtk_pn = ipn;
            }
            let mut m2 = Key { eapol_version: k.eapol_version, info: version | eapol::KI_MIC | eapol::KI_SECURE, replay: k.replay, ..Key::default() };
            let frame = m2.build_signed(akm, &ptk.kck);
            self.send_eapol(frame);
        }
    }

    // ------------------------------------------------------------------------
    // Data

    /// An Ethernet frame from the IP stack: sent to the network if
    /// connected (dropped otherwise, as on an unplugged cable).
    pub fn send(&mut self, eth: &[u8]) {
        if self.state != State::Connected || self.scan.is_some() {
            return;
        }
        self.send_frame(eth, false);
    }

    fn send_frame(&mut self, eth: &[u8], eapol: bool) {
        if eth.len() < 14 {
            return;
        }
        let seq = self.next_seq();
        let mac = self.mac;
        let Some(c) = &mut self.conn else { return };
        let da: Mac = eth[..6].try_into().unwrap();
        let ethertype = u16::from_be_bytes([eth[12], eth[13]]);
        let mut h = frame::data_header(&c.bss.bssid, &mac, &da, seq, c.bss.wmm);
        if c.bss.wmm && eapol {
            h[24] = 7; // EAPOL at the highest priority (voice)
        }
        let body = frame::llc(ethertype, &eth[14..]);
        let f = match &c.ptk {
            Some(ptk) => {
                h[1] |= frame::FLAG_PROTECTED;
                c.tx_pn += 1;
                let enc = ccmp_encrypt(&ptk.tk, &h, c.tx_pn, 0, &body);
                [h, enc].concat()
            }
            None if eapol || c.akm.is_none() => [h, body].concat(),
            None => return,
        };
        self.out.push(Out::Tx(f));
    }
}

pub fn mac_text(m: &Mac) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}
