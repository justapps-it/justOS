//! USB Bluetooth controllers: the HCI over USB of the Bluetooth core
//! specification (vol 4, part B).
//!
//! Interface 0 of a Bluetooth adapter has three endpoints: interrupt IN
//! for HCI events, bulk IN and bulk OUT for ACL data. Commands go out as
//! class requests on endpoint 0. The driver is only the transport: once the
//! controller is ready it registers it with the kernel (`hci_register`),
//! whose Bluetooth stack sends the commands and gets every event and ACL
//! packet (`hci_receive`).
//!
//! Realtek controllers start with the code in their ROM and need a patch
//! from `sys:/system/firmware/rtl_bt` first, as Linux's btrtl loads it:
//! the ROM version (vendor command 0xFC6D) picks the patch in the file,
//! the config file is appended, and the whole goes to the chip in pieces
//! of 252 bytes with vendor command 0xFC20. Both file formats are read:
//! the old "Realtech" one and the newer "RTBTCore" (8852B, 8852C, 8851B).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use justdrv::usb::*;
use justdrv::{DeviceInfo, Event, HciDesc, IrqLock, Mutex, Status, E_GONE, E_INVAL, E_IO, E_NODEV, E_TIMEOUT, OK};
use justdrv::{HCI_ACL, HCI_COMMAND, HCI_EVENT};

justdrv::driver! {
    name: "btusb",
    version: "1.0.0",
    probe: probe,
    remove: remove,
}

const CLASS_WIRELESS: u8 = 0xE0;
const OP_RESET: u16 = 0x0C03;
const OP_READ_LOCAL_VERSION: u16 = 0x1001;
const OP_RTL_DOWNLOAD: u16 = 0xFC20;
const OP_RTL_READ_REG16: u16 = 0xFC61;
const OP_RTL_ROM_VERSION: u16 = 0xFC6D;
const EV_COMMAND_COMPLETE: u8 = 0x0E;
const EV_COMMAND_STATUS: u8 = 0x0F;
const MANUFACTURER_REALTEK: u16 = 0x005D;
/// Bytes of patch per download command.
const RTL_FRAG_LEN: usize = 252;

struct Bt {
    device: u32,
    iface: &'static UsbInterface,
    ep_event: u8,
    ep_in: u8,
    ep_out: u8,
    max_in: u16,
    max_event: u16,
    /// An event that came in several packets, so far.
    partial: IrqLock<Vec<u8>>,
    /// ACL data from the bulk endpoint, until a whole packet is there.
    acl: IrqLock<Vec<u8>>,
    /// Before the kernel has the controller: the events, for the setup.
    setup_events: IrqLock<VecDeque<Vec<u8>>>,
    setup_wake: Event,
    /// `hci<N>` once registered, -1 before.
    adapter: AtomicI32,
    out: Mutex<()>,
    dead: AtomicBool,
    stopped: AtomicBool,
}

static DEVICES: IrqLock<Vec<(u32, usize)>> = IrqLock::new(Vec::new());

// ----------------------------------------------------------------------------
// Transport

unsafe extern "C" fn event_report(ctx: *mut c_void, data: *const u8, len: u32) {
    // SAFETY: the Bt this driver passed to poll_start; never freed.
    let bt = unsafe { &*(ctx as *const Bt) };
    // SAFETY: the controller passes `len` bytes.
    let data = unsafe { core::slice::from_raw_parts(data, len as usize) };
    bt.event_data(data);
}

impl Bt {
    /// A piece of an event from the interrupt endpoint. Events are longer
    /// than the endpoint's packets: a short packet ends one.
    fn event_data(&self, data: &[u8]) {
        let mut done: Vec<Vec<u8>> = Vec::new();
        {
            let mut p = self.partial.lock();
            p.extend_from_slice(data);
            while p.len() >= 2 && p.len() >= 2 + p[1] as usize {
                let n = 2 + p[1] as usize;
                done.push(p.drain(..n).collect());
            }
            if (data.len() as u16) < self.max_event && !p.is_empty() {
                p.clear(); // a short packet in the middle of an event: lost
            }
        }
        for ev in done {
            self.deliver(HCI_EVENT, ev);
        }
    }

