//! Installing justOS on a disk of the computer.
//!
//! The system runs from a USB stick, a live CD or another disk; this copies
//! it onto a disk the user picks, which is erased first. The disk gets the
//! layout of an image made by `justtool image` (tools/justtool/src/main.rs):
//!
//! ```text
//! sector 0        MBR: stage 1 of the BIOS loader, its boot table, the
//!                 disk signature and two partitions
//! sector 1…       stage 2
//! sector 2048     EFI system partition (FAT16): \EFI\BOOT\BOOTX64.EFI,
//!                 \JUSTOS\BOOT.CTL and the two system slots
//! after it        the JustFS system volume ("System"), to the end of the
//!                 disk (at most 2 TiB: MBR sectors are 32 bits)
//! ```
//!
//! What goes on it: the loaders and the system slot of the running system,
//! from the disk it started from (the slot that runs, checked against its
//! SHA-256) or, on a live CD, from `sys:/system/install` (`justtool iso`
//! puts them there); and every file of `sys:` except what belongs to this
//! computer and this session: the settings ([`SKIP`]) and what is in the
//! folders of [`EMPTY`]. The user's files in `/home` go along, so what was
//! made while justOS ran from the stick is on the computer too; the
//! installed system starts with the first-start setup.
//!
//! The boot partition is a FAT file system laid out here ([`fat`], as
//! justtool lays it out); the system volume is formatted anew at the full
//! size of the disk and filled file by file, not copied block by block.
//!
//! Order: the start of the disk is wiped first, so a disk whose
//! installation stopped half way never starts; then the system volume is
//! written, read back from the disk and compared; then the boot partition,
//! read back too; the MBR goes last.
//!
//! A disk is written only by [`install`], given the disk's erase code
//! ([`Target::code`]) that the user saw on the confirmation screen or in
//! `install-system list`. The code is made from the disk's contents, so it
//! no longer matches once the disk changed. The disk the system runs from
//! is never a target.

mod fat;

use crate::dev::block::{self, Disk, DiskBlocks};
use crate::fs::{self, Path};
use crate::sync::IrqLock;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use justfs::{Block, BlockDevice, Fs, Kind, BLOCK_SIZE};
use justtls::sha2::Sha256;

const SECTOR: usize = 512;
/// The boot partition starts at 1 MiB, as on a disk image.
const ESP_LBA: u64 = 2048;
/// The size of a slot when there is none to take it from (a live CD).
const SLOT_SIZE: u64 = 24 << 20;
/// Room the installed system gets beyond its files, at least.
const ROOM: u64 = 512 << 20;
/// The last sector an MBR partition can reach.
const MBR_MAX: u64 = 0xFFFF_FFFF;
/// Stage 2's sectors, at most (stage 1 reads it in one go; as justtool).
const STAGE2_MAX: usize = 63;
/// Bytes read and written at a time.
const CHUNK: usize = 1 << 20;
/// Bytes copied between two write-backs of the new volume's cache.
const WRITE_BACK: u64 = 8 << 20;
/// Where the installed system says when it was installed (and from what).
pub const MARKER: &str = "/system/installed";
/// Where a live CD keeps what the installer needs besides the files.
const PAYLOAD: &str = "sys:/system/install";

/// Paths of `sys:` that are not copied: what a computer's first start
/// writes (the settings, permissions given to apps, the proxy), test
/// leftovers and the installer's own files.
const SKIP: &[&str] =
    &["/system/settings", "/system/permissions.json", "/system/proxy", "/system/updates-test", "/system/install", MARKER, "/home/.dnd-test"];
/// Folders copied empty: what is in them belongs to the session that runs
/// (temporary files, downloads of updates, programs' caches).
const EMPTY: &[&str] = &["/tmp", "/system/updates", "/home/.cache"];
/// Top folders copied in the second step, "le applicazioni".
const APPS: &[&str] = &["/apps", "/store", "/usr", "/opt"];

/// The steps, as the installer shows them.
pub const PHASES: [&str; 5] = ["Preparo il disco", "Copio il sistema", "Copio le applicazioni", "Verifico la copia", "Ultimi ritocchi"];

fn get16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn get32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The sum of a sector's dwords: 0 for a sealed slot header or BOOT.CTL.
fn sum(sector: &[u8]) -> u32 {
    (0..SECTOR / 4).fold(0u32, |s, i| s.wrapping_add(get32(sector, i * 4)))
}

fn rd(disk: &Disk, lba: u64, buf: &mut [u8]) -> Result<(), String> {
    match disk.read(lba, buf) {
        justdk::OK => Ok(()),
        st => Err(format!("{}: non riesco a leggere il settore {lba} (errore {st})", disk.name)),
    }
}

fn wr(disk: &Disk, lba: u64, buf: &[u8]) -> Result<(), String> {
    match disk.write(lba, buf) {
        justdk::OK => Ok(()),
        st => Err(format!("{}: non riesco a scrivere il settore {lba} (errore {st})", disk.name)),
    }
}

fn flush(disk: &Disk) -> Result<(), String> {
    match disk.flush() {
        justdk::OK => Ok(()),
        st => Err(format!("{}: il disco non conferma la scrittura (errore {st})", disk.name)),
    }
}

/// Writes zeros over `count` sectors from `lba`; `progress(sectors done)`.
fn zero(disk: &Disk, lba: u64, count: u64, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let per = (CHUNK / SECTOR) as u64;
    let buf = vec![0u8; CHUNK];
    let mut done = 0;
    while done < count {
        let n = (count - done).min(per);
        wr(disk, lba + done, &buf[..n as usize * SECTOR])?;
        done += n;
        progress(done);
    }
    Ok(())
}

