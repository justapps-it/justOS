//! Realtek RTL8852BE (802.11ax, Wi-Fi 6) PCIe driver, unfinished.
//!
//! What it does today, each stage logged so that a run on the real card
//! tells how far it got:
//!
//!   1. maps the card's registers (BAR 2);
//!   2. reads the chip's cut (SYS_CFG1, and the analog cut over the crystal
//!      serial interface);
//!   3. powers the chip on (the 8852B sequence; off and on again if it was
//!      left on);
//!   4. reads the efuse: the card's MAC address, RFE type and country;
//!   5. picks the firmware image for the cut out of the embedded file and
//!      checks its sections;
//!
//! then powers the chip off and stops. Still to write: the download of the
//! firmware (DMA rings, H2C packets, the card's CPU started), the MAC, BB and
//! RF set-up from the 8852B tables, channel switching, the TX and RX
//! descriptors, and registering a radio with the kernel ([`justdrv::wifi_register`]),
//! whose station (scan, WPA2/WPA3) is already there and tested.
//!
//! The registers, bits and sequences come from the Linux rtw89 driver
//! (rtw8852b.c, mac.c, efuse.c, reg.h, core.c), Copyright(c) 2019-2022
//! Realtek Corporation, licensed "GPL-2.0 OR BSD-3-Clause" and used here
//! under BSD-3-Clause; the code is written anew. See README.md.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use justdrv::{DeviceInfo, Mmio, Pci, Status, E_IO, E_NODEV, E_NOMEM, RES_MEMORY};
use rtw8852b_fw as fw;

justdrv::driver! {
    name: "rtw8852be",
    version: "0.1.0",
    probe: probe,
}

/// linux-firmware's rtw89/rtw8852b_fw-2.bin, unchanged (licence:
/// firmware/LICENCE.rtlwifi_firmware.txt).
static FIRMWARE: &[u8] = include_bytes!("../firmware/rtw8852b_fw-2.bin");

// Registers (reg.h).
const SYS_ISO_CTRL: usize = 0x0000;
const SYS_FUNC_EN: usize = 0x0002;
const SYS_PW_CTRL: usize = 0x0004;
const SYS_WL_EFUSE_CTRL: usize = 0x000A;
const SYS_SWR_CTRL1: usize = 0x0010;
const SYS_ADIE_PAD_PWR_CTRL: usize = 0x0018;
const AFE_LDO_CTRL: usize = 0x0020;
const EFUSE_CTRL: usize = 0x0030;
const SYS_SDIO_CTRL: usize = 0x0070;
const PLATFORM_ENABLE: usize = 0x0088;
const WLLPS_CTRL: usize = 0x0090;
const PMC_DBG_CTRL2: usize = 0x00CC;
const SYS_CFG1: usize = 0x00F0;
const SPS_DIG_ON_CTRL0: usize = 0x0200;
const SPS_ANA_ON_CTRL2: usize = 0x0228;
const WLAN_XTAL_SI_CTRL: usize = 0x0270;
const EECS_EESK_FUNC_SEL: usize = 0x02D8;
const WLRF_CTRL: usize = 0x02F0;
const IC_PWR_STATE: usize = 0x03F0;
const SPS_DIG_OFF_CTRL0: usize = 0x0400;
const DMAC_FUNC_EN: usize = 0x8400;
const CMAC_FUNC_EN: usize = 0xC000;

const fn bit(n: u32) -> u32 {
    1 << n
}

