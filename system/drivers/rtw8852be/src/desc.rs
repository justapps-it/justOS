//! What the driver and the card exchange in memory: the H2C header of a
//! command to the firmware, the transmit descriptors (the "WD" body and
//! info, the PCIe buffer descriptor, the WD page's address info), and on
//! the way back the receive buffer's info word, the receive descriptor, the
//! PHY status header and the release report of a sent frame.
//!
//! Field layouts from the Linux rtw89 driver (txrx.h, fw.h, pci.h, core.c),
//! Copyright(c) 2019-2022 Realtek Corporation, used under BSD-3-Clause;
//! written again in Rust.

/// `value` into the bits `hi..=lo` of a word.
pub const fn field(v: u32, hi: u32, lo: u32) -> u32 {
    let mask = if hi == 31 { u32::MAX } else { (1 << (hi + 1)) - 1 };
    (v << lo) & (mask & !((1 << lo) - 1))
}

/// The bits `hi..=lo` of `w`.
pub const fn get(w: u32, hi: u32, lo: u32) -> u32 {
    let w = if hi == 31 { w } else { w & ((1 << (hi + 1)) - 1) };
    w >> lo
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

// ---------------------------------------------------------------------------
// H2C: commands to the firmware

pub const H2C_HDR_LEN: usize = 8;
/// The largest H2C rtw89 sends (RTW89_H2C_MAX_SIZE).
pub const H2C_MAX: usize = 2048;
pub const FWCMD_TYPE_H2C: u32 = 0;
pub const CAT_MAC: u32 = 1;
pub const CAT_OUTSRC: u32 = 2;
pub const CL_MAC_FWDL: u32 = 3;
pub const FUNC_FWHDR_DL: u32 = 0;

/// An H2C header (rtw89_h2c_pkt_set_hdr): `len` is the payload's length.
/// The firmware acknowledges receipt when `rack`, and completion when
/// `dack`.
pub fn h2c_hdr(cat: u32, class: u32, func: u32, seq: u8, len: usize, rack: bool, dack: bool) -> [u8; H2C_HDR_LEN] {
    let w0 = field(FWCMD_TYPE_H2C, 19, 16) | field(cat, 1, 0) | field(class, 7, 2) | field(func, 15, 8) | field(seq as u32, 31, 24);
    let w1 = field((len + H2C_HDR_LEN) as u32, 13, 0) | (rack as u32) << 14 | (dack as u32) << 15;
    let mut h = [0u8; H2C_HDR_LEN];
    h[..4].copy_from_slice(&w0.to_le_bytes());
    h[4..].copy_from_slice(&w1.to_le_bytes());
    h
}

/// The H2C sequence numbers: rtw89 (802.11ax chips) asks for a receive
/// acknowledgement on every fourth command, and counts the sequence up
/// except for the firmware header download.
#[derive(Default)]
pub struct H2cSeq(pub u8);

impl H2cSeq {
    /// The header of the next command; returns it and whether a receive
    /// acknowledgement was requested.
    pub fn next(&mut self, cat: u32, class: u32, func: u32, len: usize, rack: bool, dack: bool) -> [u8; H2C_HDR_LEN] {
        let rack = rack || self.0 % 4 == 0;
        let h = h2c_hdr(cat, class, func, self.0, len, rack, dack);
        self.0 = self.0.wrapping_add(1);
        h
    }

    /// The firmware header download's header (rtw89_h2c_pkt_set_hdr_fwdl):
    /// no acknowledgement, the sequence does not move.
    pub fn fwdl(&self, len: usize) -> [u8; H2C_HDR_LEN] {
        h2c_hdr(CAT_MAC, CL_MAC_FWDL, FUNC_FWHDR_DL, self.0, len, false, false)
    }
}

// ---------------------------------------------------------------------------
// Transmit

pub const WD_BODY_LEN: usize = 24;
pub const WD_INFO_LEN: usize = 24;
pub const WP_INFO_LEN: usize = 8;
pub const ADDR_INFO_LEN: usize = 8;
/// A WD page: body, info, the WP info and one address info.
pub const WD_PAGE_LEN: usize = WD_BODY_LEN + WD_INFO_LEN + WP_INFO_LEN + ADDR_INFO_LEN;

/// DMA channels (enum rtw89_tx_channel).
pub const CH_ACH0: u32 = 0;
pub const CH_B0MG: u32 = 8;
pub const CH_B0HI: u32 = 9;
pub const CH_FWCMD: u32 = 12;

/// Queue selectors (enum rtw89_tx_qsel).
pub const QSEL_BE: u32 = 0x00;
pub const QSEL_B0_MGMT: u32 = 0x12;

/// Hardware rates (enum rtw89_hw_rate).
pub const RATE_CCK1: u32 = 0x0;
pub const RATE_OFDM6: u32 = 0x4;
pub const RATE_OFDM12: u32 = 0x6;
pub const RATE_OFDM24: u32 = 0x8;

/// What a transmit descriptor says (the subset of rtw89_tx_desc_info this
/// driver uses).
#[derive(Clone, Copy, Default, Debug)]
pub struct TxDesc {
    pub ch_dma: u32,
    pub qsel: u32,
    pub pkt_size: usize,
    pub fw_dl: bool,
    pub wd_page: bool,
    pub en_wd_info: bool,
    pub wp_offset: u32,
    pub hw_ssn_sel: u32,
    pub hw_seq_mode: u32,
    pub mac_id: u32,
    pub seq: u32,
    pub use_rate: bool,
    pub dis_data_fb: bool,
    pub data_rate: u32,
    pub data_bw: u32,
    pub gi_ltf: u32,
    pub retry_lowest_rate: u32,
    /// Broadcast or multicast: no RTS (info4).
    pub bmc: bool,
}

impl TxDesc {
    /// A command for the firmware, or a piece of the firmware itself
    /// (rtw89_core_tx_update_h2c_info).
    pub fn fwcmd(len: usize, fw_dl: bool) -> TxDesc {
        TxDesc { ch_dma: CH_FWCMD, pkt_size: len, fw_dl, ..TxDesc::default() }
    }

    /// A management frame at a fixed rate on the management queue
    /// (rtw89_core_tx_update_mgmt_info): the hardware numbers it.
    pub fn mgmt(len: usize, rate: u32) -> TxDesc {
        TxDesc {
            ch_dma: CH_B0MG,
            qsel: QSEL_B0_MGMT,
            pkt_size: len,
            wd_page: true,
            en_wd_info: true,
            hw_ssn_sel: 1,
            hw_seq_mode: 1,
            use_rate: true,
            dis_data_fb: true,
            data_rate: rate,
            ..TxDesc::default()
        }
    }

    /// A data frame on the best-effort queue, at one fixed rate (rtw89
    /// lets the firmware's rate adaptation pick it; this driver fixes it,
    /// as rtw89 does for management frames). `seq`: the frame's sequence
    /// number, from its header.
    pub fn data(len: usize, rate: u32, seq: u32, bmc: bool) -> TxDesc {
        TxDesc {
            ch_dma: CH_ACH0,
            qsel: QSEL_BE,
            pkt_size: len,
            wd_page: true,
            en_wd_info: true,
            seq,
            use_rate: true,
            dis_data_fb: true,
            data_rate: rate,
            retry_lowest_rate: rate,
            bmc,
            ..TxDesc::default()
        }
    }

    /// The WD body, and the WD info when enabled (rtw89_core_fill_txdesc):
    /// 24 or 48 bytes.
    pub fn fill(&self, out: &mut [u8]) -> usize {
        let body0 = field(self.wp_offset, 31, 24)
            | (self.en_wd_info as u32) << 22
            | field(self.ch_dma, 19, 16)
            | (self.wd_page as u32) << 7
            | (self.fw_dl as u32) << 20
            | field(self.hw_ssn_sel, 3, 2)
            | field(self.hw_seq_mode, 1, 0);
        let body2 = field(self.qsel, 22, 17) | field(self.pkt_size as u32, 13, 0) | field(self.mac_id, 30, 24);
        let body3 = field(self.seq, 11, 0);
        let mut w = [0u32; 12];
        w[0] = body0;
        w[2] = body2;
        w[3] = body3;
        let n = if self.en_wd_info {
            w[6] = (self.use_rate as u32) << 30
                | field(self.data_bw, 29, 28)
                | field(self.gi_ltf, 27, 25)
                | field(self.data_rate, 24, 16)
                | (self.dis_data_fb as u32) << 10;
            w[7] = field(self.retry_lowest_rate, 24, 16);
            // info4: RTS for unicast frames, as rtw89 (RTS_EN = !bmc,
            // HW_RTS_EN = 1).
            w[10] = (!self.bmc as u32) << 27 | 1 << 31;
            12
        } else {
            6
        };
        for (i, v) in w[..n].iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        n * 4
    }
}

/// A transmit buffer descriptor (rtw89_pci_tx_bd_32): `len` bytes at `dma`,
/// the last (only) piece of the packet.
pub fn txbd(len: usize, dma: u64) -> [u8; 8] {
    let opt: u16 = 1 << 14 | (((dma >> 32) as u16 & 0xFF) << 6);
    let mut b = [0u8; 8];
    b[..2].copy_from_slice(&(len as u16).to_le_bytes());
    b[2..4].copy_from_slice(&opt.to_le_bytes());
    b[4..].copy_from_slice(&(dma as u32).to_le_bytes());
    b
}

/// Fills a WD page (rtw89_pci_txwd_submit): the descriptor, the WP info
/// with the page's number (`seq`, which the release report gives back),
/// and the address of the frame (`len` bytes at `dma`). Returns the page's
/// length.
pub fn wd_page(out: &mut [u8], desc: &TxDesc, seq: u16, len: usize, dma: u64) -> usize {
    let mut n = desc.fill(out);
    out[n..n + 2].copy_from_slice(&(seq | 1 << 15).to_le_bytes());
    out[n + 2..n + WP_INFO_LEN].fill(0);
    n += WP_INFO_LEN;
    // rtw8852b: rtw89_pci_fill_txaddr_info, one piece.
    let opt: u16 = 1 << 15 | (((dma >> 32) as u16 & 0xFF) << 6) | 1;
    out[n..n + 2].copy_from_slice(&(len as u16).to_le_bytes());
    out[n + 2..n + 4].copy_from_slice(&opt.to_le_bytes());
    out[n + 4..n + 8].copy_from_slice(&(dma as u32).to_le_bytes());
    n + ADDR_INFO_LEN
}

/// The free slots of a ring (rtw89_pci_get_avail_txbd_num): one stays empty.
pub fn ring_free(len: u32, wp: u32, rp: u32) -> u32 {
    if rp > wp {
        rp - wp - 1
    } else {
        len - (wp - rp) - 1
    }
}

/// Slots the hardware went through since `rp` (rtw89_pci_dma_recalc), from
/// its index register.
pub fn ring_done(len: u32, from: u32, idx_reg: u32) -> (u32, u32) {
    let hw = get(idx_reg, 27, 16);
    let n = if hw >= from { hw - from } else { len - (from - hw) };
    (n, hw)
}

// ---------------------------------------------------------------------------
// Receive

/// The receive buffer descriptor (rtw89_pci_rx_bd_32): a buffer of `size`
/// bytes at `dma`.
pub fn rxbd(size: usize, dma: u64) -> [u8; 8] {
    let opt: u16 = ((dma >> 32) as u16 & 0xFF) << 6;
    let mut b = [0u8; 8];
    b[..2].copy_from_slice(&(size as u16).to_le_bytes());
    b[2..4].copy_from_slice(&opt.to_le_bytes());
    b[4..].copy_from_slice(&(dma as u32).to_le_bytes());
    b
}

/// The word the card writes at the start of a receive buffer.
pub const RXBD_INFO_LEN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxInfo {
    pub fs: bool,
    pub ls: bool,
    pub len: usize,
    pub tag: u32,
}

pub fn rx_info(buf: &[u8]) -> RxInfo {
    let w = le32(buf, 0);
    RxInfo { fs: w & 1 << 15 != 0, ls: w & 1 << 14 != 0, len: get(w, 13, 0) as usize, tag: get(w, 28, 16) }
}

/// Kinds of received packet (enum rtw89_core_rx_type).
pub const RX_WIFI: u32 = 0;
pub const RX_PPDU_STAT: u32 = 1;
pub const RX_C2H: u32 = 10;

/// A receive descriptor (rtw89_core_query_rxdesc).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxDesc {
    pub pkt_size: usize,
    pub ppdu_cnt: u32,
    pub pkt_type: u32,
    pub long: bool,
    pub mac_info_valid: bool,
    pub data_rate: u32,
    pub bw: u32,
    pub crc32_err: bool,
    pub icv_err: bool,
    pub hw_dec: bool,
    pub sw_dec: bool,
    pub a1_match: bool,
    /// Bytes from the descriptor's start to the driver info (shift).
    pub shift: usize,
    /// Bytes of driver info (the PHY status of a frame, 8-byte units).
    pub drv_info: usize,
    pub rxd_len: usize,
}