    fn acl_data(&self, data: &[u8]) {
        let mut done: Vec<Vec<u8>> = Vec::new();
        {
            let mut a = self.acl.lock();
            a.extend_from_slice(data);
            while a.len() >= 4 {
                let n = 4 + u16::from_le_bytes([a[2], a[3]]) as usize;
                if a.len() < n {
                    break;
                }
                done.push(a.drain(..n).collect());
            }
            if a.len() > 4096 {
                a.clear();
            }
        }
        for p in done {
            self.deliver(HCI_ACL, p);
        }
    }

    fn deliver(&self, kind: u32, packet: Vec<u8>) {
        let adapter = self.adapter.load(Ordering::Acquire);
        if adapter >= 0 {
            justdrv::hci_receive(adapter as u32, kind, &packet);
        } else if kind == HCI_EVENT {
            let mut q = self.setup_events.lock();
            if q.len() < 64 {
                q.push_back(packet);
            }
            drop(q);
            self.setup_wake.signal();
        }
    }

    fn send(&self, kind: u32, data: &[u8]) -> Status {
        if self.dead.load(Ordering::Acquire) {
            return E_GONE;
        }
        let _one = self.out.lock();
        let mut buf = data.to_vec();
        let r = match kind {
            HCI_COMMAND => {
                let setup = Setup::new(TYPE_CLASS | RECIP_DEVICE, 0, 0, 0, buf.len() as u16);
                // SAFETY: the interface of a present device; `buf` holds
                // `length` bytes.
                unsafe { (self.iface.control)(self.iface.ctx, &setup, buf.as_mut_ptr(), 1000) }
            }
            // SAFETY: as above.
            HCI_ACL => unsafe { (self.iface.transfer)(self.iface.ctx, self.ep_out, buf.as_mut_ptr(), buf.len() as u32, 2000) },
            _ => return E_INVAL,
        };
        if r < 0 {
            r
        } else {
            OK
        }
    }

    // ------------------------------------------------------------------
    // Commands during setup (before the kernel's stack has the controller)

    /// Sends a command and waits for its completion: the return parameters
    /// (status first).
    fn command(&self, opcode: u16, params: &[u8]) -> Result<Vec<u8>, Status> {
        self.setup_events.lock().clear();
        let mut cmd = Vec::with_capacity(3 + params.len());
        cmd.extend_from_slice(&opcode.to_le_bytes());
        cmd.push(params.len() as u8);
        cmd.extend_from_slice(params);
        let st = self.send(HCI_COMMAND, &cmd);
        if st != OK {
            return Err(st);
        }
        let until = justdrv::time_ms() + 3000;
        loop {
            while let Some(ev) = self.setup_events.lock().pop_front() {
                match ev[0] {
                    EV_COMMAND_COMPLETE if ev.len() >= 5 && u16::from_le_bytes([ev[3], ev[4]]) == opcode => {
                        return Ok(ev[5..].to_vec());
                    }
                    EV_COMMAND_STATUS if ev.len() >= 6 && u16::from_le_bytes([ev[4], ev[5]]) == opcode && ev[2] != 0 => {
                        return Err(E_IO);
                    }
                    _ => {}
                }
            }
            let now = justdrv::time_ms();
            if now >= until || self.dead.load(Ordering::Acquire) {
                return Err(E_TIMEOUT);
            }
            self.setup_wake.wait(until - now);
        }
    }
}

unsafe extern "C" fn hci_send(ctx: *mut c_void, kind: u32, data: *const u8, len: usize) -> Status {
    // SAFETY: the Bt behind the descriptor; never freed.
    let bt = unsafe { &*(ctx as *const Bt) };
    if data.is_null() {
        return E_INVAL;
    }
    // SAFETY: the kernel passes `len` bytes.
    bt.send(kind, unsafe { core::slice::from_raw_parts(data, len) })
}

// ----------------------------------------------------------------------------
// Realtek

/// A Realtek chip as its ROM reports itself, and its files.
struct RtlChip {
    lmp_subver: u16,
    hci_rev: u16,
    hci_ver: u8,
    name: &'static str,
    fw: &'static str,
    config: &'static str,
}

