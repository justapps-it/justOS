//! How the card's packet memory is shared out: the "DLE" (the WDE buffer of
//! descriptors and the PLE buffer of packets, their page sizes and the
//! quota of each user) and the host flow control of the PCIe channels
//! ("HFC"), for the two modes rtw89 uses on the 8852B over PCIe: the
//! firmware download (DLFW) and normal operation (SCC, one channel).
//!
//! Values from the Linux rtw89 driver (rtw8852b.c, mac.c), Copyright(c)
//! 2019-2022 Realtek Corporation, used under BSD-3-Clause.

/// The WDE or PLE buffer: page size, pages linked and unlinked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DleSize {
    pub page: u32,
    pub lnk: u32,
    pub unlnk: u32,
}

/// Quotas of the WDE buffer: host interface, WLAN CPU, packet-in, CPU I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WdeQuota {
    pub hif: u32,
    pub wcpu: u32,
    pub pkt_in: u32,
    pub cpu_io: u32,
}

/// Quotas of the PLE buffer, in the order of its quota registers 0-10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PleQuota {
    pub cma0_tx: u32,
    pub cma1_tx: u32,
    pub c2h: u32,
    pub h2c: u32,
    pub wcpu: u32,
    pub mpdu_proc: u32,
    pub cma0_dma: u32,
    pub cma1_dma: u32,
    pub bb_rpt: u32,
    pub wd_rel: u32,
    pub cpu_io: u32,
}

