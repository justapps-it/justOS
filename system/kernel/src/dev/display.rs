//! Display controllers registered by drivers: the screen resolutions they
//! offer and switching between them.
//!
//! Without a driver the screen stays in the mode the loader chose. With one,
//! the graphical interface can pick another mode: the driver programs the
//! controller and describes the new framebuffer, which replaces the old one.
//!
//! A controller with several outputs (its driver called `display_heads`)
//! shows the screen on every monitor attached. They all get the same
//! resolution: side by side they make one framebuffer as wide as all of
//! them, each monitor showing its part; mirrored, each shows all of it.
//! [`monitors`] tells the interface where each one is. Until the desktop
//! arranges them ([`arrange`]) they mirror, so the splash and the
//! first-start setup show the same on all of them.

use crate::console::FbInfo;
use crate::sync::IrqLock;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use justdk::{DisplayDesc, DisplayHead, DisplayHeads, DisplayMode, DisplayOutput, Framebuffer, Status, E_INVAL, OK, OUTPUT_CONNECTED};

struct Controller {
    device: u32,
    name: String,
    desc: *const DisplayDesc,
    /// How it drives several monitors, if it can.
    heads: Option<DisplayHeads>,
}

/// A monitor: an output of the controller and the rectangle of the screen
/// it shows, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub output: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// How the user wants several monitors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// Outputs from left to right; the ones not named come after, in the
    /// controller's order.
    pub order: Vec<u32>,
    /// Every monitor shows the same picture.
    pub mirror: bool,
}

/// What the screen is now: the resolution of each monitor and what each
/// output shows.
struct Layout {
    mode: DisplayMode,
    heads: Vec<DisplayHead>,
}

static LAYOUT: IrqLock<Option<Layout>> = IrqLock::new(None);
static ARRANGEMENT: IrqLock<Arrangement> = IrqLock::new(Arrangement { order: Vec::new(), mirror: true });
/// A driver said a monitor came or went.
static CHANGED: AtomicBool = AtomicBool::new(false);

// SAFETY: the descriptor belongs to the driver; calls come from the
// interface thread (and the shell), one at a time.
unsafe impl Send for Controller {}

static CONTROLLERS: IrqLock<Vec<Option<Controller>>> = IrqLock::new(Vec::new());
/// The controller whose framebuffer is on the screen, if it needs to be told
/// about changes.
static FLUSH: IrqLock<Option<DescPtr>> = IrqLock::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
struct DescPtr(*const DisplayDesc);
// SAFETY: see Controller.
unsafe impl Send for DescPtr {}

pub fn register(device: u32, desc: *const DisplayDesc) -> Status {
    // SAFETY: the driver keeps the descriptor alive.
    let d = unsafe { &*desc };
    let len = d.name.iter().position(|&b| b == 0).unwrap_or(d.name.len());
    let name = String::from(core::str::from_utf8(&d.name[..len]).unwrap_or("display"));
    let mut c = CONTROLLERS.lock();
    let n = c.len();
    crate::info!("display", "display{n}: {name}");
    c.push(Some(Controller { device, name, desc, heads: None }));
    n as Status
}

/// Display `display` drives several monitors.
pub fn register_heads(display: u32, heads: DisplayHeads) -> Status {
    let mut c = CONTROLLERS.lock();
    let Some(ctl) = c.get_mut(display as usize).and_then(|s| s.as_mut()) else { return E_INVAL };
    ctl.heads = Some(heads);
    crate::info!("display", "display{display}: several monitors");
    OK
}

/// A monitor came or went on display `display`: the interface lays the
/// screen out again (from its own thread: the driver's calls are not made
/// from the thread that reports it).
pub fn outputs_changed(_display: u32) {
    CHANGED.store(true, Ordering::Release);
    crate::dev::input::wake_gui_soon();
}

pub fn remove_device(device: u32) {
    let mut c = CONTROLLERS.lock();
    for slot in c.iter_mut() {
        if let Some(ctl) = slot.as_ref().filter(|ctl| ctl.device == device) {
            let mut f = FLUSH.lock();
            if *f == Some(DescPtr(ctl.desc)) {
                *f = None;
            }
            *slot = None;
        }
    }
}

fn first() -> Option<(*const DisplayDesc, String)> {
    CONTROLLERS.lock().iter().flatten().next().map(|c| (c.desc, c.name.clone()))
}

fn first_heads() -> Option<DisplayHeads> {
    CONTROLLERS.lock().iter().flatten().next().and_then(|c| c.heads)
}

