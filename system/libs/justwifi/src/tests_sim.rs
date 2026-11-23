//! The station against the access point simulator (`scripts/wifi/apsim.py`,
//! independent code in Python) over UDP, on this machine: scanning, every
//! kind of network, DHCP and ping through the tunnel, a wrong password, a
//! new group key, forged and genuine deauthentications under PMF.
//!
//! Needs python3 with the 'cryptography' package; `make test-wifi` runs it
//! (`cargo test -p justwifi -- --ignored`).

use crate::frame::{Mac, Security};
use crate::sim;
use crate::sta::{Failure, Out, RxInfo, State, Station};
use std::io::{BufRead, BufReader, Write};
use std::net::UdpSocket;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::string::{String, ToString};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use std::vec::Vec;
use std::{format, println, vec};

const MAC: Mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

struct Rig {
    sta: Station,
    sock: UdpSocket,
    peer: String,
    channel: u8,
    start: Instant,
    ap: Child,
    ap_in: ChildStdin,
    log: Receiver<String>,
    ap_lines: Vec<String>,
    /// The IP client of the test: our address once DHCP gave one.
    ip: Option<[u8; 4]>,
    gateway: [u8; 4],
    xid: u32,
    pongs: u32,
    link: bool,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.ap.kill();
    }
}

fn csum(d: &[u8]) -> u16 {
    let mut s = 0u32;
    for c in d.chunks(2) {
        s += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while s > 0xFFFF {
        s = (s & 0xFFFF) + (s >> 16);
    }
    !(s as u16)
}

fn ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
    let mut h = vec![0x45, 0, 0, 0, 0, 1, 0, 0, 64, proto, 0, 0];
    h[2..4].copy_from_slice(&((20 + payload.len()) as u16).to_be_bytes());
    h.extend_from_slice(&src);
    h.extend_from_slice(&dst);
    let c = csum(&h);
    h[10..12].copy_from_slice(&c.to_be_bytes());
    h.extend_from_slice(payload);
    h
}

fn eth(dst: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut e = dst.to_vec();
    e.extend_from_slice(&MAC);
    e.extend_from_slice(&ethertype.to_be_bytes());
    e.extend_from_slice(payload);
    e
}