impl PleQuota {
    fn regs(&self) -> [u32; 11] {
        [
            self.cma0_tx,
            self.cma1_tx,
            self.c2h,
            self.h2c,
            self.wcpu,
            self.mpdu_proc,
            self.cma0_dma,
            self.cma1_dma,
            self.bb_rpt,
            self.wd_rel,
            self.cpu_io,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Dle {
    pub wde: DleSize,
    pub ple: DleSize,
    pub wde_min: WdeQuota,
    pub wde_max: WdeQuota,
    pub ple_min: PleQuota,
    pub ple_max: PleQuota,
}

/// The firmware download (rtw8852b_dle_mem_pcie, RTW89_QTA_DLFW: wde_size9,
/// ple_size8, wde_qt4, ple_qt13).
pub const DLFW: Dle = Dle {
    wde: DleSize { page: 64, lnk: 0, unlnk: 1024 },
    ple: DleSize { page: 128, lnk: 64, unlnk: 960 },
    wde_min: WdeQuota { hif: 0, wcpu: 0, pkt_in: 0, cpu_io: 0 },
    wde_max: WdeQuota { hif: 0, wcpu: 0, pkt_in: 0, cpu_io: 0 },
    ple_min: PLE_QT13,
    ple_max: PLE_QT13,
};

const PLE_QT13: PleQuota = PleQuota {
    cma0_tx: 0,
    cma1_tx: 0,
    c2h: 16,
    h2c: 48,
    wcpu: 0,
    mpdu_proc: 0,
    cma0_dma: 0,
    cma1_dma: 0,
    bb_rpt: 0,
    wd_rel: 0,
    cpu_io: 0,
};

const WDE_QT7: WdeQuota = WdeQuota { hif: 446, wcpu: 48, pkt_in: 0, cpu_io: 16 };

/// Normal operation (RTW89_QTA_SCC: wde_size7, ple_size6, wde_qt7,
/// ple_qt18 and ple_qt58).
pub const SCC: Dle = Dle {
    wde: DleSize { page: 64, lnk: 510, unlnk: 2 },
    ple: DleSize { page: 128, lnk: 496, unlnk: 16 },
    wde_min: WDE_QT7,
    wde_max: WDE_QT7,
    ple_min: PleQuota {
        cma0_tx: 147,
        cma1_tx: 0,
        c2h: 16,
        h2c: 20,
        wcpu: 17,
        mpdu_proc: 13,
        cma0_dma: 89,
        cma1_dma: 0,
        bb_rpt: 32,
        wd_rel: 14,
        cpu_io: 8,
    },
    ple_max: PleQuota {
        cma0_tx: 147,
        cma1_tx: 0,
        c2h: 16,
        h2c: 20,
        wcpu: 157,
        mpdu_proc: 13,
        cma0_dma: 229,
        cma1_dma: 0,
        bb_rpt: 172,
        wd_rel: 14,
        cpu_io: 24,
    },
};

/// The WDE's minimum quota for the WLAN CPU in the download mode comes
/// from the SCC set (rtw89_mac_dle_init's ext_wde_min_qt_wcpu).
pub const DLFW_EXT_WDE_WCPU: u32 = 48;

/// The 8852B's shared buffer: 196608 bytes, of which SCC leaves 98304 to
/// the firmware (rtw8852b_dle_mem_pcie and the chip's fifo_size).
pub const FIFO_SIZE: u32 = 196608;
pub const SCC_RSVD: u32 = 98304;

impl Dle {
    /// Bytes of buffer the configuration uses (dle_used_size).
    pub fn used(&self) -> u32 {
        self.wde.page * (self.wde.lnk + self.wde.unlnk) + self.ple.page * (self.ple.lnk + self.ple.unlnk)
    }

    /// WDE_PKTBUF_CFG / PLE_PKTBUF_CFG fields (dle_mix_cfg): page size
    /// code, start boundary (8 KiB units), free pages.
    pub fn pktbuf(&self) -> [(u32, u32, u32); 2] {
        let sel = |p: u32| match p {
            64 => 0,
            128 => 1,
            _ => 2,
        };
        let ple_start = (self.wde.lnk + self.wde.unlnk) * self.wde.page / 8192;
        [(sel(self.wde.page), 0, self.wde.lnk), (sel(self.ple.page), ple_start, self.ple.lnk)]
    }

    /// The quota registers: WDE_QTA0, 1, 3, 4 (wde_quota_cfg_ax) and
    /// PLE_QTA0-10 (ple_quota_cfg_ax), each `min | max << 16`.
    pub fn quota_regs(&self, ext_wde_wcpu: Option<u32>) -> ([u32; 4], [u32; 11]) {
        let q = |min: u32, max: u32| (min & 0xFFF) | (max & 0xFFF) << 16;
        let (a, b) = (self.wde_min, self.wde_max);
        let wcpu_min = ext_wde_wcpu.unwrap_or(a.wcpu);
        let wde = [q(a.hif, b.hif), q(wcpu_min, b.wcpu), q(a.pkt_in, b.pkt_in), q(a.cpu_io, b.cpu_io)];
        let (lo, hi) = (self.ple_min.regs(), self.ple_max.regs());
        let mut ple = [0; 11];
        for i in 0..11 {
            ple[i] = q(lo[i], hi[i]);
        }
        (wde, ple)
    }

    /// The largest receive A-MPDU of CMAC 0, in 512-byte units of
    /// RX_FLTR_OPT's RX_MPDU_MAX_LEN (rmac_init_ax: the CMAC 0 DMA quota in
    /// PLE pages, at most 127, capped at 11966 bytes).
    pub fn rx_mpdu_max_units(&self) -> u32 {
        let qta = self.ple_min.cma0_dma.min(127);
        (qta * self.ple.page).min(11966) / 512
    }
}

// ---------------------------------------------------------------------------
// Host flow control

/// One PCIe channel's pages: minimum, maximum, group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HfcCh {
    pub min: u32,
    pub max: u32,
    pub grp: u32,
}

/// rtw8852b_hfc_chcfg_pcie for ACH0-7, B0MGQ, B0HIQ, B1MGQ, B1HIQ. The
/// firmware command channel (12) has its own precedence below.
pub const HFC_CH: [HfcCh; 12] = [
    HfcCh { min: 5, max: 341, grp: 0 },
    HfcCh { min: 5, max: 341, grp: 0 },
    HfcCh { min: 4, max: 342, grp: 0 },
    HfcCh { min: 4, max: 342, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
    HfcCh { min: 4, max: 342, grp: 0 },
    HfcCh { min: 4, max: 342, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
    HfcCh { min: 0, max: 0, grp: 0 },
];

/// Channels the 8852B does not have (its tx_dma_ch_mask: ACH4-7, B1MG,
/// B1HI).
pub fn hfc_ch_unused(ch: usize) -> bool {
    (4..=7).contains(&ch) || ch == 10 || ch == 11
}

/// rtw8852b_hfc_pubcfg_pcie: group 0 and 1 pages, all public pages, the
/// WP threshold.
pub const HFC_PUB: (u32, u32, u32, u32) = (446, 0, 446, 0);

/// rtw89_mac_hfc_prec (PCIe): ch011 precedence, h2c precedence, WP ch0-7
/// and ch8-11 precedence, and the four "full" conditions; mode POH.
pub struct HfcPrec {
    pub ch011: u32,
    pub h2c: u32,
    pub wp07: u32,
    pub wp811: u32,
    pub ch011_full: u32,
    pub h2c_full: u32,
    pub wp07_full: u32,
    pub wp811_full: u32,
}

pub const HFC_PREC: HfcPrec =
    HfcPrec { ch011: 2, h2c: 40, wp07: 0, wp811: 0, ch011_full: 1, h2c_full: 0, wp07_full: 0, wp811_full: 0 };

/// ACHn_PAGE_CTRL (hfc_ch_ctrl): min, max, group.
pub fn hfc_ch_reg(c: HfcCh) -> u32 {
    (c.min & 0x1FFF) | (c.max & 0x1FFF) << 16 | (c.grp & 1) << 31
}

/// The HCI_FC_CTRL fields hfc_mix_cfg sets: mode (POH 0) at 2:1, WD full
/// condition at 5:4, the two WP full conditions at 7:6 and 9:8, the H2C
/// full condition at 11:10.
pub fn hfc_ctrl_mix(old: u32, p: &HfcPrec) -> u32 {
    let mask = 0x3 << 1 | 0x3 << 4 | 0x3 << 6 | 0x3 << 8 | 0x3 << 10;
    (old & !mask) | p.ch011_full << 4 | p.wp07_full << 6 | p.wp811_full << 8 | p.h2c_full << 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_fit_the_buffer() {
        // dle_init_ax refuses a configuration that does not fill exactly
        // the buffer left to the host.
        assert_eq!(DLFW.used(), FIFO_SIZE);
        assert_eq!(SCC.used(), FIFO_SIZE - SCC_RSVD);
    }

    #[test]
    fn register_values() {
        assert_eq!(DLFW.pktbuf(), [(0, 0, 0), (1, 8, 64)]);
        assert_eq!(SCC.pktbuf(), [(0, 0, 510), (1, 4, 496)]);
        let (wde, ple) = DLFW.quota_regs(Some(DLFW_EXT_WDE_WCPU));
        assert_eq!(wde, [0, 48, 0, 0]);
        assert_eq!(ple[2], 16 | 16 << 16);
        assert_eq!(ple[3], 48 | 48 << 16);
        let (wde, ple) = SCC.quota_regs(None);
        assert_eq!(wde, [446 | 446 << 16, 48 | 48 << 16, 0, 16 | 16 << 16]);
        assert_eq!(ple[4], 17 | 157 << 16);
        assert_eq!(ple[6], 89 | 229 << 16);
        assert_eq!(SCC.rx_mpdu_max_units(), 22);
        assert_eq!(hfc_ch_reg(HFC_CH[0]), 5 | 341 << 16);
        // The page counts given to the host channels fit the public pages.
        let max: u32 = HFC_CH.iter().enumerate().filter(|(i, _)| !hfc_ch_unused(*i)).map(|(_, c)| c.min).sum();
        assert!(max + HFC_PREC.h2c <= HFC_PUB.2 + HFC_PREC.h2c);
        assert_eq!(hfc_ctrl_mix(0xFFFF_FFFF, &HFC_PREC), 0xFFFF_F019);
    }
}