/// Name of the display controller, if a driver handles it.
pub fn controller() -> Option<String> {
    first().map(|(_, n)| n)
}

/// The outputs with a monitor attached, and the size each monitor prefers;
/// empty unless the controller drives several monitors.
pub fn outputs() -> Vec<(u32, DisplayOutput)> {
    let Some(heads) = first_heads() else { return Vec::new() };
    let mut out = alloc::vec![DisplayOutput::default(); 16];
    // SAFETY: the driver's own function with a buffer of 16 outputs.
    let n = unsafe { (heads.outputs)(heads.ctx, out.as_mut_ptr(), out.len()) }.min(out.len());
    out.truncate(n);
    out.into_iter().enumerate().filter(|(_, o)| o.flags & OUTPUT_CONNECTED != 0).map(|(i, o)| (i as u32, o)).collect()
}

/// How the monitors are to be arranged from the next [`set_mode`] on.
pub fn arrange(a: Arrangement) {
    *ARRANGEMENT.lock() = a;
}

pub fn arrangement() -> Arrangement {
    ARRANGEMENT.lock().clone()
}

/// The outputs with a monitor, from left to right as arranged.
pub fn order() -> Vec<u32> {
    let connected: Vec<u32> = outputs().iter().map(|o| o.0).collect();
    ordered(&ARRANGEMENT.lock(), &connected)
}

/// The monitors, left to right: the rectangle of the screen each shows,
/// in pixels. Mirrored monitors are one (the first output's); without a
/// driver, one monitor is the whole screen.
pub fn monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    if let Some(l) = &*LAYOUT.lock() {
        for h in &l.heads {
            let m = Monitor { output: h.output, x: h.x as i32, y: h.y as i32, w: h.width as i32, h: h.height as i32 };
            if !v.iter().any(|o| (o.x, o.y, o.w, o.h) == (m.x, m.y, m.w, m.h)) {
                v.push(m);
            }
        }
    }
    if v.is_empty() {
        let (w, h) = crate::console::pixel_size().unwrap_or((0, 0));
        v.push(Monitor { output: 0, x: 0, y: 0, w: w as i32, h: h as i32 });
    }
    v.sort_by_key(|m| (m.x, m.y));
    v
}

/// The outputs that show the screen, mirrored ones too.
pub fn shown_outputs() -> Vec<u32> {
    match &*LAYOUT.lock() {
        Some(l) => l.heads.iter().map(|h| h.output).collect(),
        None => alloc::vec![0],
    }
}

/// The resolution of each monitor (they all have the same), in pixels.
pub fn monitor_size() -> Option<(usize, usize)> {
    match &*LAYOUT.lock() {
        Some(l) => Some((l.mode.width as usize, l.mode.height as usize)),
        None => crate::console::pixel_size(),
    }
}

/// Whether the monitors attached are not the ones the screen is laid out
/// on (a driver said that something changed, and it did).
pub fn take_changed() -> bool {
    if !CHANGED.swap(false, Ordering::AcqRel) {
        return false;
    }
    let mut now: Vec<u32> = outputs().iter().map(|o| o.0).collect();
    let mut shown = shown_outputs();
    now.sort_unstable();
    shown.sort_unstable();
    crate::info!("display", "monitors on outputs {now:?} (shown: {shown:?})");
    !now.is_empty() && now != shown
}

/// The outputs of `connected` in the order `a` asks for.
fn ordered(a: &Arrangement, connected: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();
    for &o in a.order.iter().chain(connected) {
        if connected.contains(&o) && !v.contains(&o) {
            v.push(o);
        }
    }
    v
}

/// The screen for monitors of `mode` on `outputs`, left to right: side by
/// side, or each showing all of it.
fn layout(mode: DisplayMode, outputs: &[u32], mirror: bool) -> (DisplayMode, Vec<DisplayHead>) {
    let (w, h) = (mode.width, mode.height);
    let heads: Vec<DisplayHead> = outputs
        .iter()
        .enumerate()
        .map(|(i, &output)| DisplayHead { output, x: if mirror { 0 } else { i as u32 * w }, y: 0, width: w, height: h })
        .collect();
    let across = if mirror { 1 } else { heads.len().max(1) as u32 };
    (DisplayMode { width: across * w, height: h }, heads)
}

