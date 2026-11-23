//! The firmware download: the DMAC set up for it (the DLE buffers in the
//! download mode, the host flow control of the command channel), the card's
//! CPU (WCPU) started in download mode, the firmware's header and sections
//! sent as H2C packets on channel 12, and the status the CPU reports.
//!
//! From the Linux rtw89 driver (mac.c: rtw89_mac_partial_init,
//! rtw89_mac_dmac_pre_init, rtw89_mac_dle_init, rtw89_mac_hfc_init,
//! rtw89_mac_enable_cpu_ax, rtw89_mac_disable_cpu_ax; fw.c:
//! rtw89_fw_download and the functions it calls; reg.h), Copyright(c)
//! 2019-2022 Realtek Corporation, used under BSD-3-Clause; written again.

use crate::pci::Rings;
use crate::{bit, Chip, Fail};
use rtw8852b_fw::quota::{Dle, DLFW, DLFW_EXT_WDE_WCPU, HFC_PREC};
use rtw8852b_fw::{self as fw, desc};

// Registers (reg.h).
const SYS_CLK_CTRL: usize = 0x0008;
const DBG_CTRL: usize = 0x0058;
const PLATFORM_ENABLE: usize = 0x0088;
const SCOREBOARD: usize = 0x00AC;
const DBG_PORT_SEL: usize = 0x00C0;
const SYS_STATUS1: usize = 0x00F4;
const HALT_H2C_CTRL: usize = 0x0160;
const HALT_C2H_CTRL: usize = 0x0164;
const HALT_H2C: usize = 0x0168;
const HALT_C2H: usize = 0x016C;
pub(crate) const WCPU_FW_CTRL: usize = 0x01E0;
const BOOT_REASON: usize = 0x01E6;
const UDM1: usize = 0x01F4;
const UDM2: usize = 0x01F8;
const SEC_CTRL: usize = 0x0C00;
const BOOT_DBG: usize = 0x83F0;
const HCI_FUNC_EN: usize = 0x8380;
const DMAC_FUNC_EN: usize = 0x8400;
const DMAC_CLK_EN: usize = 0x8404;
const HCI_FC_CTRL: usize = 0x8A00;
const CH_PAGE_CTRL: usize = 0x8A04;
const WDE_PKTBUF_CFG: usize = 0x8C08;
const WDE_QTA_CFG: [usize; 4] = [0x8C40, 0x8C44, 0x8C4C, 0x8C50];
pub(crate) const WDE_INI_STATUS: usize = 0x8D00;
const PLE_PKTBUF_CFG: usize = 0x9008;
const PLE_QTA0_CFG: usize = 0x9040;
pub(crate) const PLE_INI_STATUS: usize = 0x9100;

// PLATFORM_ENABLE
const PLATFORM_EN: u32 = bit(0);
const WCPU_EN: u32 = bit(1);
const APB_WRAP_EN: u32 = bit(2);
// SYS_CLK_CTRL
const CPU_CLK_EN: u32 = bit(14);
// WCPU_FW_CTRL
const FWDL_EN: u32 = bit(0);
const H2C_PATH_RDY: u8 = 1 << 1;
const FWDL_PATH_RDY: u8 = 1 << 2;
const FWDL_STS: u32 = 0x7 << 5;
// HCI_FUNC_EN
const HCI_TXDMA_EN: u32 = bit(0);
const HCI_RXDMA_EN: u32 = bit(1);
// DMAC_FUNC_EN and DMAC_CLK_EN
const MAC_FUNC_EN: u32 = bit(30);
const DMAC_FUNC_EN_BIT: u32 = bit(29);
const DLE_WDE_EN: u32 = bit(26);
const DLE_PLE_EN: u32 = bit(23);
const PKT_BUF_EN: u32 = bit(22);
const DISPATCHER_EN: u32 = bit(18);
// HCI_FC_CTRL
const HCI_FC_EN: u32 = bit(0);
const HCI_FC_CH12_EN: u32 = bit(3);
const HCI_FC_CH12_FULL_COND: u32 = 0x3 << 10;
/// WDE_INI_STATUS, PLE_INI_STATUS: queue and buffer managers ready.
const MGN_INI_RDY: u32 = bit(1) | bit(0);

