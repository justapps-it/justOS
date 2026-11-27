//! Shared memory: `memfd_create`, `/dev/shm` (what `shm_open` opens) and
//! the memory behind `MAP_SHARED | MAP_ANONYMOUS`.
//!
//! A [`Shm`] owns its frames. Every shared mapping of it maps those same
//! frames, marked [`paging::SHARED`]: unmapping them or ending a process
//! never frees them, and fork shares them instead of copying. They go with
//! the last descriptor or mapping that holds the `Shm`. This is how a
//! Wayland client hands the compositor its pixels, and how a browser's
//! processes share buffers.

use super::fd::{self, Desc, Kind, O_CREAT, O_EXCL, O_RDWR, O_TRUNC};
use super::err;
use crate::mem::{frame, paging, phys_to_virt, PAGE_SIZE};
use crate::sync::IrqLock;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MFD_CLOEXEC: u32 = 1;
const MFD_ALLOW_SEALING: u32 = 2;

pub const F_SEAL_SEAL: u32 = 1;
const F_SEAL_SHRINK: u32 = 2;
const F_SEAL_GROW: u32 = 4;
const F_SEAL_WRITE: u32 = 8;
const F_SEAL_FUTURE_WRITE: u32 = 0x10;

/// A piece of shared memory.
pub struct Shm {
    /// Frames by page number; a page never touched has none (zeroes).
    pages: IrqLock<BTreeMap<u64, u64>>,
    size: AtomicU64,
    /// What /proc/self/fd shows ("/memfd:name (deleted)").
    pub name: String,
    seals: AtomicU32,
}

impl Shm {
    pub fn new(name: String, size: u64, seals: u32) -> Arc<Shm> {
        Arc::new(Shm { pages: IrqLock::new(BTreeMap::new()), size: AtomicU64::new(size), name, seals: AtomicU32::new(seals) })
    }

    /// Read-only memory holding `data` (sealed: it can never change), as a
    /// keymap is handed to a Wayland client.
    pub fn sealed(name: String, data: &[u8]) -> Arc<Shm> {
        let s = Shm::new(name, data.len() as u64, 0);
        let _ = s.write_at(0, data);
        s.seals.store(F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE, Ordering::Relaxed);
        s
    }

    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// The frame of page `n`, made (zeroed) when first wanted.
    pub fn frame(&self, n: u64) -> Option<u64> {
        let mut p = self.pages.lock();
        if let Some(&f) = p.get(&n) {
            return Some(f);
        }
        let f = frame::alloc()?;
        p.insert(n, f);
        Some(f)
    }

    /// The frame of page `n`, if it was ever touched.
    pub fn existing(&self, n: u64) -> Option<u64> {
        self.pages.lock().get(&n).copied()
    }

    /// Copies out what is at `off` (up to the size).
    pub fn read_at(&self, off: u64, buf: &mut [u8]) -> usize {
        let size = self.size();
        if off >= size {
            return 0;
        }
        let n = (buf.len() as u64).min(size - off) as usize;
        let mut done = 0;
        while done < n {
            let at = off + done as u64;
            let inpage = (at % PAGE_SIZE) as usize;
            let k = (PAGE_SIZE as usize - inpage).min(n - done);
            match self.existing(at / PAGE_SIZE) {
                // SAFETY: the frame is RAM this Shm owns, reachable through
                // the direct map.
                Some(f) => unsafe { core::ptr::copy_nonoverlapping((phys_to_virt(f) as *const u8).add(inpage), buf[done..].as_mut_ptr(), k) },
                None => buf[done..done + k].fill(0),
            }
            done += k;
        }
        n
    }

