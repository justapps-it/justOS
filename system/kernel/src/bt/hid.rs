//! HID reports from Bluetooth keyboards and mice into the input system, as
//! the usbhid driver does it for USB: keys pressed and released by
//! comparing reports, key repeat, motion, wheel and buttons.

use super::ST;
use alloc::vec::Vec;
use justdk::hid::{self, Field};
use justdk::{ABS_MAX, AXIS_WHEEL, AXIS_X, AXIS_Y, INPUT_ABS, INPUT_BUTTON, INPUT_KEY, INPUT_REL, KEY_PRESSED, KEY_RELEASED, KEY_REPEAT};

const REPEAT_DELAY_MS: u64 = 500;
const REPEAT_EVERY_MS: u64 = 33;

/// Report descriptors of the boot protocol, for devices without one.
pub const BOOT_KEYBOARD: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x75, 0x01, 0x95, 0x08, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0xff, 0x05, 0x07, 0x19, 0x00, 0x29, 0xff, 0x81, 0x00,
    0xc0,
];
pub const BOOT_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03,
    0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08,
    0x95, 0x02, 0x81, 0x06, 0xc0, 0xc0,
];

/// The input side of a connected HID device.
pub struct HidIn {
    fields: Vec<Field>,
    /// Boot protocol: reports start with a report id the descriptor has not.
    boot: bool,
    held: Vec<u8>,
    buttons: u8,
    repeat: Option<(u8, u64)>,
}

impl HidIn {
    pub fn new(report_map: &[u8], boot: bool) -> HidIn {
        HidIn { fields: hid::parse(report_map), boot, held: Vec::new(), buttons: 0, repeat: None }
    }

    pub fn uses_ids(&self) -> bool {
        hid::uses_report_ids(&self.fields)
    }
}

/// An input event for the input system: (kind, code, value).
type Ev = (u32, u32, i32);

/// The device number input events carry: Bluetooth's own.
const DEVICE: u32 = 0xB7_0000;

fn emit(evs: Vec<Ev>) {
    for (kind, code, value) in evs {
        crate::dev::input::driver_event(DEVICE, kind, code, value);
    }
}

/// A report from the device on link `handle`.
pub fn report(handle: u16, data: &[u8]) {
    let evs = {
        let mut st = ST.lock();
        let Some(h) = st.conns.iter_mut().find(|c| c.handle == handle).and_then(|c| c.hid.as_mut()) else { return };
        decode(h, data)
    };
    emit(evs);
}

fn decode(h: &mut HidIn, data: &[u8]) -> Vec<Ev> {
    let data = if h.boot { data.get(1..).unwrap_or(&[]) } else { data };
    let r = hid::decode(&h.fields, data, ABS_MAX);
    let mut out = Vec::new();
    if r.has_keys {
        let now = crate::time::uptime_ms();
        let pressed: Vec<u8> = r.keys.iter().copied().filter(|c| !h.held.contains(c)).collect();
        let released: Vec<u8> = h.held.iter().copied().filter(|c| !r.keys.contains(c)).collect();
        for &c in &pressed {
            if c < 0xE0 {
                h.repeat = Some((c, now + REPEAT_DELAY_MS));
            }
        }
        if h.repeat.is_some_and(|(c, _)| released.contains(&c)) {
            h.repeat = None;
        }
        h.held = r.keys.clone();
        out.extend(released.into_iter().map(|c| (INPUT_KEY, c as u32, KEY_RELEASED)));
        out.extend(pressed.into_iter().map(|c| (INPUT_KEY, c as u32, KEY_PRESSED)));
    }
    if r.has_pointer {
        if r.dx != 0 {
            out.push((INPUT_REL, AXIS_X, r.dx));
        }
        if r.dy != 0 {
            out.push((INPUT_REL, AXIS_Y, r.dy));
        }
        if r.wheel != 0 {
            out.push((INPUT_REL, AXIS_WHEEL, -r.wheel));
        }
        if let Some(x) = r.abs_x {
            out.push((INPUT_ABS, AXIS_X, x));
        }
        if let Some(y) = r.abs_y {
            out.push((INPUT_ABS, AXIS_Y, y));
        }
        let changed = h.buttons ^ r.buttons;
        h.buttons = r.buttons;
        for b in 0..3 {
            if changed & (1 << b) != 0 {
                out.push((INPUT_BUTTON, b, (r.buttons >> b & 1) as i32));
            }
        }
    }
    out
}

/// Repeats held keys; called every few milliseconds.
pub fn tick() {
    let now = crate::time::uptime_ms();
    let evs: Vec<Ev> = {
        let mut st = ST.lock();
        st.conns
            .iter_mut()
            .filter_map(|c| c.hid.as_mut())
            .filter_map(|h| match h.repeat {
                Some((c, at)) if now >= at && h.held.contains(&c) => {
                    h.repeat = Some((c, now + REPEAT_EVERY_MS));
                    Some((INPUT_KEY, c as u32, KEY_REPEAT))
                }
                _ => None,
            })
            .collect()
    };
    emit(evs);
}

/// A device went: whatever it held is let go.
pub fn release(h: Option<HidIn>) {
    let Some(h) = h else { return };
    let mut evs: Vec<Ev> = h.held.iter().map(|&c| (INPUT_KEY, c as u32, KEY_RELEASED)).collect();
    for b in 0..3 {
        if h.buttons & (1 << b) != 0 {
            evs.push((INPUT_BUTTON, b, 0));
        }
    }
    emit(evs);
}