/// The USB chips of Linux's btrtl table that load their patch the same
/// way. Only the 8852B's files come with justOS.
const RTL_CHIPS: &[RtlChip] = &[
    RtlChip { lmp_subver: 0x8852, hci_rev: 0xb, hci_ver: 0xb, name: "RTL8852BU", fw: "rtl_bt/rtl8852bu_fw.bin", config: "rtl_bt/rtl8852bu_config.bin" },
    RtlChip { lmp_subver: 0x8852, hci_rev: 0xa, hci_ver: 0xb, name: "RTL8852AU", fw: "rtl_bt/rtl8852au_fw.bin", config: "rtl_bt/rtl8852au_config.bin" },
    RtlChip { lmp_subver: 0x8852, hci_rev: 0xc, hci_ver: 0xc, name: "RTL8852CU", fw: "rtl_bt/rtl8852cu_fw.bin", config: "rtl_bt/rtl8852cu_config.bin" },
    RtlChip { lmp_subver: 0x8761, hci_rev: 0xb, hci_ver: 0xa, name: "RTL8761BU", fw: "rtl_bt/rtl8761bu_fw.bin", config: "rtl_bt/rtl8761bu_config.bin" },
    RtlChip { lmp_subver: 0x8822, hci_rev: 0xc, hci_ver: 0xa, name: "RTL8822CU", fw: "rtl_bt/rtl8822cu_fw.bin", config: "rtl_bt/rtl8822cu_config.bin" },
];

/// Project ids of the firmware files' trailer, and the chip each is for.
const RTL_PROJECTS: &[(u8, u16)] = &[
    (0, 0x8723),
    (1, 0x8821),
    (2, 0x8761),
    (3, 0x8822),
    (8, 0x8723),
    (9, 0x8723),
    (10, 0x8821),
    (13, 0x8822),
    (14, 0x8761),
    (18, 0x8852),
    (20, 0x8852),
    (25, 0x8852),
    (33, 0x8851),
];

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The project id in a firmware file's trailer (read backwards from the
/// signature at its end).
fn rtl_project_id(fw: &[u8]) -> Option<u8> {
    const SIG: [u8; 4] = [0x51, 0x04, 0xFD, 0x77];
    if fw.len() < 14 + 4 || fw[fw.len() - 4..] != SIG {
        return None;
    }
    let mut at = fw.len() - 4;
    while at >= 14 + 3 {
        let opcode = fw[at - 1];
        let length = fw[at - 2];
        let data = fw[at - 3];
        at -= 3;
        if opcode == 0xFF || length == 0 {
            return None;
        }
        if opcode == 0 && length == 1 {
            return Some(data);
        }
        at = at.checked_sub(length as usize)?;
    }
    None
}

/// The patch for ROM version `rom` from an old-style ("Realtech") file.
fn rtl_patch_v1(fw: &[u8], rom: u8) -> Option<Vec<u8>> {
    let fw_version = le32(fw, 8)?;
    let n = le16(fw, 12)? as usize;
    let base = 14;
    let i = (0..n).find(|&i| le16(fw, base + 2 * i) == Some(rom as u16 + 1))?;
    let len = le16(fw, base + 2 * n + 2 * i)? as usize;
    let off = le32(fw, base + 4 * n + 4 * i)? as usize;
    if len < 4 {
        return None;
    }
    let mut patch = fw.get(off..off + len)?.to_vec();
    patch[len - 4..].copy_from_slice(&fw_version.to_le_bytes());
    Some(patch)
}