    /// Writes at `off`, growing the size if it goes past it.
    pub fn write_at(&self, off: u64, data: &[u8]) -> Result<usize, i64> {
        let seals = self.seals.load(Ordering::Relaxed);
        if seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
            return Err(-err::EPERM);
        }
        let end = off.checked_add(data.len() as u64).ok_or(-err::EFBIG)?;
        if end > self.size() && seals & F_SEAL_GROW != 0 {
            return Err(-err::EPERM);
        }
        let mut done = 0;
        while done < data.len() {
            let at = off + done as u64;
            let inpage = (at % PAGE_SIZE) as usize;
            let k = (PAGE_SIZE as usize - inpage).min(data.len() - done);
            let f = self.frame(at / PAGE_SIZE).ok_or(-err::ENOMEM)?;
            // SAFETY: as in read_at.
            unsafe { core::ptr::copy_nonoverlapping(data[done..].as_ptr(), (phys_to_virt(f) as *mut u8).add(inpage), k) };
            done += k;
        }
        self.size.fetch_max(end, Ordering::Relaxed);
        Ok(done)
    }

    /// ftruncate. What is cut off reads as zeroes if the size grows again;
    /// its frames stay until the end (a mapping may still reach them).
    pub fn truncate(&self, len: u64) -> Result<(), i64> {
        let size = self.size();
        let seals = self.seals.load(Ordering::Relaxed);
        if len < size && seals & F_SEAL_SHRINK != 0 || len > size && seals & F_SEAL_GROW != 0 {
            return Err(-err::EPERM);
        }
        if len < size {
            let frames: alloc::vec::Vec<(u64, u64)> = self.pages.lock().range(len / PAGE_SIZE..).map(|(&n, &f)| (n, f)).collect();
            for (n, f) in frames {
                let from = len.saturating_sub(n * PAGE_SIZE).min(PAGE_SIZE) as usize;
                // SAFETY: as in read_at.
                unsafe { core::ptr::write_bytes((phys_to_virt(f) as *mut u8).add(from), 0, PAGE_SIZE as usize - from) };
            }
        }
        self.size.store(len, Ordering::Relaxed);
        Ok(())
    }

    pub fn seals(&self) -> u32 {
        self.seals.load(Ordering::Relaxed)
    }

    /// F_ADD_SEALS.
    pub fn add_seals(&self, s: u32) -> i64 {
        if self.seals() & F_SEAL_SEAL != 0 {
            return -err::EPERM;
        }
        self.seals.fetch_or(s & 0x1f, Ordering::Relaxed);
        0
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        for (_, f) in core::mem::take(&mut *self.pages.lock()) {
            frame::free(f);
        }
    }
}

// The frames are only ever mapped as shared pages.
const _: () = assert!(paging::SHARED & paging::ADDR_MASK == 0);

fn install(shm: Arc<Shm>, flags: u32, cloexec: bool) -> i64 {
    fd::install(Desc::new(Kind::Memfd { shm, pos: IrqLock::new(0) }, flags), cloexec)
}

pub fn sys_memfd_create(name_ptr: u64, flags: u32) -> i64 {
    let name = match crate::user::user_cstr(name_ptr, 250) {
        Ok(n) => n,
        Err(e) => return -super::errno(e),
    };
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING | 4 | 8 | 0xfc00_0000) != 0 {
        return -err::EINVAL;
    }
    // Without MFD_ALLOW_SEALING, no seals can be added.
    let seals = if flags & MFD_ALLOW_SEALING != 0 { 0 } else { F_SEAL_SEAL };
    install(Shm::new(alloc::format!("/memfd:{name} (deleted)"), 0, seals), O_RDWR, flags & MFD_CLOEXEC != 0)
}

/// Memory for `MAP_SHARED | MAP_ANONYMOUS`.
pub fn anonymous(len: u64) -> Arc<Shm> {
    Shm::new(String::from("/dev/zero (deleted)"), len, F_SEAL_SEAL)
}

// ----------------------------------------------------------------------------
// /dev/shm: named shared memory (shm_open), kept until unlinked.

static NAMED: IrqLock<BTreeMap<String, Arc<Shm>>> = IrqLock::new(BTreeMap::new());

/// The name inside /dev/shm, for a path there.
pub fn named(text: &str) -> Option<&str> {
    let n = text.strip_prefix("/dev/shm/")?;
    (!n.is_empty() && !n.contains('/')).then_some(n)
}

