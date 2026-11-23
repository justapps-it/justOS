//! The binary interface between the kernel and driver modules.
//!
//! Rules that keep the ABI stable:
//! * every type is `#[repr(C)]` and only uses fixed-size integers, raw
//!   pointers and `extern "C"` function pointers;
//! * [`KernelApi`] only ever grows at the end; its `size` field tells a module
//!   which entries exist, so new kernels run old modules;
//! * an incompatible change bumps [`ABI_VERSION`].

use core::ffi::c_void;

/// Current version of the driver ABI.
pub const ABI_VERSION: u32 = 2;

/// `DriverInfo::magic`: "ARDV".
pub const DRIVER_MAGIC: u32 = 0x5644_5241;

/// Name of the symbol every driver module exports as its ELF entry point.
pub const ENTRY_SYMBOL: &str = "just_driver_entry";

/// Signature of a module's entry point.
pub type DriverEntry = unsafe extern "C" fn(api: *const KernelApi) -> *const DriverInfo;

// ----------------------------------------------------------------------------
// Status codes

/// Result of a kernel or driver call: 0 or positive is success.
pub type Status = i32;

pub const OK: Status = 0;
/// `probe`: the device is not handled by this driver.
pub const E_NODEV: Status = -1;
pub const E_IO: Status = -2;
pub const E_NOMEM: Status = -3;
pub const E_INVAL: Status = -4;
pub const E_UNSUPPORTED: Status = -5;
pub const E_TIMEOUT: Status = -6;
pub const E_BUSY: Status = -7;
/// USB: the endpoint stalled (the device refused the request).
pub const E_STALL: Status = -8;
/// The device was unplugged.
pub const E_GONE: Status = -9;

/// Short human-readable name of a status code.
pub fn status_name(status: Status) -> &'static str {
    match status {
        OK => "ok",
        E_NODEV => "no such device",
        E_IO => "I/O error",
        E_NOMEM => "out of memory",
        E_INVAL => "invalid argument",
        E_UNSUPPORTED => "not supported",
        E_TIMEOUT => "timed out",
        E_BUSY => "busy",
        E_STALL => "stalled",
        E_GONE => "device gone",
        _ => "error",
    }
}

// ----------------------------------------------------------------------------
// Strings

/// A borrowed UTF-8 string that crosses the ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub ptr: *const u8,
    pub len: usize,
}

impl Str {
    pub const fn new(s: &'static str) -> Str {
        Str { ptr: s.as_ptr(), len: s.len() }
    }

    pub const fn empty() -> Str {
        Str { ptr: core::ptr::null(), len: 0 }
    }

    /// # Safety
    /// `ptr..ptr+len` must be valid UTF-8 that outlives the returned slice.
    pub unsafe fn as_str<'a>(&self) -> &'a str {
        if self.ptr.is_null() || self.len == 0 {
            return "";
        }
        // SAFETY: guaranteed by the caller.
        unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(self.ptr, self.len)) }
    }
}

// SAFETY: a Str only ever points at immutable, 'static or kernel-owned data.
unsafe impl Send for Str {}
unsafe impl Sync for Str {}

// ----------------------------------------------------------------------------
// Devices

pub const BUS_PCI: u32 = 1;
/// Fixed, non-enumerable hardware of the platform (PS/2, legacy ATA, ...).
pub const BUS_PLATFORM: u32 = 2;
/// A USB interface. `vendor`/`device` are the USB ids, `class`/`subclass`/
/// `prog_if` the interface class, subclass and protocol, `name` the port
/// path ("3-1.2:0": controller 3, root port 1, hub port 2, interface 0) and
/// `interface` points at a [`crate::usb::UsbInterface`].
pub const BUS_USB: u32 = 3;
/// A memory card in a card reader slot: `name` is the slot ("00:07.0-0":
/// reader at PCI 00:07.0, slot 0). The reader's driver owns it.
pub const BUS_SD: u32 = 4;

/// `device_add` flag: the parent driver handles the device itself (a USB
/// hub, for instance); the kernel does not look for a driver.
pub const DEVICE_OWNED: u32 = 1;

