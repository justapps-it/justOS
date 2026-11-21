//! Wi-Fi for justOS, written from scratch: the 802.11 frames a station
//! needs ([`frame`]), the supplicant's cryptography ([`crypto`]: PBKDF2,
//! the PRF and KDF, AES-CMAC, key wrap, CCMP, BIP), WPA3's SAE ([`sae`]),
//! the EAPOL-Key handshakes ([`eapol`]) and a station that joins open,
//! WPA2-PSK and WPA3-SAE networks ([`sta`]). It knows nothing of the card:
//! the kernel feeds it frames and carries out what it asks.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod crypto;
pub mod eapol;
pub mod frame;
pub mod sae;
pub mod sim;
pub mod sta;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_sim;