/// The scoreboard's top byte: the driver is up (MAC_AX_NOTIFY_TP_MAJOR),
/// or powered off (MAC_AX_NOTIFY_PWR_MAJOR).
const NOTIFY_TP_MAJOR: u8 = 0x81;
pub(crate) const NOTIFY_PWR_MAJOR: u8 = 0x80;

/// The download's status (WCPU_FW_CTRL 7:5, enum rtw89_fw_dl_status).
pub(crate) const FWDL_INIT_RDY: u8 = 7;

/// How long rtw89 waits for the paths and the status (FWDL_WAIT_CNT, 1 us
/// steps): 400 ms.
const FWDL_WAIT_MS: u64 = 400;

pub(crate) fn status_name(s: u8) -> &'static str {
    match s {
        0 => "initial state",
        1 => "download going on",
        2 => "checksum failed",
        3 => "security check failed",
        4 => "firmware for another chip cut",
        6 => "WCPU download ready",
        7 => "firmware running",
        _ => "unknown",
    }
}

impl Chip {
    /// Waits up to `ms` for `cond` on the byte register `a`.
    pub(crate) fn poll8(&self, what: &'static str, a: usize, ms: u64, cond: impl Fn(u8) -> bool) -> Result<u8, Fail> {
        let start = justdrv::time_ms();
        loop {
            let v = self.r.read8(a);
            if cond(v) {
                return Ok(v);
            }
            if justdrv::time_ms() - start > ms {
                return Err(Fail(what, v as u32));
            }
            core::hint::spin_loop();
        }
    }

    /// After the power-on (rtw89_mac_power_switch, rtw89_mac_partial_init):
    /// the scoreboard says the driver is up, and the host DMA is enabled.
    pub(crate) fn hci_dma_on(&self) {
        self.r.write8(SCOREBOARD + 3, NOTIFY_TP_MAJOR);
        self.set32(HCI_FUNC_EN, HCI_TXDMA_EN | HCI_RXDMA_EN);
    }

    /// Before the power-off: the scoreboard says the driver is gone.
    pub(crate) fn notify_power_off(&self) {
        self.r.write8(SCOREBOARD + 3, NOTIFY_PWR_MAJOR);
    }

    /// rtw89_mac_check_mac_en for the DMAC of MAC 0.
    fn check_mac_en(&self) -> Result<(), Fail> {
        let v = self.r32(DMAC_FUNC_EN);
        let on = MAC_FUNC_EN | DMAC_FUNC_EN_BIT;
        if v == u32::MAX || v & on != on {
            return Err(Fail("the DMAC to be on", v));
        }
        Ok(())
    }

    /// rtw89_mac_dle_init: the packet buffers shared out as `cfg` says.
    fn dle_init(&self, cfg: &Dle, ext_wde_wcpu: Option<u32>) -> Result<(), Fail> {
        self.check_mac_en()?;
        self.clr32(DMAC_FUNC_EN, DLE_WDE_EN | DLE_PLE_EN);
        self.set32(DMAC_CLK_EN, DLE_WDE_EN | DLE_PLE_EN);
        // dle_mix_cfg_ax: page size, start boundary, free pages.
        for ((sel, bound, free), reg) in cfg.pktbuf().into_iter().zip([WDE_PKTBUF_CFG, PLE_PKTBUF_CFG]) {
            let v = self.r32(reg) & !(0x3 | 0x3F << 8 | 0x1FFF << 16);
            self.w32(reg, v | sel | (bound & 0x3F) << 8 | (free & 0x1FFF) << 16);
        }
        // dle_quota_cfg.
        let (wde, ple) = cfg.quota_regs(ext_wde_wcpu);
        for (reg, v) in WDE_QTA_CFG.into_iter().zip(wde) {
            self.w32(reg, v);
        }
        for (i, v) in ple.into_iter().enumerate() {
            self.w32(PLE_QTA0_CFG + 4 * i, v);
        }
        self.set32(DMAC_FUNC_EN, DLE_WDE_EN | DLE_PLE_EN);
        self.poll("the WDE buffer to be ready", WDE_INI_STATUS, 2, |v| v & MGN_INI_RDY == MGN_INI_RDY)?;
        self.poll("the PLE buffer to be ready", PLE_INI_STATUS, 2, |v| v & MGN_INI_RDY == MGN_INI_RDY)?;
        Ok(())
    }