/// The modes the controller offers, largest first.
pub fn modes() -> Vec<DisplayMode> {
    let Some((desc, _)) = first() else { return Vec::new() };
    let mut out = alloc::vec![DisplayMode::default(); 64];
    // SAFETY: the driver's own function with a buffer of 64 modes.
    let n = unsafe { ((*desc).modes)((*desc).ctx, out.as_mut_ptr(), out.len()) }.min(out.len());
    out.truncate(n);
    out.retain(|m| m.width >= 640 && m.height >= 480);
    out.sort_by_key(|m| core::cmp::Reverse((m.width * m.height, m.width)));
    out.dedup();
    out
}

/// Switches every monitor to `mode`, arranged as [`arrange`] said. Returns
/// the new framebuffer, already installed for the console.
pub fn set_mode(mode: DisplayMode) -> Result<FbInfo, &'static str> {
    let (desc, _) = first().ok_or("no display driver")?;
    if !modes().contains(&mode) {
        return Err("the display does not offer this resolution");
    }
    let mut fb = Framebuffer::default();
    let one = DisplayHead { output: 0, x: 0, y: 0, width: mode.width, height: mode.height };
    let mut heads = alloc::vec![one];
    let mut status = -1;
    if let Some(dh) = first_heads() {
        let connected: Vec<u32> = outputs().iter().map(|o| o.0).collect();
        if !connected.is_empty() {
            let a = arrangement();
            let (size, list) = layout(mode, &ordered(&a, &connected), a.mirror);
            // SAFETY: the driver's own function with valid heads and out
            // pointer.
            status = unsafe { (dh.set_heads)(dh.ctx, size, list.as_ptr(), list.len(), &mut fb) };
            if status >= 0 {
                heads = list;
            } else {
                crate::warn!("display", "{} monitors of {}x{}: refused", list.len(), mode.width, mode.height);
            }
        }
    }
    if status < 0 {
        // One monitor, as with any controller.
        // SAFETY: the driver's own function with a valid out pointer.
        status = unsafe { ((*desc).set_mode)((*desc).ctx, mode, &mut fb) };
    }
    let covered = heads.iter().all(|h| h.x + h.width <= fb.width && h.y + h.height <= fb.height);
    if status < 0 || fb.width < 640 || fb.height < 480 || fb.pitch < fb.width * 4 || !covered {
        return Err("the display refused the resolution");
    }
    let virt = if fb.virt != 0 {
        fb.virt
    } else {
        let size = fb.pitch as u64 * fb.height as u64;
        crate::mem::region::map_device(fb.phys, size, crate::mem::paging::WRITE_COMBINE).ok_or("cannot map the framebuffer")?
    };
    let info =
        FbInfo { virt, width: fb.width as usize, height: fb.height as usize, pitch: fb.pitch as usize, shift: (fb.red, fb.green, fb.blue) };
    // SAFETY: the driver's descriptor stays valid while it is registered.
    *FLUSH.lock() = unsafe { (*desc).flush.is_some() }.then_some(DescPtr(desc));
    crate::console::set_framebuffer(info);
    DRIVER_MODE.store(true, core::sync::atomic::Ordering::Release);
    if heads.len() > 1 {
        let outs: Vec<String> = heads.iter().map(|h| alloc::format!("{}@{},{}", h.output, h.x, h.y)).collect();
        crate::info!("display", "{}x{} on {} monitors ({})", fb.width, fb.height, heads.len(), outs.join(" "));
    } else {
        crate::info!("display", "{}x{}", fb.width, fb.height);
    }
    *LAYOUT.lock() = Some(Layout { mode, heads });
    Ok(info)
}

/// A driver set the mode (the screen is no longer in the firmware's).
static DRIVER_MODE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// After sleep. Drivers set their own mode again, but not one the firmware
/// chose at boot, and the firmware does not set it up again on waking: the
/// driver is asked for the same resolution.
pub fn after_sleep() {
    if DRIVER_MODE.load(core::sync::atomic::Ordering::Acquire) || first().is_none() {
        return;
    }
    let Some((w, h)) = monitor_size() else { return };
    let mode = DisplayMode { width: w as u32, height: h as u32 };
    if let Err(e) = set_mode(mode) {
        crate::warn!("display", "{w}x{h} after sleep: {e}");
    }
}

/// Tells the controller that a rectangle of the screen changed (only
/// controllers that need it).
pub fn flush(x: i32, y: i32, w: i32, h: i32) {
    let Some(DescPtr(desc)) = *FLUSH.lock() else { return };
    if w <= 0 || h <= 0 || !super::pm::awake() {
        return;
    }
    // SAFETY: the driver's own function, while it is registered.
    unsafe {
        if let Some(f) = (*desc).flush {
            f((*desc).ctx, x.max(0) as u32, y.max(0) as u32, w as u32, h as u32);
        }
    }
}
