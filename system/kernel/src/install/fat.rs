//! The FAT16 boot partition, for the installer: laid out exactly as
//! tools/justtool/src/fat.rs makes it (every file and folder in contiguous
//! clusters, the data area on a 2048-byte boundary), so a disk installed
//! from a running system matches a disk image; and a reader for one file,
//! to take the UEFI loader from the disk the system started from and to
//! read back what was written.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

const SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: usize = 4;
const CLUSTER: usize = SECTOR * SECTORS_PER_CLUSTER;
const RESERVED_SECTORS: usize = 4;
const FATS: usize = 2;
const ROOT_ENTRIES: usize = 512;
const ATTR_LABEL: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LONG_NAME: u8 = 0x0F;
/// 2026-01-01, 00:00 in FAT's packed format (as justtool writes it).
const DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;
const LABEL: &[u8; 11] = b"JUSTOS BOOT";

#[derive(Default)]
struct Dir {
    dirs: Vec<(String, Dir)>,
    /// Name and size in bytes.
    files: Vec<(String, u64)>,
}

impl Dir {
    fn add(&mut self, path: &str, size: u64) -> Result<(), String> {
        let (first, rest) = match path.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (path, None),
        };
        short_name(first)?;
        match rest {
            None => self.files.push((first.to_string(), size)),
            Some(rest) => {
                let i = match self.dirs.iter().position(|(n, _)| n == first) {
                    Some(i) => i,
                    None => {
                        self.dirs.push((first.to_string(), Dir::default()));
                        self.dirs.len() - 1
                    }
                };
                self.dirs[i].1.add(rest, size)?;
            }
        }
        Ok(())
    }

    /// Entries the folder needs: "." and "..", or the label in the root.
    fn entries(&self, root: bool) -> usize {
        self.dirs.len() + self.files.len() + if root { 1 } else { 2 }
    }
}