pub const RES_NONE: u32 = 0;
pub const RES_MEMORY: u32 = 1;
pub const RES_IO: u32 = 2;

/// A memory or I/O port range of a device (a PCI BAR, or a fixed range).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Resource {
    pub kind: u32,
    pub flags: u32,
    pub base: u64,
    pub size: u64,
}

/// `Resource::flags`: prefetchable memory.
pub const RES_PREFETCH: u32 = 1;
/// `Resource::flags`: 64-bit memory BAR.
pub const RES_64BIT: u32 = 2;

/// No interrupt line.
pub const IRQ_NONE: u8 = 0xFF;

/// Everything the kernel knows about a device, passed to `probe`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceInfo {
    /// Kernel handle of the device, used in every device-related call.
    pub id: u32,
    pub bus: u32,
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    /// Legacy interrupt line (PIC IRQ), or [`IRQ_NONE`].
    pub irq: u8,
    pub _pad: [u8; 3],
    /// PCI location: bus << 8 | slot << 3 | function.
    pub location: u32,
    pub resources: [Resource; 6],
    /// Platform device id ("ps2-keyboard") or USB port path, NUL-padded.
    /// Empty on PCI.
    pub name: [u8; 32],
    /// Device that found this one (a USB controller), 0 for none.
    pub parent: u32,
    pub _reserved: u32,
    /// Bus-specific interface the parent driver offers to the device's
    /// driver (for USB a `*const UsbInterface`), 0 for none.
    pub interface: u64,
}

impl DeviceInfo {
    pub const fn empty() -> DeviceInfo {
        DeviceInfo {
            id: 0,
            bus: 0,
            vendor: 0,
            device: 0,
            subsystem_vendor: 0,
            subsystem: 0,
            class: 0,
            subclass: 0,
            prog_if: 0,
            revision: 0,
            irq: IRQ_NONE,
            _pad: [0; 3],
            location: 0,
            resources: [Resource { kind: 0, flags: 0, base: 0, size: 0 }; 6],
            name: [0; 32],
            parent: 0,
            _reserved: 0,
            interface: 0,
        }
    }

    /// The platform id as a string.
    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = [0; 32];
        let n = name.len().min(31);
        self.name[..n].copy_from_slice(&name.as_bytes()[..n]);
    }

    /// First I/O port resource at or after `index`.
    pub fn io(&self, index: usize) -> Option<u16> {
        self.resources.iter().filter(|r| r.kind == RES_IO).nth(index).map(|r| r.base as u16)
    }
}

// ----------------------------------------------------------------------------
// Block devices

/// `BlockDesc::flags`: the medium is read-only.
pub const BLOCK_READONLY: u32 = 1;
/// `BlockDesc::flags`: the medium can be removed (USB stick, CD).
pub const BLOCK_REMOVABLE: u32 = 2;

pub type BlockIo = unsafe extern "C" fn(ctx: *mut c_void, lba: u64, count: u32, buf: *mut u8) -> Status;
pub type BlockFlush = unsafe extern "C" fn(ctx: *mut c_void) -> Status;

/// A disk a driver hands to the kernel. The kernel serialises calls per disk.
#[repr(C)]
pub struct BlockDesc {
    pub ctx: *mut c_void,
    pub sector_size: u32,
    pub flags: u32,
    pub sector_count: u64,
    /// Reads `count` sectors starting at `lba` into `buf`.
    pub read: BlockIo,
    /// Writes `count` sectors starting at `lba` from `buf`.
    pub write: BlockIo,
    pub flush: Option<BlockFlush>,
    /// Model name, NUL-padded.
    pub model: [u8; 40],
}

// ----------------------------------------------------------------------------
// Input

/// Key press/release: `code` is a USB HID usage (page 7), `value` one of
/// [`KEY_RELEASED`], [`KEY_PRESSED`], [`KEY_REPEAT`].
pub const INPUT_KEY: u32 = 1;
/// Relative motion: `code` is [`AXIS_X`], [`AXIS_Y`] or [`AXIS_WHEEL`].
pub const INPUT_REL: u32 = 2;
/// Pointer button: `code` 0 = left, 1 = right, 2 = middle; `value` 1/0.
pub const INPUT_BUTTON: u32 = 3;
/// Absolute position (tablets, touch screens): `code` is [`AXIS_X`] or
/// [`AXIS_Y`], `value` goes from 0 to [`ABS_MAX`] across the screen.
pub const INPUT_ABS: u32 = 4;