/// The patch for ROM version `rom` and security key `key_id` from a new
/// ("RTBTCore") file: the subsections for this ROM, by priority.
fn rtl_patch_v2(fw: &[u8], rom: u8, key_id: u8) -> Option<Vec<u8>> {
    const SNIPPETS: u32 = 1;
    const DUMMY_HEADER: u32 = 2;
    const SECURITY_HEADER: u32 = 3;
    let end = fw.len().checked_sub(7)?; // the trailer
    let sections = le32(fw, 16)?;
    let mut at = 20;
    // (priority, data), in the order Linux keeps them: a new piece goes
    // before the first one of the same or a higher priority.
    let mut pieces: Vec<(u8, &[u8])> = Vec::new();
    for _ in 0..sections {
        if at + 8 > end {
            break;
        }
        let opcode = le32(fw, at)?;
        let len = le32(fw, at + 4)? as usize;
        at += 8;
        if at + len > end {
            break;
        }
        let sec = &fw[at..at + len];
        at += len;
        let wanted = match opcode {
            SNIPPETS | DUMMY_HEADER => true,
            SECURITY_HEADER => key_id != 0,
            _ => false,
        };
        if !wanted {
            continue;
        }
        let count = le16(sec, 0)? as usize;
        let mut p = 4;
        for _ in 0..count {
            if p + 8 > sec.len() {
                break;
            }
            let (eco, prio, key) = (sec[p], sec[p + 1], sec[p + 2]);
            let sub_len = le32(sec, p + 4)? as usize;
            p += 8;
            let Some(data) = sec.get(p..p + sub_len) else { break };
            p += sub_len;
            if eco != rom.wrapping_add(1) || (opcode == SECURITY_HEADER && key != key_id) {
                continue;
            }
            let i = pieces.iter().position(|(q, _)| *q >= prio).unwrap_or(pieces.len());
            pieces.insert(i, (prio, data));
        }
    }
    let patch: Vec<u8> = pieces.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    (!patch.is_empty()).then_some(patch)
}