impl RxDesc {
    /// The descriptor at `at` in `buf`, or None if it is cut short.
    pub fn parse(buf: &[u8], at: usize) -> Option<RxDesc> {
        if buf.len() < at + 16 {
            return None;
        }
        let w0 = le32(buf, at);
        let w1 = le32(buf, at + 4);
        let w3 = le32(buf, at + 12);
        let long = w0 & 1 << 31 != 0;
        let d = RxDesc {
            pkt_size: get(w0, 13, 0) as usize,
            ppdu_cnt: get(w1, 6, 4),
            pkt_type: get(w0, 27, 24),
            long,
            mac_info_valid: w0 & 1 << 23 != 0,
            data_rate: get(w1, 24, 16),
            bw: get(w1, 31, 30),
            crc32_err: w3 & 1 << 9 != 0,
            icv_err: w3 & 1 << 10 != 0,
            hw_dec: w3 & 1 << 2 != 0,
            sw_dec: w3 & 1 << 1 != 0,
            a1_match: w3 & 1 != 0,
            shift: (get(w0, 15, 14) as usize) << 1,
            drv_info: (get(w0, 30, 28) as usize) << 3,
            rxd_len: if long { 32 } else { 16 },
        };
        (buf.len() >= at + d.rxd_len).then_some(d)
    }

