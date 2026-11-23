//! The test radio's medium: 802.11 frames carried in Ethernet frames, so
//! that an access point simulator on the development machine
//! (`scripts/wifi/apsim.py`) can talk to justOS through an ordinary virtual
//! network card. Only for tests: no real card speaks it.
//!
//! An Ethernet frame of type 0x88B5 (IEEE "local experimental") with:
//!
//! ```text
//! offset  size
//!      0     4  "JWSM"
//!      4     1  version (1)
//!      5     1  channel
//!      6     1  signal strength (dBm, signed), from the access point
//!      7     1  reserved
//!      8     2  length of the 802.11 frame (big-endian)
//!     10        the 802.11 frame, without FCS
//! ```

use crate::frame::{Mac, BROADCAST};
use alloc::vec::Vec;

pub const ETHERTYPE: u16 = 0x88B5;
const MAGIC: &[u8; 4] = b"JWSM";

/// The Ethernet frame carrying `frame` on `channel`.
pub fn wrap(src: &Mac, channel: u8, rssi: i8, frame: &[u8]) -> Vec<u8> {
    let mut e = Vec::with_capacity(24 + frame.len());
    e.extend_from_slice(&BROADCAST);
    e.extend_from_slice(src);
    e.extend_from_slice(&ETHERTYPE.to_be_bytes());
    e.extend_from_slice(MAGIC);
    e.extend_from_slice(&[1, channel, rssi as u8, 0]);
    e.extend_from_slice(&(frame.len() as u16).to_be_bytes());
    e.extend_from_slice(frame);
    e
}

/// The channel, signal and 802.11 frame in an Ethernet frame of the medium.
pub fn unwrap(e: &[u8]) -> Option<(u8, i8, &[u8])> {
    if e.len() < 24 || u16::from_be_bytes([e[12], e[13]]) != ETHERTYPE || &e[14..18] != MAGIC || e[18] != 1 {
        return None;
    }
    let len = u16::from_be_bytes([e[22], e[23]]) as usize;
    let f = e.get(24..24 + len)?;
    Some((e[19], e[20] as i8, f))
}