pub const ABS_MAX: i32 = 0x7FFF;

pub const KEY_RELEASED: i32 = 0;
pub const KEY_PRESSED: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

pub const AXIS_X: u32 = 0;
pub const AXIS_Y: u32 = 1;
pub const AXIS_WHEEL: u32 = 2;

// ----------------------------------------------------------------------------
// Network

pub type NetSend = unsafe extern "C" fn(ctx: *mut c_void, frame: *const u8, len: usize) -> Status;

/// A network interface a driver hands to the kernel. Received frames are
/// passed to `KernelApi::net_receive`.
#[repr(C)]
pub struct NetDesc {
    pub ctx: *mut c_void,
    pub mac: [u8; 6],
    pub _pad: [u8; 2],
    pub mtu: u32,
    pub send: NetSend,
}

// ----------------------------------------------------------------------------
// Wi-Fi

/// Sends one 802.11 frame (header and body, without the FCS) as it is: the
/// kernel builds and encrypts frames itself (software crypto, like mac80211
/// without hardware keys).
pub type WifiSend = unsafe extern "C" fn(ctx: *mut c_void, frame: *const u8, len: usize) -> Status;
/// Tunes the radio to a channel (1-14: 2.4 GHz, 36-177: 5 GHz, 20 MHz wide).
pub type WifiChannel = unsafe extern "C" fn(ctx: *mut c_void, channel: u32) -> Status;
/// The station joined a network (`link` describes it) or left it (null):
/// cards that filter by BSSID or keep power save state need to know.
pub type WifiSetLink = unsafe extern "C" fn(ctx: *mut c_void, link: *const WifiLink) -> Status;

/// The network a Wi-Fi station joined.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WifiLink {
    pub bssid: [u8; 6],
    pub channel: u8,
    pub ssid_len: u8,
    pub ssid: [u8; 32],
    pub aid: u16,
    pub _pad: [u8; 6],
}

/// A Wi-Fi radio a driver hands to the kernel: it sends and receives raw
/// 802.11 frames, and the kernel's station (scanning, authentication, WPA2
/// and WPA3, the data path) does the rest. Received frames are passed to
/// `KernelApi::wifi_receive`.
#[repr(C)]
pub struct WifiDesc {
    pub ctx: *mut c_void,
    pub mac: [u8; 6],
    pub _pad: [u8; 2],
    /// The channels the card may use; null for every channel justOS knows.
    pub channels: *const u8,
    pub channel_count: u32,
    pub _pad2: u32,
    pub send: WifiSend,
    pub set_channel: WifiChannel,
    pub set_link: WifiSetLink,
}

// ----------------------------------------------------------------------------
// Sound

/// Queues `frames` frames of interleaved signed 16-bit samples (`channels`
/// per frame, at `rate`). Blocks while the device's buffer is full; returns
/// [`OK`] once everything is queued.
pub type AudioWrite = unsafe extern "C" fn(ctx: *mut c_void, samples: *const i16, frames: usize) -> Status;
/// Plays what is queued to the end, then stops the device.
pub type AudioDrain = unsafe extern "C" fn(ctx: *mut c_void) -> Status;
/// Frames written and not played yet: how long the device takes to play a
/// frame written now. 0 while the device is stopped.
pub type AudioQueued = unsafe extern "C" fn(ctx: *mut c_void) -> usize;

/// A sound output a driver hands to the kernel. The kernel converts every
/// sound to the output's format and applies the volume itself; the driver
/// leaves its own gain at a fixed, sensible level. Calls are serialised.
#[repr(C)]
pub struct AudioDesc {
    pub ctx: *mut c_void,
    /// Frames per second.
    pub rate: u32,
    /// Samples per frame (2: left, right).
    pub channels: u32,
    pub write: AudioWrite,
    pub drain: AudioDrain,
    /// Name of the output ("Intel HD Audio, line out"), NUL-padded.
    pub name: [u8; 40],
}

