//! virtio GPU display driver (2D).
//!
//! The screen is a resource on the host backed by a framebuffer in guest
//! memory: justOS draws into that memory, and every change is copied to the
//! host and shown with two commands (transfer, flush). Commands go one at a
//! time through the control queue and are polled.
//!
//! The device has up to 16 outputs (scanouts), one per monitor. They all
//! show the same resource, each its own rectangle of it: monitors side by
//! side are one wide framebuffer, a mirror is the same rectangle twice. A
//! thread watches the device's configuration for monitors coming and going
//! (the host's windows opened or closed) and tells the kernel.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use justdrv::{
    DeviceInfo, DisplayDesc, DisplayHead, DisplayMode, DisplayOutput, Dma, Framebuffer, Mutex, Status, E_INVAL, E_IO, E_NODEV, E_NOMEM, OK,
    OUTPUT_CONNECTED,
};
use justvirtio::{Queue, Virtio, DESC_NEXT, DESC_WRITE};
use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

justdrv::driver! {
    name: "virtiogpu",
    version: "1.2.0",
    probe: probe,
}

const GET_DISPLAY_INFO: u32 = 0x0100;
const RESOURCE_CREATE_2D: u32 = 0x0101;
const RESOURCE_UNREF: u32 = 0x0102;
const SET_SCANOUT: u32 = 0x0103;
const RESOURCE_FLUSH: u32 = 0x0104;
const TRANSFER_TO_HOST_2D: u32 = 0x0105;
const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

/// Blue, green, red, unused: 0x00RRGGBB in a little-endian word.
const FORMAT_B8G8R8X8: u32 = 2;
const HEADER: usize = 24;
/// The largest monitor, and the largest screen of several side by side.
const MAX_SIZE: (u32, u32) = (3840, 2160);
const MAX_SCREEN: (u32, u32) = (7680, 4320);
/// Outputs in a display info answer.
const MAX_OUTPUTS: usize = 16;
/// The device's configuration: events to read and to clear, outputs.
const CONFIG_EVENTS_READ: usize = 0;
const CONFIG_EVENTS_CLEAR: usize = 4;
const CONFIG_SCANOUTS: usize = 8;
/// The event of a monitor attached or taken away.
const EVENT_DISPLAY: u32 = 1;
/// How often the thread looks for monitors coming and going (ms).
const WATCH_MS: u64 = 500;
/// The PCI command register's "interrupt disable" bit.
const PCI_INTX_OFF: u16 = 1 << 10;

const MODES: &[(u32, u32)] = &[
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1280, 1024),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
];

struct Screen {
    resource: u32,
    width: u32,
    height: u32,
    memory: Option<Dma>,
    /// What each monitor shows: its output and a rectangle of the screen.
    heads: Vec<DisplayHead>,
}

struct Gpu {
    vio: Virtio,
    /// The control queue, its command page and the current screen.
    state: Mutex<(Queue, Dma, Screen)>,
    /// The host's preferred size for the first output.
    preferred: (u32, u32),
    /// The features agreed on (again after sleep).
    features: u64,
    /// How many outputs the device has.
    scanouts: u32,
    /// The kernel's number for this display (for monitors coming and
    /// going), once registered.
    display: AtomicI32,
}

impl Gpu {
    /// Shows `screen` on its monitors: a resource of its size on the host,
    /// backed by its memory, and each head's rectangle of it on that head's
    /// output. Outputs without a head go dark.
    fn show(&self, q: &mut Queue, page: &Dma, screen: &Screen) -> bool {
        let Some(memory) = &screen.memory else { return false };
        let (id, w, h) = (screen.resource, screen.width, screen.height);
        let made = self.ok(q, page, RESOURCE_CREATE_2D, |c| {
            c.u32(id).u32(FORMAT_B8G8R8X8).u32(w).u32(h);
        }) && self.ok(q, page, RESOURCE_ATTACH_BACKING, |c| {
            c.u32(id).u32(1).u64(memory.phys()).u32(memory.len() as u32).u32(0);
        });
        if !made {
            return false;
        }
        let mut shown = true;
        for output in 0..self.scanouts {
            // A rectangle and resource 0 turn an output off.
            let (r, res) = match screen.heads.iter().find(|h| h.output == output) {
                Some(h) => ([h.x, h.y, h.width, h.height], id),
                None => ([0; 4], 0),
            };
            let ok = self.ok(q, page, SET_SCANOUT, |c| {
                c.u32(r[0]).u32(r[1]).u32(r[2]).u32(r[3]).u32(output).u32(res);
            });
            shown &= ok || res == 0;
        }
        shown
    }

