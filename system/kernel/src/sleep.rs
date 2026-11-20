//! Sleep: the whole machine stops with its memory kept and starts again,
//! a second later, where it was. Two kinds (see [`Kind`]): suspend to RAM
//! (S3), where everything but the memory loses power, and suspend to idle
//! (s2idle), where the power stays on and everything waits in its
//! deepest idle state: computers made for Modern Standby have only that.
//!
//! Going to sleep, each step undone if a later one fails:
//! 1. checks: the firmware offers S3, or something can wake the computer
//!    from s2idle, and every device's driver can follow (see `dev::pm`);
//! 2. the screen goes dark (and locks), the file systems are written out,
//!    in case the battery runs out;
//! 3. for S3 the firmware is told (`_TTS`); disk transfers are held back,
//!    the PCI configuration is saved and the drivers stop their devices;
//! 4. the devices that may wake the computer are armed and the firmware
//!    prepares (`_PTS` for S3, Modern Standby's `_DSM` for s2idle); the
//!    other processors stop;
//! 5. S3: the chipset cuts the power (see `acpi::sleep`). s2idle: only the
//!    wake events can interrupt (the timer is held back too, so no thread
//!    runs) and the first processor waits for one.
//!
//! Waking up from S3, on the first processor with interrupts off: the
//! clock, the PCI configuration (the chipset's included), the interrupt
//! controllers, the system timer, the serial port and the chipset's events;
//! then with interrupts on: message interrupts, the other processors, the
//! PS/2 controller, the drivers' devices, the held-back I/O, `_WAK`, a
//! redraw. From s2idle, where nothing lost power: the clock, the interrupt
//! lines, the chipset's events, the other processors, the devices, the
//! I/O, the firmware, a redraw.

use crate::acpi::sleep::{self as acpi_sleep, Outcome};
use crate::arch::{apic, cpu, pic, pit, smp};
use crate::dev::{self, pm};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// How the machine sleeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// S3: everything but the memory loses power.
    Ram,
    /// s2idle: the power stays on; the devices stop, the screen is dark,
    /// the processors wait in their deepest idle state for a wake event.
    Idle,
}

impl Kind {
    pub fn describe(self) -> &'static str {
        match self {
            Kind::Ram => "suspend to RAM (S3)",
            Kind::Idle => "suspend to idle (s2idle)",
        }
    }
}

/// A sleep is under way.
static BUSY: AtomicBool = AtomicBool::new(false);
/// How many times the machine slept.
static COUNT: AtomicU64 = AtomicU64::new(0);
/// When the machine last woke up (uptime, ms; 0 never).
static WOKE_AT: AtomicU64 = AtomicU64::new(0);
/// The first processor waits in s2idle.
static IDLE: AtomicBool = AtomicBool::new(false);
/// What MONITOR watches while the first processor waits in s2idle: nothing
/// writes to it, interrupts are what wake the processor.
static IDLE_LINE: AtomicU8 = AtomicU8::new(0);

/// The devices whose drivers cannot follow the machine into sleep, as an
/// error.
fn devices_ready() -> Result<(), String> {
    let unready = pm::unready();
    if !unready.is_empty() {
        return Err(format!("these devices cannot sleep: {}", unready.join("; ")));
    }
    Ok(())
}

/// Whether something could wake the computer from s2idle: the chipset's
/// events (power button, lid, clock) or a PS/2 keyboard. USB controllers
/// are stopped like every other device.
fn idle_wakes() -> Result<(), &'static str> {
    if crate::acpi::sci_line().is_some() || dev::platform::ps2_keyboard() {
        Ok(())
    } else {
        Err("nothing could wake the computer from suspend to idle (no ACPI events, no PS/2 keyboard)")
    }
}

/// How the machine sleeps when asked: S3 if the firmware offers it, unless
/// it would rather use s2idle; s2idle if not. Err with the reasons if it
/// cannot sleep now.
pub fn kind() -> Result<Kind, String> {
    devices_ready()?;
    match (acpi_sleep::supported(), idle_wakes()) {
        (Ok(()), Ok(())) if acpi_sleep::prefers_idle() => Ok(Kind::Idle),
        (Ok(()), _) => Ok(Kind::Ram),
        (Err(_), Ok(())) => Ok(Kind::Idle),
        (Err(ram), Err(idle)) => Err(format!("{ram}; {idle}")),
    }
}