    /// rtw89_mac_dmac_pre_init: the DMAC's functions, the DLE in the
    /// download mode, and the host flow control with only the command
    /// channel on (rtw89_mac_hfc_init(reset, !en, h2c_en)).
    pub(crate) fn dmac_pre_init(&self) -> Result<(), Fail> {
        self.w32(DMAC_FUNC_EN, MAC_FUNC_EN | DMAC_FUNC_EN_BIT | DISPATCHER_EN | PKT_BUF_EN);
        self.w32(DMAC_CLK_EN, DISPATCHER_EN);
        self.dle_init(&DLFW, Some(DLFW_EXT_WDE_WCPU))?;
        self.check_mac_en()?;
        self.clr32(HCI_FC_CTRL, HCI_FC_EN | HCI_FC_CH12_EN);
        self.w32(CH_PAGE_CTRL, (HFC_PREC.h2c & 0x1FF) << 16);
        self.mask32(HCI_FC_CTRL, HCI_FC_CH12_FULL_COND, HFC_PREC.h2c_full);
        let v = self.r32(HCI_FC_CTRL) & !HCI_FC_EN | HCI_FC_CH12_EN;
        self.w32(HCI_FC_CTRL, v);
        Ok(())
    }

    /// rtw89_mac_disable_cpu_ax, with rtw89_disable_fw_watchdog of the
    /// 8852B (the APB wrapper reset).
    pub(crate) fn disable_cpu(&self) {
        self.clr32(PLATFORM_ENABLE, WCPU_EN);
        self.clr32(WCPU_FW_CTRL, FWDL_EN | (H2C_PATH_RDY | FWDL_PATH_RDY) as u32);
        self.clr32(SYS_CLK_CTRL, CPU_CLK_EN);
        self.clr32(PLATFORM_ENABLE, APB_WRAP_EN);
        self.set32(PLATFORM_ENABLE, APB_WRAP_EN);
        self.clr32(PLATFORM_ENABLE, PLATFORM_EN);
        self.set32(PLATFORM_ENABLE, PLATFORM_EN);
    }

    /// rtw89_mac_enable_cpu_ax(boot reason 0, download mode).
    fn enable_cpu(&self) -> Result<(), Fail> {
        let p = self.r32(PLATFORM_ENABLE);
        if p & WCPU_EN != 0 {
            return Err(Fail("the WCPU to be off before it is started", p));
        }
        for r in [UDM1, UDM2, HALT_H2C_CTRL, HALT_C2H_CTRL, HALT_H2C, HALT_C2H] {
            self.w32(r, 0);
        }
        self.set32(SYS_CLK_CTRL, CPU_CLK_EN);
        let v = self.r32(WCPU_FW_CTRL) & !(FWDL_EN | (H2C_PATH_RDY | FWDL_PATH_RDY) as u32 | FWDL_STS);
        self.w32(WCPU_FW_CTRL, v | FWDL_EN);
        // The 8852B's instruction and data memories (SEC_IDMEM_SIZE_CONFIG).
        self.mask32(SEC_CTRL, 0x3 << 16, 0x2);
        let b = self.r.read16(BOOT_REASON) & !0x7;
        self.r.write16(BOOT_REASON, b);
        self.set32(PLATFORM_ENABLE, WCPU_EN);
        Ok(())
    }