impl Bt {
    /// Loads the Realtek patch if the chip still runs its ROM. Returns the
    /// chip's name.
    fn realtek_setup(&self, hci_ver: u8, hci_rev: u16, lmp_subver: u16) -> Option<&'static str> {
        let Some(chip) = RTL_CHIPS.iter().find(|c| c.lmp_subver == lmp_subver && c.hci_rev == hci_rev && c.hci_ver == hci_ver) else {
            // Also what a patched chip reports: its version is the patch's.
            justdrv::info!("Realtek controller {lmp_subver:04x}/{hci_rev:04x}: no patch needed");
            return None;
        };
        let Some(fw) = justdrv::firmware(chip.fw) else {
            justdrv::warn!("{}: firmware {} missing in sys:/system/firmware; the ROM's code is used", chip.name, chip.fw);
            return Some(chip.name);
        };
        match rtl_project_id(&fw).and_then(|id| RTL_PROJECTS.iter().find(|p| p.0 == id)) {
            Some(&(_, lmp)) if lmp == chip.lmp_subver => {}
            _ => {
                justdrv::warn!("{}: {} is not for this chip", chip.name, chip.fw);
                return Some(chip.name);
            }
        }
        let rom = match self.command(OP_RTL_ROM_VERSION, &[]) {
            Ok(r) if r.len() >= 2 && r[0] == 0 => r[1],
            _ => {
                justdrv::warn!("{}: cannot read the ROM version", chip.name);
                return Some(chip.name);
            }
        };
        let patch = if fw.starts_with(b"RTBTCore") {
            // The key the chip checks the patch's security header with.
            let key_id = match self.command(OP_RTL_READ_REG16, &[0x10, 0xA4, 0x0D, 0x00, 0xB0]) {
                Ok(r) if r.len() >= 3 && r[0] == 0 => r[1],
                _ => {
                    justdrv::warn!("{}: cannot read the security key", chip.name);
                    return Some(chip.name);
                }
            };
            rtl_patch_v2(&fw, rom, key_id)
        } else if fw.starts_with(b"Realtech") {
            rtl_patch_v1(&fw, rom)
        } else {
            None
        };
        let Some(mut patch) = patch else {
            justdrv::warn!("{}: no patch for ROM version {rom} in {}", chip.name, chip.fw);
            return Some(chip.name);
        };
        if let Some(config) = justdrv::firmware(chip.config) {
            patch.extend_from_slice(&config);
        }
        justdrv::info!("{}: ROM version {rom}, loading {} ({} bytes)", chip.name, chip.fw, patch.len());
        if let Err(e) = self.rtl_download(&patch) {
            justdrv::warn!("{}: firmware download failed ({})", chip.name, justdrv::status_name(e));
            return Some(chip.name);
        }
        let _ = self.command(OP_RESET, &[]);
        if let Ok(v) = self.command(OP_READ_LOCAL_VERSION, &[]) {
            if v.len() >= 9 && v[0] == 0 {
                justdrv::info!("{}: firmware running, version {:04x}{:04x}", chip.name, le16(&v, 2).unwrap_or(0), le16(&v, 7).unwrap_or(0));
            }
        }
        Some(chip.name)
    }

    /// Vendor command 0xFC20, as btrtl sends it: an index byte (bit 7 on
    /// the last piece; after 0x7F the count starts again at 1) and up to
    /// 252 bytes.
    fn rtl_download(&self, data: &[u8]) -> Result<(), Status> {
        let frags = data.len() / RTL_FRAG_LEN + 1;
        let mut j: u8 = 0;
        for i in 0..frags {
            let mut index = j;
            j += 1;
            if index == 0x7F {
                j = 1;
            }
            let start = i * RTL_FRAG_LEN;
            let len = if i == frags - 1 {
                index |= 0x80;
                data.len() % RTL_FRAG_LEN
            } else {
                RTL_FRAG_LEN
            };
            let mut cmd = Vec::with_capacity(1 + len);
            cmd.push(index);
            cmd.extend_from_slice(&data[start..start + len]);
            let r = self.command(OP_RTL_DOWNLOAD, &cmd)?;
            if r.len() < 2 || r[0] != 0 || r[1] != index {
                return Err(E_IO);
            }
        }
        Ok(())
    }

    /// Gets the controller ready and hands it to the kernel.
    fn setup(&'static self, vendor: u16, product: u16) -> bool {
        if self.command(OP_RESET, &[]).is_err() {
            justdrv::warn!("controller does not answer HCI_Reset");
            return false;
        }
        let Ok(v) = self.command(OP_READ_LOCAL_VERSION, &[]) else { return false };
        if v.len() < 9 || v[0] != 0 {
            return false;
        }
        let (hci_ver, hci_rev, manufacturer, lmp_subver) = (v[1], le16(&v, 2).unwrap_or(0), le16(&v, 5).unwrap_or(0), le16(&v, 7).unwrap_or(0));
        let mut name = String::new();
        if manufacturer == MANUFACTURER_REALTEK || vendor == 0x0BDA {
            name = format!("Realtek {}", self.realtek_setup(hci_ver, hci_rev, lmp_subver).unwrap_or("Bluetooth"));
        }
        if name.is_empty() {
            name = format!("Bluetooth {vendor:04x}:{product:04x}");
        }
        let mut n = [0u8; 40];
        let len = name.len().min(39);
        n[..len].copy_from_slice(&name.as_bytes()[..len]);
        let desc: &'static HciDesc = Box::leak(Box::new(HciDesc { ctx: self as *const Bt as *mut c_void, send: hci_send, name: n }));
        let adapter = justdrv::hci_register(self.device, desc);
        if adapter < 0 {
            justdrv::warn!("the kernel refused the controller ({})", justdrv::status_name(adapter));
            return false;
        }
        self.adapter.store(adapter, Ordering::Release);
        justdrv::info!("hci{adapter}: {name} (Bluetooth {})", version_name(hci_ver));
        true
    }
}

fn version_name(hci_ver: u8) -> &'static str {
    match hci_ver {
        6 => "4.0",
        7 => "4.1",
        8 => "4.2",
        9 => "5.0",
        10 => "5.1",
        11 => "5.2",
        12 => "5.3",
        13 => "5.4",
        14 => "6.0",
        _ => "?",
    }
}