/// Starts capturing (`on` 1: the device fills its buffer from now on) or
/// stops (`on` 0). Captured frames wait in the driver until read.
pub type AudioCapture = unsafe extern "C" fn(ctx: *mut c_void, on: u32) -> Status;
/// Copies up to `frames` captured frames (interleaved signed 16-bit,
/// `channels` per frame) into `samples`, the oldest first. Waits for some
/// to arrive, about one period at most; returns how many it copied (0: none
/// came, or the device is not capturing).
pub type AudioRead = unsafe extern "C" fn(ctx: *mut c_void, samples: *mut i16, frames: usize) -> Status;

/// A sound input (a microphone, a line in) a driver hands to the kernel.
/// The kernel converts what it reads to what programs ask for and applies
/// the input volume itself; the driver leaves its own gain at 0 dB. Calls
/// are serialised.
#[repr(C)]
pub struct CaptureDesc {
    pub ctx: *mut c_void,
    /// Frames per second.
    pub rate: u32,
    /// Samples per frame (1 or 2).
    pub channels: u32,
    pub capture: AudioCapture,
    pub read: AudioRead,
    /// Name of the input ("HD Audio, mic"), NUL-padded.
    pub name: [u8; 40],
}

// ----------------------------------------------------------------------------
// Displays

/// A screen resolution.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
}

/// The linear framebuffer of a mode: 32 bits per pixel.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Framebuffer {
    /// Physical address.
    pub phys: u64,
    /// Where the driver already maps it (memory it allocated), or 0 for the
    /// kernel to map device memory at `phys`.
    pub virt: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub pitch: u32,
    /// Bit positions of red, green and blue.
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub _pad: u8,
}

pub type DisplayModes = unsafe extern "C" fn(ctx: *mut c_void, out: *mut DisplayMode, max: usize) -> usize;
pub type DisplaySetMode = unsafe extern "C" fn(ctx: *mut c_void, mode: DisplayMode, fb: *mut Framebuffer) -> Status;
/// Shows a changed rectangle (for devices that do not scan the framebuffer
/// out by themselves).
pub type DisplayFlush = unsafe extern "C" fn(ctx: *mut c_void, x: u32, y: u32, w: u32, h: u32);

/// A display controller that can change the screen's resolution.
#[repr(C)]
pub struct DisplayDesc {
    pub ctx: *mut c_void,
    /// Fills `out` with up to `max` modes; returns how many it has.
    pub modes: DisplayModes,
    /// Switches to a mode and describes its framebuffer.
    pub set_mode: DisplaySetMode,
    pub flush: Option<DisplayFlush>,
    /// Name of the controller, NUL-padded.
    pub name: [u8; 40],
}

/// [`DisplayOutput::flags`]: a monitor is attached to the output.
pub const OUTPUT_CONNECTED: u32 = 1;

/// One of a controller's outputs: a connector a monitor can be attached to.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayOutput {
    /// The size the monitor prefers, or 0 x 0 when it does not say.
    pub width: u32,
    pub height: u32,
    /// [`OUTPUT_CONNECTED`].
    pub flags: u32,
    pub _pad: u32,
}

/// What one output shows: a rectangle of the framebuffer, in pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayHead {
    /// The output, as numbered by [`DisplayOutputs`].
    pub output: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Fills `out` with up to `max` outputs; returns how many the controller
/// has (connected or not).
pub type DisplayOutputs = unsafe extern "C" fn(ctx: *mut c_void, out: *mut DisplayOutput, max: usize) -> usize;
/// Makes one framebuffer of `size` and shows the `count` heads in `heads`
/// on their outputs (two heads may show the same rectangle: a mirror);
/// outputs without a head go dark. Describes the framebuffer in `fb`.
pub type DisplaySetHeads =
    unsafe extern "C" fn(ctx: *mut c_void, size: DisplayMode, heads: *const DisplayHead, count: usize, fb: *mut Framebuffer) -> Status;