/// Whether the machine can sleep in this way now.
fn check(want: Option<Kind>) -> Result<Kind, String> {
    match want {
        None => kind(),
        Some(Kind::Ram) => {
            devices_ready()?;
            acpi_sleep::supported().map(|()| Kind::Ram).map_err(String::from)
        }
        Some(Kind::Idle) => {
            devices_ready()?;
            idle_wakes().map(|()| Kind::Idle).map_err(String::from)
        }
    }
}

/// Whether the machine can sleep now: Err with the reasons if not.
pub fn available() -> Result<(), String> {
    kind().map(|_| ())
}

/// How many times the machine slept since it started.
pub fn count() -> u64 {
    COUNT.load(Ordering::Relaxed)
}

/// The first processor is waiting in s2idle: a press of the power button
/// only wakes the computer.
pub fn idling() -> bool {
    IDLE.load(Ordering::Acquire)
}

/// The machine woke up in the last few seconds: the press that woke it,
/// handed on by the firmware a moment later, does not also turn it off.
pub fn just_woke() -> bool {
    let at = WOKE_AT.load(Ordering::Relaxed);
    at != 0 && crate::time::uptime_ms() < at + 3000
}

/// Puts the machine to sleep until a wake event (power button, lid,
/// keyboard on some machines) or, with `wake_after`, until that many
/// seconds have passed. Returns the seconds it slept.
pub fn suspend(wake_after: Option<u64>) -> Result<u64, String> {
    suspend_as(None, wake_after)
}

/// [`suspend`] in a given way (None: the usual one, see [`kind`]).
pub fn suspend_as(want: Option<Kind>, wake_after: Option<u64>) -> Result<u64, String> {
    if BUSY.swap(true, Ordering::AcqRel) {
        return Err(String::from("the computer is already going to sleep"));
    }
    let r = run(want, wake_after);
    // The screen lights up again, whether the machine slept or not.
    crate::gui::after_sleep();
    BUSY.store(false, Ordering::Release);
    if let Err(e) = &r {
        crate::warn!("sleep", "{e}");
    }
    r
}

/// Sleep asked for by an event (sleep button, lid, the menu): on a thread
/// of its own, so the caller goes on.
pub fn request(reason: &'static str) {
    if BUSY.load(Ordering::Acquire) {
        return;
    }
    crate::task::spawn("sleep", move || {
        crate::info!("sleep", "{reason}");
        if let Err(e) = suspend(None) {
            crate::gui::notify("Sospensione non riuscita", &e, "about");
        }
    });
}

fn run(want: Option<Kind>, wake_after: Option<u64>) -> Result<u64, String> {
    let kind = check(want)?;
    crate::info!("sleep", "going to sleep: {}", kind.describe());
    if crate::gui::active() {
        // The screen goes dark and the lock screen goes up (if there is a
        // password), so it is what the screen shows on waking up.
        crate::gui::before_sleep();
        crate::task::sleep_ms(300);
    }
    crate::fs::sync_all();
    if kind == Kind::Ram {
        acpi_sleep::will_sleep();
    }
    pm::pause_io();
    let mut pci: Vec<u32> = dev::list().iter().filter(|d| d.info.bus == justdk::BUS_PCI).map(|d| d.info.location).collect();
    pci.sort_unstable();
    pci.dedup();
    let saved = dev::pci::save_config(&pci);
    if let Err(e) = pm::suspend_devices() {
        pm::resume_io();
        match kind {
            Kind::Ram => acpi_sleep::finish(),
            Kind::Idle => {}
        }
        return Err(e);
    }
    let slept = match kind {
        Kind::Ram => ram(&saved, wake_after)?,
        Kind::Idle => idle(&saved, wake_after)?,
    };
    COUNT.fetch_add(1, Ordering::Relaxed);
    WOKE_AT.store(crate::time::uptime_ms().max(1), Ordering::Relaxed);
    crate::info!("sleep", "awake after {slept} s");
    Ok(slept)
}