    /// Where the packet starts, from the descriptor's start: after the
    /// descriptor, the shift and the driver info (desc_info->offset +
    /// rxd_len).
    pub fn payload(&self) -> usize {
        self.rxd_len + self.shift + self.drv_info
    }
}

/// The average signal strength in a frame's PHY status, if it carries one
/// (rtw89_core_parse_phy_status_hdr: RSSI_AVG in half-dB above -110 dBm).
pub fn phy_rssi(drv_info: &[u8]) -> Option<i8> {
    if drv_info.len() < 8 {
        return None;
    }
    let raw = drv_info[3] as i32;
    if raw == 0 {
        return None;
    }
    Some(((raw >> 1) - 110).clamp(-110, 0) as i8)
}

/// The PHY status in a PPDU status packet (its payload): after the MAC
/// info, its per-user words (an even number of them), the receive
/// counters when present and the PLCP header (rtw89_core_rx_process_mac_ppdu).
pub fn ppdu_phy_status(p: &[u8]) -> Option<&[u8]> {
    if p.len() < 8 {
        return None;
    }
    let (w0, w1) = (le32(p, 0), le32(p, 4));
    let users = get(w0, 3, 0) as usize;
    if users > 4 {
        return None;
    }
    let mut at = 8 + users.next_multiple_of(2) * 4;
    if w0 & 1 << 29 != 0 {
        at += 96;
    }
    at += (get(w1, 23, 16) as usize) << 3;
    p.get(at..)
}