// SYS_PW_CTRL
const APFN_ONMAC: u32 = bit(8);
const APFM_OFFMAC: u32 = bit(9);
const APFM_SWLPS: u32 = bit(10);
const AFSM_WLSUS_EN: u32 = bit(11);
const AFSM_PCIE_SUS_EN: u32 = bit(12);
const APDM_HPDN: u32 = bit(15);
const EN_WLON: u32 = bit(16);
const RDY_SYSPWR: u32 = bit(17);
const DIS_WLBT_PDNSUSEN_SOPC: u32 = bit(18);
// SYS_ISO_CTRL
const ISO_EB2CORE: u32 = bit(8);
const PWC_EV2EF_B14: u32 = bit(14);
const PWC_EV2EF_B15: u32 = bit(15);
// Others
const DIS_WLBT_LPSEN_LOPC: u32 = bit(1);
const AON_OFF_PC_EN: u32 = bit(23);
const PLATFORM_EN: u8 = 1;
const PCIE_CALIB_EN_V1: u32 = bit(12);
const SYM_PADPDN_WL_RFC_1P3: u32 = bit(5);
const SYM_PADPDN_WL_PTA_1P3: u32 = bit(6);
const SYSON_DIS_PMCR_AX_WRMSK: u32 = bit(2);
const AFC_AFEDIG: u32 = bit(17);
const FEN_BBRSTB: u8 = 1;
const FEN_BB_GLB_RSTN: u8 = 2;
const SYM_CTRL_SPS_PWMFREQ: u32 = bit(10);
const AUTOLOAD_SUS: u16 = 1 << 5;
const EF_RDY: u32 = bit(29);
const SW_LPS_OPTION: u32 = 0x0001_A0B2;
const DEFAULT_SPS_ANA: u16 = 0x4F42;
const MAC_ON: u32 = 1;

const DMAC_ALL: u32 = bit(30) | bit(29) | bit(28) | bit(27) | bit(26) | bit(25) | bit(24) | bit(23) | bit(22)
    | bit(21) | bit(20) | bit(19) | bit(18) | bit(17) | bit(16) | bit(15);
const CMAC_ALL: u32 = bit(30) | bit(29) | bit(28) | bit(15) | bit(5) | bit(4) | bit(3) | bit(2) | bit(1) | bit(0);

// The crystal's serial interface: offsets and bits (mac.h).
const XTAL_SI_CV: u8 = 0x41;
const XTAL_SI_XTAL_XMD_2: u8 = 0x24;
const XTAL_SI_XTAL_XMD_4: u8 = 0x26;
const XTAL_SI_WL_RFC_S0: u8 = 0x80;
const XTAL_SI_WL_RFC_S1: u8 = 0x81;
const XTAL_SI_ANAPAR_WL: u8 = 0x90;
const XTAL_SI_SRAM_CTRL: u8 = 0xA1;
const SI_PON_WEI: u8 = 1 << 0;
const SI_PON_EI: u8 = 1 << 1;
const SI_OFF_WEI: u8 = 1 << 2;
const SI_OFF_EI: u8 = 1 << 3;
const SI_RFC2RF: u8 = 1 << 4;
const SI_SHDN_WL: u8 = 1 << 5;
const SI_GND_SHDN_WL: u8 = 1 << 6;
const SI_SRAM2RFC: u8 = 1 << 7;
const SI_SRAM_DIS: u8 = 1 << 1;
const SI_LDO_LPS: u8 = 0x70;
const SI_LPS_CAP: u8 = 0x0F;
const SI_RF00: u8 = 1;

// The efuse (rtw8852b.c, rtw8852b_common.h).
const PHYSICAL_EFUSE: usize = 1216;
const LOGICAL_EFUSE: usize = 2048;
const SEC_CTRL_EFUSE: usize = 4;
const EFUSE_RFE_TYPE: usize = 0x2CA;
const EFUSE_COUNTRY: usize = 0x2CB;
const EFUSE_PCIE_MAC: usize = 0x400;