/// A display controller that drives several monitors (see
/// `KernelApi::display_heads`). Without it a controller has one monitor,
/// which shows all of the framebuffer of `DisplayDesc::set_mode`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DisplayHeads {
    pub ctx: *mut c_void,
    pub outputs: DisplayOutputs,
    pub set_heads: DisplaySetHeads,
}

// SAFETY: the context belongs to the driver, which only uses it from its
// own functions.
unsafe impl Send for DisplayHeads {}

// ----------------------------------------------------------------------------
// Logging

pub const LOG_ERROR: u32 = 1;
pub const LOG_WARN: u32 = 2;
pub const LOG_INFO: u32 = 3;
pub const LOG_DEBUG: u32 = 4;

pub type IrqHandler = unsafe extern "C" fn(ctx: *mut c_void);
/// An event a thread can wait for (see `KernelApi::event_new`).
pub type Event = *mut c_void;
/// `event_wait`: wait without a time limit.
pub const WAIT_FOREVER: u64 = u64::MAX;
pub type ThreadEntry = unsafe extern "C" fn(ctx: *mut c_void);

/// The services the kernel offers a driver module.
#[repr(C)]
pub struct KernelApi {
    pub abi_version: u32,
    /// Size of this structure in bytes: entries past it do not exist.
    pub size: u32,

    pub log: unsafe extern "C" fn(level: u32, msg: *const u8, len: usize),

