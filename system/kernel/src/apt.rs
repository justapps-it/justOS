//! Debian's archive: what a `.deb` needs and is not in the system comes
//! from there, as with `apt install ./file.deb` (the window of
//! gui/apps/debinst.rs, `deb install` and `deb get` in the shell).
//!
//! The archives are those of `/etc/apt/sources.list` (and
//! `sources.list.d/*.list`, `*.sources`), Debian 13 "trixie" as the image
//! ships it. For each one justOS fetches `dists/<suite>/InRelease` and
//! checks its OpenPGP signature with the keys of debian-archive-keyring
//! (in the image, `/usr/share/keyrings/debian-archive-keyring.pgp`, and
//! those added to `/etc/apt/trusted.gpg.d`): an `InRelease` that no
//! trusted key signed, or whose `Valid-Until` has passed, is refused. The
//! index of packages (`main/binary-amd64/Packages.gz`, or the plain one
//! where there is no gzip one) must then have the size and SHA-256 that
//! the signed `InRelease` gives, and each downloaded `.deb` those of the
//! index: the chain apt follows, from the keys to every file.
//!
//! The index is kept on disk for a day (`/var/cache/justos-apt`), with only
//! the fields the resolver reads (justdeb::index::compact): 34 of the 56 MB
//! of the main archive's.

use crate::fs;
use crate::net::http;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use justdeb::index::{self, Index, Pkg};
use justdeb::release::{self, Release, Source};
use justdeb::resolve::{self, Plan, Problem};
use justdeb::{hex, pgp};

/// What `sources.list` would say if there is none.
const DEFAULT_SOURCES: &str = "deb https://deb.debian.org/debian trixie main
deb https://deb.debian.org/debian trixie-updates main
deb https://deb.debian.org/debian-security trixie-security main
";
const SOURCES: &str = "sys:/etc/apt/sources.list";
const SOURCES_D: &str = "sys:/etc/apt/sources.list.d";
const KEYRING: &str = "sys:/usr/share/keyrings/debian-archive-keyring.pgp";
const TRUSTED_D: &str = "sys:/etc/apt/trusted.gpg.d";
const CACHE: &str = "sys:/var/cache/justos-apt";
const STATUS: &str = "sys:/var/lib/dpkg/status";
/// How long a fetched index is good for.
const DAY: u64 = 24 * 3600;
/// The first line of a cached index: format, when it was fetched.
const CACHE_MAGIC: &str = "justos-apt-index 1";

/// One index at a time: two windows asking at once share the download.
static LOADING: Mutex<()> = Mutex::new(());

/// What is being done, for a window's bar or the shell's lines.
pub enum Step<'a> {
    /// Fetching the index of an archive: its name, bytes so far and total.
    Index(&'a str, u64, u64),
    /// Reading the indexes.
    Reading,
}

/// The archives of `sources.list` and the files beside it, or Debian's own
/// when there are none. One per component: each has its own index.
pub fn sources() -> Vec<Source> {
    let mut text = read_text(SOURCES).unwrap_or_default();
    let mut deb822 = String::new();
    if let Ok(dir) = fs::abs(SOURCES_D) {
        let mut list = fs::list_dir(&dir).unwrap_or_default();
        list.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        for (e, _) in list {
            let t = read_text(&format!("{SOURCES_D}/{}", e.name)).unwrap_or_default();
            if e.name.ends_with(".list") {
                text.push('\n');
                text.push_str(&t);
            } else if e.name.ends_with(".sources") {
                deb822.push_str(&t);
                deb822.push_str("\n\n");
            }
        }
    }
    let mut all = release::sources_list(&text);
    all.extend(release::sources_deb822(&deb822));
    if all.is_empty() {
        all = release::sources_list(DEFAULT_SOURCES);
    }
    all.into_iter().flat_map(|s| s.components.iter().map(|c| Source { uri: s.uri.clone(), suite: s.suite.clone(), components: alloc::vec![c.clone()] }).collect::<Vec<_>>()).collect()
}