impl Rig {
    fn new() -> Rig {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(2))).unwrap();
        let ours = sock.local_addr().unwrap().port();
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let theirs = probe.local_addr().unwrap().port();
        drop(probe);
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../scripts/wifi/apsim.py");
        let mut ap = Command::new("python3")
            .args([script, "--listen", &theirs.to_string(), "--peer", &ours.to_string(), "--rekey", "2"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("python3");
        let ap_in = ap.stdin.take().unwrap();
        let out = ap.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                println!("  | {line}");
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(500));
        Rig {
            sta: Station::new(MAC, &[7; 32], &[1, 6, 11, 36, 44, 52]),
            sock,
            peer: format!("127.0.0.1:{theirs}"),
            channel: 0,
            start: Instant::now(),
            ap,
            ap_in,
            log: rx,
            ap_lines: Vec::new(),
            ip: None,
            gateway: [0; 4],
            xid: 0x1234,
            pongs: 0,
            link: false,
        }
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn ap(&mut self, cmd: &str) {
        writeln!(self.ap_in, "{cmd}").unwrap();
        self.ap_in.flush().unwrap();
    }

    /// Runs the medium until `done` says so, or `ms` pass.
    fn run(&mut self, ms: u64, done: &dyn Fn(&Rig) -> bool) -> bool {
        let until = self.now() + ms;
        let mut buf = [0u8; 4096];
        while self.now() < until {
            if let Ok(n) = self.sock.recv(&mut buf) {
                if let Some((ch, rssi, f)) = sim::unwrap(&buf[..n]) {
                    if ch == self.channel {
                        let now = self.now();
                        self.sta.input(f, RxInfo { channel: ch, rssi }, now);
                    }
                }
            }
            let now = self.now();
            self.sta.poll(now);
            for o in self.sta.take() {
                match o {
                    Out::Tx(f) => {
                        let e = sim::wrap(&MAC, self.channel, 0, &f);
                        self.sock.send_to(&e, &self.peer).unwrap();
                    }
                    Out::Channel(c) => self.channel = c,
                    Out::Ether(e) => self.ether(&e),
                    Out::Link(l) => {
                        println!("link: {l:?}");
                        self.link = l.is_some();
                        if l.is_none() {
                            self.ip = None;
                        }
                    }
                    Out::Log(s) => println!("sta: {s}"),
                }
            }
            while let Ok(l) = self.log.try_recv() {
                self.ap_lines.push(l);
            }
            if done(self) {
                return true;
            }
        }
        false
    }

    fn ap_said(&self, s: &str) -> bool {
        self.ap_lines.iter().any(|l| l.contains(s))
    }

    fn send_ether(&mut self, e: Vec<u8>) {
        self.sta.send(&e);
    }

    fn dhcp(&mut self, kind: u8, requested: Option<[u8; 4]>) {
        let mut d = vec![0u8; 240];
        d[0] = 1;
        d[1] = 1;
        d[2] = 6;
        d[4..8].copy_from_slice(&self.xid.to_be_bytes());
        d[10] = 0x80;
        d[28..34].copy_from_slice(&MAC);
        d[236..240].copy_from_slice(&[99, 130, 83, 99]);
        d.extend_from_slice(&[53, 1, kind]);
        if let Some(ip) = requested {
            d.extend_from_slice(&[50, 4]);
            d.extend_from_slice(&ip);
        }
        d.push(255);
        let mut u = vec![0, 68, 0, 67, 0, 0, 0, 0];
        u[4..6].copy_from_slice(&((8 + d.len()) as u16).to_be_bytes());
        u.extend_from_slice(&d);
        let p = ipv4([0; 4], [255; 4], 17, &u);
        self.send_ether(eth([0xFF; 6], 0x0800, &p));
    }

    fn ping(&mut self, seq: u16) {
        let Some(ip) = self.ip else { return };
        let mut m = vec![8, 0, 0, 0, 0x12, 0x34];
        m.extend_from_slice(&seq.to_be_bytes());
        m.extend_from_slice(b"justOS wifi test");
        let c = csum(&m);
        m[2..4].copy_from_slice(&c.to_be_bytes());
        let p = ipv4(ip, self.gateway, 1, &m);
        // The gateway's MAC from ARP is not needed: the AP routes by IP.
        self.send_ether(eth([0x02, 0xB0, 0, 0, 0, 0], 0x0800, &p));
    }

    fn ether(&mut self, e: &[u8]) {
        let ty = u16::from_be_bytes([e[12], e[13]]);
        let p = &e[14..];
        if ty == 0x0806 && p.len() >= 28 && p[7] == 1 {
            // An ARP request for us: answer.
            if Some(<[u8; 4]>::try_from(&p[24..28]).unwrap()) == self.ip {
                let mut r = vec![0, 1, 8, 0, 6, 4, 0, 2];
                r.extend_from_slice(&MAC);
                r.extend_from_slice(&self.ip.unwrap());
                r.extend_from_slice(&p[8..18]);
                let to: Mac = p[8..14].try_into().unwrap();
                self.send_ether(eth(to, 0x0806, &r));
            }
        }
        if ty != 0x0800 || p.len() < 28 {
            return;
        }
        let body = &p[20..];
        if p[9] == 17 && u16::from_be_bytes([body[2], body[3]]) == 68 {
            let d = &body[8..];
            let yiaddr: [u8; 4] = d[16..20].try_into().unwrap();
            let kind = d[242];
            if kind == 2 {
                self.dhcp(3, Some(yiaddr));
            } else if kind == 5 {
                self.ip = Some(yiaddr);
                self.gateway = [yiaddr[0], yiaddr[1], yiaddr[2], 1];
                println!("dhcp: {yiaddr:?}");
            }
        } else if p[9] == 1 && body[0] == 0 {
            self.pongs += 1;
        }
    }

    /// Joins, gets an address, pings twice.
    fn join(&mut self, ssid: &str, pw: Option<&str>) {
        let now = self.now();
        self.sta.connect(ssid.as_bytes(), pw.map(|p| p.as_bytes()), now);
        assert!(self.run(8000, &|r| r.link), "joined {ssid}: {:?}", self.sta.status());
        self.xid += 1;
        self.dhcp(1, None);
        assert!(self.run(3000, &|r| r.ip.is_some()), "DHCP on {ssid}");
        let before = self.pongs;
        for seq in 1..=2 {
            self.ping(seq);
            assert!(self.run(2000, &|r| r.pongs >= before + seq as u32), "ping {seq} on {ssid}");
        }
    }
}

