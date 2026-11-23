//! The PCIe side of the card: the DMA rings (transmit channels ACH0-3, the
//! management and high queues of band 0, the firmware command channel 12;
//! the receive queue and the release report queue), the set-up before the
//! firmware download and after the MAC init, the MDIO and DBI side doors
//! to the PCIe PHY and configuration space, and the interrupt masks.
//!
//! From the Linux rtw89 driver (pci.c, pci.h, rtw8852be.c), Copyright(c)
//! 2019-2022 Realtek Corporation, used under BSD-3-Clause; written again.
//!
//! The firmware download uses the rings and `pci_pre_init`; the rest (the
//! data channels, `pci_post_init`, the interrupts) waits for the later
//! stages.

#![allow(dead_code)]

use crate::{bit, Chip, Fail};
use alloc::vec::Vec;
use justdrv::Dma;
use rtw8852b_fw::desc;

// Registers (reg.h, pci.h).
const PCIE_INIT_CFG1: usize = 0x1000;
const PCIE_INIT_CFG2: usize = 0x1004;
const PCIE_PS_CTRL: usize = 0x1008;
const PCIE_DMA_STOP1: usize = 0x1010;
const TXBD_RWPTR_CLR1: usize = 0x1014;
const RXBD_RWPTR_CLR: usize = 0x1018;
const PCIE_DMA_BUSY1: usize = 0x101C;
const PCIE_DBI_FLAG: usize = 0x1090;
const PCIE_DBI_WDATA: usize = 0x1094;
const PCIE_DBI_RDATA: usize = 0x1098;
const MDIO_CFG: usize = 0x10A0;
const MDIO_WDATA: usize = 0x10A4;
const MDIO_RDATA: usize = 0x10A6;
const PCIE_HIMR00: usize = 0x10B0;
const PCIE_HISR00: usize = 0x10B4;
const LBC_WATCHDOG: usize = 0x11D8;
const PCIE_DBG_CTRL: usize = 0x11C0;
const PCIE_HIMR10: usize = 0x13B0;
const PCIE_HISR10: usize = 0x13B4;
const PCIE_EXP_CTRL: usize = 0x13F0;
const TX_ADDRESS_INFO_MODE: usize = 0x8810;
const PKTIN_SETTING: usize = 0x9A00;
const HIMR0: usize = 0x01A0;
const HISR0: usize = 0x01A4;
const SYS_PW_CTRL: usize = 0x0004;
const SYS_SDIO_CTRL: usize = 0x0070;
const HCI_OPT_CTRL: usize = 0x0074;

// PCIE_INIT_CFG1
const RST_BDRAM: u32 = bit(3);
const TXHCI_EN: u32 = bit(11);
const LATENCY_CONTROL: u32 = bit(12);
const RXHCI_EN: u32 = bit(13);
const RXBD_MODE: u32 = bit(18);
const RXRST_KEEP_REG: u32 = bit(23);
const TXRST_KEEP_REG: u32 = bit(22);
// PCIE_DMA_STOP1
const STOP_WPDMA: u32 = bit(19);
const STOP_PCIEIO: u32 = bit(20);
/// ACH0-3, CH8, CH9, CH12 (TX_STOP1_MASK_V1 on the 8852B).
const TX_STOP1_MASK: u32 = bit(8) | bit(9) | bit(10) | bit(11) | bit(16) | bit(17) | bit(18);
const STOP_CH12: u32 = bit(18);

/// The PCI configuration space bytes the set-up reads (pci.h).
const CFG_LINK_STATUS: u16 = 0x82;
const CFG_L1_CTRL: u16 = 0x719;
const CFG_L1_CTRL_CLKREQ: u8 = 1 << 3;
// MDIO registers of the PCIe PHY (pci.h).
const RAC_REG_REV2: u8 = 0x1B;
const RAC_REG_FLD_0: u8 = 0x1D;
const RAC_CTRL_PPR_V1: u8 = 0x30;
const CALIB_EN: u16 = 1 << 13;

