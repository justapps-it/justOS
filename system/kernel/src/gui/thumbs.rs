//! Thumbnails of pictures, for the grid of the Files app.
//!
//! They are made by a thread of their own, so that a large photo never holds
//! up the interface: the first time a thumbnail is asked for ([`get`]) it is
//! queued and the answer is [`Thumb::Waiting`]; the thread reads the file a
//! piece at a time, decodes it (QOI, PNG, JPEG, GIF) and scales it down with
//! area averaging to the box asked for, at the screen's pixels per unit.
//! Asking again later gives [`Thumb::Ready`]. Files that are too large,
//! pictures with too many pixels and anything that does not decode answer
//! [`Thumb::Never`], and are not tried again.
//!
//! A thumbnail is known by its file's path, size and modification time (a
//! file that changes gets a new one) and by its box and scale. The cache is
//! bounded in bytes and in entries; the least recently asked for go first.

use super::gfx::Canvas;
use super::qoi;
use crate::fs;
use crate::sync::IrqLock;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Files larger than this keep their type icon.
pub const MAX_FILE: u64 = 20 << 20;
/// Pictures with more pixels than this keep it too: up to a 16-megapixel
/// photo.
const MAX_PIXELS: u64 = 16 << 20;
/// Bytes a pixel may take while it is decoded: the picture itself, and the
/// coefficients a JPEG keeps for each colour.
const BYTES_PER_PIXEL: u64 = 12;
/// Memory left free after decoding, at least.
const RESERVE: u64 = 64 << 20;
/// Bytes of thumbnails kept at most.
const MAX_BYTES: usize = 8 << 20;
/// Thumbnails (and pictures found unusable) remembered at most.
const MAX_ENTRIES: usize = 1024;
/// Thumbnails waiting to be made at most: the oldest asks go first (what is
/// still in sight is asked for again).
const MAX_QUEUE: usize = 64;
/// Files are read in pieces this large.
const CHUNK: usize = 256 << 10;
/// The extensions of the pictures that get a thumbnail.
const PICTURES: [&str; 5] = ["qoi", "png", "jpg", "jpeg", "gif"];

/// Which thumbnail: the file as it is now, and how it is drawn.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub path: String,
    pub size: u64,
    pub modified: u64,
    /// The box it must fit, in units, and the pixels per unit.
    pub w: i32,
    pub h: i32,
    pub scale: i32,
}

pub enum Thumb {
    /// Queued or being made: the type icon meanwhile.
    Waiting,
    /// Too large, unreadable or not a picture after all: the type icon stays.
    Never,
    Ready(Arc<Canvas>),
}

struct Entry {
    /// None for a picture that could not be made.
    thumb: Option<Arc<Canvas>>,
    /// When it was last asked for, on [`Cache::clock`].
    used: u64,
}

struct Cache {
    done: BTreeMap<Key, Entry>,
    /// The thread takes the last one first: the latest ask.
    queue: VecDeque<Key>,
    /// Being made now.
    busy: Option<Key>,
    /// A thread is making them.
    running: bool,
    /// Bytes of pixels in `done`.
    bytes: usize,
    /// Counts the asks, to know which entry was asked for least recently.
    clock: u64,
}

static CACHE: IrqLock<Cache> = IrqLock::new(Cache { done: BTreeMap::new(), queue: VecDeque::new(), busy: None, running: false, bytes: 0, clock: 0 });

/// Can a file with this name have a thumbnail?
pub fn is_picture(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| PICTURES.iter().any(|p| ext.eq_ignore_ascii_case(p)))
}

/// The thumbnail if it is made already; nothing is queued.
pub fn cached(key: &Key) -> Option<Arc<Canvas>> {
    let mut c = CACHE.lock();
    c.clock += 1;
    let now = c.clock;
    let e = c.done.get_mut(key)?;
    e.used = now;
    e.thumb.clone()
}

/// The thumbnail of `key`, or why there is none yet. Asking for one that is
/// waiting moves it to the front of the queue: what is asked for last is
/// made first.
pub fn get(key: &Key) -> Thumb {
    if key.size > MAX_FILE {
        return Thumb::Never;
    }
    let mut c = CACHE.lock();
    c.clock += 1;
    let now = c.clock;
    if let Some(e) = c.done.get_mut(key) {
        e.used = now;
        return match &e.thumb {
            Some(t) => Thumb::Ready(t.clone()),
            None => Thumb::Never,
        };
    }
    if c.busy.as_ref() != Some(key) {
        c.queue.retain(|k| k != key);
        c.queue.push_back(key.clone());
        if c.queue.len() > MAX_QUEUE {
            c.queue.pop_front();
        }
    }
    let start = !c.running;
    c.running = true;
    drop(c);
    // The decoders keep tables on the stack: a larger one than usual.
    if start && crate::task::spawn_big("thumbs", 256 << 10, work).is_none() {
        CACHE.lock().running = false;
    }
    Thumb::Waiting
}