    /// The outputs as the host sees them: for each, its preferred size and
    /// whether a monitor (a window of the host's) shows it.
    fn outputs(&self) -> Vec<DisplayOutput> {
        let mut st = self.state.lock();
        let (q, page, _) = &mut *st;
        let n = (self.scanouts as usize).min(MAX_OUTPUTS);
        if self.command(q, page, GET_DISPLAY_INFO, |_| {}) != RESP_OK_DISPLAY_INFO {
            return alloc::vec![DisplayOutput::default(); n];
        }
        // Each output: a rectangle (x, y, width, height), enabled, flags.
        (0..n)
            .map(|i| {
                let at = 2048 + HEADER + i * 24;
                let enabled = page.read::<u32>(at + 16) != 0;
                DisplayOutput {
                    width: page.read::<u32>(at + 8),
                    height: page.read::<u32>(at + 12),
                    flags: if enabled { OUTPUT_CONNECTED } else { 0 },
                    _pad: 0,
                }
            })
            .collect()
    }

    /// Makes a screen of `size` and shows it on `heads`; the old screen
    /// goes once the new one shows.
    fn set_screen(&self, size: DisplayMode, heads: Vec<DisplayHead>, fb: &mut Framebuffer) -> Status {
        let (w, h) = (size.width, size.height);
        let fits = |hd: &DisplayHead| {
            hd.output < self.scanouts
                && hd.width >= 640
                && hd.height >= 480
                && hd.width <= MAX_SIZE.0
                && hd.height <= MAX_SIZE.1
                && hd.x.checked_add(hd.width).is_some_and(|r| r <= w)
                && hd.y.checked_add(hd.height).is_some_and(|b| b <= h)
        };
        if w > MAX_SCREEN.0 || h > MAX_SCREEN.1 || heads.is_empty() || !heads.iter().all(fits) {
            return E_INVAL;
        }
        let Some(memory) = Dma::new(w as usize * h as usize * 4) else { return E_NOMEM };
        let mut st = self.state.lock();
        let (q, page, screen) = &mut *st;
        let (phys, virt) = (memory.phys(), memory.as_ptr() as u64);
        let new = Screen { resource: screen.resource + 1, width: w, height: h, memory: Some(memory), heads };
        if !self.show(q, page, &new) {
            return E_IO;
        }
        if screen.resource != 0 {
            let old = screen.resource;
            self.ok(q, page, RESOURCE_UNREF, |c| {
                c.u32(old).u32(0);
            });
        }
        *fb = Framebuffer { phys, virt, width: w, height: h, pitch: w * 4, red: 16, green: 8, blue: 0, _pad: 0 };
        *screen = new;
        OK
    }
}

/// Before sleep: the command under way ends, then the device stops.
unsafe extern "C" fn suspend(ctx: *mut c_void) -> Status {
    // SAFETY: ctx is the leaked Gpu.
    let g = unsafe { &*(ctx as *const Gpu) };
    let _st = g.state.lock();
    g.vio.reset();
    OK
}

/// After sleep the host forgot the screen's resource: the device set up
/// again and the same screen (same memory) shown anew.
unsafe extern "C" fn resume(ctx: *mut c_void) -> Status {
    // SAFETY: ctx is the leaked Gpu.
    let g = unsafe { &*(ctx as *const Gpu) };
    let mut st = g.state.lock();
    let (q, page, screen) = &mut *st;
    if !g.vio.restore(g.features, &mut [&mut *q]) {
        return E_IO;
    }
    q.quiet();
    g.vio.ready();
    if screen.memory.is_none() {
        return OK;
    }
    if !g.show(q, page, screen) {
        return E_IO;
    }
    let (id, w, h) = (screen.resource, screen.width, screen.height);
    let whole = g.ok(q, page, TRANSFER_TO_HOST_2D, |c| {
        c.u32(0).u32(0).u32(w).u32(h).u64(0).u32(id).u32(0);
    }) && g.ok(q, page, RESOURCE_FLUSH, |c| {
        c.u32(0).u32(0).u32(w).u32(h).u32(id).u32(0);
    });
    if whole {
        OK
    } else {
        E_IO
    }
}