    /// The download's status (rtw89_fw_get_rdy_ax).
    pub(crate) fn fw_status(&self) -> u8 {
        (self.r.read8(WCPU_FW_CTRL) >> 5) & 0x7
    }

    /// What rtw89 prints when the download fails (rtw89_fw_dl_fail_dump):
    /// WCPU_FW_CTRL, BOOT_DBG, and the CPU's program counter.
    pub(crate) fn fw_fail_dump(&self) {
        justdrv::warn!(
            "  WCPU_FW_CTRL {:#010x}, BOOT_DBG {:#010x}, PLATFORM_ENABLE {:#010x}",
            self.r32(WCPU_FW_CTRL),
            self.r32(BOOT_DBG),
            self.r32(PLATFORM_ENABLE)
        );
        self.w32(DBG_CTRL, 0xF2 | 0xF2 << 16);
        self.mask32(SYS_STATUS1, 0x3 << 16, 1);
        let mut pc = [0u32; 15];
        for p in pc.iter_mut() {
            *p = self.r32(DBG_PORT_SEL);
            justdrv::sleep_ms(1);
        }
        justdrv::warn!("  WCPU program counter: {:#010x?}", pc);
    }
}

/// One attempt of __rtw89_fw_download: the CPU restarted in download mode,
/// the header, the sections, then the status. An error says at which
/// stage (9 to 12) it stopped.
pub(crate) fn download(chip: &Chip, rings: &mut Rings, img: &fw::Image<'_>) -> Result<(usize, usize), (u32, Fail)> {
    // Stage 9: the CPU, and the command path.
    chip.disable_cpu();
    chip.enable_cpu().map_err(|e| (9, e))?;
    chip.poll8("the H2C path to be ready", WCPU_FW_CTRL, FWDL_WAIT_MS, |v| v & H2C_PATH_RDY != 0)
        .map_err(|e| (9, e))?;
    justdrv::info!("stage 9: WCPU started in download mode, H2C path ready (WCPU_FW_CTRL {:#010x})", chip.r32(WCPU_FW_CTRL));

    // Stage 10: the header (H2C class FWDL, function FWHDR_DL).
    let hdr = fw::download_header(img);
    let mut pkt = alloc::vec::Vec::with_capacity(desc::H2C_HDR_LEN + hdr.len());
    pkt.extend_from_slice(&desc::H2cSeq::default().fwdl(hdr.len()));
    pkt.extend_from_slice(&hdr);
    rings.h2c(chip, &pkt, false).map_err(|e| (10, e))?;
    chip.poll8("the download path to be ready (header taken)", WCPU_FW_CTRL, FWDL_WAIT_MS, |v| v & FWDL_PATH_RDY != 0)
        .map_err(|e| (10, e))?;
    chip.w32(HALT_H2C_CTRL, 0);
    chip.w32(HALT_C2H_CTRL, 0);
    justdrv::info!("stage 10: header sent ({} bytes), download path ready", hdr.len());

    // Stage 11: the sections, in pieces of PART_LEN bytes.
    let mut packets = 0;
    let mut bytes = 0;
    for s in &img.sections {
        for (at, len) in fw::parts(s) {
            rings.h2c(chip, &img.data[at..at + len], true).map_err(|e| (11, e))?;
            packets += 1;
            bytes += len;
        }
    }
    if !rings.h2c_flush(chip, 500) {
        return Err((11, Fail("the card to take every firmware packet", chip.r32(crate::pci::CH12.idx))));
    }
    justdrv::info!("stage 11: {} sections sent ({bytes} bytes in {packets} packets), all taken by the card", img.sections.len());

    // Stage 12: the firmware starts (rtw89_fw_check_rdy, FREERTOS_DONE).
    justdrv::sleep_ms(5);
    chip.poll8("the firmware to start", WCPU_FW_CTRL, FWDL_WAIT_MS, |v| (v >> 5) & 0x7 == FWDL_INIT_RDY)
        .map_err(|e| (12, e))?;
    Ok((packets, bytes))
}