/// Why a stage stopped: what it waited for, and the register's value then.
struct Fail(&'static str, u32);

struct Chip {
    r: Mmio,
}

impl Chip {
    fn r32(&self, a: usize) -> u32 {
        self.r.read32(a)
    }
    fn w32(&self, a: usize, v: u32) {
        self.r.write32(a, v)
    }
    fn set32(&self, a: usize, b: u32) {
        self.w32(a, self.r32(a) | b)
    }
    fn clr32(&self, a: usize, b: u32) {
        self.w32(a, self.r32(a) & !b)
    }
    fn mask32(&self, a: usize, mask: u32, v: u32) {
        let shift = mask.trailing_zeros();
        self.w32(a, (self.r32(a) & !mask) | ((v << shift) & mask))
    }
    fn set8(&self, a: usize, b: u8) {
        self.r.write8(a, self.r.read8(a) | b)
    }
    fn clr8(&self, a: usize, b: u8) {
        self.r.write8(a, self.r.read8(a) & !b)
    }
    fn set16(&self, a: usize, b: u16) {
        self.r.write16(a, self.r.read16(a) | b)
    }
    fn clr16(&self, a: usize, b: u16) {
        self.r.write16(a, self.r.read16(a) & !b)
    }

    /// Waits up to `ms` for `cond` on register `a`.
    fn poll(&self, what: &'static str, a: usize, ms: u64, cond: impl Fn(u32) -> bool) -> Result<u32, Fail> {
        let start = justdrv::time_ms();
        loop {
            let v = self.r32(a);
            if cond(v) {
                return Ok(v);
            }
            if justdrv::time_ms() - start > ms {
                return Err(Fail(what, v));
            }
            core::hint::spin_loop();
        }
    }

    /// A write over the crystal's serial interface (the bits of `mask`).
    fn xtal_write(&self, off: u8, val: u8, mask: u8) -> Result<(), Fail> {
        self.w32(WLAN_XTAL_SI_CTRL, off as u32 | (val as u32) << 8 | (mask as u32) << 16 | bit(31));
        self.poll("crystal serial interface (write)", WLAN_XTAL_SI_CTRL, 50, |v| v & bit(31) == 0)?;
        Ok(())
    }

    fn xtal_read(&self, off: u8) -> Result<u8, Fail> {
        self.w32(WLAN_XTAL_SI_CTRL, off as u32 | 1 << 24 | bit(31));
        let v = self.poll("crystal serial interface (read)", WLAN_XTAL_SI_CTRL, 50, |v| v & bit(31) == 0)?;
        Ok((v >> 8) as u8)
    }

    /// The 8852B power-on sequence (rtw8852b_pwr_on_func), as at probe:
    /// the efuse not read yet.
    fn power_on(&self) -> Result<(), Fail> {
        self.r.write16(SPS_ANA_ON_CTRL2, DEFAULT_SPS_ANA);
        self.clr32(SYS_PW_CTRL, AFSM_WLSUS_EN | AFSM_PCIE_SUS_EN);
        self.set32(SYS_PW_CTRL, DIS_WLBT_PDNSUSEN_SOPC);
        self.set32(WLLPS_CTRL, DIS_WLBT_LPSEN_LOPC);
        self.clr32(SYS_PW_CTRL, APDM_HPDN);
        self.clr32(SYS_PW_CTRL, APFM_SWLPS);
        self.poll("system power ready", SYS_PW_CTRL, 20, |v| v & RDY_SYSPWR != 0)?;
        self.set32(AFE_LDO_CTRL, AON_OFF_PC_EN);
        self.poll("always-on power", AFE_LDO_CTRL, 20, |v| v & AON_OFF_PC_EN != 0)?;
        // Digital switching regulator, RFE type not known yet.
        self.mask32(SPS_DIG_OFF_CTRL0, 0x03, 0x1);
        self.mask32(SPS_DIG_OFF_CTRL0, 0x30, 0x3);
        self.set32(SYS_PW_CTRL, EN_WLON);
        self.set32(SYS_PW_CTRL, APFN_ONMAC);
        self.poll("MAC power on", SYS_PW_CTRL, 20, |v| v & APFN_ONMAC == 0)?;
        for _ in 0..2 {
            self.set8(PLATFORM_ENABLE, PLATFORM_EN);
            self.clr8(PLATFORM_ENABLE, PLATFORM_EN);
        }
        self.set8(PLATFORM_ENABLE, PLATFORM_EN);
        self.clr32(SYS_SDIO_CTRL, PCIE_CALIB_EN_V1);
        self.set32(SYS_ADIE_PAD_PWR_CTRL, SYM_PADPDN_WL_PTA_1P3);
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_GND_SHDN_WL, SI_GND_SHDN_WL)?;
        self.set32(SYS_ADIE_PAD_PWR_CTRL, SYM_PADPDN_WL_RFC_1P3);
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_SHDN_WL, SI_SHDN_WL)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_OFF_WEI, SI_OFF_WEI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_OFF_EI, SI_OFF_EI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_RFC2RF)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_PON_WEI, SI_PON_WEI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_PON_EI, SI_PON_EI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_SRAM2RFC)?;
        self.xtal_write(XTAL_SI_SRAM_CTRL, 0, SI_SRAM_DIS)?;
        self.xtal_write(XTAL_SI_XTAL_XMD_2, 0, SI_LDO_LPS)?;
        self.xtal_write(XTAL_SI_XTAL_XMD_4, 0, SI_LPS_CAP)?;
        self.set32(PMC_DBG_CTRL2, SYSON_DIS_PMCR_AX_WRMSK);
        self.set32(SYS_ISO_CTRL, ISO_EB2CORE);
        self.clr32(SYS_ISO_CTRL, PWC_EV2EF_B15);
        justdrv::sleep_ms(1);
        self.clr32(SYS_ISO_CTRL, PWC_EV2EF_B14);
        self.clr32(PMC_DBG_CTRL2, SYSON_DIS_PMCR_AX_WRMSK);
        self.set32(DMAC_FUNC_EN, DMAC_ALL);
        self.set32(CMAC_FUNC_EN, CMAC_ALL);
        // EESK pin: Bluetooth log.
        self.mask32(EECS_EESK_FUNC_SEL, 0xF0, 0x1);
        Ok(())
    }

    /// The 8852B power-off sequence (rtw8852b_pwr_off_func).
    fn power_off(&self) -> Result<(), Fail> {
        self.r.write16(SPS_ANA_ON_CTRL2, DEFAULT_SPS_ANA);
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_RFC2RF, SI_RFC2RF)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_OFF_EI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_OFF_WEI)?;
        self.xtal_write(XTAL_SI_WL_RFC_S0, 0, SI_RF00)?;
        self.xtal_write(XTAL_SI_WL_RFC_S1, 0, SI_RF00)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, SI_SRAM2RFC, SI_SRAM2RFC)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_PON_EI)?;
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_PON_WEI)?;
        self.set32(SYS_PW_CTRL, EN_WLON);
        self.clr32(WLRF_CTRL, AFC_AFEDIG);
        self.clr8(SYS_FUNC_EN, FEN_BB_GLB_RSTN | FEN_BBRSTB);
        self.clr32(SYS_ADIE_PAD_PWR_CTRL, SYM_PADPDN_WL_RFC_1P3);
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_SHDN_WL)?;
        self.clr32(SYS_ADIE_PAD_PWR_CTRL, SYM_PADPDN_WL_PTA_1P3);
        self.xtal_write(XTAL_SI_ANAPAR_WL, 0, SI_GND_SHDN_WL)?;
        self.set32(SYS_PW_CTRL, APFM_OFFMAC);
        self.poll("MAC power off", SYS_PW_CTRL, 20, |v| v & APFM_OFFMAC == 0)?;
        self.w32(WLLPS_CTRL, SW_LPS_OPTION);
        self.set32(SYS_SWR_CTRL1, SYM_CTRL_SPS_PWMFREQ);
        self.mask32(SPS_DIG_ON_CTRL0, 0x3 << 17, 0x3);
        self.set32(SYS_PW_CTRL, APFM_SWLPS);
        Ok(())
    }

    fn mac_on(&self) -> bool {
        (self.r32(IC_PWR_STATE) >> 8) & 3 == MAC_ON
    }

    /// The efuse's physical contents (rtw89_dump_physical_efuse_map_ddv).
    fn efuse_physical(&self, cut_a: bool) -> Result<Vec<u8>, Fail> {
        self.set8(PMC_DBG_CTRL2, SYSON_DIS_PMCR_AX_WRMSK as u8);
        self.set16(SYS_ISO_CTRL, PWC_EV2EF_B14 as u16);
        justdrv::sleep_ms(1);
        self.set16(SYS_ISO_CTRL, PWC_EV2EF_B15 as u16);
        self.clr16(SYS_ISO_CTRL, ISO_EB2CORE as u16);
        if cut_a {
            // OTP burst mode (EFUSE_CTRL_1_V1 0x0038, bit 31)... only on cut A.
            self.set32(0x0038, bit(31));
        }
        let mut map = vec![0u8; PHYSICAL_EFUSE];
        let mut res = Ok(());
        for (addr, b) in map.iter_mut().enumerate() {
            self.w32(EFUSE_CTRL, ((addr as u32) << 16) & 0x07FF_0000);
            match self.poll("efuse read", EFUSE_CTRL, 1000, |v| v & EF_RDY != 0) {
                Ok(v) => *b = v as u8,
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        if cut_a {
            self.clr32(0x0038, bit(31));
        }
        self.set16(SYS_ISO_CTRL, ISO_EB2CORE as u16);
        self.clr16(SYS_ISO_CTRL, PWC_EV2EF_B15 as u16);
        justdrv::sleep_ms(1);
        self.clr16(SYS_ISO_CTRL, PWC_EV2EF_B14 as u16);
        self.clr8(PMC_DBG_CTRL2, SYSON_DIS_PMCR_AX_WRMSK as u8);
        res.map(|_| map)
    }
}

/// The logical efuse map from the physical one (rtw89_dump_logical_efuse_map):
/// blocks of up to four words, each with a two-byte header.
fn efuse_logical(phy: &[u8]) -> Option<Vec<u8>> {
    let mut log = vec![0xFFu8; LOGICAL_EFUSE];
    let end = phy.len() - SEC_CTRL_EFUSE;
    let mut i = SEC_CTRL_EFUSE;
    while i + 1 < end {
        let (h1, h2) = (phy[i], phy[i + 1]);
        if h1 == 0xFF || h2 == 0xFF {
            break;
        }
        let block = ((h2 as usize & 0xF0) >> 4) | ((h1 as usize & 0x0F) << 4);
        let words = h2 & 0x0F;
        i += 2;
        for w in 0..4 {
            if words & (1 << w) != 0 {
                continue;
            }
            let at = (block << 3) + (w << 1);
            if i + 1 > end - 1 || at + 1 >= LOGICAL_EFUSE {
                return None;
            }
            log[at] = phy[i];
            log[at + 1] = phy[i + 1];
            i += 2;
        }
    }
    Some(log)
}

fn mac_text(m: &[u8]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for (i, b) in m.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&alloc::format!("{b:02x}"));
    }
    s
}