fn fs_err(what: &str) -> impl Fn(justfs::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// Sizes as disk makers write them: "512 GB", "1,5 TB".
pub fn size_text(bytes: u64) -> String {
    let tenths = |unit: u64| {
        let t = (bytes * 10 + unit / 2) / unit;
        if t % 10 == 0 || t >= 100 {
            format!("{}", (t + 5) / 10)
        } else {
            format!("{},{}", t / 10, t % 10)
        }
    };
    const G: u64 = 1_000_000_000;
    if bytes >= 1000 * G {
        format!("{} TB", tenths(1000 * G))
    } else if bytes >= G {
        format!("{} GB", tenths(G))
    } else {
        format!("{} MB", (bytes + 500_000) / 1_000_000)
    }
}

/// Whether `path` is `top` or inside it (any case).
fn under(path: &str, top: &str) -> bool {
    path.len() >= top.len() && path[..top.len()].eq_ignore_ascii_case(top) && (path.len() == top.len() || path.as_bytes()[top.len()] == b'/')
}

fn listed(list: &[&str], path: &str) -> bool {
    list.iter().any(|s| s.eq_ignore_ascii_case(path))
}

// ----------------------------------------------------------------------------
// The disks

/// A partition of a disk, as the confirmation names it.
#[derive(Clone)]
pub struct Part {
    /// "NTFS", "EFI", "Linux (ext4)", "JustFS"...
    pub kind: String,
    pub label: String,
    pub size: u64,
}

/// A disk justOS could be installed on.
#[derive(Clone)]
pub struct Target {
    pub disk: Arc<Disk>,
    /// What the user confirms: six letters and digits, made from the
    /// disk's name, size and first sectors.
    pub code: String,
    /// The model, or "Disco senza nome".
    pub name: String,
    /// "SATA", "NVMe", "USB"...
    pub bus: &'static str,
    pub size: u64,
    pub parts: Vec<Part>,
    /// What is on it, in plain words: "è vuoto", "contiene Windows (NTFS)".
    pub contents: String,
    /// Why justOS cannot go there, if it cannot: "troppo piccolo: servono
    /// almeno 6 GB".
    pub problem: Option<String>,
    /// Something to know before going on (a disk bigger than 2 TB).
    pub note: Option<String>,
}

impl Target {
    pub fn usable(&self) -> bool {
        self.problem.is_none()
    }

    /// "EFI 105 MB · NTFS 499 GB", or "nessuna partizione".
    pub fn parts_text(&self) -> String {
        if self.parts.is_empty() {
            return String::from("nessuna partizione");
        }
        let all: Vec<String> = self
            .parts
            .iter()
            .map(|p| if p.label.is_empty() { format!("{} {}", p.kind, size_text(p.size)) } else { format!("{} «{}» {}", p.kind, p.label, size_text(p.size)) })
            .collect();
        all.join(" · ")
    }
}

/// The whole disk under a disk or partition name.
fn whole(name: &str) -> Option<Arc<Disk>> {
    let d = block::find(name)?;
    Some(match &d.parent {
        Some((p, _)) => p.clone(),
        None => d,
    })
}

/// Whether the disk or partition `name` is on `disk`.
fn on_disk(name: &str, disk: &Arc<Disk>) -> bool {
    whole(name).is_some_and(|w| Arc::ptr_eq(&w, disk))
}

/// The disks the running system uses: the one under `sys:` and the one the
/// loader started from (by its MBR signature). They are never targets.
fn running() -> Vec<Arc<Disk>> {
    let mut out = Vec::new();
    if let Some(d) = fs::volume("sys").and_then(|v| whole(&v.source)) {
        out.push(d);
    }
    let id = crate::boot_disk_id();
    if id != 0 {
        for d in block::list().into_iter().filter(|d| d.parent.is_none()) {
            if block::signature(&d) == Some(id) && !out.iter().any(|o| Arc::ptr_eq(o, &d)) {
                out.push(d);
            }
        }
    }
    out
}

/// A CD or DVD drive: never a target, not even listed.
fn optical(d: &Disk) -> bool {
    d.sector_size == 2048 && d.readonly
}

/// Whether the system runs from a live CD or a removable disk (a USB
/// stick, a memory card).
pub fn running_live() -> bool {
    let Some(v) = fs::volume("sys") else { return false };
    v.source.starts_with("memory") || whole(&v.source).is_some_and(|d| d.removable)
}

/// What the running system started from, for the installer's words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Medium {
    /// A live CD (its system volume is in memory).
    Cd,
    Usb,
    Card,
    Disk,
}

pub fn medium() -> Medium {
    let Some(v) = fs::volume("sys") else { return Medium::Disk };
    if v.source.starts_with("memory") {
        return Medium::Cd;
    }
    match whole(&v.source).map(|d| bus(&d)) {
        Some("USB") => Medium::Usb,
        Some("scheda SD") => Medium::Card,
        _ => Medium::Disk,
    }
}

/// Whether this system was installed by the installer (it never offers to
/// install itself).
pub fn installed() -> bool {
    fs::abs(&format!("sys:{MARKER}")).is_ok_and(|p| fs::exists(&p))
}

/// Whether to offer the installer when the interface starts: the system
/// runs from an installation medium (a live CD, a USB stick, a memory
/// card), it was not installed by the installer, and the computer has a
/// disk it fits on. A disk image started as a disk (in a virtual machine,
/// say) is not offered it; `install-system` in the shell still works there.
pub fn offer_at_start() -> bool {
    !installed() && running_live() && targets().iter().any(Target::usable)
}

fn bus(d: &Disk) -> &'static str {
    let Some(dev) = crate::dev::get(d.device) else { return "disco" };
    let i = &dev.info;
    match (i.bus, i.class, i.subclass) {
        (justdk::BUS_USB, _, _) => "USB",
        (justdk::BUS_SD, _, _) => "scheda SD",
        (justdk::BUS_PCI, 0x01, 0x08) => "NVMe",
        (justdk::BUS_PCI, 0x01, 0x06) => "SATA",
        (justdk::BUS_PCI, 0x01, 0x01) => "IDE",
        (justdk::BUS_PCI, _, _) if i.vendor == 0x1AF4 => "virtio",
        _ => "disco",
    }
}