pub fn open_named(name: &str, flags: u32) -> i64 {
    let shm = {
        let mut all = NAMED.lock();
        match all.get(name) {
            Some(_) if flags & O_CREAT != 0 && flags & O_EXCL != 0 => return -err::EEXIST,
            Some(s) => s.clone(),
            None if flags & O_CREAT != 0 => {
                let s = Shm::new(alloc::format!("/dev/shm/{name}"), 0, F_SEAL_SEAL);
                all.insert(String::from(name), s.clone());
                s
            }
            None => return -err::ENOENT,
        }
    };
    if flags & O_TRUNC != 0 {
        let _ = shm.truncate(0);
    }
    install(shm, flags & 3, flags & fd::O_CLOEXEC != 0)
}

/// unlink of /dev/shm/name.
pub fn unlink_named(name: &str) -> i64 {
    match NAMED.lock().remove(name) {
        Some(s) => {
            drop(s);
            0
        }
        None => -err::ENOENT,
    }
}

/// Whether /dev/shm/name exists (for stat and access).
pub fn named_size(name: &str) -> Option<u64> {
    NAMED.lock().get(name).map(|s| s.size())
}

// ----------------------------------------------------------------------------
// fallocate

pub fn sys_fallocate(fdn: i32, mode: u64, off: u64, len: u64) -> i64 {
    let Some(d) = fd::get(fdn) else { return -err::EBADF };
    match &d.kind {
        Kind::Memfd { shm, .. } => {
            if mode != 0 {
                return -err::EOPNOTSUPP;
            }
            let end = off.saturating_add(len);
            if end > shm.size() {
                if let Err(e) = shm.truncate(end) {
                    return e;
                }
            }
            0
        }
        // glibc's posix_fallocate writes the zeroes itself after this.
        Kind::File { .. } => -err::EOPNOTSUPP,
        _ => -err::ESPIPE,
    }
}

// ----------------------------------------------------------------------------
// System V shared memory: shmget, shmat, shmdt, shmctl (Audacity keeps one
// instance with it; X11's MIT-SHM too).

const IPC_CREAT: u32 = 0o1000;
const IPC_EXCL: u32 = 0o2000;
const IPC_RMID: u32 = 0;
const IPC_SET: u32 = 1;
const IPC_STAT: u32 = 2;
const SHM_RDONLY: u32 = 0o10000;
const SHM_RND: u32 = 0o20000;
const SHMLBA: u64 = PAGE_SIZE;

struct Segment {
    key: i32,
    shm: Arc<Shm>,
    size: u64,
    mode: u32,
    cpid: u32,
    lpid: u32,
    attached: u64,
    /// Removed (IPC_RMID): it goes when the last process detaches.
    removed: bool,
}

struct SysV {
    next: i32,
    segs: BTreeMap<i32, Segment>,
    /// Where each process attached which segment: (pid, address) → id.
    at: BTreeMap<(u32, u64), i32>,
}

static SYSV: IrqLock<SysV> = IrqLock::new(SysV { next: 1, segs: BTreeMap::new(), at: BTreeMap::new() });

pub fn sys_shmget(key: i32, size: u64, flags: u32) -> i64 {
    let mut s = SYSV.lock();
    if key != 0 {
        if let Some((&id, seg)) = s.segs.iter().find(|(_, g)| g.key == key && !g.removed) {
            if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                return -err::EEXIST;
            }
            if size > seg.size {
                return -err::EINVAL;
            }
            return id as i64;
        }
        if flags & IPC_CREAT == 0 {
            return -err::ENOENT;
        }
    }
    if size == 0 || size > 1 << 32 {
        return -err::EINVAL;
    }
    let id = s.next;
    s.next = s.next.wrapping_add(1).max(1);
    let shm = Shm::new(alloc::format!("/SYSV{key:08x} (deleted)"), size, F_SEAL_SEAL);
    let pid = crate::task::current_pid();
    s.segs.insert(id, Segment { key, shm, size, mode: flags & 0o777, cpid: pid, lpid: 0, attached: 0, removed: false });
    id as i64
}

pub fn sys_shmat(id: i32, addr: u64, flags: u32) -> i64 {
    let (shm, size) = {
        let s = SYSV.lock();
        let Some(g) = s.segs.get(&id) else { return -err::EINVAL };
        (g.shm.clone(), g.size)
    };
    let addr = if flags & SHM_RND != 0 { addr & !(SHMLBA - 1) } else { addr };
    if addr % PAGE_SIZE != 0 {
        return -err::EINVAL;
    }
    let at = super::mm::map_shared(addr, size, flags & SHM_RDONLY == 0, shm);
    if at >= 0 {
        let pid = crate::task::current_pid();
        let mut s = SYSV.lock();
        s.at.insert((pid, at as u64), id);
        if let Some(g) = s.segs.get_mut(&id) {
            g.attached += 1;
            g.lpid = pid;
        }
    }
    at
}