fn cut_name(cv: u8) -> char {
    (b'A' + cv.min(25)) as char
}

fn probe(dev: &DeviceInfo) -> Status {
    if dev.vendor != 0x10EC || !(dev.device == 0xB852 || dev.device == 0xB85B) {
        return E_NODEV;
    }
    justdrv::info!(
        "stage 1: card {:04x}:{:04x}, subsystem {:04x}:{:04x}, revision {:#x}",
        dev.vendor,
        dev.device,
        dev.subsystem_vendor,
        dev.subsystem,
        dev.revision
    );
    // rtw89 uses BAR 2; take the first memory BAR big enough otherwise.
    let bar = [2usize, 0, 1, 3, 4, 5]
        .into_iter()
        .map(|i| (i, dev.resources[i]))
        .find(|(_, r)| r.kind == RES_MEMORY && r.size >= 0x10000);
    let Some((n, bar)) = bar else {
        for (i, r) in dev.resources.iter().enumerate() {
            justdrv::warn!("  BAR {i}: kind {} base {:#x} size {:#x}", r.kind, r.base, r.size);
        }
        justdrv::warn!("stage 1 failed: no memory BAR of at least 64 KiB");
        return E_IO;
    };
    Pci(dev.id).enable(false, true, true);
    let Some(r) = Mmio::map(bar.base, (bar.size as usize).min(0x20000)) else { return E_NOMEM };
    let chip = Chip { r };
    let cfg1 = chip.r32(SYS_CFG1);
    justdrv::info!("stage 1: BAR {n} at {:#x} ({} KiB), SYS_CFG1 {:#010x}", bar.base, bar.size >> 10, cfg1);
    if cfg1 == u32::MAX {
        justdrv::warn!("stage 1 failed: the card does not answer (registers read all ones)");
        return E_IO;
    }

    // 2. The cut.
    let cv = ((cfg1 >> 12) & 0xF) as u8;
    match chip.xtal_read(XTAL_SI_CV) {
        Ok(a) => justdrv::info!("stage 2: chip cut {} (cv {cv}), analog cut {}", cut_name(cv), a & 0xF),
        Err(Fail(w, v)) => justdrv::warn!("stage 2: chip cut {} (cv {cv}); {w} timed out ({v:#010x})", cut_name(cv)),
    }

    // 3. Power.
    let state = chip.r32(IC_PWR_STATE);
    if chip.mac_on() {
        justdrv::info!("stage 3: the MAC was left on (IC_PWR_STATE {state:#010x}): off first");
        if let Err(Fail(w, v)) = chip.power_off() {
            justdrv::warn!("stage 3: power off: {w} timed out ({v:#010x})");
        }
    }
    if let Err(Fail(w, v)) = chip.power_on() {
        justdrv::warn!("stage 3 failed: power on: {w} timed out (register {v:#010x})");
        return E_IO;
    }
    justdrv::info!(
        "stage 3: powered on (SYS_PW_CTRL {:#010x}, IC_PWR_STATE {:#010x})",
        chip.r32(SYS_PW_CTRL),
        chip.r32(IC_PWR_STATE)
    );

    // 4. The efuse.
    let autoload = chip.r.read16(SYS_WL_EFUSE_CTRL) & AUTOLOAD_SUS != 0;
    match chip.efuse_physical(cv == 0) {
        Err(Fail(w, v)) => justdrv::warn!("stage 4: {w} timed out ({v:#010x})"),
        Ok(phy) => match efuse_logical(&phy) {
            None => justdrv::warn!("stage 4: the efuse map does not decode (first bytes {:02x?})", &phy[..16]),
            Some(log) => {
                let mac = &log[EFUSE_PCIE_MAC..EFUSE_PCIE_MAC + 6];
                let cc = &log[EFUSE_COUNTRY..EFUSE_COUNTRY + 2];
                justdrv::info!(
                    "stage 4: efuse {}, MAC {}, RFE type {}, country {}{}",
                    if autoload { "autoloaded" } else { "not autoloaded" },
                    mac_text(mac),
                    log[EFUSE_RFE_TYPE],
                    if cc[0].is_ascii_alphabetic() { cc[0] as char } else { '?' },
                    if cc[1].is_ascii_alphabetic() { cc[1] as char } else { '?' },
                );
            }
        },
    }

    // 5. The firmware.
    let ok = match fw::select(FIRMWARE, cv).and_then(|(e, data)| Ok((e, fw::parse(data)?))) {
        Err(e) => {
            justdrv::warn!("stage 5 failed: firmware: {e}");
            false
        }
        Ok((e, img)) => {
            let v = img.version;
            let (y, mo, d, ..) = img.built;
            justdrv::info!(
                "stage 5: firmware {}.{}.{}.{} for cut {} ({y}-{mo:02}-{d:02}), {} sections, {} elements",
                v[0],
                v[1],
                v[2],
                v[3],
                cut_name(e.cv),
                img.sections.len(),
                fw::elements(FIRMWARE).len()
            );
            true
        }
    };

    if let Err(Fail(w, v)) = chip.power_off() {
        justdrv::warn!("power off: {w} timed out ({v:#010x})");
    }
    if ok {
        justdrv::warn!("stopping after stage 5: the firmware download and the radio are not written yet, no Wi-Fi radio added");
    }
    E_IO
}
