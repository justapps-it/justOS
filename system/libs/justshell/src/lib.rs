//! The desktop shell's logic, kept apart from the drawing so that it can be
//! tested on the host without a screen: the dynamic bar (one surface at the
//! bottom of the screen that becomes the dock, the control centre, the lock
//! keypad, the notifications...), its curves and timers, the windows'
//! geometry (open, resize from eight handles, maximise, full screen, genie)
//! and the bar's peek over a full-screen window.
//!
//! See docs/design/dynamic-bar/ for the design this follows. Units are
//! logical pixels (the kernel's units); times are milliseconds of uptime.

#![no_std]

extern crate alloc;

pub mod anim;
pub mod bar;
pub mod launchpad;
pub mod vector;
pub mod wm;

/// |x| without std.
#[inline]
pub(crate) fn fabs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// `x` rounded to the nearest integer.
#[inline]
pub fn round(x: f32) -> i32 {
    if x < 0.0 {
        (x - 0.5) as i32
    } else {
        (x + 0.5) as i32
    }
}
