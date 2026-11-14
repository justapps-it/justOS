//! A census of the system calls Linux programs make, for the shell's `top`:
//! while it samples, every call is counted by thread and number, in a
//! table without locks (a call only adds one to a counter), and so are the
//! timer ticks that find the kernel serving each call (the processor time
//! it costs). Off, a call costs one load.

use crate::task::Tid;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

static ON: AtomicBool = AtomicBool::new(false);

const SLOTS: usize = 4096;
/// Each slot's thread and call (`tid << 16 | call + 1`; 0: free), and how
/// many times that thread made that call.
static KEYS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static COUNTS: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
/// Calls that found the table full.
static LOST: AtomicU32 = AtomicU32::new(0);
/// Timer ticks that found the kernel in each call (by number).
const CALLS: usize = 512;
static TICKS: [AtomicU32; CALLS] = [const { AtomicU32::new(0) }; CALLS];

/// Counts system call `n` of the running thread, while sampling.
#[inline]
pub fn count(n: u64) {
    if ON.load(Relaxed) {
        add(crate::task::current_tid(), n);
    }
}

/// A timer tick found the kernel serving call `n` (`task::set_in_call`).
pub fn kernel_tick(n: u32) {
    if ON.load(Relaxed) {
        if let Some(t) = TICKS.get(n as usize) {
            t.fetch_add(1, Relaxed);
        }
    }
}

/// The ticks the kernel spent in call `n` while counting.
pub fn ticks(n: u16) -> u64 {
    TICKS.get(n as usize).map_or(0, |t| t.load(Relaxed) as u64)
}

fn add(tid: Tid, n: u64) {
    let key = (tid as u64) << 16 | ((n & 0x7fff) + 1);
    let mut i = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 54) as usize % SLOTS;
    for _ in 0..SLOTS {
        let k = KEYS[i].load(Relaxed);
        // A free slot is taken, unless another processor just took it
        // (for the same pair, or another).
        let mine = k == key
            || (k == 0
                && match KEYS[i].compare_exchange(0, key, Relaxed, Relaxed) {
                    Ok(_) => true,
                    Err(x) => x == key,
                });
        if mine {
            COUNTS[i].fetch_add(1, Relaxed);
            return;
        }
        i = (i + 1) % SLOTS;
    }
    LOST.fetch_add(1, Relaxed);
}

/// Starts counting from zero.
pub fn start() {
    ON.store(false, Relaxed);
    for (k, c) in KEYS.iter().zip(&COUNTS) {
        k.store(0, Relaxed);
        c.store(0, Relaxed);
    }
    for t in &TICKS {
        t.store(0, Relaxed);
    }
    LOST.store(0, Relaxed);
    ON.store(true, Relaxed);
}

/// Stops counting: (thread, call, count) for each pair seen, and the calls
/// the table had no room for.
pub fn stop() -> (Vec<(Tid, u16, u32)>, u32) {
    ON.store(false, Relaxed);
    let seen = KEYS
        .iter()
        .zip(&COUNTS)
        .filter_map(|(k, c)| {
            let k = k.load(Relaxed);
            (k != 0).then(|| ((k >> 16) as Tid, ((k & 0xffff) - 1) as u16, c.load(Relaxed)))
        })
        .collect();
    (seen, LOST.load(Relaxed))
}

/// The name of a system call of x86-64 Linux (the ones programs make
/// often; "" for the others).
pub fn name(n: u16) -> &'static str {
    match n {
        0 => "read",
        1 => "write",
        2 => "open",
        3 => "close",
        4 => "stat",
        5 => "fstat",
        6 => "lstat",
        7 => "poll",
        8 => "lseek",
        9 => "mmap",
        10 => "mprotect",
        11 => "munmap",
        12 => "brk",
        13 => "rt_sigaction",
        14 => "rt_sigprocmask",
        15 => "rt_sigreturn",
        16 => "ioctl",
        17 => "pread64",
        18 => "pwrite64",
        19 => "readv",
        20 => "writev",
        21 => "access",
        22 => "pipe",
        23 => "select",
        24 => "sched_yield",
        25 => "mremap",
        28 => "madvise",
        32 => "dup",
        33 => "dup2",
        35 => "nanosleep",
        39 => "getpid",
        41 => "socket",
        42 => "connect",
        43 => "accept",
        44 => "sendto",
        45 => "recvfrom",
        46 => "sendmsg",
        47 => "recvmsg",
        49 => "bind",
        53 => "socketpair",
        54 => "setsockopt",
        55 => "getsockopt",
        56 => "clone",
        57 => "fork",
        59 => "execve",
        60 => "exit",
        61 => "wait4",
        62 => "kill",
        63 => "uname",
        72 => "fcntl",
        74 => "fsync",
        75 => "fdatasync",
        77 => "ftruncate",
        79 => "getcwd",
        82 => "rename",
        83 => "mkdir",
        84 => "rmdir",
        87 => "unlink",
        89 => "readlink",
        96 => "gettimeofday",
        102 => "getuid",
        110 => "getppid",
        128 => "rt_sigtimedwait",
        130 => "rt_sigsuspend",
        157 => "prctl",
        186 => "gettid",
        201 => "time",
        202 => "futex",
        217 => "getdents64",
        218 => "set_tid_address",
        228 => "clock_gettime",
        229 => "clock_getres",
        230 => "clock_nanosleep",
        231 => "exit_group",
        232 => "epoll_wait",
        233 => "epoll_ctl",
        234 => "tgkill",
        257 => "openat",
        262 => "newfstatat",
        263 => "unlinkat",
        269 => "faccessat",
        270 => "pselect6",
        271 => "ppoll",
        273 => "set_robust_list",
        281 => "epoll_pwait",
        284 | 290 => "eventfd",
        285 => "fallocate",
        286 => "timerfd_settime",
        287 => "timerfd_gettime",
        288 => "accept4",
        291 => "epoll_create1",
        293 => "pipe2",
        302 => "prlimit64",
        316 => "renameat2",
        318 => "getrandom",
        319 => "memfd_create",
        332 => "statx",
        334 => "rseq",
        441 => "epoll_pwait2",
        _ => "",
    }
}