    // Memory
    pub alloc: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub free: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize),
    /// Maps device memory (uncached) and returns its virtual address.
    pub map_mmio: unsafe extern "C" fn(phys: u64, size: usize) -> *mut u8,
    /// Physically contiguous, zeroed, page-aligned memory below 4 GiB for DMA.
    pub dma_alloc: unsafe extern "C" fn(size: usize, phys: *mut u64) -> *mut u8,
    pub dma_free: unsafe extern "C" fn(ptr: *mut u8, size: usize),
    pub virt_to_phys: unsafe extern "C" fn(ptr: *const u8) -> u64,

    // PCI configuration space of a device (width 1, 2 or 4).
    pub pci_read: unsafe extern "C" fn(device: u32, offset: u16, width: u8) -> u32,
    pub pci_write: unsafe extern "C" fn(device: u32, offset: u16, width: u8, value: u32),

    // Interrupts
    pub irq_attach: unsafe extern "C" fn(device: u32, irq: u8, handler: IrqHandler, ctx: *mut c_void) -> Status,
    pub irq_detach: unsafe extern "C" fn(device: u32, irq: u8),

    // Time and threads
    pub time_ms: unsafe extern "C" fn() -> u64,
    pub sleep_ms: unsafe extern "C" fn(ms: u64),
    pub yield_now: unsafe extern "C" fn(),
    pub spawn: unsafe extern "C" fn(name: *const u8, name_len: usize, entry: ThreadEntry, ctx: *mut c_void) -> Status,

    // Device classes
    /// Registers a disk; returns its number (`disk<N>`) or an error.
    pub block_register: unsafe extern "C" fn(device: u32, desc: *const BlockDesc) -> Status,
    /// Reports an input event (see `INPUT_*`).
    pub input_event: unsafe extern "C" fn(device: u32, kind: u32, code: u32, value: i32),
    /// Registers a network interface; returns its number (`net<N>`).
    pub net_register: unsafe extern "C" fn(device: u32, desc: *const NetDesc) -> Status,
    /// Hands a received Ethernet frame to the kernel.
    pub net_receive: unsafe extern "C" fn(iface: u32, frame: *const u8, len: usize),

    // Message-signalled interrupts
    /// Routes the device's MSI-X or MSI to `handler(ctx)`. Returns the
    /// vector, or [`E_UNSUPPORTED`] if the device has neither (use the
    /// legacy line then). `irq_detach` with any line number releases it.
    pub irq_attach_msi: unsafe extern "C" fn(device: u32, handler: IrqHandler, ctx: *mut c_void) -> Status,

    // Events: a flag a thread sleeps on until another thread or an
    // interrupt handler sets it.
    pub event_new: unsafe extern "C" fn() -> Event,
    pub event_free: unsafe extern "C" fn(event: Event),
    /// Sets the event and wakes its waiter. Safe in interrupt handlers.
    pub event_signal: unsafe extern "C" fn(event: Event),
    /// Waits until the event is set (then clears it). Returns [`OK`] or
    /// [`E_TIMEOUT`] after `timeout_ms` (or never, for [`WAIT_FOREVER`]).
    pub event_wait: unsafe extern "C" fn(event: Event, timeout_ms: u64) -> Status,

    // Devices found by drivers (USB devices behind a controller)
    /// Adds a child of `parent`; returns its device id. The kernel looks for
    /// a driver in the background unless `flags` has [`DEVICE_OWNED`].
    pub device_add: unsafe extern "C" fn(parent: u32, info: *const DeviceInfo, description: Str, flags: u32) -> Status,
    /// The device (and its children) went away: their drivers are told,
    /// their disks and interfaces removed. Returns [`OK`] once no driver
    /// uses the device's `interface` any more, so the parent may free it;
    /// [`E_BUSY`] if a driver could not let go (keep the memory alive).
    /// Never call it from `probe`.
    pub device_remove: unsafe extern "C" fn(device: u32) -> Status,

    // Sound and displays
    /// Registers a sound output; returns its number.
    pub audio_register: unsafe extern "C" fn(device: u32, desc: *const AudioDesc) -> Status,
    /// Registers a display controller; returns its number.
    pub display_register: unsafe extern "C" fn(device: u32, desc: *const DisplayDesc) -> Status,
    /// Maps device memory write-combined (a framebuffer).
    pub map_framebuffer: unsafe extern "C" fn(phys: u64, size: usize) -> *mut u8,
    /// The framebuffer the firmware set up before the kernel started (UEFI
    /// GOP or VESA), already mapped: [`OK`], or [`E_NODEV`] without one.
    pub boot_framebuffer: unsafe extern "C" fn(out: *mut Framebuffer) -> Status,

    // Sleep (suspend to RAM)
    /// Tells the kernel how a device the driver owns goes to sleep and wakes
    /// up (see [`PowerHooks`]). The kernel copies `hooks`. A device with a
    /// driver but without hooks keeps the machine from sleeping.
    pub power_hooks: unsafe extern "C" fn(device: u32, hooks: *const PowerHooks) -> Status,

    // Several monitors
    /// Display `display` (the number `display_register` returned) drives
    /// several monitors (see [`DisplayHeads`]). The kernel copies `heads`.
    pub display_heads: unsafe extern "C" fn(display: u32, heads: *const DisplayHeads) -> Status,
    /// A monitor was attached to display `display` or taken away: the
    /// kernel asks for its outputs again and lays the screen out anew.
    /// Safe from any thread (not from interrupt handlers).
    pub display_changed: unsafe extern "C" fn(display: u32),
    // Sound timing
    /// Tells the kernel how many frames sound output `output` (the number
    /// `audio_register` returned) still has queued, called with the
    /// output's `ctx`. The kernel's mixer then keeps only a little queued
    /// (sounds start and stop at once) and tells programs exactly when what
    /// they write will be heard. Without it the kernel counts by the clock.
    pub audio_queued: unsafe extern "C" fn(output: u32, queued: AudioQueued) -> Status,
    // Recording
    /// Registers a sound input (see [`CaptureDesc`]); returns its number.
    /// The kernel starts it only while a program records.
    pub capture_register: unsafe extern "C" fn(device: u32, desc: *const CaptureDesc) -> Status,
    // Bluetooth
    /// Registers a Bluetooth controller (see [`HciDesc`]), once the driver
    /// has set it up (firmware loaded); returns its number (`hci<N>`). The
    /// kernel's Bluetooth stack then drives it with HCI commands.
    pub hci_register: unsafe extern "C" fn(device: u32, desc: *const HciDesc) -> Status,
    /// Hands a packet from controller `adapter` (the number `hci_register`
    /// returned) to the kernel: an event ([`HCI_EVENT`]) or ACL data
    /// ([`HCI_ACL`]), whole, without the H4 type byte. Safe from any
    /// thread and from the controller's callbacks.
    pub hci_receive: unsafe extern "C" fn(adapter: u32, kind: u32, data: *const u8, len: usize),
    // Firmware
    /// Reads firmware file `name` ("rtl_bt/rtl8852bu_fw.bin", from
    /// `sys:/system/firmware`): copies up to `cap` bytes into `buf` (none
    /// when `buf` is null) and returns the file's size, or [`E_NODEV`] when
    /// there is no such file.
    pub firmware_read: unsafe extern "C" fn(name: Str, buf: *mut u8, cap: usize) -> Status,
    // Wi-Fi
    /// Registers a Wi-Fi radio (see [`WifiDesc`]); returns its number
    /// (`wifi<N>`). The kernel adds a network interface for it.
    pub wifi_register: unsafe extern "C" fn(device: u32, desc: *const WifiDesc) -> Status,
    /// Hands a received 802.11 frame (without the FCS, which the card has
    /// checked) to the kernel, with the channel it came on and its signal
    /// strength in dBm. Safe in interrupt handlers.
    pub wifi_receive: unsafe extern "C" fn(radio: u32, frame: *const u8, len: usize, channel: u32, rssi: i32),
}