/// The 8.3 name as stored: "BOOTX64.EFI" becomes "BOOTX64 EFI".
fn short_name(name: &str) -> Result<[u8; 11], String> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    let ok = |s: &str, max| !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok(base, 8) || !(ext.is_empty() || ok(ext, 3)) {
        return Err(format!("{name}: non è un nome 8.3"));
    }
    let mut out = [b' '; 11];
    for (i, b) in base.bytes().enumerate() {
        out[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().enumerate() {
        out[8 + i] = b.to_ascii_uppercase();
    }
    Ok(out)
}

fn dir_entry(name: [u8; 11], attr: u8, cluster: u16, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&name);
    e[11] = attr;
    e[16..18].copy_from_slice(&DATE.to_le_bytes());
    e[18..20].copy_from_slice(&DATE.to_le_bytes());
    e[24..26].copy_from_slice(&DATE.to_le_bytes());
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// What to write for a new boot partition. Offsets count bytes from the
/// partition's first sector; the files' data is written by the caller.
pub struct Layout {
    /// Everything before the data area: boot sector, both FATs, the root
    /// folder.
    pub head: Vec<u8>,
    /// The other folders' tables, each its whole clusters.
    pub dirs: Vec<(u64, Vec<u8>)>,
    /// Where each file's data starts, by path.
    pub files: Vec<(String, u64)>,
}

struct Writer {
    fat: Vec<u16>,
    next: usize,
    data_start: usize,
    out: Layout,
}

impl Writer {
    fn alloc(&mut self, bytes: u64) -> Result<u16, String> {
        let count = (bytes as usize).div_ceil(CLUSTER).max(1);
        if self.next + count > self.fat.len() {
            return Err("la partizione di avvio è piena".to_string());
        }
        let first = self.next;
        for c in first..first + count {
            self.fat[c] = if c + 1 == first + count { 0xFFFF } else { (c + 1) as u16 };
        }
        self.next += count;
        Ok(first as u16)
    }

    fn offset(&self, cluster: u16) -> u64 {
        (self.data_start + (cluster as usize - 2) * CLUSTER) as u64
    }

    fn write_dir(&mut self, dir: &Dir, own: u16, parent: u16, path: &str) -> Result<Vec<u8>, String> {
        let mut table = Vec::new();
        if own == 0 {
            table.extend_from_slice(&dir_entry(*LABEL, ATTR_LABEL, 0, 0));
        } else {
            table.extend_from_slice(&dir_entry(*b".          ", ATTR_DIR, own, 0));
            table.extend_from_slice(&dir_entry(*b"..         ", ATTR_DIR, parent, 0));
        }
        for (name, size) in &dir.files {
            let cluster = if *size == 0 { 0 } else { self.alloc(*size)? };
            if cluster != 0 {
                let at = self.offset(cluster);
                self.out.files.push((format!("{path}{name}"), at));
            }
            table.extend_from_slice(&dir_entry(short_name(name)?, ATTR_ARCHIVE, cluster, *size as u32));
        }
        for (name, sub) in &dir.dirs {
            let bytes = sub.entries(false) as u64 * 32;
            let cluster = self.alloc(bytes)?;
            let mut contents = self.write_dir(sub, cluster, own, &format!("{path}{name}/"))?;
            // Whole clusters: what the disk held there before must not
            // read as more entries.
            contents.resize((bytes as usize).div_ceil(CLUSTER).max(1) * CLUSTER, 0);
            let at = self.offset(cluster);
            self.out.dirs.push((at, contents));
            table.extend_from_slice(&dir_entry(short_name(name)?, ATTR_DIR, cluster, 0));
        }
        Ok(table)
    }
}

/// Lays out a FAT16 volume of `size` bytes holding `files` (paths with
/// '/', and sizes); `hidden` is the partition's first sector on the disk.
pub fn build(size: u64, hidden: u64, files: &[(&str, u64)]) -> Result<Layout, String> {
    let total = (size as usize) / SECTOR;
    let root_sectors = ROOT_ENTRIES * 32 / SECTOR;
    let clusters_estimate = (total - RESERVED_SECTORS - root_sectors) / SECTORS_PER_CLUSTER;
    let fat_sectors = ((clusters_estimate + 2) * 2).div_ceil(SECTOR);
    let unaligned = RESERVED_SECTORS + FATS * fat_sectors + root_sectors;
    let reserved = RESERVED_SECTORS + unaligned.next_multiple_of(SECTORS_PER_CLUSTER) - unaligned;
    let data_start = reserved + FATS * fat_sectors + root_sectors;
    let clusters = (total - data_start) / SECTORS_PER_CLUSTER;
    if !(4085..65525).contains(&clusters) {
        return Err(format!("una partizione di avvio di {} MiB non può essere FAT16", size >> 20));
    }
    let mut root = Dir::default();
    for (path, len) in files {
        root.add(path, *len)?;
    }
    if root.entries(true) > ROOT_ENTRIES {
        return Err("troppi file nella partizione di avvio".to_string());
    }
    let mut w = Writer {
        fat: vec![0; clusters + 2],
        next: 2,
        data_start: data_start * SECTOR,
        out: Layout { head: vec![0; data_start * SECTOR], dirs: Vec::new(), files: Vec::new() },
    };
    w.fat[0] = 0xFFF8;
    w.fat[1] = 0xFFFF;
    let root_table = w.write_dir(&root, 0, 0, "")?;
    let mut out = w.out;
    let root_at = (reserved + FATS * fat_sectors) * SECTOR;
    out.head[root_at..root_at + root_table.len()].copy_from_slice(&root_table);

    let b = &mut out.head[..SECTOR];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"JUSTOS  ");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = SECTORS_PER_CLUSTER as u8;
    b[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
    b[16] = FATS as u8;
    b[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    if total < 0x10000 {
        b[19..21].copy_from_slice(&(total as u16).to_le_bytes());
    } else {
        b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    }
    b[21] = 0xF8;
    b[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&64u16.to_le_bytes());
    b[28..32].copy_from_slice(&(hidden as u32).to_le_bytes());
    b[36] = 0x80;
    b[38] = 0x29;
    b[39..43].copy_from_slice(&0x4152_4331u32.to_le_bytes());
    b[43..54].copy_from_slice(LABEL);
    b[54..62].copy_from_slice(b"FAT16   ");
    b[510] = 0x55;
    b[511] = 0xAA;

    let fat_bytes: Vec<u8> = w.fat.iter().flat_map(|e| e.to_le_bytes()).collect();
    for i in 0..FATS {
        let at = (reserved + i * fat_sectors) * SECTOR;
        out.head[at..at + fat_bytes.len()].copy_from_slice(&fat_bytes);
    }
    Ok(out)
}

/// Reads whole sectors of a partition: (first sector, buffer).
pub type ReadFn<'a> = dyn FnMut(u64, &mut [u8]) -> Result<(), String> + 'a;

/// A FAT16 volume being read.
struct Volume {
    per_cluster: u64,
    data_lba: u64,
    clusters: u64,
    fat: Vec<u8>,
}

impl Volume {
    fn open(read: &mut ReadFn<'_>) -> Result<(Volume, u64, u64), String> {
        let mut b = vec![0u8; SECTOR];
        read(0, &mut b)?;
        let get16 = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]) as u64;
        if b[510] != 0x55 || b[511] != 0xAA || get16(11) != SECTOR as u64 || b[13] == 0 || b[16] == 0 {
            return Err("la partizione di avvio non è FAT".to_string());
        }
        let per_cluster = b[13] as u64;
        let reserved = get16(14);
        let fats = b[16] as u64;
        let root_entries = get16(17);
        let fat_sectors = get16(22);
        if fat_sectors == 0 || root_entries == 0 {
            return Err("la partizione di avvio non è FAT16".to_string());
        }
        let total = match get16(19) {
            0 => u32::from_le_bytes([b[32], b[33], b[34], b[35]]) as u64,
            n => n,
        };
        let root_lba = reserved + fats * fat_sectors;
        let root_sectors = (root_entries * 32).div_ceil(SECTOR as u64);
        let data_lba = root_lba + root_sectors;
        let clusters = total.saturating_sub(data_lba) / per_cluster;
        let mut fat = vec![0u8; fat_sectors as usize * SECTOR];
        read(reserved, &mut fat)?;
        Ok((Volume { per_cluster, data_lba, clusters, fat }, root_lba, root_sectors))
    }

    fn next(&self, c: u64) -> u64 {
        let at = c as usize * 2;
        if at + 1 >= self.fat.len() {
            return 0xFFF7;
        }
        u16::from_le_bytes([self.fat[at], self.fat[at + 1]]) as u64
    }

    /// The clusters of the chain that starts at `first`, in order.
    fn chain(&self, first: u64) -> Result<Vec<u64>, String> {
        let mut out = Vec::new();
        let mut c = first;
        while (2..self.clusters + 2).contains(&c) {
            if out.len() as u64 > self.clusters {
                return Err("la FAT della partizione di avvio gira in tondo".to_string());
            }
            out.push(c);
            c = self.next(c);
        }
        if c < 0xFFF8 {
            return Err("la FAT della partizione di avvio è danneggiata".to_string());
        }
        Ok(out)
    }

    /// The data of a chain, cut to `len` bytes if given.
    fn read_chain(&self, read: &mut ReadFn<'_>, first: u64, len: Option<u64>) -> Result<Vec<u8>, String> {
        let cl = self.chain(first)?;
        let cluster_bytes = self.per_cluster as usize * SECTOR;
        let mut data = vec![0u8; cl.len() * cluster_bytes];
        // Clusters that follow each other in one read.
        let mut i = 0;
        while i < cl.len() {
            let mut n = 1;
            while i + n < cl.len() && cl[i + n] == cl[i] + n as u64 && n < 64 {
                n += 1;
            }
            let at = i * cluster_bytes;
            read(self.data_lba + (cl[i] - 2) * self.per_cluster, &mut data[at..at + n * cluster_bytes])?;
            i += n;
        }
        if let Some(len) = len {
            if len as usize > data.len() {
                return Err("un file della partizione di avvio ha meno dati della sua dimensione".to_string());
            }
            data.truncate(len as usize);
        }
        Ok(data)
    }
}