/// The erase code of a disk.
fn code(d: &Disk) -> String {
    let mut h = Sha256::new();
    h.update(d.name.as_bytes());
    h.update(d.model.as_bytes());
    h.update(&d.sectors.to_le_bytes());
    let mut first = vec![0u8; d.sector_size as usize * 2];
    if d.sectors >= 2 && d.read(0, &mut first) == justdk::OK {
        h.update(&first);
    }
    let x = h.finish();
    format!("{:02x}{:02x}{:02x}", x[0], x[1], x[2])
}

/// The file system at the start of a partition: (name, label).
fn probe(p: &Disk) -> (&'static str, String) {
    let ss = p.sector_size as usize;
    let mut b = vec![0u8; BLOCK_SIZE.max(ss)];
    if p.sectors < (b.len() / ss) as u64 || p.read(0, &mut b) != justdk::OK {
        return ("", String::new());
    }
    let text = |r: core::ops::Range<usize>| {
        let s: String = b[r].iter().map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { ' ' }).collect();
        String::from(s.trim())
    };
    // FAT labels: "NO NAME" means none.
    let fat_label = |l: String| if l == "NO NAME" { String::new() } else { l };
    let boot = b[510] == 0x55 && b[511] == 0xAA;
    if &b[3..11] == b"NTFS    " {
        ("NTFS", String::new())
    } else if &b[3..11] == b"-FVE-FS-" {
        ("BitLocker", String::new())
    } else if &b[3..11] == b"EXFAT   " {
        ("exFAT", String::new())
    } else if boot && &b[0x52..0x57] == b"FAT32" {
        ("FAT32", fat_label(text(0x47..0x52)))
    } else if boot && &b[0x36..0x39] == b"FAT" {
        ("FAT", fat_label(text(0x2B..0x36)))
    } else if get16(&b, 1024 + 0x38) == 0xEF53 {
        ("Linux (ext4)", text(1024 + 0x78..1024 + 0x88))
    } else if &b[BLOCK_SIZE - 10..BLOCK_SIZE] == b"SWAPSPACE2" {
        ("swap Linux", String::new())
    } else if Fs::<justfs::MemDisk>::probe(b[..BLOCK_SIZE].try_into().unwrap()) {
        // The label: bytes 64..96 of the superblock (see justfs).
        let n = b[64..96].iter().position(|&c| c == 0).unwrap_or(32);
        ("JustFS", String::from_utf8_lossy(&b[64..64 + n]).into_owned())
    } else {
        ("", String::new())
    }
}

/// What is on a disk, in plain words.
fn contents(d: &Disk, parts: &[Part]) -> String {
    if parts.is_empty() {
        let mut first = vec![0u8; d.sector_size as usize * 8];
        let blank = d.sectors >= 8 && d.read(0, &mut first) == justdk::OK && first.iter().all(|&b| b == 0);
        return String::from(if blank { "è vuoto" } else { "contiene dati che justOS non riconosce" });
    }
    let mut things: Vec<String> = Vec::new();
    let mut add = |s: String| {
        if !things.contains(&s) {
            things.push(s);
        }
    };
    for p in parts {
        match p.kind.as_str() {
            "NTFS" => add(String::from("Windows (NTFS)")),
            "BitLocker" => add(String::from("Windows (cifrato con BitLocker)")),
            "Linux (ext4)" | "swap Linux" | "Linux" | "Linux swap" => add(String::from("Linux")),
            "JustFS" if p.label == "System" => add(String::from("justOS")),
            "JustFS" => add(format!("dati di justOS («{}»)", p.label)),
            "EFI" | "Microsoft reserved" => {}
            "FAT32" | "FAT" | "exFAT" if !p.label.is_empty() => add(format!("dati ({} «{}»)", p.kind, p.label)),
            "FAT32" | "FAT" | "exFAT" => add(format!("dati ({})", p.kind)),
            _ => add(String::from("dati che justOS non riconosce")),
        }
    }
    if things.is_empty() {
        return String::from("contiene solo una partizione di avvio");
    }
    let last = things.pop().unwrap();
    if things.is_empty() {
        format!("contiene {last}")
    } else {
        format!("contiene {} e {last}", things.join(", "))
    }
}

/// The bytes the system takes now (what is copied, about).
fn used() -> u64 {
    fs::volume("sys").map_or(0, |v| {
        let (total, free) = v.space();
        total.saturating_sub(free)
    })
}

/// The smallest disk the system fits on: the boot partition, the files
/// with a tenth more, [`ROOM`] and JustFS's node table.
pub fn needed() -> u64 {
    let esp = 2 * SLOT_SIZE + (8 << 20);
    let volume = (used() * 11 / 10 + ROOM) * 129 / 128;
    ESP_LBA * SECTOR as u64 + esp + volume
}

/// Every disk of the computer justOS could go on, those it does not fit on
/// too (with [`Target::problem`]). Not the disks the system runs from, not
/// CD drives.
pub fn targets() -> Vec<Target> {
    let running = running();
    let need = needed();
    let all = block::list();
    let mut out = Vec::new();
    for d in all.iter().filter(|d| d.parent.is_none() && !optical(d)) {
        if running.iter().any(|r| Arc::ptr_eq(r, d)) {
            continue;
        }
        let parts: Vec<Part> = all
            .iter()
            .filter(|p| p.parent.as_ref().is_some_and(|(w, _)| Arc::ptr_eq(w, d)))
            .map(|p| {
                let (fs, label) = probe(p);
                let kind = match (fs, p.kind.as_str()) {
                    ("", "EFI system") | ("FAT", "EFI system") | ("FAT32", "EFI system") => "EFI",
                    ("", "unknown") => "altro",
                    ("", k) => k,
                    (f, _) => f,
                };
                Part { kind: String::from(kind), label, size: p.size_bytes() }
            })
            .collect();
        let problem = if d.readonly {
            Some(String::from("protetto da scrittura"))
        } else if d.sector_size as usize != SECTOR {
            Some(format!("settori da {} byte: per ora justOS va solo su dischi con settori da 512 byte", d.sector_size))
        } else if d.size_bytes() < need {
            Some(format!("troppo piccolo: servono almeno {}", size_text(need)))
        } else {
            None
        };
        let note = (d.sectors > MBR_MAX).then(|| format!("justOS userà i primi 2,2 TB dei {} del disco", size_text(d.size_bytes())));
        out.push(Target {
            code: code(d),
            name: if d.model.is_empty() { String::from("Disco senza nome") } else { d.model.clone() },
            bus: bus(d),
            size: d.size_bytes(),
            contents: contents(d, &parts),
            parts,
            problem,
            note,
            disk: d.clone(),
        });
    }
    out
}