pub fn sys_shmdt(addr: u64) -> i64 {
    let pid = crate::task::current_pid();
    let size = {
        let mut s = SYSV.lock();
        let Some(id) = s.at.remove(&(pid, addr)) else { return -err::EINVAL };
        let Some(g) = s.segs.get_mut(&id) else { return -err::EINVAL };
        g.attached = g.attached.saturating_sub(1);
        g.lpid = pid;
        let size = g.size;
        if g.removed && g.attached == 0 {
            s.segs.remove(&id);
        }
        size
    };
    super::mm::sys_munmap(addr, size)
}

pub fn sys_shmctl(id: i32, cmd: u32, buf: u64) -> i64 {
    let mut s = SYSV.lock();
    let Some(g) = s.segs.get_mut(&id) else { return -err::EINVAL };
    match cmd & 0xff {
        IPC_RMID => {
            g.removed = true;
            if g.attached == 0 {
                s.segs.remove(&id);
            }
            0
        }
        IPC_SET => 0,
        IPC_STAT => {
            // struct shmid_ds: struct ipc_perm (48 bytes), then the size,
            // the times, the pids and the number attached.
            let (key, mode, size, cpid, lpid, n) = (g.key, g.mode, g.size, g.cpid, g.lpid, g.attached);
            drop(s);
            let out = match super::user_bytes(buf, 112) {
                Ok(b) => b,
                Err(e) => return e,
            };
            out.fill(0);
            out[0..4].copy_from_slice(&key.to_le_bytes());
            out[20..22].copy_from_slice(&(mode as u16).to_le_bytes());
            out[48..56].copy_from_slice(&size.to_le_bytes());
            out[80..84].copy_from_slice(&cpid.to_le_bytes());
            out[84..88].copy_from_slice(&lpid.to_le_bytes());
            out[88..96].copy_from_slice(&n.to_le_bytes());
            0
        }
        _ => -err::EINVAL,
    }
}

// ----------------------------------------------------------------------------
// System V semaphores: semget, semop, semctl (with the shared memory above,
// how Audacity keeps to one instance).

const IPC_NOWAIT: i16 = 0o4000;
const GETPID: u32 = 11;
const GETVAL: u32 = 12;
const GETALL: u32 = 13;
const GETNCNT: u32 = 14;
const GETZCNT: u32 = 15;
const SETVAL: u32 = 16;
const SETALL: u32 = 17;

struct SemSet {
    key: i32,
    vals: alloc::vec::Vec<u16>,
    pids: alloc::vec::Vec<u32>,
    mode: u32,
}

struct Sems {
    next: i32,
    sets: BTreeMap<i32, SemSet>,
}

static SEMS: IrqLock<Sems> = IrqLock::new(Sems { next: 1, sets: BTreeMap::new() });

pub fn sys_semget(key: i32, n: i32, flags: u32) -> i64 {
    let mut s = SEMS.lock();
    if key != 0 {
        if let Some((&id, set)) = s.sets.iter().find(|(_, g)| g.key == key) {
            if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                return -err::EEXIST;
            }
            if n > set.vals.len() as i32 {
                return -err::EINVAL;
            }
            return id as i64;
        }
        if flags & IPC_CREAT == 0 {
            return -err::ENOENT;
        }
    }
    if !(1..=32000).contains(&n) {
        return -err::EINVAL;
    }
    let id = s.next;
    s.next = s.next.wrapping_add(1).max(1);
    s.sets.insert(id, SemSet { key, vals: alloc::vec![0; n as usize], pids: alloc::vec![0; n as usize], mode: flags & 0o777 });
    id as i64
}