/// Reads the file at `path` ('/' between names, any case) from a FAT16
/// volume.
pub fn read_file(read: &mut ReadFn<'_>, path: &str) -> Result<Vec<u8>, String> {
    let (vol, root_lba, root_sectors) = Volume::open(read)?;
    let mut table = vec![0u8; root_sectors as usize * SECTOR];
    read(root_lba, &mut table)?;
    let names: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for (i, name) in names.iter().enumerate() {
        let want = short_name(name)?;
        let mut found = None;
        for e in table.chunks_exact(32) {
            if e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 || e[11] == ATTR_LONG_NAME || e[11] & ATTR_LABEL != 0 {
                continue;
            }
            if e[..11].eq_ignore_ascii_case(&want) {
                let cluster = u16::from_le_bytes([e[26], e[27]]) as u64;
                let size = u32::from_le_bytes([e[28], e[29], e[30], e[31]]) as u64;
                found = Some((e[11] & ATTR_DIR != 0, cluster, size));
                break;
            }
        }
        let Some((dir, cluster, size)) = found else {
            return Err(format!("{path} non c'è nella partizione di avvio"));
        };
        if i + 1 == names.len() {
            if dir {
                return Err(format!("{path} è una cartella"));
            }
            if size == 0 {
                return Ok(Vec::new());
            }
            return vol.read_chain(read, cluster, Some(size));
        }
        if !dir {
            return Err(format!("{name} non è una cartella"));
        }
        table = vol.read_chain(read, cluster, None)?;
    }
    Err(format!("{path}: percorso vuoto"))
}