// ----------------------------------------------------------------------------
// What is copied

/// A system slot image, as its header describes it (see
/// tools/justtool/src/slot.rs).
struct Slot {
    kernel_sectors: u32,
    memsz: u32,
    entry: u64,
    pack_sector: u32,
    pack_sectors: u32,
    total: u64,
    sha256: [u8; 32],
    version: String,
}

impl Slot {
    fn parse(image: &[u8]) -> Option<Slot> {
        let h = image.get(..SECTOR)?;
        if &h[..8] != b"ARCSLOT1" || sum(h) != 0 {
            return None;
        }
        let v = &h[0x50..0x70];
        let len = v.iter().position(|&b| b == 0).unwrap_or(v.len());
        let s = Slot {
            kernel_sectors: get32(h, 0x08),
            memsz: get32(h, 0x0C),
            entry: u64::from_le_bytes(h[0x10..0x18].try_into().ok()?),
            pack_sector: get32(h, 0x18),
            pack_sectors: get32(h, 0x1C),
            total: get32(h, 0x28) as u64,
            sha256: h[0x30..0x50].try_into().ok()?,
            version: String::from_utf8_lossy(&v[..len]).into_owned(),
        };
        let fits = s.kernel_sectors != 0 && s.pack_sectors != 0 && s.pack_sector > s.kernel_sectors && s.pack_sector as u64 + s.pack_sectors as u64 <= s.total;
        fits.then_some(s)
    }

    /// Parses and checks a whole image.
    fn check(image: &[u8]) -> Result<Slot, String> {
        let s = Slot::parse(image).ok_or("l'immagine del sistema non ha un'intestazione valida")?;
        let bytes = s.total as usize * SECTOR;
        if image.len() < bytes || Sha256::digest(&image[SECTOR..bytes]) != s.sha256 {
            return Err("l'immagine del sistema è danneggiata".to_string());
        }
        Ok(s)
    }
}

/// The loaders and the system, besides the files.
struct BootFiles {
    /// The MBR's code (the boot table and partitions are written anew).
    stage1: Vec<u8>,
    /// Whole sectors.
    stage2: Vec<u8>,
    efi: Vec<u8>,
    /// The slot image, exactly its sectors.
    slot: Vec<u8>,
    info: Slot,
    /// Sectors per slot on the new disk.
    slot_sectors: u64,
}

fn boot_files() -> Result<BootFiles, String> {
    let (stage1, mut stage2, efi, mut slot, slot_sectors) = if crate::sysupdate::booted().is_some() {
        // From the disk the system started from.
        let (disk, slot, slot_sectors) = crate::sysupdate::running_image()?;
        let mut mbr = vec![0u8; SECTOR];
        rd(&disk, 0, &mut mbr)?;
        let n = get16(&mbr, 0x194) as usize;
        if &mbr[0x190..0x194] != b"ARCB" || n == 0 || n > STAGE2_MAX {
            return Err("il disco di avvio non ha il caricatore BIOS di justOS".to_string());
        }
        let mut stage2 = vec![0u8; n * SECTOR];
        rd(&disk, 1, &mut stage2)?;
        let esp = (0..4)
            .map(|i| &mbr[0x1BE + i * 16..0x1CE + i * 16])
            .find(|e| e[4] == 0xEF)
            .map(|e| (get32(e, 8) as u64, get32(e, 12) as u64))
            .ok_or("il disco di avvio non ha la partizione EFI")?;
        let efi = fat::read_file(
            &mut |lba, buf: &mut [u8]| {
                if lba + (buf.len() / SECTOR) as u64 > esp.1 {
                    return Err("la partizione di avvio finisce prima".to_string());
                }
                rd(&disk, esp.0 + lba, buf)
            },
            "EFI/BOOT/BOOTX64.EFI",
        )?;
        (mbr, stage2, efi, slot, slot_sectors.max(SLOT_SIZE / SECTOR as u64))
    } else {
        // A live CD: what justtool iso put on the system volume.
        let read = |name: &str| {
            fs::abs(&format!("{PAYLOAD}/{name}"))
                .and_then(|p| fs::read_file(&p))
                .map_err(|_| format!("manca {PAYLOAD}/{name}: questa copia di justOS non sa installarsi"))
        };
        let (stage1, stage2, efi, slot) = (read("stage1.bin")?, read("stage2.bin")?, read("BOOTX64.EFI")?, read("justos.slot")?);
        if stage1.len() != SECTOR || stage2.is_empty() || stage2.len() > STAGE2_MAX * SECTOR {
            return Err(format!("{PAYLOAD}: i caricatori non sono validi"));
        }
        let sectors = (slot.len() / SECTOR) as u64;
        (stage1, stage2, efi, slot, (SLOT_SIZE / SECTOR as u64).max(sectors.next_multiple_of(2048)))
    };
    if efi.len() < 1024 || &efi[..2] != b"MZ" {
        return Err("il caricatore UEFI non è valido".to_string());
    }
    stage2.resize(stage2.len().next_multiple_of(SECTOR), 0);
    let info = Slot::check(&slot)?;
    slot.truncate(info.total as usize * SECTOR);
    Ok(BootFiles { stage1, stage2, efi, slot, info, slot_sectors })
}

/// One thing to copy.
struct Item {
    path: String,
    kind: Kind,
    size: u64,
    flags: u8,
    mode: u16,
    /// Where a link points.
    link: String,
    /// In the second step ([`APPS`]).
    apps: bool,
    /// SHA-256 of what was copied (files).
    sha: [u8; 32],
}

fn sys(inner: &str) -> Path {
    Path { volume: String::from("sys"), inner: String::from(inner) }
}