fn read_text(path: &str) -> Option<String> {
    let p = fs::abs(path).ok()?;
    fs::read_file(&p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The keys the archives' signatures are checked with.
pub fn keys() -> Vec<pgp::Key> {
    let mut files = alloc::vec![String::from(KEYRING)];
    if let Ok(dir) = fs::abs(TRUSTED_D) {
        for (e, _) in fs::list_dir(&dir).unwrap_or_default() {
            if [".pgp", ".gpg", ".asc"].iter().any(|x| e.name.ends_with(x)) {
                files.push(format!("{TRUSTED_D}/{}", e.name));
            }
        }
    }
    let mut keys: Vec<pgp::Key> = Vec::new();
    for f in files {
        let Some(data) = fs::abs(&f).ok().and_then(|p| fs::read_file(&p).ok()) else { continue };
        for k in pgp::keyring(&data) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys
}

/// A short name for an archive: "deb.debian.org trixie-updates".
pub fn label(s: &Source) -> String {
    let host = s.uri.split("://").nth(1).unwrap_or(&s.uri).split('/').next().unwrap_or("");
    format!("{host} {}", s.suite)
}

fn fetch(url: &str, progress: http::Progress<'_>) -> Result<Vec<u8>, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{url}: solo indirizzi http e https"));
    }
    let r = http::fetch(&http::Request::get(url), progress)?;
    if !r.ok() {
        return Err(format!("{url}: {}", r.status));
    }
    Ok(r.body)
}

/// The signed `InRelease` of an archive, checked.
fn release_of(s: &Source, keys: &[pgp::Key]) -> Result<Release, String> {
    let data = fetch(&format!("{}/InRelease", s.dists()), &mut |_, _| true)?;
    let text = String::from_utf8_lossy(&data);
    let body = pgp::verify_cleartext(&text, keys).map_err(|e| match e {
        pgp::Error::NoGoodSignature => format!("l'indice di {} non ha una firma valida di Debian: non lo uso", label(s)),
        _ => format!("l'indice di {} non è firmato: non lo uso", label(s)),
    })?;
    let r = Release::parse(&body);
    // A signed index that has expired could be an old one passed off as
    // new (with fixes missing). A clock that is clearly not set (before
    // the index was even made) does not count.
    let now = crate::time::unix_now();
    if r.valid_until != 0 && now > r.valid_until && now > r.date {
        return Err(format!("l'indice di {} è scaduto (o l'orologio è avanti)", label(s)));
    }
    Ok(r)
}

/// The packages of an archive's component, fetched and checked against
/// its `InRelease`: compact text.
fn packages_of(s: &Source, r: &Release, step: &mut dyn FnMut(Step<'_>)) -> Result<String, String> {
    let comp = &s.components[0];
    let Some(listed) = r.packages_index(comp) else {
        // Not every archive has every component (or a plain/gzip index).
        crate::println!("apt: {} has no {comp} index justOS can read", label(s));
        return Ok(String::new());
    };
    // By hash first (it cannot change while it downloads), then by name:
    // Debian's mirrors keep no by-hash copy of the plain `Packages` of
    // trixie-updates. The hash check below covers both.
    let plain = format!("{}/{}", s.dists(), listed.path);
    let urls: Vec<String> = match (r.by_hash, listed.path.rsplit_once('/')) {
        (true, Some((dir, _))) => alloc::vec![format!("{}/{dir}/by-hash/SHA256/{}", s.dists(), listed.sha256), plain],
        _ => alloc::vec![plain],
    };
    let name = label(s);
    let total = listed.size;
    let mut got = Err(String::new());
    for url in &urls {
        got = fetch(url, &mut |done, _| {
            step(Step::Index(&name, done, total));
            true
        });
        if got.is_ok() {
            break;
        }
    }
    let data = match got {
        // Listed but not there: trixie-updates and trixie-security list
        // their plain `Packages` (for its hash) and serve only the .xz,
        // which justOS cannot open. As an archive without the component:
        // nothing from it, and the empty index is cached for the day, so
        // it is not asked again at every installation.
        Err(e) if e.ends_with(": 404") => {
            crate::println!("apt: {name} serves its {comp} index only in a form justOS cannot read (.xz); going on without it");
            return Ok(String::new());
        }
        r => r?,
    };
    if data.len() as u64 != listed.size || hex(&justtls::sha2::Sha256::digest(&data)) != listed.sha256 {
        return Err(format!("l'indice dei pacchetti di {name} non corrisponde a quello firmato"));
    }
    step(Step::Reading);
    let data = if listed.path.ends_with(".gz") { justzip::gunzip(&data).ok_or_else(|| format!("l'indice di {name} è danneggiato"))? } else { data };
    Ok(index::compact(&String::from_utf8_lossy(&data)))
}

/// Where an archive's compact index is cached.
fn cache_file(s: &Source) -> String {
    let id: String = format!("{}_{}_{}", s.uri.split("://").nth(1).unwrap_or(&s.uri), s.suite, s.components[0])
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("{CACHE}/{id}")
}

/// A cached index and when it was fetched.
fn cached(s: &Source) -> Option<(u64, String)> {
    let text = read_text(&cache_file(s))?;
    let (head, body) = text.split_once('\n')?;
    let when = head.strip_prefix(CACHE_MAGIC)?.trim().parse().ok()?;
    Some((when, String::from(body)))
}

fn save(s: &Source, text: &str) {
    crate::store::make_dirs(CACHE);
    let mut data = format!("{CACHE_MAGIC} {}\n", crate::time::unix_now()).into_bytes();
    data.extend_from_slice(text.as_bytes());
    if let Ok(p) = fs::abs(&cache_file(s)) {
        if fs::write_file(&p, &data).is_err() {
            crate::println!("apt: cannot cache the index of {}", label(s));
        }
    }
}

/// The packages of all archives: from the cache when it is less than a
/// day old (or when the network is down), else fetched. `fresh` fetches
/// all again (`deb update`).
pub fn load(fresh: bool, step: &mut dyn FnMut(Step<'_>)) -> Result<(Index, Vec<Source>), String> {
    let _one = LOADING.lock();
    let sources = sources();
    let mut trusted: Option<Vec<pgp::Key>> = None;
    let mut idx = Index::new();
    let (mut read, mut failed) = (0, None);
    for (n, s) in sources.iter().enumerate() {
        let old = cached(s);
        let now = crate::time::unix_now();
        let text = match old {
            Some((when, text)) if !fresh && now >= when && now - when < DAY => text,
            old => {
                let keys = trusted.get_or_insert_with(keys);
                if keys.is_empty() {
                    return Err(String::from("mancano le chiavi dell'archivio di Debian (debian-archive-keyring): non posso controllare cosa scarico"));
                }
                match release_of(s, keys).and_then(|r| packages_of(s, &r, step)) {
                    Ok(text) => {
                        save(s, &text);
                        crate::println!("apt: fetched the index of {} ({} KiB)", label(s), text.len() / 1024);
                        text
                    }
                    // Offline: yesterday's index is better than none (the
                    // downloads are still checked against it).
                    Err(e) => match old {
                        Some((_, text)) => {
                            crate::println!("apt: {e}; using the cached index");
                            text
                        }
                        // One archive less is not the end: trixie-updates
                        // and trixie-security serve their index only as
                        // .xz, which justOS cannot open, and then main
                        // alone still has almost everything. Only when no
                        // archive at all can be read is it an error.
                        None => {
                            crate::println!("apt: {e}; going on without {}", label(s));
                            failed = Some(e);
                            continue;
                        }
                    },
                }
            }
        };
        step(Step::Reading);
        idx.add(text, n);
        read += 1;
    }
    match failed {
        Some(e) if read == 0 => Err(e),
        _ => Ok((idx, sources)),
    }
}

/// The packages dpkg has installed.
pub fn installed() -> Vec<Pkg> {
    read_text(STATUS).map(|t| index::installed(&t)).unwrap_or_default()
}

/// A resolver's problem in the owner's words.
pub fn explain(p: &Problem, installed: &[Pkg]) -> String {
    match p {
        Problem::Missing { what, needed_by } => {
            format!("«{needed_by}» ha bisogno di «{what}», che non c'è né nel sistema né nell'archivio di Debian.")
        }
        Problem::Conflict { new, other, breaks } => {
            let there = if installed.iter().any(|q| q.name == *other) { ", che è installato" } else { "" };
            let how = if *breaks { "romperebbe" } else { "non va d'accordo con" };
            format!("«{new}» {how} «{other}»{there}: non tolgo pacchetti per fargli posto.")
        }
        Problem::Upgrade { upgraded, installed, what } => {
            format!("Servirebbe una versione più nuova di «{upgraded}», ma «{installed}» ha bisogno di quella che c'è ({what}).")
        }
        Problem::Arch { name, arch } => format!("«{name}» è per un altro tipo di processore ({arch})."),
    }
}

/// The plan to install `wanted`: first with what the system has; only
/// when something is missing, with the archives (fetched if need be).
/// A package that is itself from an archive (`deb get`) always needs
/// them, to be downloaded.
pub fn plan(wanted: Vec<Pkg>, step: &mut dyn FnMut(Step<'_>)) -> Result<(Plan, Vec<Source>), String> {
    let installed = installed();
    if wanted.iter().all(|p| p.source == resolve::LOCAL) {
        match resolve::plan(&installed, &Index::new(), wanted.clone()) {
            Ok(p) => return Ok((p, Vec::new())),
            Err(Problem::Missing { .. }) => {}
            Err(e) => return Err(explain(&e, &installed)),
        }
    }
    let (idx, sources) = load(false, step)?;
    plan_in(wanted, &idx, sources)
}

/// The plan to install `wanted` with an index already loaded.
pub fn plan_in(wanted: Vec<Pkg>, idx: &Index, sources: Vec<Source>) -> Result<(Plan, Vec<Source>), String> {
    let installed = installed();
    resolve::plan(&installed, idx, wanted).map(|p| (p, sources)).map_err(|e| explain(&e, &installed))
}

/// The archive's package for a name (`deb get <name>`).
pub fn find(idx: &Index, name: &str) -> Result<Pkg, String> {
    resolve::by_name(idx, name).map_err(|_| {
        if idx.knows(name) {
            format!("«{name}» è un pacchetto virtuale: lo danno più pacchetti, scegline uno")
        } else {
            format!("«{name}» non c'è nell'archivio di Debian")
        }
    })
}

/// The file name a downloaded package gets (apt's: `:` of an epoch as
/// `%3a`).
pub fn file_name(p: &Pkg) -> String {
    format!("{}_{}_{}.deb", p.name, p.version.replace(':', "%3a"), p.arch)
}

/// Downloads a package of a plan into `dir`, checked against the index.
pub fn download(p: &Pkg, sources: &[Source], dir: &str, progress: http::Progress<'_>) -> Result<String, String> {
    let s = sources.get(p.source).ok_or("archivio sconosciuto")?;
    if p.sha256.len() != 64 || p.filename.is_empty() {
        return Err(format!("l'indice non dà la somma SHA-256 di «{}»: non lo scarico", p.name));
    }
    let data = fetch(&s.pool(&p.filename), progress)?;
    if data.len() as u64 != p.size || hex(&justtls::sha2::Sha256::digest(&data)) != p.sha256 {
        return Err(format!("«{}» scaricato non corrisponde all'indice firmato", p.name));
    }
    let file = format!("{dir}/{}", file_name(p));
    let path = fs::abs(&file).map_err(|e| e.to_string())?;
    fs::write_file(&path, &data).map_err(|_| String::from("non riesco a salvare i pacchetti scaricati (spazio?)"))?;
    Ok(file)
}
