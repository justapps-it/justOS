//! Debian's archive, read by justOS: what `apt` does before it hands the
//! packages to `dpkg`, written from scratch.
//!
//! - [`version`]: Debian's order of versions (epochs, `~`, revisions).
//! - [`control`]: control files (`Packages`, dpkg's `status`) and the
//!   relation fields in them (`Depends: a (>= 1) | b, c:any`).
//! - [`pgp`]: OpenPGP signatures, enough to check an `InRelease` file with
//!   the keys of debian-archive-keyring (RSA and Ed25519).
//! - [`release`]: what an `InRelease` file lists (the indexes and their
//!   SHA-256), and the `sources.list` that names the archives.
//! - [`index`]: the packages of an archive and of the system, kept with
//!   just the fields the resolver needs.
//! - [`resolve`]: which packages a new one needs, in which order dpkg
//!   installs them, and what does not go together.
//!
//! Nothing here touches the network or the disk: the kernel fetches the
//! files (src/apt.rs) and the host runs the unit tests (`make check`).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod control;
pub mod index;
pub mod pgp;
pub mod release;
pub mod resolve;
pub mod version;

#[cfg(test)]
mod tests;

/// Lower-case hex of some bytes (SHA-256 sums, as the indexes write them).
pub fn hex(b: &[u8]) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}