/// One semop: all its operations at once, or none, waiting (unless
/// IPC_NOWAIT) until they can be done.
pub fn sys_semop(id: i32, sops: u64, n: u64, timeout: u64) -> i64 {
    if n == 0 || n > 500 {
        return -err::EINVAL;
    }
    let ops: alloc::vec::Vec<(usize, i16, i16)> = match super::user_bytes(sops, n * 6) {
        Ok(b) => b.chunks(6).map(|c| (u16::from_le_bytes([c[0], c[1]]) as usize, i16::from_le_bytes([c[2], c[3]]), i16::from_le_bytes([c[4], c[5]]))).collect(),
        Err(e) => return e,
    };
    // A timeout (semtimedop): its seconds and nanoseconds, in milliseconds.
    let limit = if timeout != 0 {
        match super::user_bytes(timeout, 16) {
            Ok(b) => {
                let sec = u64::from_le_bytes(b[0..8].try_into().unwrap_or([0; 8]));
                let ns = u64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8]));
                Some(crate::time::uptime_ms() + sec * 1000 + ns / 1_000_000)
            }
            Err(e) => return e,
        }
    } else {
        None
    };
    let pid = crate::task::current_pid();
    loop {
        {
            let mut s = SEMS.lock();
            let Some(set) = s.sets.get_mut(&id) else { return -err::EINVAL };
            if ops.iter().any(|o| o.0 >= set.vals.len()) {
                return -err::EFBIG;
            }
            let mut vals = set.vals.clone();
            let mut ok = true;
            for &(i, op, _) in &ops {
                let v = vals[i] as i32;
                if op == 0 {
                    if v != 0 {
                        ok = false;
                        break;
                    }
                } else if v + (op as i32) < 0 {
                    ok = false;
                    break;
                } else if v + op as i32 > 32767 {
                    return -err::ERANGE;
                } else {
                    vals[i] = (v + op as i32) as u16;
                }
            }
            if ok {
                for &(i, _, _) in &ops {
                    set.pids[i] = pid;
                }
                set.vals = vals;
                return 0;
            }
            if ops.iter().any(|o| o.2 & IPC_NOWAIT != 0) {
                return -err::EAGAIN;
            }
        }
        if limit.is_some_and(|l| crate::time::uptime_ms() >= l) {
            return -err::EAGAIN;
        }
        if let Some(e) = super::signal::interrupted() {
            return e;
        }
        crate::task::sleep_ms(2);
    }
}

pub fn sys_semctl(id: i32, num: i32, cmd: u32, arg: u64) -> i64 {
    let mut s = SEMS.lock();
    let Some(set) = s.sets.get_mut(&id) else { return -err::EINVAL };
    let n = set.vals.len();
    let one = || (num >= 0 && (num as usize) < n).then_some(num as usize);
    match cmd & 0xff {
        IPC_RMID => {
            s.sets.remove(&id);
            0
        }
        IPC_SET => 0,
        GETVAL => one().map_or(-err::EINVAL, |i| set.vals[i] as i64),
        GETPID => one().map_or(-err::EINVAL, |i| set.pids[i] as i64),
        GETNCNT | GETZCNT => 0,
        SETVAL => match one() {
            Some(i) if arg <= 32767 => {
                set.vals[i] = arg as u16;
                0
            }
            Some(_) => -err::ERANGE,
            None => -err::EINVAL,
        },
        GETALL | SETALL => {
            let get = cmd & 0xff == GETALL;
            let vals = set.vals.clone();
            drop(s);
            let b = match super::user_bytes(arg, n as u64 * 2) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if get {
                for (i, v) in vals.iter().enumerate() {
                    b[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            } else {
                let new: alloc::vec::Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                if let Some(set) = SEMS.lock().sets.get_mut(&id) {
                    set.vals = new;
                }
            }
            0
        }
        IPC_STAT => {
            // struct semid_ds: struct ipc_perm (48 bytes), the times, the
            // number of semaphores.
            let (key, mode) = (set.key, set.mode);
            drop(s);
            let b = match super::user_bytes(arg, 104) {
                Ok(b) => b,
                Err(e) => return e,
            };
            b.fill(0);
            b[0..4].copy_from_slice(&key.to_le_bytes());
            b[20..22].copy_from_slice(&(mode as u16).to_le_bytes());
            b[64..72].copy_from_slice(&(n as u64).to_le_bytes());
            0
        }
        _ => -err::EINVAL,
    }
}