/// Sets the controller up, then reads ACL data from the bulk endpoint
/// until the device goes.
unsafe extern "C" fn worker(ctx: *mut c_void) {
    // SAFETY: the leaked Bt.
    let bt = unsafe { &*(ctx as *const Bt) };
    let ids = IDS.lock().iter().find(|(p, _, _)| *p == ctx as usize).map(|(_, v, d)| (*v, *d)).unwrap_or((0, 0));
    if bt.setup(ids.0, ids.1) {
        let mut buf = alloc::vec![0u8; bt.max_in as usize];
        while !bt.dead.load(Ordering::Acquire) {
            // SAFETY: the interface stays valid while the device exists;
            // afterwards the calls fail with E_GONE.
            let n = unsafe { (bt.iface.transfer)(bt.iface.ctx, bt.ep_in, buf.as_mut_ptr(), buf.len() as u32, 1000) };
            match n {
                n if n > 0 => bt.acl_data(&buf[..n as usize]),
                0 | E_TIMEOUT => {}
                E_GONE => break,
                _ => {
                    // SAFETY: as above.
                    unsafe { (bt.iface.clear_halt)(bt.iface.ctx, bt.ep_in) };
                    justdrv::sleep_ms(50);
                }
            }
        }
    }
    bt.stopped.store(true, Ordering::Release);
}

/// The USB ids of each device, for the worker.
static IDS: IrqLock<Vec<(usize, u16, u16)>> = IrqLock::new(Vec::new());

fn probe(dev: &DeviceInfo) -> Status {
    if dev.interface == 0 || dev.class != CLASS_WIRELESS {
        return E_NODEV;
    }
    // SAFETY: the host controller driver keeps the interface alive while
    // the device exists.
    let iface: &'static UsbInterface = unsafe { &*(dev.interface as *const UsbInterface) };
    // SAFETY: as above.
    let config = unsafe { iface.config() };
    let eps: Vec<EndpointDescriptor> = endpoints(config, iface.number, 0).collect();
    let event = eps.iter().find(|e| e.is_in() && e.kind() == EP_INTERRUPT);
    let bulk_in = eps.iter().find(|e| e.is_in() && e.kind() == EP_BULK);
    let bulk_out = eps.iter().find(|e| !e.is_in() && e.kind() == EP_BULK);
    // The other interfaces (isochronous voice) have none of these.
    let (Some(event), Some(bulk_in), Some(bulk_out)) = (event, bulk_in, bulk_out) else { return E_NODEV };
    let bt: &'static Bt = Box::leak(Box::new(Bt {
        device: dev.id,
        iface,
        ep_event: event.address,
        ep_in: bulk_in.address,
        ep_out: bulk_out.address,
        max_in: bulk_in.packet_size().max(8),
        max_event: event.packet_size().max(8),
        partial: IrqLock::new(Vec::new()),
        acl: IrqLock::new(Vec::new()),
        setup_events: IrqLock::new(VecDeque::new()),
        setup_wake: Event::new(),
        adapter: AtomicI32::new(-1),
        out: Mutex::new(()),
        dead: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    }));
    let ctx = bt as *const Bt as *mut c_void;
    // SAFETY: the interface's own function; `bt` is never freed.
    let st = unsafe { (iface.poll_start)(iface.ctx, bt.ep_event, event_report, ctx) };
    if st != OK {
        return st;
    }
    IDS.lock().push((ctx as usize, dev.vendor, dev.device));
    DEVICES.lock().push((dev.id, ctx as usize));
    if justdrv::spawn("btusb", worker, ctx) < 0 {
        // SAFETY: as above.
        unsafe { (iface.poll_stop)(iface.ctx, bt.ep_event) };
        return E_IO;
    }
    OK
}

fn remove(device: u32) {
    let ptr = {
        let mut d = DEVICES.lock();
        let Some(i) = d.iter().position(|(id, _)| *id == device) else { return };
        d.remove(i).1
    };
    // SAFETY: the Bt leaked in probe.
    let bt = unsafe { &*(ptr as *const Bt) };
    bt.dead.store(true, Ordering::Release);
    bt.setup_wake.signal();
    // SAFETY: the interface stays valid during remove; stopping waits for a
    // running callback.
    unsafe { (bt.iface.poll_stop)(bt.iface.ctx, bt.ep_event) };
    let until = justdrv::time_ms() + 3000;
    while !bt.stopped.load(Ordering::Acquire) && justdrv::time_ms() < until {
        justdrv::sleep_ms(5);
    }
    IDS.lock().retain(|(p, _, _)| *p != ptr);
    // The Bt itself stays: the kernel may still hold the descriptor for a
    // moment, and a send then fails with E_GONE instead of touching freed
    // memory. It is a few hundred bytes per unplugged adapter.
}