/// S3, the devices stopped: the chipset cuts the power until a wake event.
fn ram(saved: &[dev::pci::SavedConfig], wake_after: Option<u64>) -> Result<u64, String> {
    let plan = match acpi_sleep::prepare() {
        Ok(p) => p,
        Err(e) => {
            wake_devices();
            acpi_sleep::finish();
            return Err(String::from(e));
        }
    };
    if !smp::park() {
        wake_devices();
        acpi_sleep::finish();
        return Err(String::from("a processor did not stop"));
    }
    pic::suspend();
    if let Some(secs) = wake_after {
        crate::time::set_alarm(secs);
    }
    let outcome = acpi_sleep::enter(&plan, wake_after.is_some());

    // Interrupts are off; only this processor runs.
    let slept = crate::time::resync();
    if wake_after.is_some() {
        crate::time::clear_alarm();
    }
    dev::pci::restore_config(saved);
    apic::resume();
    pic::resume();
    pit::init();
    crate::console::serial::resume();
    acpi_sleep::after_wake(&plan);
    cpu::enable_irq();

    dev::irq::restore_msi();
    if outcome == Outcome::Slept {
        smp::resume();
    } else {
        smp::unpark();
    }
    dev::platform::resume_ps2();
    wake_devices();
    acpi_sleep::finish();
    dev::display::after_sleep();
    match outcome {
        Outcome::Slept => Ok(slept),
        Outcome::WokeAtOnce => Err(String::from("the computer woke up at once (a wake event was waiting)")),
        Outcome::Refused => Err(String::from("the firmware did not put the computer to sleep")),
    }
}

/// s2idle, the devices stopped: the other processors stop, only the wake
/// events reach this one, and it waits in its deepest idle state until one
/// comes. The timer's line is held back with the rest, so no thread runs
/// and the uptime stands still, as across S3.
fn idle(saved: &[dev::pci::SavedConfig], wake_after: Option<u64>) -> Result<u64, String> {
    let plan = acpi_sleep::prepare_idle();
    if !smp::park() {
        wake_devices();
        acpi_sleep::finish_idle();
        return Err(String::from("a processor did not stop"));
    }
    let alarm = wake_after.is_some();
    if let Some(secs) = wake_after {
        crate::time::set_alarm(secs);
    }
    // The lines whose interrupts wake the computer: the chipset's events,
    // the keyboard, the clock's alarm (when there is no SCI to bring it).
    let mut wake: Vec<u8> = crate::acpi::sci_line().into_iter().collect();
    if dev::platform::ps2_keyboard() {
        wake.push(1);
    }
    if alarm {
        wake.push(8);
    }
    let lines: u16 = wake.iter().fold(0, |m, &l| m | 1 << l);
    let seen = |wake: &[u8]| wake.iter().map(|&l| dev::irq::count(l)).sum::<u64>();

    cpu::disable_irq();
    acpi_sleep::arm_idle(&plan, alarm);
    let before = seen(&wake);
    IDLE.store(true, Ordering::Release);
    // The mouse shares the PS/2 controller's output with the keyboard: it
    // is served (its bytes would hold up the keyboard's) but wakes nothing.
    pic::only(lines | 1 << 12);
    // MWAIT reaches deeper idle states than HLT, where the processor can
    // be woken by an interrupt with interrupts off.
    let hint = cpu::mwait_hint().filter(|_| cpu::mwait_breaks_on_irq());
    while seen(&wake) == before {
        match hint {
            Some(h) => {
                cpu::monitor(IDLE_LINE.as_ptr());
                cpu::mwait(h, true);
                cpu::let_interrupts_in();
            }
            None => {
                cpu::enable_and_halt();
                cpu::disable_irq();
            }
        }
    }
    pic::restore();
    IDLE.store(false, Ordering::Release);

    let slept = crate::time::resync();
    crate::time::recalibrate();
    if alarm {
        crate::time::clear_alarm();
    }
    dev::pci::restore_config(saved);
    acpi_sleep::after_idle(&plan);
    cpu::enable_irq();
    smp::unpark();
    wake_devices();
    acpi_sleep::finish_idle();
    dev::display::after_sleep();
    Ok(slept)
}

/// The drivers start their devices again and the held-back I/O goes on.
fn wake_devices() {
    pm::resume_devices();
    pm::resume_io();
}