/// The DMA channels the driver sets up: their registers (NUM, IDX, BDRAM,
/// DESA_L; rtw89_pci_ch_dma_addr_set) and their slice of the BD RAM
/// (rtw89_bd_ram_table_single).
pub struct TxCh {
    pub num: usize,
    pub idx: usize,
    pub bdram: usize,
    pub desa: usize,
    pub ram: (u32, u32, u32),
}

pub const ACH0: TxCh = TxCh { num: 0x1024, idx: 0x1058, bdram: 0x1200, desa: 0x1110, ram: (0, 5, 2) };
pub const ACH1: TxCh = TxCh { num: 0x1026, idx: 0x105C, bdram: 0x1204, desa: 0x1118, ram: (5, 5, 2) };
pub const ACH2: TxCh = TxCh { num: 0x1028, idx: 0x1060, bdram: 0x1208, desa: 0x1120, ram: (10, 5, 2) };
pub const ACH3: TxCh = TxCh { num: 0x102A, idx: 0x1064, bdram: 0x120C, desa: 0x1128, ram: (15, 5, 2) };
pub const CH8: TxCh = TxCh { num: 0x1034, idx: 0x1078, bdram: 0x1220, desa: 0x1150, ram: (20, 4, 1) };
pub const CH9: TxCh = TxCh { num: 0x1036, idx: 0x107C, bdram: 0x1224, desa: 0x1158, ram: (24, 4, 1) };
pub const CH12: TxCh = TxCh { num: 0x1038, idx: 0x1080, bdram: 0x1228, desa: 0x1160, ram: (28, 4, 1) };

pub struct RxCh {
    pub num: usize,
    pub idx: usize,
    pub desa: usize,
}

pub const RXQ: RxCh = RxCh { num: 0x1020, idx: 0x1050, desa: 0x1100 };
pub const RPQ: RxCh = RxCh { num: 0x1022, idx: 0x1054, desa: 0x1108 };

/// Bytes of a buffer: transmit slots, and receive buffers
/// (RTW89_PCI_RX_BUF_SIZE: the largest A-MSDU, the descriptor, the info).
pub const TX_BUF: usize = 4096;
pub const RX_BUF: usize = 11454 + 40 + 4;
/// Where a frame sits in its transmit buffer, after the WD page.
pub const TX_FRAME_AT: usize = 128;

/// A transmit ring: buffer descriptors, and one buffer per slot. A
/// firmware command uses the buffer of its BD; a frame uses a WD page
/// (`pages`), which stays busy until its release report comes back.
pub struct TxRing {
    pub ch: &'static TxCh,
    pub bd: Dma,
    pub len: u32,
    pub wp: u32,
    pub rp: u32,
    pub bufs: Vec<Dma>,
    pub busy: Vec<bool>,
}

impl TxRing {
    fn new(ch: &'static TxCh, len: u32, bufs: usize) -> Option<TxRing> {
        let bd = Dma::new(len as usize * 8)?;
        let mut v = Vec::new();
        for _ in 0..bufs {
            v.push(Dma::new(TX_BUF)?);
        }
        Some(TxRing { ch, bd, len, wp: 0, rp: 0, busy: alloc::vec![false; bufs], bufs: v })
    }

    /// Catches up with the hardware (rtw89_pci_txbd_recalc).
    pub fn reclaim(&mut self, chip: &Chip) {
        let (_, hw) = desc::ring_done(self.len, self.rp, chip.r32(self.ch.idx));
        if hw < self.len {
            self.rp = hw;
        }
    }

    pub fn free(&self) -> u32 {
        desc::ring_free(self.len, self.wp, self.rp)
    }

    /// Queues `len` bytes at `dma` in the next slot and kicks the channel
    /// (rtw89_pci_tx_bd_ring_update, __rtw89_pci_tx_kick_off).
    pub fn push(&mut self, chip: &Chip, len: usize, dma: u64) {
        let b = desc::txbd(len, dma);
        let at = self.wp as usize * 8;
        self.bd.as_mut_slice()[at..at + 8].copy_from_slice(&b);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.wp = (self.wp + 1) % self.len;
        chip.r.write16(self.ch.idx, self.wp as u16);
    }
}