/// A release report (rtw89_pci_rpp_fmt, 802.11ax chips before the 8852C):
/// the WD page number, the queue, how it went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rpp {
    pub seq: u32,
    pub qsel: u32,
    pub status: u32,
    pub mac_id: u32,
}

pub fn rpp(w: u32) -> Rpp {
    Rpp { seq: get(w, 30, 16), status: get(w, 15, 13), qsel: get(w, 12, 8), mac_id: get(w, 7, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(b: &[u8], i: usize) -> u32 {
        le32(b, i * 4)
    }

    #[test]
    fn fields() {
        assert_eq!(field(0x3F, 21, 16), 0x003F_0000);
        assert_eq!(field(0x1FF, 3, 0), 0xF);
        assert_eq!(field(1, 31, 31), 1 << 31);
        assert_eq!(get(0xABCD_1234, 27, 16), 0xBCD);
        assert_eq!(get(0x8000_0000, 31, 31), 1);
    }

    #[test]
    fn h2c_headers() {
        // The firmware header download: MAC category, class FWDL, function
        // 0, no acknowledgements, length + 8.
        let s = H2cSeq(0);
        let h = s.fwdl(320);
        assert_eq!(w(&h, 0), 0x0000_000D);
        assert_eq!(w(&h, 1), 328);
        // Ordinary commands: sequence in the top byte, receive ack forced
        // every fourth.
        let mut s = H2cSeq(0);
        let a = s.next(CAT_MAC, 8, 1, 4, false, false);
        assert_eq!(w(&a, 0), 0x0000_0121);
        assert_eq!(w(&a, 1), 12 | 1 << 14);
        let b = s.next(CAT_MAC, 8, 1, 4, false, true);
        assert_eq!(w(&b, 0) >> 24, 1);
        assert_eq!(w(&b, 1), 12 | 1 << 15);
        s.next(CAT_MAC, 8, 1, 4, false, false);
        s.next(CAT_MAC, 8, 1, 4, false, false);
        let e = s.next(CAT_MAC, 8, 1, 4, false, false);
        assert_eq!(w(&e, 0) >> 24, 4);
        assert_ne!(w(&e, 1) & 1 << 14, 0);
        assert_eq!(s.0, 5);
    }

    #[test]
    fn fwcmd_descriptor() {
        let mut out = [0xEEu8; 48];
        assert_eq!(TxDesc::fwcmd(2020, true).fill(&mut out), 24);
        assert_eq!(w(&out, 0), 12 << 16 | 1 << 20);
        assert_eq!(w(&out, 1), 0);
        assert_eq!(w(&out, 2), 2020);
        assert_eq!(w(&out, 3), 0);
        assert_eq!(out[24], 0xEE);
        TxDesc::fwcmd(100, false).fill(&mut out);
        assert_eq!(w(&out, 0), 12 << 16);
        assert_eq!(w(&out, 2), 100);
    }

    #[test]
    fn mgmt_descriptor() {
        let mut out = [0u8; 128];
        let d = TxDesc::mgmt(60, RATE_CCK1);
        let n = wd_page(&mut out, &d, 7, 60, 0x1_2345_6780);
        assert_eq!(n, WD_PAGE_LEN);
        assert_eq!(w(&out, 0), 1 << 22 | 8 << 16 | 1 << 7 | 1 << 2 | 1);
        assert_eq!(w(&out, 2), 0x12 << 17 | 60);
        assert_eq!(w(&out, 6), 1 << 30 | 1 << 10);
        assert_eq!(w(&out, 10), 1 << 31 | 1 << 27);
        assert_eq!(w(&out, 12), 7 | 1 << 15);
        assert_eq!(w(&out, 13), 0);
        assert_eq!(w(&out, 14), 60 | (1 << 15 | 1 << 6 | 1) << 16);
        assert_eq!(w(&out, 15), 0x2345_6780);
        let d = TxDesc::mgmt(60, RATE_OFDM6);
        d.fill(&mut out);
        assert_eq!(get(w(&out, 6), 24, 16), 4);
        let d = TxDesc::data(1500, RATE_OFDM24, 0x123, true);
        d.fill(&mut out);
        assert_eq!(w(&out, 0), 1 << 22 | 1 << 7);
        assert_eq!(w(&out, 2), 1500);
        assert_eq!(w(&out, 3), 0x123);
        assert_eq!(w(&out, 7), 8 << 16);
        assert_eq!(w(&out, 10), 1 << 31);
    }

    #[test]
    fn buffer_descriptors() {
        let b = txbd(2044, 0x0000_0012_8000_1000);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 2044);
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), 1 << 14 | 0x12 << 6);
        assert_eq!(w(&b, 1), 0x8000_1000);
        let r = rxbd(11498, 0x4000);
        assert_eq!(w(&r, 0), 11498);
        assert_eq!(w(&r, 1), 0x4000);
        assert_eq!(ring_free(32, 0, 0), 31);
        assert_eq!(ring_free(32, 5, 6), 0);
        assert_eq!(ring_free(32, 31, 0), 0);
        assert_eq!(ring_done(32, 30, 2 << 16 | 2), (4, 2));
        assert_eq!(ring_done(32, 3, 7 << 16), (4, 7));
    }

    #[test]
    fn receive_side() {
        let mut buf = [0u8; 64];
        let info: u32 = 1 << 15 | 1 << 14 | 300 | 5 << 16;
        buf[..4].copy_from_slice(&info.to_le_bytes());
        assert_eq!(rx_info(&buf), RxInfo { fs: true, ls: true, len: 300, tag: 5 });
        // A long descriptor with one 8-byte unit of driver info and a
        // 2-unit shift.
        let w0: u32 = 1 << 31 | 1 << 28 | 1 << 23 | 2 << 14 | 250;
        let w3: u32 = 1 | 1 << 9;
        buf[4..8].copy_from_slice(&w0.to_le_bytes());
        buf[16..20].copy_from_slice(&w3.to_le_bytes());
        let d = RxDesc::parse(&buf, 4).unwrap();
        assert!(d.long && d.a1_match && d.crc32_err && d.mac_info_valid);
        assert_eq!((d.pkt_size, d.pkt_type, d.rxd_len, d.shift, d.drv_info), (250, RX_WIFI, 32, 4, 8));
        assert_eq!(d.payload(), 44);
        assert!(RxDesc::parse(&buf[..20], 4).is_none());
        assert_eq!(phy_rssi(&[0, 0, 0, 120, 0, 0, 0, 0]), Some(-50));
        assert_eq!(phy_rssi(&[0; 8]), None);
        // One user (padded to two), no counters, a 2-unit PLCP header.
        let mut p = [0u8; 40];
        p[0] = 1;
        p[6] = 2;
        p[32] = 0xAB;
        assert_eq!(ppdu_phy_status(&p).unwrap()[0], 0xAB);
        p[3] = 1 << 5;
        assert!(ppdu_phy_status(&p).is_none());
        let r = rpp(1 << 31 | 17 << 16 | 2 << 13 | 0x12 << 8 | 3);
        assert_eq!(r, Rpp { seq: 17, status: 2, qsel: 0x12, mac_id: 3 });
    }
}