/// Writes little-endian words into a command buffer.
struct Cmd<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Cmd<'_> {
    fn u32(&mut self, v: u32) -> &mut Self {
        self.buf[self.len..self.len + 4].copy_from_slice(&v.to_le_bytes());
        self.len += 4;
        self
    }

    fn u64(&mut self, v: u64) -> &mut Self {
        self.buf[self.len..self.len + 8].copy_from_slice(&v.to_le_bytes());
        self.len += 8;
        self
    }
}

impl Gpu {
    /// Sends a command built by `build` (after its header) and waits for
    /// the answer. Returns the answer's type; the answer stays in the page
    /// at offset 2048.
    fn command(&self, q: &mut Queue, page: &Dma, kind: u32, build: impl FnOnce(&mut Cmd<'_>)) -> u32 {
        let buf = page.as_mut_slice();
        buf[..4096].fill(0);
        let mut c = Cmd { buf: &mut buf[..2048], len: 0 };
        c.u32(kind).u32(0).u64(0).u32(0).u32(0);
        build(&mut c);
        let len = c.len;
        q.desc(0, page.phys(), len as u32, DESC_NEXT, 1);
        q.desc(1, page.phys() + 2048, 2048, DESC_WRITE, 0);
        q.publish(0);
        self.vio.notify(q);
        if !justdrv::wait_until(1000, || q.take_used().is_some()) {
            return 0;
        }
        page.read::<u32>(2048)
    }

    fn ok(&self, q: &mut Queue, page: &Dma, kind: u32, build: impl FnOnce(&mut Cmd<'_>)) -> bool {
        self.command(q, page, kind, build) == RESP_OK_NODATA
    }
}

unsafe extern "C" fn modes(ctx: *mut c_void, out: *mut DisplayMode, max: usize) -> usize {
    // SAFETY: ctx is the leaked Gpu; out holds max modes.
    let g = unsafe { &*(ctx as *const Gpu) };
    let mut list: Vec<(u32, u32)> = MODES.to_vec();
    if g.preferred.0 >= 640 && !list.contains(&g.preferred) {
        list.push(g.preferred);
    }
    let n = list.len().min(max);
    for (i, &(width, height)) in list.iter().take(n).enumerate() {
        // SAFETY: i < max.
        unsafe { out.add(i).write(DisplayMode { width, height }) };
    }
    n
}

/// One monitor: the first output shows all of the screen.
unsafe extern "C" fn set_mode(ctx: *mut c_void, mode: DisplayMode, fb: *mut Framebuffer) -> Status {
    // SAFETY: ctx is the leaked Gpu; fb is valid for the call.
    let (g, fb) = unsafe { (&*(ctx as *const Gpu), &mut *fb) };
    let head = DisplayHead { output: 0, x: 0, y: 0, width: mode.width, height: mode.height };
    g.set_screen(mode, alloc::vec![head], fb)
}

unsafe extern "C" fn outputs(ctx: *mut c_void, out: *mut DisplayOutput, max: usize) -> usize {
    // SAFETY: ctx is the leaked Gpu; out holds max outputs.
    let g = unsafe { &*(ctx as *const Gpu) };
    let list = g.outputs();
    for (i, o) in list.iter().take(max).enumerate() {
        // SAFETY: i < max.
        unsafe { out.add(i).write(*o) };
    }
    list.len()
}

unsafe extern "C" fn set_heads(ctx: *mut c_void, size: DisplayMode, heads: *const DisplayHead, count: usize, fb: *mut Framebuffer) -> Status {
    if heads.is_null() || count == 0 || count > MAX_OUTPUTS {
        return E_INVAL;
    }
    // SAFETY: ctx is the leaked Gpu; heads holds count heads and fb is
    // valid for the call.
    let (g, list, fb) = unsafe { (&*(ctx as *const Gpu), core::slice::from_raw_parts(heads, count), &mut *fb) };
    g.set_screen(size, list.to_vec(), fb)
}

/// Watches the device's configuration for monitors attached or taken
/// away. The device would raise an interrupt for it, but commands are
/// polled and nothing else needs one: a look twice a second is enough.
unsafe extern "C" fn watch(ctx: *mut c_void) {
    // SAFETY: ctx is the leaked Gpu.
    let g = unsafe { &*(ctx as *const Gpu) };
    loop {
        justdrv::sleep_ms(WATCH_MS);
        let events = g.vio.config32(CONFIG_EVENTS_READ);
        if events & EVENT_DISPLAY == 0 {
            continue;
        }
        g.vio.set_config32(CONFIG_EVENTS_CLEAR, events);
        let n = g.display.load(Ordering::Acquire);
        if n >= 0 {
            justdrv::info!("monitors changed");
            justdrv::display_changed(n as u32);
        }
    }
}

unsafe extern "C" fn flush(ctx: *mut c_void, x: u32, y: u32, w: u32, h: u32) {
    // SAFETY: ctx is the leaked Gpu.
    let g = unsafe { &*(ctx as *const Gpu) };
    let mut st = g.state.lock();
    let (q, page, screen) = &mut *st;
    if screen.memory.is_none() || x >= screen.width || y >= screen.height {
        return;
    }
    let (w, h) = (w.min(screen.width - x), h.min(screen.height - y));
    let (id, pitch) = (screen.resource, screen.width as u64 * 4);
    g.ok(q, page, TRANSFER_TO_HOST_2D, |c| {
        c.u32(x).u32(y).u32(w).u32(h).u64(y as u64 * pitch + x as u64 * 4).u32(id).u32(0);
    });
    g.ok(q, page, RESOURCE_FLUSH, |c| {
        c.u32(x).u32(y).u32(w).u32(h).u32(id).u32(0);
    });
}

fn probe(dev: &DeviceInfo) -> Status {
    let Some(vio) = Virtio::open(dev) else {
        justdrv::warn!("no usable virtio registers");
        return E_NODEV;
    };
    let Some(features) = vio.negotiate(0) else {
        return E_IO;
    };
    let (Some(q), Some(page)) = (vio.queue(0, 16), Dma::new(4096)) else {
        vio.set_status(justvirtio::STATUS_FAILED);
        return E_IO;
    };
    q.quiet();
    // Commands are polled and a thread watches the monitors: the device's
    // interrupt line stays down. A monitor attached or taken away (or a
    // host window resized) raises the configuration interrupt, and a line
    // nobody takes keeps the processor in interrupts.
    let pci = justdrv::Pci(dev.id);
    pci.write16(0x04, pci.read16(0x04) | PCI_INTX_OFF);
    vio.ready();
    let scanouts = vio.config32(CONFIG_SCANOUTS).clamp(1, MAX_OUTPUTS as u32);
    let mut g = Gpu {
        vio,
        state: Mutex::new((q, page, Screen { resource: 0, width: 0, height: 0, memory: None, heads: Vec::new() })),
        preferred: (0, 0),
        features,
        scanouts,
        display: AtomicI32::new(-1),
    };
    let list = g.outputs();
    for (i, o) in list.iter().enumerate() {
        justdrv::info!("output {i}: {}x{} preferred{}", o.width, o.height, if o.flags & OUTPUT_CONNECTED != 0 { "" } else { ", off" });
    }
    g.preferred = list.first().filter(|o| o.flags & OUTPUT_CONNECTED != 0).map_or((0, 0), |o| (o.width, o.height));
    let g: &'static Gpu = Box::leak(Box::new(g));
    let desc: &'static DisplayDesc = Box::leak(Box::new(DisplayDesc {
        ctx: g as *const Gpu as *mut c_void,
        modes,
        set_mode,
        flush: Some(flush),
        name: justdrv::model_name("virtio GPU"),
    }));
    let n = justdrv::display_register(dev.id, desc);
    if n < 0 {
        return n;
    }
    let ctx = g as *const Gpu as *mut c_void;
    if g.scanouts > 1 {
        justdrv::display_heads(n as u32, ctx, outputs, set_heads);
        g.display.store(n, Ordering::Release);
        justdrv::spawn("virtiogpu-watch", watch, ctx);
    }
    justdrv::power_hooks(dev.id, ctx, Some(suspend), resume);
    OK
}