pub struct RxRing {
    pub ch: &'static RxCh,
    pub bd: Dma,
    pub len: u32,
    pub wp: u32,
    pub bufs: Vec<Dma>,
}

impl RxRing {
    fn new(ch: &'static RxCh, len: u32) -> Option<RxRing> {
        let bd = Dma::new(len as usize * 8)?;
        let mut bufs = Vec::new();
        for i in 0..len as usize {
            let b = Dma::new(RX_BUF)?;
            bd.as_mut_slice()[i * 8..i * 8 + 8].copy_from_slice(&desc::rxbd(RX_BUF, b.phys()));
            bufs.push(b);
        }
        Some(RxRing { ch, bd, len, wp: 0, bufs })
    }

    /// Buffers the card filled since the last call (rtw89_pci_rxbd_recalc,
    /// rx_ring_eq_is_full false on the 8852B).
    pub fn filled(&self, chip: &Chip) -> u32 {
        let (n, hw) = desc::ring_done(self.len, self.wp, chip.r32(self.ch.idx));
        if hw >= self.len {
            0
        } else {
            n
        }
    }

    /// Hands `n` buffers back to the card.
    pub fn give_back(&mut self, chip: &Chip, n: u32) {
        self.wp = (self.wp + n) % self.len;
        chip.r.write16(self.ch.idx, self.wp as u16);
    }
}

/// All the rings.
pub struct Rings {
    /// ACH0 (data), CH8 (management), CH12 (firmware commands); then the
    /// rings rtw89 sets up and this driver leaves empty: ACH1-3, CH9.
    pub ach0: TxRing,
    pub mgmt: TxRing,
    pub fwcmd: TxRing,
    pub idle: Vec<TxRing>,
    pub rxq: RxRing,
    pub rpq: RxRing,
}

impl Rings {
    pub fn new() -> Option<Rings> {
        let mut idle = Vec::new();
        for ch in [&ACH1, &ACH2, &ACH3, &CH9] {
            idle.push(TxRing::new(ch, 16, 0)?);
        }
        Some(Rings {
            ach0: TxRing::new(&ACH0, 64, 64)?,
            mgmt: TxRing::new(&CH8, 64, 64)?,
            fwcmd: TxRing::new(&CH12, 32, 32)?,
            idle,
            rxq: RxRing::new(&RXQ, 128)?,
            rpq: RxRing::new(&RPQ, 64)?,
        })
    }

    fn tx_rings(&mut self) -> impl Iterator<Item = &mut TxRing> {
        [&mut self.ach0, &mut self.mgmt, &mut self.fwcmd].into_iter().chain(self.idle.iter_mut())
    }

    /// Gives the rings to the card (rtw89_pci_reset_trx_rings).
    fn reset(&mut self, chip: &Chip) {
        for t in self.tx_rings() {
            t.wp = 0;
            t.rp = 0;
            t.busy.iter_mut().for_each(|b| *b = false);
            let (sidx, max, min) = t.ch.ram;
            chip.r.write16(t.ch.num, t.len as u16);
            chip.w32(t.ch.bdram, sidx | max << 8 | min << 16);
            chip.w32(t.ch.desa, t.bd.phys() as u32);
            chip.w32(t.ch.desa + 4, (t.bd.phys() >> 32) as u32);
        }
        for r in [&mut self.rxq, &mut self.rpq] {
            r.wp = 0;
            chip.r.write16(r.ch.num, r.len as u16);
            chip.w32(r.ch.desa, r.bd.phys() as u32);
            chip.w32(r.ch.desa + 4, (r.bd.phys() >> 32) as u32);
        }
    }