/// Everything on `sys:` to copy, each folder before what is in it: the
/// system first, then the applications.
fn walk() -> Result<Vec<Item>, String> {
    let mut out = Vec::new();
    let mut dirs = vec![String::from("/")];
    while let Some(dir) = dirs.pop() {
        let entries = fs::list_dir(&sys(&dir)).map_err(fs_err(&dir))?;
        for (e, _) in entries {
            let path = if dir == "/" { format!("/{}", e.name) } else { format!("{dir}/{}", e.name) };
            if listed(SKIP, &path) {
                continue;
            }
            let n = fs::node_info(&sys(&path)).map_err(fs_err(&path))?;
            let link = if n.kind == Kind::Link { fs::read_link(&sys(&path)).map_err(fs_err(&path))? } else { String::new() };
            if n.kind == Kind::Dir && !listed(EMPTY, &path) {
                dirs.push(path.clone());
            }
            let apps = APPS.iter().any(|a| under(&path, a));
            out.push(Item { path, kind: n.kind, size: n.size, flags: n.flags, mode: n.mode, link, apps, sha: [0; 32] });
        }
    }
    // Stable: folders stay before what is in them.
    out.sort_by_key(|i| i.apps);
    Ok(out)
}

// ----------------------------------------------------------------------------
// The new system volume

struct Clock;

impl justfs::Clock for Clock {
    fn now(&self) -> u64 {
        crate::time::now()
    }
}

static CLOCK: Clock = Clock;

/// The new system volume's blocks: through the cache, except that while it
/// is formatted, the zeros meant for its node table (written beforehand
/// in big writes) are not written again a block at a time.
struct NewVolume {
    blocks: DiskBlocks,
    zeroed: core::ops::Range<u64>,
    formatting: bool,
}

impl BlockDevice for NewVolume {
    fn read_block(&mut self, block: u64, buf: &mut Block) -> justfs::Result<()> {
        self.blocks.read_block(block, buf)
    }

    fn write_block(&mut self, block: u64, buf: &Block) -> justfs::Result<()> {
        if self.formatting && self.zeroed.contains(&block) && buf.iter().all(|&b| b == 0) {
            return Ok(());
        }
        self.blocks.write_block(block, buf)
    }

    fn block_count(&self) -> u64 {
        self.blocks.block_count()
    }

    fn write_back(&mut self) -> justfs::Result<()> {
        self.blocks.write_back()
    }

    fn flush(&mut self) -> justfs::Result<()> {
        self.blocks.flush()
    }
}

/// Where JustFS puts its node table on a volume of `blocks` blocks (as
/// `Fs::format` does): (first block, end).
fn node_table(blocks: u64) -> (u64, u64) {
    let per_block = (BLOCK_SIZE / justfs::NODE_SIZE) as u64;
    let nodes = (blocks / 4).clamp(256, u32::MAX as u64 / 2).next_multiple_of(per_block);
    let bitmap = blocks.div_ceil(BLOCK_SIZE as u64 * 8);
    let start = 1 + bitmap;
    (start, (start + nodes / per_block).min(blocks))
}

// ----------------------------------------------------------------------------
// Progress

/// How far an installation is.
#[derive(Clone, Default, Debug)]
pub struct Progress {
    /// Index in [`PHASES`].
    pub phase: usize,
    /// Of the whole installation, 0..=1000 (1000 once it is finished).
    pub permille: u32,
    /// Bytes done and in all, in this phase.
    pub done: u64,
    pub total: u64,
    /// What is being copied or checked.
    pub item: String,
    /// Something was written to the disk: its old contents are gone.
    pub touched: bool,
}

/// What a byte of each phase costs in time, about (copying one is 4):
/// zeros go fast, reading back costs about half a copy.
const COST: [u64; 5] = [1, 4, 4, 2, 2];

struct Meter<'a> {
    report: &'a mut dyn FnMut(&Progress),
    p: Progress,
    /// Bytes of each phase.
    totals: [u64; 5],
    last: u64,
}

impl Meter<'_> {
    /// The work done when `done` bytes of `phase` are.
    fn work(&self, phase: usize, done: u64) -> u64 {
        let before: u64 = (0..phase).map(|i| self.totals[i] * COST[i]).sum();
        before + done.min(self.totals[phase]) * COST[phase]
    }

    fn phase(&mut self, phase: usize) {
        self.p.phase = phase;
        self.p.done = 0;
        self.p.total = self.totals[phase];
        self.p.item.clear();
        crate::info!("install", "step {} of {}: {} ({})", phase + 1, PHASES.len(), PHASES[phase], size_text(self.p.total));
        self.update(0, true);
    }

    fn item(&mut self, item: &str) {
        self.p.item.clear();
        self.p.item.push_str(item);
    }

    /// `done` bytes of this phase; reported at most every 50 ms unless
    /// `now`. Below 100% until [`finish`](Self::finish).
    fn update(&mut self, done: u64, now: bool) {
        self.p.done = done.min(self.p.total);
        let all = self.work(4, self.totals[4]).max(1);
        self.p.permille = ((self.work(self.p.phase, self.p.done) as u128 * 1000 / all as u128) as u32).min(999);
        let t = crate::time::uptime_ms();
        if now || t >= self.last + 50 {
            self.last = t;
            (self.report)(&self.p);
        }
    }

    fn finish(&mut self) {
        self.p.done = self.p.total;
        self.p.permille = 1000;
        (self.report)(&self.p);
    }
}

// ----------------------------------------------------------------------------
// Installing

static BUSY: AtomicBool = AtomicBool::new(false);

struct Busy;