// ----------------------------------------------------------------------------
// Bluetooth

/// HCI packet types, as in the H4 transport.
pub const HCI_COMMAND: u32 = 1;
pub const HCI_ACL: u32 = 2;
pub const HCI_SCO: u32 = 3;
pub const HCI_EVENT: u32 = 4;

/// Sends one HCI packet to the controller: a command ([`HCI_COMMAND`],
/// opcode and parameters) or ACL data ([`HCI_ACL`], with its 4-byte
/// header). Blocks until the controller took it.
pub type HciSend = unsafe extern "C" fn(ctx: *mut c_void, kind: u32, data: *const u8, len: usize) -> Status;

/// A Bluetooth controller a driver hands to the kernel: the transport
/// (USB, UART) under the kernel's HCI. The kernel calls `send` from its
/// Bluetooth threads, never from an interrupt handler.
#[repr(C)]
pub struct HciDesc {
    pub ctx: *mut c_void,
    pub send: HciSend,
    /// Name of the controller ("Realtek RTL8852BU"), NUL-padded.
    pub name: [u8; 40],
}

// ----------------------------------------------------------------------------
// Sleep

/// A device's part in suspending the machine: [`OK`] or an error.
pub type PowerHook = unsafe extern "C" fn(ctx: *mut c_void) -> Status;

/// While the machine sleeps its devices lose power: memory is kept, but
/// every register is back to its reset value when it wakes up.
///
/// * `suspend` (optional) stops the device: it finishes or abandons what it
///   was doing and does no more DMA. An error keeps the machine awake.
/// * `resume` sets the device up again as `probe` left it, after the kernel
///   has restored its PCI configuration (BARs, command register, message
///   interrupts).
///
/// Both run in a kernel thread; the kernel holds back I/O to the device's
/// disks, network interfaces and sound outputs from before `suspend` until
/// after `resume`. Devices are suspended children first and resumed
/// parents first.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PowerHooks {
    pub ctx: *mut c_void,
    pub suspend: Option<PowerHook>,
    pub resume: PowerHook,
}

// SAFETY: the context belongs to the driver, which only uses it from its
// own hooks.
unsafe impl Send for PowerHooks {}

/// What a module tells the kernel about itself.
#[repr(C)]
pub struct DriverInfo {
    pub magic: u32,
    pub abi_version: u32,
    pub name: Str,
    pub version: Str,
    /// Called for every device whose manifest rule matched. Returns [`OK`]
    /// once the driver owns the device, [`E_NODEV`] to decline it.
    pub probe: unsafe extern "C" fn(device: *const DeviceInfo) -> Status,
    /// Releases a device before the module is unloaded. Without it the
    /// module can never be unloaded.
    pub remove: Option<unsafe extern "C" fn(device: u32)>,
}

// SAFETY: DriverInfo is immutable static data.
unsafe impl Sync for DriverInfo {}