    /// Sends a firmware command, or a piece of the firmware (`fw_dl`):
    /// the WD body, then `data` (rtw89_pci_fwcmd_submit).
    pub fn h2c(&mut self, chip: &Chip, data: &[u8], fw_dl: bool) -> Result<(), Fail> {
        let ring = &mut self.fwcmd;
        let start = justdrv::time_ms();
        loop {
            ring.reclaim(chip);
            if ring.free() > 0 {
                break;
            }
            if justdrv::time_ms() - start > 200 {
                return Err(Fail("a free slot on the firmware command channel", chip.r32(ring.ch.idx)));
            }
            core::hint::spin_loop();
        }
        let slot = ring.wp as usize;
        let buf = &ring.bufs[slot];
        let n = desc::WD_BODY_LEN + data.len();
        if n > TX_BUF {
            return Err(Fail("a firmware command too long", n as u32));
        }
        let b = buf.as_mut_slice();
        b[..desc::WD_BODY_LEN].fill(0);
        desc::TxDesc::fwcmd(data.len(), fw_dl).fill(b);
        b[desc::WD_BODY_LEN..n].copy_from_slice(data);
        let phys = buf.phys();
        ring.push(chip, n, phys);
        Ok(())
    }

    /// Waits until the card took every queued firmware command.
    pub fn h2c_flush(&mut self, chip: &Chip, ms: u64) -> bool {
        let ring = &mut self.fwcmd;
        justdrv::wait_until(ms, || {
            ring.reclaim(chip);
            ring.rp == ring.wp
        })
    }
}

impl Chip {
    /// A byte of the PCI configuration space through the card's DBI window
    /// (rtw89_dbi_read8; the kernel's configuration access stops at 256).
    pub(crate) fn dbi_read8(&self, addr: u16) -> Result<u8, Fail> {
        let a = (addr & 0xFFC) as u16;
        self.r.write16(PCIE_DBI_FLAG, a);
        self.r.write8(PCIE_DBI_FLAG + 2, 2);
        self.poll8("the DBI read", PCIE_DBI_FLAG + 2, 2, |v| v == 0)?;
        Ok(self.r.read8(PCIE_DBI_RDATA + (addr & 3) as usize))
    }

    pub(crate) fn dbi_write8(&self, addr: u16, v: u8) -> Result<(), Fail> {
        let a = addr & 0xFFC | (1 << (addr & 3)) << 12;
        self.r.write8(PCIE_DBI_WDATA + (addr & 3) as usize, v);
        self.r.write16(PCIE_DBI_FLAG, a);
        self.r.write8(PCIE_DBI_FLAG + 2, 1);
        self.poll8("the DBI write", PCIE_DBI_FLAG + 2, 2, |v| v == 0)?;
        Ok(())
    }

    /// The PCIe PHY's registers (rtw89_pci_check_mdio, rtw89_read16_mdio,
    /// rtw89_write16_mdio): page by address, generation 1 or 2.
    fn mdio_set(&self, addr: u8, gen2: bool) {
        self.r.write8(MDIO_CFG, addr & 0x1F);
        let page: u16 = match (gen2, addr < 0x20) {
            (false, true) => 0,
            (false, false) => 1,
            (true, true) => 2,
            (true, false) => 3,
        };
        let v = self.r.read16(MDIO_CFG) & !(0x3 << 12) | page << 12;
        self.r.write16(MDIO_CFG, v);
    }