impl Busy {
    fn take() -> Result<Busy, Failure> {
        if BUSY.swap(true, Ordering::AcqRel) {
            return Err(Failure::new("un'installazione è già in corso".to_string(), false));
        }
        Ok(Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Why an installation did not happen or stopped.
#[derive(Clone, Debug)]
pub struct Failure {
    pub message: String,
    /// The disk was written to before it stopped: it no longer holds what
    /// it held, and it does not start.
    pub touched: bool,
}

impl Failure {
    fn new(message: String, touched: bool) -> Failure {
        Failure { message, touched }
    }
}

/// Erases the disk `disk` and installs justOS on it. `code` must be the
/// disk's erase code as [`targets`] gives it now. Returns the version
/// installed.
pub fn install(disk: &str, code: &str, report: &mut dyn FnMut(&Progress)) -> Result<String, Failure> {
    let _one = Busy::take()?;
    run(disk, code, report)
}

fn run(name: &str, code: &str, report: &mut dyn FnMut(&Progress)) -> Result<String, Failure> {
    let early = |e: String| Failure::new(e, false);
    let t = targets()
        .into_iter()
        .find(|t| t.disk.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| early(format!("{name} non è un disco su cui installare justOS (il disco del sistema in uso non lo è mai)")))?;
    if !t.code.eq_ignore_ascii_case(code) {
        return Err(early(format!("il codice di conferma per {} non è {code}: il disco è cambiato o il codice è sbagliato", t.disk.name)));
    }
    if let Some(p) = &t.problem {
        return Err(early(format!("{} non si può usare: {p}", t.disk.name)));
    }
    crate::info!("install", "installing justOS {} on {} ({}, {})", crate::VERSION, t.disk.name, t.name, size_text(t.size));
    // Everything needed is read before the disk is touched.
    let boot = boot_files().map_err(early)?;
    let items = walk().map_err(early)?;
    let mut m = Meter { report, p: Progress::default(), totals: [0; 5], last: 0 };
    let result = write(&t.disk, &boot, items, &mut m);
    let touched = m.p.touched;
    match result {
        Ok(()) => {
            m.finish();
            Ok(boot.info.version.clone())
        }
        Err(e) => {
            let e = if block::find(&t.disk.name).is_none() { format!("il disco «{}» è stato scollegato durante l'installazione", t.name) } else { e };
            crate::warn!("install", "{}: {e}", t.disk.name);
            Err(Failure::new(e, touched))
        }
    }
}

/// The installation proper, once everything is checked and read.
fn write(disk: &Arc<Disk>, boot: &BootFiles, mut items: Vec<Item>, m: &mut Meter<'_>) -> Result<(), String> {
    let sys_bytes: u64 = items.iter().filter(|i| i.kind == Kind::File && !i.apps).map(|i| i.size).sum();
    let app_bytes: u64 = items.iter().filter(|i| i.kind == Kind::File && i.apps).map(|i| i.size).sum();

    // Where everything goes.
    let slot_bytes = boot.slot_sectors * SECTOR as u64;
    let esp_bytes = 2 * slot_bytes + (8 << 20);
    let esp_sectors = esp_bytes / SECTOR as u64;
    let part_lba = ESP_LBA + esp_sectors;
    let end = disk.sectors.min(MBR_MAX);
    let part_sectors = (end - part_lba.min(end)) / 8 * 8;
    let blocks = part_sectors * SECTOR as u64 / BLOCK_SIZE as u64;
    let (nodes_from, nodes_to) = node_table(blocks);
    let room = blocks.saturating_sub(nodes_to) * BLOCK_SIZE as u64;
    if room < (sys_bytes + app_bytes) * 11 / 10 + ROOM {
        return Err(format!("{}: il disco è troppo piccolo: servono almeno {}", disk.name, size_text(needed())));
    }
    let lay = fat::build(
        esp_bytes,
        ESP_LBA,
        &[("EFI/BOOT/BOOTX64.EFI", boot.efi.len() as u64), ("JUSTOS/BOOT.CTL", SECTOR as u64), ("JUSTOS/SLOTA.BIN", slot_bytes), ("JUSTOS/SLOTB.BIN", slot_bytes)],
    )?;
    let at = |name: &str| lay.files.iter().find(|(n, _)| n == name).map(|(_, o)| *o / SECTOR as u64).ok_or("la partizione di avvio non ha posto");
    let (efi_at, ctl_at, a_at, b_at) = (at("EFI/BOOT/BOOTX64.EFI")?, at("JUSTOS/BOOT.CTL")?, at("JUSTOS/SLOTA.BIN")?, at("JUSTOS/SLOTB.BIN")?);
    let mut efi = boot.efi.clone();
    efi.resize(efi.len().next_multiple_of(SECTOR), 0);
    let table_sectors = (nodes_to - nodes_from) * (BLOCK_SIZE / SECTOR) as u64;
    let boot_bytes = lay.head.len() as u64 + lay.dirs.iter().map(|d| d.1.len() as u64).sum::<u64>() + efi.len() as u64 + 2 * boot.slot.len() as u64;
    m.totals = [(ESP_LBA + 34 + table_sectors) * SECTOR as u64, sys_bytes, app_bytes, sys_bytes + app_bytes, 2 * boot_bytes];

    // A signature of its own: the kernel tells the disk it started from by
    // it (in the MBR, repeated in BOOT.CTL for the UEFI loader).
    let taken: Vec<u32> = block::list().iter().filter_map(|d| block::signature(d)).collect();
    let mut sig = 0u32;
    while sig == 0 || sig == crate::boot_disk_id() || taken.contains(&sig) {
        sig = crate::rand::u64() as u32;
    }

    // Nothing else may use the disk: its volumes go (written out first).
    for v in fs::volumes() {
        if on_disk(&v.source, disk) {
            fs::unmount(&v.name).map_err(|e| format!("non riesco a staccare il volume {}: dal disco: {e}", v.name))?;
            crate::info!("install", "{}: unmounted", v.name);
        }
    }

    // 1. The start of the disk (old MBR, GPT and loaders) and the end (the
    //    GPT's copy) are wiped; the new volume's node table is zeroed.
    m.phase(0);
    m.p.touched = true;
    m.item("inizio del disco");
    zero(disk, 0, ESP_LBA, &mut |_| {})?;
    if disk.sectors > part_lba + 64 {
        zero(disk, disk.sectors - 34, 34, &mut |_| {})?;
    }
    flush(disk)?;
    let base = (ESP_LBA + 34) * SECTOR as u64;
    m.update(base, true);
    let part = block::view(disk, part_lba, part_sectors).ok_or("il disco è troppo piccolo")?;
    m.item("volume di sistema");
    zero(&part, nodes_from * (BLOCK_SIZE / SECTOR) as u64, table_sectors, &mut |done| m.update(base + done * SECTOR as u64, false))?;
    flush(disk)?;
    let dev = NewVolume { blocks: DiskBlocks::new(part.clone()), zeroed: nodes_from..nodes_to, formatting: true };
    let mut tfs = Fs::format(dev, "System", &CLOCK).map_err(fs_err("formattazione"))?;
    tfs.device().formatting = false;

    // 2. The files.
    let mut buf = vec![0u8; CHUNK];
    let mut since = 0u64;
    let mut done = 0u64;
    let mut phase = 0;
    for item in items.iter_mut() {
        let want = if item.apps { 2 } else { 1 };
        if want != phase {
            phase = want;
            done = 0;
            m.phase(phase);
        }
        m.item(&item.path);
        let err = fs_err(&item.path);
        let (dir, name) = tfs.lookup_parent(&item.path).map_err(&err)?;
        let id = match item.kind {
            Kind::Dir => tfs.create(dir, name, Kind::Dir).map_err(&err)?,
            Kind::Link => tfs.symlink(&item.path, &item.link).map_err(&err)?,
            Kind::File => {
                let id = tfs.create(dir, name, Kind::File).map_err(&err)?;
                let src = sys(&item.path);
                let mut sha = Sha256::new();
                let mut off = 0u64;
                loop {
                    let n = fs::read_at(&src, off, &mut buf).map_err(&err)?;
                    if n == 0 {
                        break;
                    }
                    tfs.write(id, off, &buf[..n]).map_err(&err)?;
                    sha.update(&buf[..n]);
                    off += n as u64;
                    done += n as u64;
                    since += n as u64;
                    if since >= WRITE_BACK {
                        tfs.write_back().map_err(&err)?;
                        since = 0;
                    }
                    m.update(done, false);
                }
                item.size = off;
                item.sha = sha.finish();
                id
            }
            Kind::Free => continue,
        };
        if item.flags != 0 {
            tfs.set_flags(id, item.flags).map_err(&err)?;
        }
        if item.mode & justfs::MODE_SET != 0 {
            tfs.set_mode(id, item.mode).map_err(&err)?;
        }
    }
    let marker = format!(
        "justOS {} installed {} from {}\n",
        boot.info.version,
        crate::time::unix_now(),
        fs::volume("sys").map_or(String::new(), |v| v.source.clone())
    );
    tfs.write_file(MARKER, marker.as_bytes()).map_err(fs_err(MARKER))?;
    tfs.sync().map_err(fs_err("volume di sistema"))?;
    drop(tfs);

    // 3. Read back from the disk (a cache of its own, empty) and compared.
    m.totals[3] = items.iter().filter(|i| i.kind == Kind::File).map(|i| i.size).sum();
    m.phase(3);
    let mut vfs = Fs::mount(DiskBlocks::new(part.clone()), &CLOCK).map_err(fs_err("il volume scritto non si rilegge"))?;
    let mut checked = 0u64;
    for item in &items {
        m.item(&item.path);
        let err = fs_err(&item.path);
        match item.kind {
            Kind::Dir => {
                let id = vfs.lookup(&item.path).map_err(&err)?;
                if !vfs.node(id).map_err(&err)?.is_dir() {
                    return Err(format!("{}: riletto non è una cartella", item.path));
                }
            }
            Kind::Link => {
                let id = vfs.lookup_link(&item.path).map_err(&err)?;
                if vfs.read_link(id).map_err(&err)? != item.link {
                    return Err(format!("{}: il collegamento riletto non corrisponde", item.path));
                }
            }
            Kind::File => {
                let id = vfs.lookup(&item.path).map_err(&err)?;
                if vfs.node(id).map_err(&err)?.size != item.size {
                    return Err(format!("{}: la dimensione riletta non corrisponde", item.path));
                }
                let mut sha = Sha256::new();
                let mut off = 0u64;
                while off < item.size {
                    let n = vfs.read(id, off, &mut buf).map_err(&err)?;
                    if n == 0 {
                        break;
                    }
                    sha.update(&buf[..n]);
                    off += n as u64;
                    checked += n as u64;
                    m.update(checked, false);
                }
                if sha.finish() != item.sha {
                    return Err(format!("{}: il file riletto dal disco non corrisponde: il disco ha problemi", item.path));
                }
            }
            Kind::Free => {}
        }
    }
    m.item("controllo del volume");
    let problems = vfs.check().map_err(fs_err("controllo del volume"))?;
    if problems != 0 {
        return Err(format!("il controllo del volume scritto ha trovato {problems} problemi"));
    }
    drop(vfs);

    // 4. The boot partition, stage 2, and the MBR last.
    m.phase(4);
    let mut written = 0u64;
    m.item("partizione di avvio");
    let esp = block::view(disk, ESP_LBA, esp_sectors).ok_or("il disco è troppo piccolo")?;
    wr(&esp, 0, &lay.head)?;
    for (off, table) in &lay.dirs {
        wr(&esp, off / SECTOR as u64, table)?;
    }
    wr(&esp, efi_at, &efi)?;
    written += lay.head.len() as u64 + efi.len() as u64;
    m.update(written, true);
    for slot_at in [a_at, b_at] {
        // The header last, as for an update.
        let mut off = SECTOR;
        while off < boot.slot.len() {
            let n = (boot.slot.len() - off).min(CHUNK);
            wr(&esp, slot_at + (off / SECTOR) as u64, &boot.slot[off..off + n])?;
            off += n;
            m.update(written + off as u64, false);
        }
        wr(&esp, slot_at, &boot.slot[..SECTOR])?;
        written += boot.slot.len() as u64;
    }
    let (slot_a, slot_b) = (ESP_LBA + a_at, ESP_LBA + b_at);
    let mut ctl = vec![0u8; SECTOR];
    ctl[..8].copy_from_slice(b"ARCCTL01");
    put32(&mut ctl, 0x08, slot_a as u32);
    put32(&mut ctl, 0x0C, slot_b as u32);
    put32(&mut ctl, 0x10, boot.slot_sectors as u32);
    ctl[0x16] = 0b11;
    put32(&mut ctl, 0x18, 1);
    put32(&mut ctl, 0x1C, sig);
    let s = sum(&ctl);
    put32(&mut ctl, 0x1FC, s.wrapping_neg());
    wr(&esp, ctl_at, &ctl)?;
    flush(disk)?;
    m.update(written, true);

    // Read back: the loader as the firmware finds it, both slots whole,
    // BOOT.CTL.
    m.item("controllo della partizione di avvio");
    let efi_back = fat::read_file(&mut |lba, buf: &mut [u8]| rd(&esp, lba, buf), "EFI/BOOT/BOOTX64.EFI")?;
    if efi_back != boot.efi {
        return Err("il caricatore UEFI riletto non corrisponde: il disco ha problemi".to_string());
    }
    let ctl_back = fat::read_file(&mut |lba, buf: &mut [u8]| rd(&esp, lba, buf), "JUSTOS/BOOT.CTL")?;
    if ctl_back != ctl {
        return Err("BOOT.CTL riletto non corrisponde".to_string());
    }
    written += lay.head.len() as u64 + efi.len() as u64;
    let mut back = vec![0u8; boot.slot.len()];
    for slot_at in [a_at, b_at] {
        rd(&esp, slot_at, &mut back)?;
        if Slot::check(&back).map(|s| s.sha256) != Ok(boot.info.sha256) {
            return Err("uno slot di sistema riletto non corrisponde: il disco ha problemi".to_string());
        }
        written += boot.slot.len() as u64;
        m.update(written, true);
    }

    m.item("caricatore BIOS");
    let stage2_sectors = boot.stage2.len() / SECTOR;
    wr(disk, 1, &boot.stage2)?;
    let mut mbr = boot.stage1.clone();
    mbr.resize(SECTOR, 0);
    let i = &boot.info;
    put32(&mut mbr, 0x180, 1);
    put32(&mut mbr, 0x184, 0);
    put32(&mut mbr, 0x188, 0);
    put32(&mut mbr, 0x18C, (ESP_LBA + ctl_at) as u32);
    mbr[0x190..0x194].copy_from_slice(b"ARCB");
    mbr[0x194..0x196].copy_from_slice(&(stage2_sectors as u16).to_le_bytes());
    mbr[0x196..0x198].copy_from_slice(&0u16.to_le_bytes());
    put32(&mut mbr, 0x198, (slot_a + 1) as u32);
    put32(&mut mbr, 0x19C, i.kernel_sectors);
    put32(&mut mbr, 0x1A0, i.memsz);
    put32(&mut mbr, 0x1A4, (slot_a + i.pack_sector as u64) as u32);
    put32(&mut mbr, 0x1A8, i.pack_sectors);
    mbr[0x1AC..0x1B4].copy_from_slice(&i.entry.to_le_bytes());
    mbr[0x1B4..0x1B8].fill(0);
    put32(&mut mbr, 0x1B8, sig);
    mbr[0x1BC..0x1BE].fill(0);
    let mut entry = |n: usize, active: bool, kind: u8, lba: u64, sectors: u64| {
        let e = &mut mbr[0x1BE + n * 16..0x1CE + n * 16];
        e.fill(0);
        if kind != 0 {
            e[0] = if active { 0x80 } else { 0 };
            e[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
            e[4] = kind;
            e[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
            e[8..12].copy_from_slice(&(lba as u32).to_le_bytes());
            e[12..16].copy_from_slice(&(sectors as u32).to_le_bytes());
        }
    };
    entry(0, true, 0xAC, part_lba, part_sectors);
    entry(1, false, 0xEF, ESP_LBA, esp_sectors);
    entry(2, false, 0, 0, 0);
    entry(3, false, 0, 0, 0);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    let mut back = vec![0u8; boot.stage2.len()];
    flush(disk)?;
    rd(disk, 1, &mut back)?;
    if back != boot.stage2 {
        return Err("il caricatore BIOS riletto non corrisponde: il disco ha problemi".to_string());
    }
    m.item("tabella delle partizioni");
    wr(disk, 0, &mbr)?;
    flush(disk)?;
    let mut back = vec![0u8; SECTOR];
    rd(disk, 0, &mut back)?;
    if back != mbr {
        return Err("l'MBR riletto non corrisponde: il disco ha problemi".to_string());
    }
    if let Err(e) = block::rescan(disk) {
        crate::warn!("install", "{}: partitions not read again: {e}", disk.name);
    }
    crate::info!(
        "install",
        "justOS {} installed on {}: {} files, {} copied and checked, system volume {}",
        boot.info.version,
        disk.name,
        items.iter().filter(|i| i.kind == Kind::File).count(),
        crate::util::Size(sys_bytes + app_bytes),
        crate::util::Size(blocks * BLOCK_SIZE as u64)
    );
    Ok(())
}

// ----------------------------------------------------------------------------
// For the interface: an installation in its own thread

#[derive(Clone, Debug)]
pub enum State {
    Idle,
    Working(Progress),
    /// The version installed.
    Done(String),
    Failed(Failure),
}

static STATE: IrqLock<State> = IrqLock::new(State::Idle);

pub fn state() -> State {
    STATE.lock().clone()
}

/// Starts [`install`] in a thread of its own; [`state`] follows it.
pub fn start(disk: String, code: String) -> Result<(), String> {
    let one = Busy::take().map_err(|f| f.message)?;
    *STATE.lock() = State::Working(Progress::default());
    let spawned = crate::task::spawn_big("just.install", 256 << 10, move || {
        let _one = one;
        let result = run(&disk, &code, &mut |p| *STATE.lock() = State::Working(p.clone()));
        *STATE.lock() = match result {
            Ok(v) => State::Done(v),
            Err(f) => State::Failed(f),
        };
    });
    if spawned.is_none() {
        *STATE.lock() = State::Idle;
        return Err("non riesco ad avviare l'installazione".to_string());
    }
    Ok(())
}