#[test]
#[ignore]
fn station_against_simulated_access_points() {
    let mut r = Rig::new();
    // Scan: all five networks, with their security.
    let now = r.now();
    r.sta.scan(now);
    assert!(r.run(5000, &|r| !r.sta.status().scanning));
    let nets = r.sta.networks();
    for n in &nets {
        println!("seen: {} ch {} {} dBm {:?}", n.name(), n.channel, n.rssi, n.security());
    }
    let sec = |name: &str| nets.iter().find(|n| n.name() == name).map(|n| n.security());
    assert_eq!(sec("justOS-aperta"), Some(Security::Open));
    assert_eq!(sec("justOS-wpa2"), Some(Security::Wpa2));
    assert_eq!(sec("justOS-misto"), Some(Security::Wpa2Wpa3));
    assert_eq!(sec("justOS-wpa3"), Some(Security::Wpa3));
    assert_eq!(sec("justOS-h2e"), Some(Security::Wpa3));
    assert_eq!(nets[0].name(), "justOS-aperta"); // the strongest first

    // Open network.
    r.join("justOS-aperta", None);

    // WPA2 with the wrong password, then the right one.
    let now = r.now();
    r.sta.connect(b"justOS-wpa2", Some(b"sbagliata!"), now);
    assert!(r.run(10000, &|r| r.sta.status().state == State::Idle));
    assert_eq!(r.sta.status().failure, Some(Failure::WrongPassword));
    assert!(r.ap_said("wrong MIC: wrong password"));
    r.join("justOS-wpa2", Some("password-wpa2"));
    assert!(r.ap_said("4-way handshake done"));
    // The group key changes 2 s after joining; the AP checks it with an ARP.
    assert!(r.run(5000, &|r| r.ap_said("the group key works")), "new group key");
    r.ping(9);

    // WPA3 (hunting and pecking, PMF required).
    let now = r.now();
    r.sta.connect(b"justOS-wpa3", Some(b"errata-wpa3"), now);
    assert!(r.run(10000, &|r| r.sta.status().state == State::Idle));
    assert_eq!(r.sta.status().failure, Some(Failure::WrongPassword));
    r.join("justOS-wpa3", Some("password-wpa3"));
    assert!(r.ap_said("SAE authenticated"));
    // A forged deauthentication is ignored, a signed one is obeyed.
    r.ap("deauth justOS-wpa3 forged");
    r.run(500, &|_| false);
    assert!(r.link, "a forged deauthentication was obeyed");
    r.ap("deauth justOS-wpa3 bip");
    assert!(r.run(1000, &|r| !r.link), "the signed deauthentication was ignored");
    r.sta.disconnect();

    // WPA3 with hash-to-element only.
    r.join("justOS-h2e", Some("password-h2e"));
    assert!(r.ap_said("(hash-to-element)"));

    // Transition mode: SAE is chosen.
    r.join("justOS-misto", Some("password-misto"));
    assert!(r.ap_said("4-way handshake done with 52:54:00:12:34:56 (AKM 8)"));
    r.sta.disconnect();
}