    fn mdio_go(&self, flag: u16) -> Result<(), Fail> {
        self.r.write16(MDIO_CFG, self.r.read16(MDIO_CFG) | flag);
        let start = justdrv::time_ms();
        while self.r.read16(MDIO_CFG) & flag != 0 {
            if justdrv::time_ms() - start > 2 {
                return Err(Fail("the PCIe PHY (MDIO)", self.r.read16(MDIO_CFG) as u32));
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    fn mdio_read(&self, addr: u8, gen2: bool) -> Result<u16, Fail> {
        self.mdio_set(addr, gen2);
        self.mdio_go(1 << 9)?;
        Ok(self.r.read16(MDIO_RDATA))
    }

    fn mdio_write(&self, addr: u8, v: u16, gen2: bool) -> Result<(), Fail> {
        self.r.write16(MDIO_WDATA, v);
        self.mdio_set(addr, gen2);
        self.mdio_go(1 << 8)
    }

    fn mdio_mask(&self, addr: u8, mask: u16, v: u16, gen2: bool) -> Result<(), Fail> {
        let old = self.mdio_read(addr, gen2)?;
        let shift = mask.trailing_zeros();
        self.mdio_write(addr, old & !mask | (v << shift) & mask, gen2)
    }

    /// rtw89_pci_auto_refclk_cal(false), the 8852B part: the reference
    /// clock calibration off.
    fn refclk_cal_off(&self, pci: &justdrv::Pci) -> Result<(), Fail> {
        let gen = pci.read8(CFG_LINK_STATUS) & 0x3;
        let gen2 = match gen {
            1 => false,
            2 => true,
            _ => return Err(Fail("a PCIe link speed of generation 1 or 2", gen as u32)),
        };
        let l1 = self.dbi_read8(CFG_L1_CTRL)?;
        if l1 & CFG_L1_CTRL_CLKREQ != 0 {
            self.dbi_write8(CFG_L1_CTRL, l1 & !CFG_L1_CTRL_CLKREQ)?;
        }
        let v = self.mdio_read(RAC_CTRL_PPR_V1, gen2)?;
        if v & CALIB_EN != 0 {
            self.mdio_write(RAC_CTRL_PPR_V1, v & !CALIB_EN, gen2)?;
        }
        if l1 & CFG_L1_CTRL_CLKREQ != 0 {
            self.dbi_write8(CFG_L1_CTRL, l1)?;
        }
        Ok(())
    }

    pub(crate) fn ctrl_dma_all(&self, enable: bool) {
        if enable {
            self.clr32(PCIE_DMA_STOP1, STOP_PCIEIO);
            self.set32(PCIE_INIT_CFG1, TXHCI_EN | RXHCI_EN);
        } else {
            self.set32(PCIE_DMA_STOP1, STOP_PCIEIO);
            self.clr32(PCIE_INIT_CFG1, TXHCI_EN | RXHCI_EN);
        }
    }

    /// rtw89_pci_ops_mac_pre_init_ax for the 8852B: the PCIe set-up before
    /// the firmware download, the DMA stopped while the rings are given
    /// to the card, then the firmware command channel opened.
    pub(crate) fn pci_pre_init(&self, pci: &justdrv::Pci, rings: &mut Rings) -> Result<(), Fail> {
        self.clr32(PCIE_PS_CTRL, bit(5));
        self.clr32(SYS_PW_CTRL, bit(14));
        self.set32(SYS_SDIO_CTRL, bit(15));
        self.clr32(SYS_SDIO_CTRL, bit(14));
        self.mdio_mask(RAC_REG_REV2, 0xF << 12, 1, false)?;
        self.mdio_mask(RAC_REG_FLD_0, 0x3 << 2, 3, false)?;
        self.refclk_cal_off(pci)?;
        self.set32(HCI_OPT_CTRL, bit(5));
        self.clr32(PCIE_EXP_CTRL, bit(4));
        // The local bus watchdog: 2 ms, on (rtw89_pci_lbc_watchdog).
        let v = self.r32(LBC_WATCHDOG) & !(0xF << 4) | 8 << 4 | bit(1) | bit(0);
        self.w32(LBC_WATCHDOG, v);
        self.set32(LBC_WATCHDOG, bit(1) | bit(0));
        self.set32(PCIE_DBG_CTRL, bit(1) | bit(0));
        self.set32(PCIE_INIT_CFG1, TXRST_KEEP_REG | RXRST_KEEP_REG);
        self.set32(PCIE_DMA_STOP1, STOP_WPDMA);
        self.ctrl_dma_all(false);
        self.poll("the PCIe DMA to stop (TX)", PCIE_DMA_BUSY1, 2, |v| v & (TX_STOP1_MASK) == 0)?;
        self.poll("the PCIe DMA to stop (RX)", PCIE_DMA_BUSY1, 2, |v| v & (bit(0) | bit(1)) == 0)?;
        self.set32(TXBD_RWPTR_CLR1, bit(0) | bit(1) | bit(2) | bit(3) | bit(8) | bit(9) | bit(10));
        self.set32(RXBD_RWPTR_CLR, bit(0) | bit(1));
        // Mode: truncated BDs (no register to write on the 8852B), DMA
        // burst sizes, tag number, write-back intervals.
        self.clr32(PCIE_INIT_CFG1, RXBD_MODE);
        self.mask32(PCIE_INIT_CFG1, 0x7 << 8, 7);
        self.mask32(PCIE_INIT_CFG1, 0x7 << 14, 3);
        self.set32(PCIE_INIT_CFG1, LATENCY_CONTROL);
        self.mask32(PCIE_EXP_CTRL, 0x7 << 16, 7);
        self.mask32(PCIE_INIT_CFG2, 0xF << 24, 1);
        self.mask32(PCIE_INIT_CFG2, 0xF << 16, 1);
        self.set32(TX_ADDRESS_INFO_MODE, bit(0));
        self.clr32(PKTIN_SETTING, bit(1));
        rings.reset(self);
        self.set32(PCIE_INIT_CFG1, RST_BDRAM);
        self.poll("the BD RAM reset", PCIE_INIT_CFG1, 2, |v| v & RST_BDRAM == 0)?;
        self.set32(PCIE_DMA_STOP1, TX_STOP1_MASK);
        self.clr32(PCIE_DMA_STOP1, STOP_CH12);
        self.ctrl_dma_all(true);
        Ok(())
    }

    /// rtw89_pci_ops_mac_post_init_ax for the 8852B: every transmit
    /// channel opened. (rtw89 also sets the latency tolerance values when
    /// the device's LTR is on; this driver leaves them.)
    pub(crate) fn pci_post_init(&self) {
        self.set32(TX_ADDRESS_INFO_MODE, bit(0));
        self.clr32(PKTIN_SETTING, bit(1));
        self.clr32(PCIE_DMA_STOP1, TX_STOP1_MASK);
        self.clr32(PCIE_DMA_STOP1, STOP_WPDMA | STOP_PCIEIO);
    }

    /// The interrupts rtw89 enables on the 8852B (rtw89_pci_config_intr_mask):
    /// the firmware's halt, receive and release DMA, the stuck and
    /// overflow warnings.
    pub(crate) fn irq_enable(&self) {
        self.w32(HIMR0, bit(21));
        self.w32(
            PCIE_HIMR00,
            bit(17) | bit(0) | bit(1) | bit(2) | bit(18) | bit(19) | bit(20) | bit(24),
        );
        self.w32(PCIE_HIMR10, bit(28));
    }

    pub(crate) fn irq_disable(&self) {
        self.w32(HIMR0, 0);
        self.w32(PCIE_HIMR00, 0);
        self.w32(PCIE_HIMR10, 0);
    }

    /// Reads and clears the causes (rtw89_pci_recognize_intrs): the
    /// firmware's, the PCIe ones.
    pub(crate) fn irq_causes(&self) -> (u32, u32) {
        let isr0 = self.r32(HISR0) & self.r32(HIMR0);
        let isr00 = self.r32(PCIE_HISR00) & self.r32(PCIE_HIMR00);
        let isr10 = self.r32(PCIE_HISR10) & self.r32(PCIE_HIMR10);
        self.w32(HISR0, isr0);
        self.w32(PCIE_HISR00, isr00);
        self.w32(PCIE_HISR10, isr10);
        (isr0, isr00)
    }
}

/// PCIE_HISR00 bits: receive DMA, release report DMA.
pub const ISR_RXDMA: u32 = bit(0) | bit(1);
pub const ISR_RPQDMA: u32 = bit(2);