/// The thread: makes the queued thumbnails, latest first, and ends when
/// there are none left (the next ask starts another).
fn work() {
    loop {
        let key = {
            let mut c = CACHE.lock();
            match c.queue.pop_back() {
                Some(k) => {
                    c.busy = Some(k.clone());
                    k
                }
                None => {
                    c.running = false;
                    return;
                }
            }
        };
        let thumb = make(&key).map(Arc::new);
        let mut c = CACHE.lock();
        c.busy = None;
        c.keep(key, thumb);
    }
}

impl Cache {
    /// Remembers a thumbnail (or that there is none), then lets the least
    /// recently asked for go until the cache is within its bounds again.
    fn keep(&mut self, key: Key, thumb: Option<Arc<Canvas>>) {
        self.clock += 1;
        self.bytes += thumb.as_ref().map_or(0, |t| bytes(t));
        if let Some(old) = self.done.insert(key, Entry { thumb, used: self.clock }) {
            self.bytes -= old.thumb.as_ref().map_or(0, |t| bytes(t));
        }
        while self.done.len() > 1 && (self.bytes > MAX_BYTES || self.done.len() > MAX_ENTRIES) {
            let Some(oldest) = self.done.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            if let Some(e) = self.done.remove(&oldest) {
                self.bytes -= e.thumb.as_ref().map_or(0, |t| bytes(t));
            }
        }
    }
}

fn bytes(t: &Canvas) -> usize {
    t.pixel_width() as usize * t.pixel_height() as usize * 4
}

/// Reads, decodes and scales one picture.
fn make(key: &Key) -> Option<Canvas> {
    let path = fs::abs(&key.path).ok()?;
    let data = read(&path, key.size)?;
    let img = decode(&data)?;
    drop(data);
    Some(fit(&img, key.w, key.h, key.scale))
}

/// A picture file decoded (QOI, PNG, JPEG, GIF), if its header gives a size
/// within bounds and there is the memory for it (the Anteprima window opens
/// pictures with it too).
pub fn decode(data: &[u8]) -> Option<Canvas> {
    if !fits(data) {
        return None;
    }
    qoi::decode(data).or_else(|| justimage::decode(data))
}

/// The file, a piece at a time: the file system's lock is let go between
/// pieces, so the interface never waits long for it. Stops early when the
/// first piece already says the picture is too large.
fn read(path: &fs::Path, size: u64) -> Option<Vec<u8>> {
    let size = size.min(MAX_FILE) as usize;
    let mut data = alloc::vec![0u8; size];
    let mut at = 0;
    while at < size {
        let end = (at + CHUNK).min(size);
        let n = fs::read_at(path, at as u64, &mut data[at..end]).ok()?;
        if n == 0 {
            break;
        }
        if at == 0 && dimensions(&data[..n]).is_some() && !fits(&data[..n]) {
            return None;
        }
        at += n;
    }
    data.truncate(at);
    Some(data)
}

/// Does the picture's header give a size we can decode, with the memory
/// for it?
fn fits(data: &[u8]) -> bool {
    let Some((w, h)) = dimensions(data) else { return false };
    let pixels = w * h;
    let (free, _) = crate::mem::frame::stats();
    pixels != 0 && pixels <= MAX_PIXELS && free > pixels * BYTES_PER_PIXEL + RESERVE
}

/// The size a picture's header gives, in pixels.
fn dimensions(data: &[u8]) -> Option<(u64, u64)> {
    if data.starts_with(b"qoif") && data.len() >= 14 {
        let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as u64;
        return Some((be(4), be(8)));
    }
    justimage::size(data).map(|(w, h)| (w as u64, h as u64))
}

/// `img` scaled to fit `w` x `h` units, keeping its shape and never larger
/// than it is, at `k` pixels per unit.
fn fit(img: &Canvas, w: i32, h: i32, k: i32) -> Canvas {
    let s = (w as i64 * 1000 / img.w as i64).min(h as i64 * 1000 / img.h as i64).min(1000);
    img.scaled_at(((img.w as i64 * s / 1000) as i32).max(1), ((img.h as i64 * s / 1000) as i32).max(1), k)
}
