//! The justOS shell: the system console's command interpreter.

use crate::dev::input::{self, Key};
use crate::fs::{self, Path};
use crate::util::Size;
use crate::{dev, driver, mem, task, time};
use crate::{print, println};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const ACCENT: &str = "\x1b[95m";
const DIM: &str = "\x1b[90m";
const OK: &str = "\x1b[92m";
const WARN: &str = "\x1b[93m";
const ERR: &str = "\x1b[91m";
const BOLD: &str = "\x1b[97m";
const RESET: &str = "\x1b[0m";

pub fn banner(boot: &crate::boot::BootInfo) {
    if let Some((w, _)) = crate::console::pixel_size() {
        // A thin accent bar across the top of the screen.
        crate::console::fill_rect_px(0, 0, w, 3, 0xC9A7FF);
    }
    println!();
    println!("  {BOLD}justOS{ACCENT}█{RESET} {DIM}{}{RESET}", crate::VERSION);
    let mut brand = [0u8; 48];
    let cpu = crate::arch::cpu::brand(&mut brand);
    let screen = match boot.fb_kind {
        crate::boot::FB_RGB => alloc::format!("{}x{} framebuffer", boot.fb_width, boot.fb_height),
        crate::boot::FB_TEXT => String::from("text screen"),
        _ => String::from("no screen"),
    };
    println!("  {DIM}{cpu} · {screen} · loader: {}{RESET}", if boot.loader == crate::boot::LOADER_UEFI { "UEFI" } else { "BIOS" });
    println!();
}

struct Shell {
    cwd: Path,
    history: Vec<String>,
}

const COMMANDS: &[(&str, &str, &str)] = &[
    ("help", "[comando]", "elenca i comandi, o ne spiega uno"),
    ("ls", "[percorso]", "elenca i file di una cartella (anche: dir)"),
    ("cd", "<percorso>", "cambia la cartella corrente"),
    ("cat", "<file>", "mostra un file di testo (anche: type)"),
    ("hex", "<file>", "mostra i byte di un file in esadecimale"),
    ("write", "<file> <testo>", "riscrive un file con un testo"),
    ("append", "<file> <testo>", "aggiunge una riga di testo a un file"),
    ("mkdir", "<percorso>", "crea una cartella (anche quelle intermedie)"),
    ("rm", "[-r] <percorso>", "elimina un file o una cartella (anche: del)"),
    ("mv", "<da> <a>", "rinomina o sposta dentro lo stesso volume"),
    ("cp", "<da> <a>", "copia un file"),
    ("vol", "", "elenca i volumi montati"),
    ("disks", "", "elenca dischi e partizioni"),
    ("format", "<disco> <etichetta>", "crea un volume JustFS su un disco"),
    ("check", "<volume>", "cerca errori in un volume"),
    ("unmount", "<volume>", "smonta un volume (prima di togliere il disco)"),
    ("dev", "[id | scan]", "elenca i dispositivi e i loro driver"),
    ("drv", "[list|info|add|remove|reload|inbox]", "gestisce i driver"),
    ("kbd", "[us|it]", "mostra o cambia il layout della tastiera"),
    ("mem", "", "quanta memoria è in uso"),
    ("ps", "", "programmi e thread in esecuzione"),
    ("kill", "<pid>", "termina un programma"),
    ("cpus", "", "cosa esegue ogni processore, e gli svuotamenti della TLB causati dai programmi"),
    ("locks", "[reset]", "dove il kernel ha aspettato i lock, e dove ha tenuto più a lungo i suoi mutex"),
    ("where", "<tid>|busy [campioni]", "dove si trova un thread di un programma Linux (libreria+offset), a campione; busy: i quattro più occupati"),
    ("top", "[secondi]", "in pochi secondi (2): quanto è rimasto inattivo ogni processore, i thread più occupati, le chiamate di sistema dei programmi Linux"),
    ("ports", "", "le porte di messaggi con un nome, e di chi sono"),
    ("perms", "[allow|deny|ask <app> <permesso>]", "cosa possono usare le app fuori dalla loro sandbox"),
    ("updates", "[check|install <app>|all|auto on|off]", "gli aggiornamenti delle app installate"),
    ("sysupdate", "[check|install [file]|rollback|cancel]", "gli aggiornamenti di justOS: i due slot di sistema"),
    ("install-system", "[list | <disco> [--yes-erase <codice>]]", "installa justOS su un disco di questo computer, cancellandolo"),
    ("dnd", "test", "prova a spostare e copiare file come quando li trascini"),
    ("run", "<programma> [argomenti]", "avvia un programma (basta anche: <programma>)"),
    ("irq", "", "le linee di interrupt e gli interrupt a messaggio"),
    ("net", "", "interfacce di rete e indirizzi"),
    ("ping", "<host> [numero]", "manda richieste ICMP echo"),
    ("host", "<nome>", "cerca l'indirizzo di un nome di host"),
    ("fetch", "<url> [file]", "scarica dal web (http e https)"),
    ("sha256", "<file>", "la somma SHA-256 di un file"),
    ("sysinfo", "", "informazioni su questo computer"),
    ("acpi", "[tree [percorso] | eval <percorso> [n...]]", "batteria, temperature e firmware (ACPI)"),
    ("date", "", "data e ora"),
    ("uptime", "", "da quanto tempo il computer è acceso"),
    ("log", "[righe]", "il giornale di sistema"),
    ("verbose", "[on|off]", "mostra sullo schermo i messaggi di debug"),
    ("strace", "[on|off] [failed] [n,n…] [t<tid>]", "scrive nel giornale le chiamate di sistema dei programmi Linux: solo quelle fallite, solo alcune, solo quelle di un thread"),
    ("export", "[NOME=valore | NOME=]", "l'ambiente dei programmi Linux avviati da adesso in poi"),
    ("echo", "<testo>", "scrive un testo"),
    ("notify", "<titolo> <testo> [icona]", "mostra una notifica sulla scrivania"),
    ("clear", "", "pulisce lo schermo (anche: cls)"),
    ("restart", "", "riavvia il computer (anche: reboot)"),
    ("shutdown", "", "spegne il computer (anche: off)"),
    ("suspend", "[idle | ram] [secondi] | check", "sospende con la memoria accesa (anche: sleep): in RAM (S3) o a corrente accesa (s2idle); si risveglia con un tasto, con il pulsante o dopo i secondi dati"),
    ("install", "[nome]", "installa un programma dal Negozio (anche: pkg); un .deb lo passa a dpkg"),
    ("deb", "[list | install <file.deb> | remove <nome>]", "i pacchetti Debian installati con dpkg (il doppio clic su un .deb fa lo stesso, con una finestra)"),
    ("uninstall", "<nome>", "rimuove un programma installato dal Negozio"),
    ("play", "<file.wav> | stop", "riproduce un file audio"),
    ("tone", "[hz] [ms]", "suona una nota (senza valori: 440 Hz per 500 ms)"),
    ("volume", "[0-100]", "mostra o cambia il volume"),
    ("hostname", "[nome | check]", "il nome del computer: lo mostra, lo cambia o controlla il pacchetto DHCP che lo manda"),
    ("setup", "[skip]", "la configurazione del primo avvio: a che punto è, o la segna come fatta senza chiedere (per i test automatici)"),
    ("display", "[LxA | scale auto|1|2]", "elenca le risoluzioni dello schermo, passa a una di loro, o cambia la dimensione dell'interfaccia"),
    ("fps", "[auto|60|120|144|240]", "quanti fotogrammi l'interfaccia ha disegnato nell'ultimo secondo; o ne imposta il massimo al secondo"),
];

/// The BusyBox applets justOS exposes as commands once `busybox` is installed.
/// Running one loads `sys:/apps/busybox` with that name as `argv[0]`.
const BUSYBOX_APPLETS: &[&str] = &[
    "ash",
    "cat",
    "cp",
    "cut",
    "date",
    "dirname",
    "basename",
    "du",
    "env",
    "expr",
    "factor",
    "find",
    "grep",
    "head",
    "hexdump",
    "md5sum",
    "mkdir",
    "mv",
    "od",
    "printf",
    "pwd",
    "rev",
    "rm",
    "rmdir",
    "seq",
    "sha256sum",
    "sleep",
    "sort",
    "tail",
    "tee",
    "test",
    "touch",
    "tr",
    "true",
    "false",
    "uname",
    "uniq",
    "wc",
    "which",
    "whoami",
    "xargs",
    "yes",
];

/// The other names of the shell's commands (`dir` for `ls`) and those `help`
/// does not list: with `COMMANDS`, every name the shell takes itself.
const ALIASES: &[&str] = &[
    "?", "pwd", "dir", "type", "md", "del", "ren", "move", "copy", "volumes", "eject", "devices", "drivers",
    "keyboard", "tasks", "journal", "cls", "reboot", "off", "poweroff", "sleep", "pkg", "kpanic",
];

/// Whether typing `name` alone runs something else before a Linux program
/// of that name in Debian's PATH: a command of the shell, a program in
/// `sys:/apps` (justOS's own `hello`) or a BusyBox applet.
pub fn shadowed(name: &str) -> bool {
    let root = fs::abs("sys:/").ok();
    COMMANDS.iter().any(|c| c.0 == name)
        || ALIASES.contains(&name)
        || root.is_some_and(|r| crate::user::find_program(name, &r).is_some())
        || busybox_applet(name).is_some()
}

/// If `name` is a BusyBox applet and `busybox` is installed, the path to it.
fn busybox_applet(name: &str) -> Option<Path> {
    if !BUSYBOX_APPLETS.contains(&name) {
        return None;
    }
    let p = fs::abs("sys:/apps/busybox").ok()?;
    fs::exists(&p).then_some(p)
}

pub fn run() -> ! {
    let mut sh = Shell { cwd: Path::root(if fs::volume("sys").is_some() { "sys" } else { "ram" }), history: Vec::new() };
    println!("Type {BOLD}help{RESET} for the list of commands.\n");
    if let Ok(path) = fs::abs("sys:/system/startup") {
        if let Ok(script) = fs::read_file(&path) {
            for line in String::from_utf8_lossy(&script).lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    sh.execute(line);
                }
            }
        }
    }
    loop {
        let line = sh.read_line();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if sh.history.last().map(|s| s.as_str()) != Some(line) {
            sh.history.push(line.to_string());
            if sh.history.len() > 100 {
                sh.history.remove(0);
            }
        }
        sh.execute(line);
    }
}

/// Splits a command line into words; "double quotes" group words.
fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(core::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn fmt_time(secs: u64) -> String {
    let t = time::from_epoch(secs);
    alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.min)
}

impl Shell {
    fn prompt(&self) -> String {
        alloc::format!("{ACCENT}{}{RESET} {DIM}›{RESET} ", self.cwd)
    }

    fn redraw(&self, line: &[char], cursor: usize) {
        let text: String = line.iter().collect();
        print!("\r{}{}\x1b[K", self.prompt(), text);
        let back = line.len() - cursor;
        if back > 0 {
            print!("\x1b[{back}D");
        }
    }

    fn read_line(&mut self) -> String {
        let mut line: Vec<char> = Vec::new();
        let mut cursor = 0;
        let mut hist = self.history.len();
        print!("{}", self.prompt());
        crate::console::set_cursor_visible(true);
        loop {
            let ev = input::read_key();
            match ev.key {
                Key::Enter => break,
                Key::Char('c') if ev.ctrl => {
                    println!("^C");
                    line.clear();
                    cursor = 0;
                    print!("{}", self.prompt());
                    continue;
                }
                Key::Char('l') if ev.ctrl => {
                    crate::console::clear();
                    self.redraw(&line, cursor);
                    continue;
                }
                Key::Char('a') if ev.ctrl => cursor = 0,
                Key::Char('e') if ev.ctrl => cursor = line.len(),
                Key::Char('u') if ev.ctrl => {
                    line.drain(..cursor);
                    cursor = 0;
                }
                Key::Char(c) if !ev.ctrl => {
                    line.insert(cursor, c);
                    cursor += 1;
                    if cursor == line.len() {
                        print!("{c}");
                        continue;
                    }
                }
                Key::Backspace => {
                    if cursor == 0 {
                        continue;
                    }
                    cursor -= 1;
                    line.remove(cursor);
                }
                Key::Delete => {
                    if cursor < line.len() {
                        line.remove(cursor);
                    }
                }
                Key::Left => cursor = cursor.saturating_sub(1),
                Key::Right => cursor = (cursor + 1).min(line.len()),
                Key::Home => cursor = 0,
                Key::End => cursor = line.len(),
                Key::Up => {
                    if hist > 0 {
                        hist -= 1;
                        line = self.history[hist].chars().collect();
                        cursor = line.len();
                    }
                }
                Key::Down => {
                    if hist < self.history.len() {
                        hist += 1;
                        line = self.history.get(hist).map(|s| s.chars().collect()).unwrap_or_default();
                        cursor = line.len();
                    }
                }
                Key::Tab => {
                    if let Some(new) = self.complete(&line[..cursor]) {
                        let rest: Vec<char> = line[cursor..].to_vec();
                        line = new;
                        cursor = line.len();
                        line.extend(rest);
                    }
                }
                _ => continue,
            }
            self.redraw(&line, cursor);
        }
        crate::console::set_cursor_visible(false);
        println!();
        line.into_iter().collect()
    }

    /// Completes the last word: a command name, or a path.
    fn complete(&self, before: &[char]) -> Option<Vec<char>> {
        let text: String = before.iter().collect();
        let start = text.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let word = &text[start..];
        let mut options: Vec<String> = Vec::new();
        if start == 0 {
            options = COMMANDS.iter().map(|c| c.0.to_string()).filter(|c| c.starts_with(word)).collect();
        } else {
            let (dir_text, prefix) = match word.rfind('/') {
                Some(i) => (&word[..=i], &word[i + 1..]),
                None if word.ends_with(':') => (word, ""),
                None => ("", word),
            };
            let dir = if dir_text.is_empty() { Ok(self.cwd.clone()) } else { Path::parse(dir_text, &self.cwd) };
            if let Ok(dir) = dir {
                if let Ok(list) = fs::list_dir(&dir) {
                    for (e, st) in list {
                        if e.name.to_lowercase().starts_with(&prefix.to_lowercase()) {
                            let slash = if st.kind == justfs::Kind::Dir { "/" } else { "" };
                            options.push(alloc::format!("{dir_text}{}{slash}", e.name));
                        }
                    }
                }
            }
            if !word.contains(':') && !word.contains('/') {
                for v in fs::volumes() {
                    let name = alloc::format!("{}:/", v.name);
                    if name.starts_with(word) {
                        options.push(name);
                    }
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        // Longest common prefix of the options.
        let mut common: String = options[0].clone();
        for o in &options[1..] {
            let n = common.chars().zip(o.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            common = common.chars().take(n).collect();
        }
        if options.len() > 1 && common.len() <= word.len() {
            println!();
            for o in &options {
                print!("{o}  ");
            }
            println!();
            return Some(text.chars().collect());
        }
        let mut out: String = text[..start].to_string();
        out.push_str(&common);
        if options.len() == 1 && start == 0 {
            out.push(' ');
        }
        Some(out.chars().collect())
    }

    fn path(&self, text: &str) -> Option<Path> {
        match Path::parse(text, &self.cwd) {
            Ok(p) => Some(p),
            Err(e) => {
                println!("{ERR}{text}: {e}{RESET}");
                None
            }
        }
    }

    fn execute(&mut self, line: &str) {
        // `program &`: the program starts and the shell does not wait for it.
        if let Some(job) = line.trim_end().strip_suffix('&') {
            self.start_background(job);
            return;
        }
        let args = split(line);
        let Some(cmd) = args.first() else { return };
        let cmd = cmd.to_ascii_lowercase();
        let rest: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
        let need = |n: usize, usage: &str| -> bool {
            if rest.len() < n {
                println!("usage: {cmd} {usage}");
                false
            } else {
                true
            }
        };
        match cmd.as_str() {
            "help" | "?" => self.help(rest.first().copied()),
            "ls" | "dir" => self.ls(rest.first().copied().unwrap_or(".")),
            "cd" => {
                let target = rest.first().copied().unwrap_or("/");
                if let Some(p) = self.path(target) {
                    if fs::is_dir(&p) {
                        // Keep the names as stored on disk.
                        self.cwd = p;
                    } else {
                        println!("{ERR}{target}: not a folder{RESET}");
                    }
                }
            }
            "pwd" => println!("{}", self.cwd),
            "cat" | "type" => {
                if need(1, "<file>") {
                    for f in &rest {
                        if let Some(p) = self.path(f) {
                            match fs::read_file(&p) {
                                Ok(data) => {
                                    let text = String::from_utf8_lossy(&data);
                                    print!("{text}");
                                    if !text.ends_with('\n') && !text.is_empty() {
                                        println!();
                                    }
                                }
                                Err(e) => println!("{ERR}{p}: {e}{RESET}"),
                            }
                        }
                    }
                }
            }
            "hex" => {
                if need(1, "<file>") {
                    if let Some(p) = self.path(rest[0]) {
                        match fs::read_file(&p) {
                            Ok(data) => hexdump(&data[..data.len().min(4096)]),
                            Err(e) => println!("{ERR}{p}: {e}{RESET}"),
                        }
                    }
                }
            }
            "write" | "append" => {
                if need(1, "<file> <text>") {
                    if let Some(p) = self.path(rest[0]) {
                        let mut text = rest[1..].join(" ");
                        text.push('\n');
                        let r = if cmd == "write" { fs::write_file(&p, text.as_bytes()) } else { fs::append_file(&p, text.as_bytes()) };
                        if let Err(e) = r {
                            println!("{ERR}{p}: {e}{RESET}");
                        }
                    }
                }
            }
            "mkdir" | "md" => {
                if need(1, "<path>") {
                    for f in &rest {
                        if let Some(p) = self.path(f) {
                            if let Err(e) = fs::make_dir(&p) {
                                println!("{ERR}{p}: {e}{RESET}");
                            }
                        }
                    }
                }
            }
            "rm" | "del" => {
                let recursive = rest.contains(&"-r");
                let targets: Vec<&&str> = rest.iter().filter(|a| **a != "-r").collect();
                if targets.is_empty() {
                    println!("usage: {cmd} [-r] <path>");
                }
                for f in targets {
                    if let Some(p) = self.path(f) {
                        let r = if recursive { fs::remove_all(&p) } else { fs::remove(&p) };
                        if let Err(e) = r {
                            println!("{ERR}{p}: {e}{RESET}");
                        }
                    }
                }
            }
            "mv" | "ren" | "move" => {
                if need(2, "<from> <to>") {
                    if let (Some(a), Some(mut b)) = (self.path(rest[0]), self.path(rest[1])) {
                        if fs::is_dir(&b) {
                            b = b.join(a.name());
                        }
                        let r = if a.volume == b.volume { fs::rename(&a, &b) } else { fs::copy_all(&a, &b).and_then(|_| fs::remove_all(&a)) };
                        if let Err(e) = r {
                            println!("{ERR}{e}{RESET}");
                        }
                    }
                }
            }
            "cp" | "copy" => {
                if need(2, "<from> <to>") {
                    if let (Some(a), Some(mut b)) = (self.path(rest[0]), self.path(rest[1])) {
                        if fs::is_dir(&b) {
                            b = b.join(a.name());
                        }
                        if b.within(&a) {
                            println!("{ERR}cannot copy a folder into itself{RESET}");
                        } else if let Err(e) = fs::copy_all(&a, &b) {
                            println!("{ERR}{e}{RESET}");
                        }
                    }
                }
            }
            "vol" | "volumes" => self.volumes(),
            "disks" => self.disks(),
            "format" => {
                if need(2, "<disk> <label>") {
                    match dev::block::find(rest[0]) {
                        Some(d) => match fs::format(&d, rest[1]) {
                            Ok(name) => println!("{OK}{} formatted, mounted as {name}:{RESET}", d.name),
                            Err(e) => println!("{ERR}{}: {e}{RESET}", d.name),
                        },
                        None => println!("{ERR}no disk named {}{RESET}", rest[0]),
                    }
                }
            }
            "check" => {
                let v = rest.first().copied().unwrap_or(&self.cwd.volume).trim_end_matches(':').to_string();
                match fs::check(&v) {
                    Ok(0) => println!("{OK}{v}: no problems found{RESET}"),
                    Ok(n) => println!("{WARN}{v}: {n} problems found{RESET}"),
                    Err(e) => println!("{ERR}{v}: {e}{RESET}"),
                }
            }
            "unmount" | "eject" => {
                if need(1, "<volume>") {
                    let v = rest[0].trim_end_matches(':');
                    if v.eq_ignore_ascii_case("sys") || v.eq_ignore_ascii_case("boot") {
                        println!("{ERR}{v}: the system needs this volume{RESET}");
                    } else {
                        match fs::unmount(v) {
                            Ok(()) => {
                                if self.cwd.volume.eq_ignore_ascii_case(v) {
                                    self.cwd = fs::Path::root("sys");
                                }
                                println!("{OK}{v}: unmounted{RESET}");
                            }
                            Err(e) => println!("{ERR}{v}: {e}{RESET}"),
                        }
                    }
                }
            }
            "dev" | "devices" => self.devices(rest.first().copied()),
            "drv" | "drivers" => self.drivers(&rest),
            "kbd" | "keyboard" => match rest.first() {
                None => {
                    let names: Vec<&str> = dev::keymap::Layout::ALL.iter().map(|l| l.name()).collect();
                    println!("layout: {} (available: {})", input::layout().name(), names.join(", "));
                    if let Some(p) = input::pointer() {
                        println!("mouse:  moved {}, {}  wheel {}  buttons {:03b}", p.x, p.y, p.wheel, p.buttons);
                    }
                }
                Some(name) => match dev::keymap::Layout::from_name(name) {
                    Some(l) => {
                        input::set_layout(l);
                        println!("keyboard layout: {}", l.name());
                    }
                    None => println!("{ERR}unknown layout {name}{RESET}"),
                },
            },
            "mem" => {
                let (free, usable) = mem::frame::stats();
                let (heap_used, heap_size) = mem::heap::stats();
                println!("physical  {} total, {} used, {} free", Size(usable), Size(usable - free), Size(free));
                println!("heap      {} in use of {} mapped", Size(heap_used as u64), Size(heap_size as u64));
            }
            "ps" | "tasks" => {
                println!("{DIM}  tid  pid  state      cpu     name{RESET}");
                for t in task::list() {
                    let pid = if t.pid == 0 { String::from("-") } else { t.pid.to_string() };
                    println!("{:>5} {:>4}  {:<9}  {:>5}ms  {}", t.tid, pid, t.state.name(), t.cpu_ms, t.name);
                }
            }
            "cpus" => {
                let (running, ready) = task::on_cpus();
                for (i, tid) in running.iter().enumerate() {
                    println!("cpu {i}: thread {tid}");
                }
                let (flushes, ipis, spins) = crate::arch::smp::user_flush_stats();
                println!("{ready} threads ready; program TLB flushes on other processors: {flushes} ({ipis} IPIs, {spins} spins waiting)");
                let mut sig = [0u8; 12];
                match crate::arch::cpu::hypervisor(&mut sig) {
                    Some(h) => println!("hypervisor {h}{}", if crate::arch::cpu::emulated() { " (emulated: spin loops skip pause)" } else { "" }),
                    None => println!("no hypervisor"),
                }
            }
            "locks" => {
                use crate::sync::contention::{HOLDS, WAITS};
                if rest.first() == Some(&"reset") {
                    WAITS.reset();
                    HOLDS.reset();
                } else {
                    println!("  waits  Mcycles  place");
                    for (at, waits, cycles) in WAITS.top(20) {
                        println!("{waits:>7}  {:>7}  {}:{}", cycles / 1_000_000, at.file(), at.line());
                    }
                    println!("  holds  Mcycles  place (Mutex)");
                    for (at, holds, cycles) in HOLDS.top(12) {
                        println!("{holds:>7}  {:>7}  {}:{}", cycles / 1_000_000, at.file(), at.line());
                    }
                }
            }
            "top" => self.top(rest.first().and_then(|n| n.parse().ok()).unwrap_or(2)),
            "where" => {
                if need(1, "<tid> [samples]") {
                    self.where_thread(rest[0], rest.get(1).and_then(|n| n.parse().ok()).unwrap_or(1));
                }
            }
            "kill" => {
                if need(1, "<pid>") {
                    match rest[0].parse::<u32>() {
                        Ok(pid) if crate::user::kill(pid) => println!("program {pid} stopped"),
                        _ => println!("{ERR}no running program {}{RESET}", rest[0]),
                    }
                }
            }
            "run" => {
                if need(1, "<program> [args]") {
                    let args = line.trim_start()[3..].trim_start();
                    let (prog, prog_args) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
                    self.run_program(prog, prog_args.trim());
                }
            }
            "irq" => {
                for (line, count, owners) in dev::irq::stats() {
                    let names: Vec<String> = owners
                        .iter()
                        .map(|&o| {
                            if o == 0 {
                                String::from("kernel")
                            } else {
                                dev::get(o).and_then(|d| d.driver).unwrap_or_else(|| alloc::format!("device {o}"))
                            }
                        })
                        .collect();
                    println!("IRQ {:>2}  {:>10}  {}", line, count, names.join(", "));
                }
                for (vector, count, owner) in dev::irq::msi_stats() {
                    let name = dev::get(owner).and_then(|d| d.driver).unwrap_or_else(|| alloc::format!("device {owner}"));
                    println!("MSI {vector:>2}  {count:>10}  {name}");
                }
            }
            "net" => {
                let list = dev::net::list();
                if list.is_empty() {
                    println!("no network interfaces (is a network driver installed? try: dev)");
                }
                for i in list {
                    let m = i.mac;
                    let card = dev::get(i.device).map(|d| d.description()).unwrap_or_default();
                    println!(
                        "{BOLD}net{}{RESET}  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  {DIM}{card}{RESET}",
                        i.index, m[0], m[1], m[2], m[3], m[4], m[5]
                    );
                    println!("      mtu {}  received {} ({})  sent {} ({})", i.mtu, i.rx.0, Size(i.rx.1), i.tx.0, Size(i.tx.1));
                    match crate::net::config(i.index) {
                        Some((c, true)) => {
                            println!("      address {} mask {} gateway {} DNS {}", c.ip, c.mask, c.gateway, c.dns);
                            println!("      {DIM}lease {} min (DHCP){RESET}", c.lease_s / 60);
                        }
                        _ => println!("      {DIM}waiting for an address (DHCP){RESET}"),
                    }
                }
                let arp = crate::net::arp_table();
                if !arp.is_empty() {
                    println!("{DIM}neighbours{RESET}");
                    for (ip, m, age) in arp {
                        println!(
                            "      {:<15} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  {age}s",
                            ip.to_string(),
                            m[0],
                            m[1],
                            m[2],
                            m[3],
                            m[4],
                            m[5]
                        );
                    }
                }
            }
            "ping" => {
                if need(1, "<host> [count]") {
                    let count: u16 = rest.get(1).and_then(|c| c.parse().ok()).unwrap_or(4);
                    if !crate::net::wait_configured(5000) {
                        println!("{ERR}the network is not configured{RESET}");
                        return;
                    }
                    match crate::net::resolve(rest[0]) {
                        Ok(ip) => {
                            println!("ping {} ({ip})", rest[0]);
                            let mut ok = 0;
                            for seq in 0..count {
                                match crate::net::ping(ip, seq, 2000) {
                                    Ok(ms) => {
                                        ok += 1;
                                        println!("  reply from {ip}: seq {seq}, time {ms} ms");
                                    }
                                    Err(e) => println!("  {WARN}seq {seq}: {e}{RESET}"),
                                }
                                if seq + 1 < count {
                                    task::sleep_ms(500);
                                }
                            }
                            println!("{ok} of {count} replies");
                        }
                        Err(e) => println!("{ERR}{}: {e}{RESET}", rest[0]),
                    }
                }
            }
            "host" => {
                if need(1, "<name>") {
                    if !crate::net::wait_configured(5000) {
                        println!("{ERR}the network is not configured{RESET}");
                        return;
                    }
                    match crate::net::resolve(rest[0]) {
                        Ok(ip) => println!("{} has address {ip}", rest[0]),
                        Err(e) => println!("{ERR}{}: {e}{RESET}", rest[0]),
                    }
                }
            }
            "fetch" => {
                if need(1, "<url> [file]") {
                    if !crate::net::wait_configured(5000) {
                        println!("{ERR}the network is not configured{RESET}");
                        return;
                    }
                    let started = time::uptime_ms();
                    let mut last = 0;
                    let mut report = |done: u64, total: Option<u64>| {
                        let now = time::uptime_ms();
                        if now - last > 1000 {
                            last = now;
                            match total {
                                Some(t) => println!("  {} of {}", Size(done), Size(t)),
                                None => println!("  {}", Size(done)),
                            }
                        }
                        true
                    };
                    match crate::net::http::fetch(&crate::net::http::Request::get(rest[0]), &mut report) {
                        Ok(r) => {
                            let ms = (time::uptime_ms() - started).max(1);
                            println!(
                                "{} {} · {} in {} ms · {}",
                                r.status,
                                r.url,
                                Size(r.body.len() as u64),
                                ms,
                                r.header("content-type").unwrap_or("")
                            );
                            match rest.get(1) {
                                Some(file) => match fs::abs(file).and_then(|p| fs::write_file(&p, &r.body)) {
                                    Ok(()) => println!("saved to {file}"),
                                    Err(e) => println!("{ERR}{file}: {e}{RESET}"),
                                },
                                None => {
                                    let text = String::from_utf8_lossy(&r.body);
                                    for line in text.lines().take(12) {
                                        println!("  {}", line.chars().take(100).collect::<String>());
                                    }
                                }
                            }
                        }
                        Err(e) => println!("{ERR}{e}{RESET}"),
                    }
                }
            }
            "sha256" => {
                if need(1, "<file>") {
                    match fs::abs(rest[0]).and_then(|p| fs::read_file(&p)) {
                        Ok(data) => {
                            let d = justtls::sha2::Sha256::digest(&data);
                            let hex: String = d.iter().map(|b| alloc::format!("{b:02x}")).collect();
                            println!("{hex}  {}", rest[0]);
                        }
                        Err(e) => println!("{ERR}{}: {e}{RESET}", rest[0]),
                    }
                }
            }
            "sysinfo" => self.sysinfo(),
            "acpi" => self.acpi(&rest),
            "date" => {
                let t = time::from_epoch(time::now());
                println!(
                    "{} {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    time::WEEKDAYS[t.weekday as usize],
                    t.year,
                    t.month,
                    t.day,
                    t.hour,
                    t.min,
                    t.sec
                );
            }
            "ports" => {
                println!("{DIM}  owner  queued  name{RESET}");
                for (name, owner, queued) in crate::ipc::list() {
                    let who = crate::user::process_name(owner).unwrap_or_default();
                    println!("  {owner:<5}  {queued:>6}  {name} {DIM}({who}){RESET}");
                }
            }
            "perms" => match (rest.first().copied(), rest.get(1).copied(), rest.get(2).copied()) {
                (None, _, _) => {
                    let all = crate::sandbox::all();
                    if all.is_empty() {
                        println!("{DIM}no app has asked for anything yet{RESET}");
                    }
                    for g in all {
                        match g.name.as_str() {
                            "" => println!("{}", g.id),
                            name => println!("{name} {DIM}{}{RESET}", g.id),
                        }
                        for (p, allow) in g.perms {
                            let (mark, word) = if allow { ("\x1b[92m+", "allowed") } else { ("\x1b[91m-", "denied") };
                            println!("  {mark}{RESET} {p:<22} {DIM}{word}{RESET}");
                        }
                    }
                }
                (Some(op @ ("allow" | "deny" | "ask")), Some(id), Some(perm)) => {
                    if !crate::sandbox::valid_id(id) || !crate::sandbox::known(perm) {
                        println!("{ERR}perms: not an app id or a permission (files:<folder>, files:<disk>:, camera, microphone, location, network){RESET}");
                    } else {
                        let answer = match op {
                            "allow" => Some(true),
                            "deny" => Some(false),
                            _ => None,
                        };
                        crate::sandbox::set(id, "", perm, answer);
                    }
                }
                (Some("forget"), Some(id), None) => {
                    if !crate::sandbox::forget(id) {
                        println!("{DIM}nothing known about {id}{RESET}");
                    }
                }
                _ => println!("usage: perms | perms allow|deny|ask <app_id> <permission> | perms forget <app_id>"),
            },
            "updates" => {
                use crate::appupdate as up;
                match (rest.first().copied(), rest.get(1).copied()) {
                    (None, _) => {
                        let when = match up::last_check() {
                            0 => String::from("never"),
                            t => {
                                let t = time::from_epoch(t);
                                alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.min)
                            }
                        };
                        println!("{DIM}last check: {when} · automatic: {}{RESET}", if up::auto() { "on" } else { "off" });
                        if let Some(e) = up::last_error() {
                            println!("{DIM}the index: {e}{RESET}");
                        }
                        let list = up::list();
                        if list.is_empty() {
                            println!("{DIM}no updates{RESET}");
                        }
                        for u in list {
                            println!("  {}", up::describe(&u));
                            if !u.new_permissions.is_empty() {
                                let what: Vec<String> = u.new_permissions.iter().map(|p| crate::sandbox::describe(p)).collect();
                                println!("    {DIM}asks for: {}{RESET}", what.join(", "));
                            }
                        }
                    }
                    (Some("check"), None) => match up::check() {
                        Ok(0) => println!("everything is up to date"),
                        Ok(n) => {
                            println!("{n} update(s):");
                            for u in up::list() {
                                println!("  {}", up::describe(&u));
                            }
                        }
                        Err(e) => println!("{ERR}updates: {e}{RESET}"),
                    },
                    (Some("install"), Some(id)) => match up::install(id) {
                        Ok(up::Applied::Done) => println!("updated"),
                        Ok(up::Applied::InUse) => println!("downloaded: it installs when the app is closed"),
                        Err(e) => println!("{ERR}updates: {e}{RESET}"),
                    },
                    (Some("auto"), Some(v @ ("on" | "off"))) => up::set_auto(v == "on"),
                    (Some("test"), None) => {
                        up::self_test();
                    }
                    _ => println!("usage: updates | updates check | updates install <app>|all | updates auto on|off | updates test"),
                }
            }
            "sysupdate" => {
                use crate::sysupdate as su;
                use crate::update::State;
                match (rest.first().copied(), rest.get(1).copied(), rest.get(2).copied()) {
                    (None, _, _) => match su::status() {
                        Ok(s) => {
                            for (i, slot) in s.slots.iter().enumerate() {
                                let mut notes = alloc::vec![if slot.good { "works" } else { "not confirmed" }];
                                if i == s.booted {
                                    notes.push("running");
                                }
                                if i == s.active && i != s.booted {
                                    notes.push("starts next");
                                }
                                if i == s.active && !slot.good && s.tries > 0 {
                                    notes.push(if s.tries == 1 { "1 try left" } else { "2 tries left" });
                                }
                                let version = slot.version.as_deref().unwrap_or("empty");
                                println!("slot {}: {version} · {}", su::name(i), notes.join(" · "));
                            }
                            println!("{DIM}{} per slot{RESET}", crate::util::Size(s.slots[0].size));
                            if !s.confirmed {
                                println!("{DIM}this version is on trial: it is confirmed once it has run for a while{RESET}");
                            }
                            if s.fallback {
                                println!("{DIM}the other slot did not start: this one took over{RESET}");
                            }
                        }
                        Err(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                    },
                    (Some("check"), None, _) => match crate::update::check_now(false) {
                        State::UpToDate => println!("justOS is up to date"),
                        State::Available(v) => println!("justOS {v} is available: sysupdate install"),
                        State::Released(v) => println!("justOS {v} is released on GitHub (not in the update index yet)"),
                        State::Staged(v) => println!("justOS {v} starts at the next restart"),
                        State::Failed(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                        _ => println!("already busy"),
                    },
                    (Some("install"), None, _) => match crate::update::install() {
                        Ok(v) => println!("justOS {v} is installed in the other slot: it starts at the next restart"),
                        Err(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                    },
                    (Some("install"), Some(a), b) if (a == "--unsigned") == b.is_some() => {
                        let (unsigned, file) = match b {
                            Some(f) => (true, f),
                            None => (false, a),
                        };
                        let result = fs::abs(file).and_then(|p| fs::read_file(&p)).map_err(|e| alloc::format!("{file}: {e}")).and_then(|data| {
                            if !unsigned {
                                let sig = fs::abs(&alloc::format!("{file}.sig"))
                                    .and_then(|p| fs::read_file(&p))
                                    .map_err(|_| alloc::format!("{file}.sig is missing (an image without a signature: --unsigned)"))?;
                                if !crate::appupdate::signed(&data, &sig) {
                                    return Err(String::from("the signature does not match"));
                                }
                            }
                            su::install(&data, &mut |_, _| {})
                        });
                        match result {
                            Ok(v) => println!("justOS {v} is installed in the other slot: it starts at the next restart"),
                            Err(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                        }
                    }
                    (Some("rollback"), None, _) => match su::rollback() {
                        Ok(v) => println!("justOS {v} starts at the next restart"),
                        Err(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                    },
                    (Some("cancel"), None, _) => match su::cancel() {
                        Ok(()) => println!("this version starts again at the next restart"),
                        Err(e) => println!("{ERR}sysupdate: {e}{RESET}"),
                    },
                    _ => println!("usage: sysupdate | sysupdate check | sysupdate install [[--unsigned] <file>] | sysupdate rollback | sysupdate cancel"),
                }
            }
            "install-system" => self.install_system(&rest),
            "dnd" => match rest.first().copied() {
                Some("test") => {
                    crate::gui::dnd::self_test();
                }
                _ => println!("usage: dnd test"),
            },
            "uptime" => {
                let s = time::uptime_ms() / 1000;
                println!("up {}h {:02}m {:02}s", s / 3600, s / 60 % 60, s % 60);
            }
            "log" | "journal" => {
                let n: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(40);
                let j = crate::console::journal();
                let text = String::from_utf8_lossy(&j);
                let lines: Vec<&str> = text.lines().collect();
                for l in &lines[lines.len().saturating_sub(n)..] {
                    println!("{l}");
                }
            }
            "verbose" => {
                let on = !matches!(rest.first(), Some(&"off"));
                crate::console::set_verbose(on);
                println!("debug messages {}", if on { "on" } else { "off" });
            }
            "strace" => {
                let on = !rest.contains(&"off");
                let failed = rest.contains(&"failed");
                let only: Vec<u16> = rest.iter().flat_map(|w| w.split(',')).filter_map(|n| n.parse().ok()).collect();
                let tid = rest.iter().find_map(|w| w.strip_prefix('t')?.parse().ok()).unwrap_or(0);
                crate::linux::set_trace(on, &only, failed, tid);
                println!("system call tracing {}", if on { "on" } else { "off" });
            }
            "export" => {
                if rest.is_empty() {
                    for v in crate::linux::extra_env() {
                        println!("{v}");
                    }
                }
                for v in &rest {
                    match v.split_once('=') {
                        Some((name, value)) if !name.is_empty() => crate::linux::set_env(name, value),
                        _ => println!("{ERR}export: {v}: NAME=value{RESET}"),
                    }
                }
            }
            "echo" => println!("{}", rest.join(" ")),
            "notify" => match rest[..] {
                [title, text] => crate::gui::notify(title, text, "info"),
                [title, text, icon] => crate::gui::notify(title, text, icon),
                _ => println!("{ERR}usage: notify \"title\" \"text\" [icon]{RESET}"),
            },
            "clear" | "cls" => crate::console::clear(),
            "restart" | "reboot" => {
                println!("restarting…");
                crate::acpi::reboot();
            }
            "shutdown" | "off" | "poweroff" => {
                println!("turning off…");
                crate::acpi::power_off();
                println!("{WARN}this computer cannot be turned off by software; it is safe to switch it off now{RESET}");
                crate::arch::cpu::halt_forever();
            }
            "suspend" | "sleep" => match rest.first().copied() {
                Some("check") => match crate::sleep::kind() {
                    Ok(k) => println!("this computer can sleep: {} (slept {} times since it started)", k.describe(), crate::sleep::count()),
                    Err(e) => println!("{WARN}cannot sleep: {e}{RESET}"),
                },
                first => {
                    let (want, arg) = match first {
                        Some("idle") => (Some(crate::sleep::Kind::Idle), rest.get(1).copied()),
                        Some("ram") => (Some(crate::sleep::Kind::Ram), rest.get(1).copied()),
                        _ => (None, first),
                    };
                    let secs = match arg.map(str::parse::<u64>) {
                        None => None,
                        Some(Ok(s)) if (1..86_400).contains(&s) => Some(s),
                        Some(_) => {
                            println!("{WARN}usage: suspend [idle | ram] [seconds (1..86399)] | check{RESET}");
                            return;
                        }
                    };
                    match secs {
                        Some(s) => println!("going to sleep for {s} s…"),
                        None => println!("going to sleep; press the power button or a key to wake up…"),
                    }
                    match crate::sleep::suspend_as(want, secs) {
                        Ok(s) => println!("awake again after {s} s"),
                        Err(e) => println!("{WARN}cannot sleep: {e}{RESET}"),
                    }
                }
            },
            "install" | "pkg" => self.install(&rest),
            "deb" => self.deb(&rest),
            "uninstall" => self.uninstall(rest.first().copied()),
            "kpanic" => panic!("stopped on purpose (kpanic)"),
            "play" => {
                if need(1, "<file.wav> | stop") {
                    if rest[0] == "stop" {
                        crate::dev::audio::stop();
                        return;
                    }
                    let Some(p) = self.path(rest[0]) else { return };
                    let sound = fs::read_file(&p).map_err(|e| alloc::format!("{e}"));
                    match sound.and_then(|b| crate::dev::audio::parse_wav(&b).map_err(String::from)) {
                        Ok(s) => {
                            let (rate, ch, len) = (s.rate, s.channels, s.seconds_x10());
                            match crate::dev::audio::play(s) {
                                Ok(()) => println!(
                                    "playing {}: {rate} Hz, {ch} channel{}, {}.{} s",
                                    rest[0],
                                    if ch == 1 { "" } else { "s" },
                                    len / 10,
                                    len % 10
                                ),
                                Err(e) => println!("{ERR}{e}{RESET}"),
                            }
                        }
                        Err(e) => println!("{ERR}{}: {e}{RESET}", rest[0]),
                    }
                }
            }
            "tone" => {
                let hz = rest.first().and_then(|v| v.parse().ok()).unwrap_or(440);
                let ms = rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(500u32).min(10_000);
                if let Err(e) = crate::dev::audio::play(crate::dev::audio::tone(hz, ms)) {
                    println!("{ERR}{e}{RESET}");
                }
            }
            "volume" => match rest.first() {
                None => println!("volume {}", crate::dev::audio::volume()),
                Some(v) => match v.parse::<u32>() {
                    Ok(v) if v <= 100 => {
                        crate::gui::change_volume(v);
                        println!("volume {v}");
                    }
                    _ => println!("{ERR}usage: volume [0-100]{RESET}"),
                },
            },
            "hostname" => match rest.first().copied() {
                None => println!("{} ({})", crate::host::hostname(), crate::host::name()),
                Some("check") => {
                    if crate::net::dhcp_self_test() {
                        println!("dhcp host name option: ok ({})", crate::host::hostname());
                    } else {
                        println!("{ERR}dhcp host name option: wrong{RESET}");
                    }
                }
                Some(n) => match crate::host::problem(n) {
                    Some(p) => println!("{ERR}hostname: {p}{RESET}"),
                    None => {
                        if crate::gui::active() {
                            crate::gui::set_device_name(n);
                        } else {
                            crate::host::set(n);
                        }
                        println!("{} ({})", crate::host::hostname(), crate::host::name());
                    }
                },
            },
            "setup" => match rest.first().copied() {
                None => println!("first-start setup: {}", if crate::gui::setup_done() { "done" } else { "not done yet" }),
                Some("skip") => {
                    crate::gui::setup_skip();
                    println!("first-start setup: done");
                }
                _ => println!("{ERR}usage: setup [skip]{RESET}"),
            },
            "display" => self.display(rest.first().copied(), rest.get(1).copied()),
            "fps" => self.fps(rest.first().copied()),
            _ => {
                let prog_args = line.trim_start()[args[0].len()..].trim();
                if crate::user::find_program(&args[0], &self.cwd).is_some() {
                    self.run_program(&args[0], prog_args);
                } else if let Some(busybox) = busybox_applet(&args[0]) {
                    self.run_applet(busybox, &args[0], prog_args);
                } else if let Some(path) = crate::user::find_linux_program(&args[0]) {
                    // A program of a Debian package (/usr/bin/hello).
                    self.report(&args[0], crate::user::run_foreground(&path, prog_args, self.cwd.clone()));
                } else {
                    println!("{ERR}{cmd}: unknown command{RESET} {DIM}(type help){RESET}");
                }
            }
        }
    }

    fn run_program(&self, name: &str, args: &str) {
        let Some(path) = crate::user::find_program(name, &self.cwd) else {
            println!("{ERR}{name}: no such program{RESET}");
            return;
        };
        self.report(name, crate::user::run_foreground(&path, args, self.cwd.clone()));
    }

    /// `where <tid> [samples]`: where a thread of a Linux program is, as a
    /// library and an offset in it; with samples, the places it was most
    /// often, 10 ms apart (to find what a busy program is doing). `busy`
    /// does it for the four program threads that used the most time.
    fn where_thread(&self, tid: &str, samples: usize) {
        if tid == "busy" {
            let mut all: Vec<task::ThreadInfo> = task::list().into_iter().filter(|t| t.pid != 0).collect();
            all.sort_by(|a, b| b.cpu_ms.cmp(&a.cpu_ms));
            for t in all.iter().take(4) {
                println!("thread {} (process {}, {} ms):", t.tid, t.pid, t.cpu_ms);
                self.where_thread(&alloc::format!("{}", t.tid), samples);
            }
            return;
        }
        let Ok(tid) = tid.parse::<task::Tid>() else {
            println!("{ERR}usage: where <tid> [samples]{RESET}");
            return;
        };
        let mut seen: Vec<(String, usize)> = Vec::new();
        let mut tries = 0;
        let mut taken = 0;
        while taken < samples.clamp(1, 2000) && tries < samples * 4 + 20 {
            tries += 1;
            if let Some((pid, rip, call)) = task::user_frame(tid) {
                let mut place = crate::linux::mm::describe(pid, rip);
                if let Some(n) = call {
                    place.push_str(&alloc::format!("  in system call {n}"));
                }
                match seen.iter_mut().find(|(p, _)| *p == place) {
                    Some((_, n)) => *n += 1,
                    None => seen.push((place, 1)),
                }
                taken += 1;
            }
            task::sleep_ms(10);
        }
        if taken == 0 {
            println!("{ERR}thread {tid} is not a program's thread, or never stopped{RESET}");
            return;
        }
        seen.sort_by(|a, b| b.1.cmp(&a.1));
        for (place, n) in seen.iter().take(20) {
            println!("  {n:>5}  {place}");
        }
    }

    /// `top`: what the processors did over `secs` seconds. Time is counted
    /// in timer ticks (1 ms): a thread's share is the ticks that found it
    /// running, "user" those that found it in its program rather than in
    /// the kernel; a processor's idle share is its idle thread's.
    fn top(&self, secs: u64) {
        use crate::linux::calls;
        let ms = secs.clamp(1, 60) * 1000;
        let before: BTreeMap<task::Tid, (u64, u64, u64)> = task::list().iter().map(|t| (t.tid, (t.ticks, t.user_ticks, t.runs))).collect();
        let cpus0 = task::cpu_stats();
        calls::start();
        task::sleep_ms(ms);
        let (calls, lost) = calls::stop();
        let cpus1 = task::cpu_stats();
        let after = task::list();
        let span = cpus1.first().zip(cpus0.first()).map_or(1, |(b, a)| b.ticks.saturating_sub(a.ticks)).max(1);
        let per_s = |n: u64| n * crate::arch::pit::HZ / span;
        let pct = |n: u64| n * 100 / span;
        println!("{DIM}over {} ms, {} processors{RESET}", span * 1000 / crate::arch::pit::HZ, cpus1.len());
        for (i, (a, b)) in cpus0.iter().zip(&cpus1).enumerate() {
            let ticks = b.ticks.saturating_sub(a.ticks).max(1);
            let idle = b.idle_ticks.saturating_sub(a.idle_ticks);
            println!("cpu {i}: {:>3}% idle, {} switches/s", idle * 100 / ticks, per_s(b.switches.saturating_sub(a.switches)));
        }
        let idle: Vec<task::Tid> = cpus1.iter().map(|c| c.idle).collect();
        let mut by_thread: BTreeMap<task::Tid, u64> = BTreeMap::new();
        let mut by_call: BTreeMap<u16, (u64, Vec<(u32, task::Tid)>)> = BTreeMap::new();
        for &(tid, n, c) in &calls {
            *by_thread.entry(tid).or_default() += c as u64;
            let e = by_call.entry(n).or_default();
            e.0 += c as u64;
            e.1.push((c, tid));
        }
        let mut rows: Vec<(u64, u64, u64, &task::ThreadInfo)> = after
            .iter()
            .filter(|t| !idle.contains(&t.tid))
            .map(|t| {
                let (b, bu, br) = before.get(&t.tid).copied().unwrap_or((0, 0, 0));
                (t.ticks.saturating_sub(b), t.user_ticks.saturating_sub(bu), t.runs.saturating_sub(br), t)
            })
            .filter(|&(d, _, r, t)| d > 0 || r > 0 || by_thread.contains_key(&t.tid))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.2.cmp(&a.2)));
        println!("{DIM}  tid  pid  cpu%  user%  runs/s  calls/s  state     name{RESET}");
        for (d, u, r, t) in rows.iter().take(16) {
            let pid = if t.pid == 0 { String::from("-") } else { t.pid.to_string() };
            let n = by_thread.get(&t.tid).copied().unwrap_or(0);
            println!("{:>5} {:>4}  {:>4}  {:>5}  {:>6}  {:>7}  {:<8}  {}", t.tid, pid, pct(*d), pct(*u), per_s(*r), per_s(n), t.state.name(), t.name);
        }
        let total: u64 = by_call.values().map(|c| c.0).sum();
        println!("{DIM}system calls: {}/s{}; kernel%: the share of one processor the kernel spent in them{RESET}", per_s(total), if lost > 0 { alloc::format!(" ({lost} not counted)") } else { String::new() });
        let mut costly: Vec<(u64, u16)> = (0..512).map(|n| (calls::ticks(n), n)).filter(|c| c.0 > 0).collect();
        let mut list: Vec<(u16, (u64, Vec<(u32, task::Tid)>))> = by_call.into_iter().collect();
        list.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for (n, (c, mut threads)) in list.into_iter().take(14) {
            threads.sort_by(|a, b| b.0.cmp(&a.0));
            let who: Vec<String> = threads.iter().take(3).map(|&(c, t)| alloc::format!("{t}:{}", per_s(c as u64))).collect();
            println!("{:>8}/s  {:<16} {:>3}%  {}", per_s(c), alloc::format!("{} {n}", calls::name(n)), pct(calls::ticks(n)), who.join(" "));
        }
        // The calls the kernel spent most time in, often or not.
        costly.sort_by(|a, b| b.cmp(a));
        let most: Vec<String> = costly.iter().take(8).map(|&(t, n)| alloc::format!("{} {n} {}%", calls::name(n), pct(t))).collect();
        if !most.is_empty() {
            println!("{DIM}kernel time:{RESET} {}", most.join(", "));
        }
    }

    /// `program [arguments] &`: started, not waited for; `ps` shows it and
    /// `kill` stops it.
    fn start_background(&self, line: &str) {
        let args = split(line);
        let Some(name) = args.first() else {
            println!("{ERR}usage: <program> [arguments] &{RESET}");
            return;
        };
        let Some(path) = crate::user::find_program(name, &self.cwd).or_else(|| crate::user::find_linux_program(name)) else {
            println!("{ERR}{name}: no such program{RESET}");
            return;
        };
        let prog_args = line.trim_start()[name.len()..].trim();
        match crate::user::spawn(&path, prog_args, self.cwd.clone()) {
            Ok(pid) => println!("{DIM}[{pid}]{RESET} {name}"),
            Err(e) => println!("{ERR}{name}: {}{RESET}", e.as_str()),
        }
    }

    /// Runs a BusyBox applet (`ls`, `cat`, …) out of the installed `busybox`.
    fn run_applet(&self, busybox: Path, applet: &str, args: &str) {
        self.report(applet, crate::user::run_applet_foreground(&busybox, applet, args, self.cwd.clone()));
    }

    fn report(&self, name: &str, result: Result<i64, justdk::syscall::Error>) {
        match result {
            Ok(0) => {}
            Ok(-130) => println!("{DIM}^C{RESET}"),
            Ok(code) => println!("{DIM}{name} ended with code {code}{RESET}"),
            Err(e) => println!("{ERR}{name}: {}{RESET}", e.as_str()),
        }
    }

    /// `deb list`: the packages installed from .deb files; `deb install
    /// <file>` and `deb remove <name>` run dpkg through the Linux layer.
    fn deb(&self, args: &[&str]) {
        use crate::gui::debinst;
        match (args.first().copied(), args.get(1).copied()) {
            (None, _) | (Some("list"), _) => {
                let list = debinst::user_packages();
                if list.is_empty() {
                    println!("no packages installed from .deb files");
                }
                for (name, version, about) in list {
                    println!("  {BOLD}{name:<24}{RESET}{DIM}{version} · {about}{RESET}");
                }
            }
            (Some("install"), Some(file)) => {
                let Some(p) = self.path(file) else { return };
                match debinst::install_blocking(&alloc::format!("{p}")) {
                    Ok(log) => println!("{log}"),
                    Err(e) => println!("{ERR}{file}: {e}{RESET}"),
                }
            }
            (Some("remove"), Some(name)) => match debinst::remove(name) {
                Ok(log) => println!("{log}"),
                Err(e) => println!("{ERR}{name}: {e}{RESET}"),
            },
            _ => println!("usage: deb [list | install <file.deb> | remove <name>]"),
        }
    }

    /// Installs a program into `sys:/apps`, so it can be run by name: from
    /// this computer (`sys:/store`) or, failing that, from the online
    /// catalog. With no argument, lists both; `install --refresh` downloads
    /// the newest catalog; `install --search <words>` searches GitHub.
    fn install(&self, args: &[&str]) {
        let packages = crate::store::available();
        let Some(&name) = args.first() else {
            println!("On this computer (install <name>):");
            for p in &packages {
                let mark = if crate::store::installed(&p.id).is_some() { " (installed)" } else { "" };
                println!("  {BOLD}{:<14}{RESET}{DIM}{}{mark}{RESET}", p.id, p.about);
            }
            let online = crate::store::catalog();
            if !online.is_empty() {
                println!("From the internet:");
                for a in &online {
                    let mark = if crate::store::installed(&a.id).is_some() { " (installed)" } else { "" };
                    println!("  {BOLD}{:<14}{RESET}{DIM}{} · {}{mark}{RESET}", a.id, a.platform.name(), a.about);
                }
            }
            return;
        };
        if name.to_ascii_lowercase().ends_with(".deb") {
            return self.deb(&["install", name]);
        }
        // A package file (.tar.gz, .zip, .pkg...).
        if crate::store::is_package(name) {
            let Some(p) = self.path(name) else { return };
            match crate::store::install_file(&alloc::format!("{p}")) {
                Ok(msg) => println!("{msg}"),
                Err(e) => println!("{ERR}{name}: {e}{RESET}"),
            }
            return;
        }
        if name == "--refresh" {
            match crate::store::refresh_catalog() {
                Ok(n) => println!("the catalog lists {n} programs"),
                Err(e) => println!("{ERR}{e}{RESET}"),
            }
            return;
        }
        if name == "--search" {
            let q = args[1..].join(" ");
            match crate::store::github_search(&q) {
                Ok(v) => {
                    for r in v {
                        println!("  {BOLD}{}{RESET} {DIM}★{} {}{RESET}", r.full_name, r.stars, r.description);
                    }
                }
                Err(e) => println!("{ERR}{e}{RESET}"),
            }
            return;
        }
        if packages.iter().any(|p| p.id == name) {
            match crate::store::install(name) {
                Ok(p) => {
                    println!("installed {BOLD}{}{RESET} → {}", p.name, crate::store::program_path(&p));
                    if p.id == "busybox" {
                        println!("{DIM}{} commands are now available (ls, cat, grep, …){RESET}", BUSYBOX_APPLETS.len());
                    } else if p.gui {
                        println!("{DIM}it is in the launcher (Ctrl+Space){RESET}");
                    }
                }
                Err(e) => println!("{ERR}{name}: {}{RESET}", e.as_str()),
            }
            return;
        }
        let Some(app) = crate::store::catalog().into_iter().find(|a| a.id == name || a.command == name) else {
            println!("{ERR}no package named {name}{RESET} {DIM}(type install){RESET}");
            return;
        };
        if !crate::net::wait_configured(5000) {
            println!("{ERR}the network is not configured{RESET}");
            return;
        }
        let id = app.id.clone();
        crate::store::start_install(app);
        let mut last = (String::new(), 0u64);
        loop {
            let Some(j) = crate::store::job(&id) else { break };
            if let Some(r) = j.result {
                crate::store::clear_job(&id);
                match r {
                    Ok(()) => {
                        let p = crate::store::installed(&id);
                        let file = p.as_ref().map(crate::store::program_path).unwrap_or_default();
                        let version = p.map(|p| p.version).unwrap_or_default();
                        println!("installed {BOLD}{id}{RESET} {version} → {file}");
                    }
                    Err(e) => println!("{ERR}{id}: {e}{RESET}"),
                }
                break;
            }
            let now = (alloc::string::String::from(j.phase), j.done / (256 * 1024));
            if now != last {
                match j.total {
                    Some(t) => println!("  {} {} of {}", j.phase, Size(j.done), Size(t)),
                    None => println!("  {}", j.phase),
                }
                last = now;
            }
            task::sleep_ms(100);
        }
    }

    fn uninstall(&self, name: Option<&str>) {
        let Some(name) = name else {
            println!("usage: uninstall <name>");
            return;
        };
        match crate::store::remove(name) {
            Ok(()) => println!("removed {BOLD}{name}{RESET}"),
            Err(fs::Error::NotFound) => println!("{ERR}{name} is not installed{RESET}"),
            Err(e) => println!("{ERR}{name}: {}{RESET}", e.as_str()),
        }
    }

    fn display(&self, arg: Option<&str>, value: Option<&str>) {
        let Some(arg) = arg else {
            match crate::console::pixel_size() {
                Some((w, h)) => println!("screen {w}x{h}"),
                None => println!("text screen"),
            }
            if crate::gui::active() {
                let choice = match crate::gui::scale_choice() {
                    0 => "auto",
                    _ => "chosen",
                };
                println!("interface: {} pixels per unit ({choice})", crate::gui::scale());
            }
            match crate::dev::display::controller() {
                Some(name) => {
                    println!("controller: {name}");
                    let modes = crate::dev::display::modes();
                    let list: Vec<String> = modes.iter().map(|m| alloc::format!("{}x{}", m.width, m.height)).collect();
                    println!("resolutions: {}", list.join(" "));
                }
                None => println!("no display driver: the screen stays in the mode the firmware chose"),
            }
            for o in crate::dev::audio::list() {
                println!("sound: audio{} {} ({} Hz, {} channels)", o.index, o.name, o.rate, o.channels);
            }
            return;
        };
        if arg == "scale" {
            let choice = match value {
                Some("auto") => 0,
                Some("1") => 1,
                Some("2") => 2,
                _ => {
                    println!("{ERR}usage: display scale auto|1|2{RESET}");
                    return;
                }
            };
            if choice == 2 && !crate::gui::can_double() {
                println!("{ERR}the screen is too small for the double size (it needs 1600x1200 pixels){RESET}");
                return;
            }
            match crate::gui::change_scale(choice) {
                Ok(()) => println!("interface size: {}", value.unwrap_or("")),
                Err(e) => println!("{ERR}{e}{RESET}"),
            }
            return;
        }
        let mode = arg.split_once('x').and_then(|(w, h)| Some(justdk::DisplayMode { width: w.parse().ok()?, height: h.parse().ok()? }));
        let Some(mode) = mode else {
            println!("{ERR}usage: display [WxH | scale auto|1|2]{RESET}");
            return;
        };
        match crate::gui::change_resolution(mode) {
            Ok(()) => println!("screen {}x{}", mode.width, mode.height),
            Err(e) => println!("{ERR}{e}{RESET}"),
        }
    }

    /// How the interface's frames went in the last second, after setting
    /// the most a second when asked.
    fn fps(&self, arg: Option<&str>) {
        use crate::gui::pace;
        if let Some(a) = arg {
            let hz = match a {
                "auto" => Some(0),
                n => n.parse::<u32>().ok().filter(|&n| n != 0 && pace::CHOICES.contains(&n)),
            };
            let Some(hz) = hz else {
                println!("{ERR}usage: fps [auto|60|120|144|240]{RESET}");
                return;
            };
            if let Err(e) = crate::gui::set_fluidity(hz) {
                println!("{ERR}{e}{RESET}");
                return;
            }
        }
        let how = match (pace::choice(), pace::display_hz()) {
            (0, Some(_)) => "automatic: the display's rate",
            (0, None) => "automatic: the display does not say its rate",
            _ => "chosen",
        };
        println!("cap {} Hz ({how})", pace::cap());
        if !crate::gui::active() {
            println!("no graphical interface");
            return;
        }
        let s = pace::stats();
        let ms = |us: u32| alloc::format!("{}.{} ms", us / 1000, us % 1000 / 100);
        println!(
            "last second: {} frames, {} each on average, {} at most; longest gap {}",
            s.frames,
            ms(s.avg_us),
            ms(s.max_us),
            ms(s.max_gap_us)
        );
        println!("loop: {} wake-ups in the last whole second", s.wakes);
    }

    fn help(&self, topic: Option<&str>) {
        if let Some(t) = topic {
            match COMMANDS.iter().find(|c| c.0 == t) {
                Some((name, args, text)) => println!("{BOLD}{name}{RESET} {args}\n  {text}"),
                None => println!("{ERR}non c'è un comando che si chiama {t}{RESET}"),
            }
            if t == "drv" {
                println!(
                    "  drv                 tutti i driver: magazzino, pacchetto di avvio, caricati\n  drv info <nome>     il manifesto e i dispositivi di un driver\n  drv add <file>      aggiunge un pacchetto al magazzino e lo installa\n  drv remove <nome>   scarica un driver e lo cancella\n  drv reload <nome>   scarica un driver e lo ricarica (versione nuova)\n  drv inbox           installa i pacchetti che aspettano in {}\n  drv scan            cerca hardware nuovo",
                    driver::INBOX
                );
            }
            return;
        }
        for (name, args, text) in COMMANDS {
            println!("  {BOLD}{name:<10}{RESET}{DIM}{args:<22}{RESET} {text}");
        }
        println!("\nUn percorso comincia dal volume: {BOLD}sys:/system/drivers{RESET}. Tab completa comandi e percorsi.");
        println!("Un programma seguito da {BOLD}&{RESET} gira senza che la shell lo aspetti.");
        if let Ok(apps) = fs::abs("sys:/apps").and_then(|p| fs::list_dir(&p)) {
            let names: Vec<String> = apps
                .iter()
                .filter_map(|(e, _)| e.name.strip_suffix(".app").map(String::from))
                .collect();
            if !names.is_empty() {
                println!("Programmi in sys:/apps: {BOLD}{}{RESET}", names.join("  "));
            }
        }
    }

    fn ls(&self, target: &str) {
        let Some(p) = self.path(target) else { return };
        match fs::list_dir(&p) {
            Ok(list) => {
                for (e, st) in &list {
                    if st.kind == justfs::Kind::Dir {
                        println!("  {DIM}{}{RESET}  {:>10}  {ACCENT}{}/{RESET}", fmt_time(st.modified), "", e.name);
                    } else {
                        println!("  {DIM}{}{RESET}  {:>10}  {}", fmt_time(st.modified), Size(st.size).to_string(), e.name);
                    }
                }
                if list.is_empty() {
                    println!("  {DIM}(empty){RESET}");
                }
            }
            Err(fs::Error::NotDir) => match fs::stat(&p) {
                Ok(st) => println!("  {DIM}{}{RESET}  {:>10}  {}", fmt_time(st.modified), Size(st.size).to_string(), p.name()),
                Err(e) => println!("{ERR}{p}: {e}{RESET}"),
            },
            Err(e) => println!("{ERR}{p}: {e}{RESET}"),
        }
    }

    fn volumes(&self) {
        println!("{DIM}  volume   label          size        free        on{RESET}");
        for v in fs::volumes() {
            let (total, free) = v.space();
            let ro = if v.read_only() { " (read-only)" } else { "" };
            println!(
                "  {ACCENT}{:<8}{RESET} {:<14} {:>10}  {:>10}  {}{}",
                alloc::format!("{}:", v.name),
                v.label,
                Size(total).to_string(),
                Size(free).to_string(),
                v.source,
                ro
            );
        }
    }

    fn disks(&self) {
        let list = dev::block::list();
        if list.is_empty() {
            println!("no disks (is a storage driver installed? try: drv)");
        }
        for d in list {
            let indent = if d.parent.is_some() { "    " } else { "" };
            let vol = fs::volumes().into_iter().find(|v| v.source == d.name).map(|v| alloc::format!(" → {}:", v.name)).unwrap_or_default();
            let what = if d.parent.is_some() { d.kind.clone() } else { d.model.clone() };
            println!("  {indent}{BOLD}{:<10}{RESET} {:>10}  {}{ACCENT}{}{RESET}", d.name, Size(d.size_bytes()).to_string(), what, vol);
        }
    }

    /// `install-system`: lists the disks justOS can go on; with a disk,
    /// says what installing there erases; with `--yes-erase` and the disk's
    /// code, installs.
    fn install_system(&self, rest: &[&str]) {
        use crate::install;
        let show = |t: &install::Target| {
            let tag = if t.usable() { OK } else { DIM };
            println!("  {tag}{:<8}{RESET} code {BOLD}{}{RESET} · {} · {} · {} · {}", t.disk.name, t.code, install::size_text(t.size), t.bus, t.name, t.contents);
            println!("           {DIM}{}{RESET}", t.parts_text());
            if let Some(p) = &t.problem {
                println!("           {WARN}cannot be used: {p}{RESET}");
            }
            if let Some(n) = &t.note {
                println!("           {DIM}{n}{RESET}");
            }
        };
        match rest {
            [] | ["list"] => {
                let all = install::targets();
                if all.is_empty() {
                    println!("no disk to install justOS on (the disk this system runs from is never one)");
                    return;
                }
                println!("disks justOS can be installed on ({} needed):", install::size_text(install::needed()));
                for t in &all {
                    show(t);
                }
                println!("{DIM}install-system <disk> shows what installing there erases{RESET}");
            }
            [name] => match install::targets().into_iter().find(|t| t.disk.name.eq_ignore_ascii_case(name)) {
                Some(t) => {
                    show(&t);
                    if t.usable() {
                        println!("{WARN}installing justOS on {} erases everything on it: {} ({}).{RESET}", t.disk.name, t.contents, t.parts_text());
                        println!("to go ahead: install-system {} --yes-erase {}", t.disk.name, t.code);
                    }
                }
                None => println!("{ERR}install-system: {name} is not a disk justOS can be installed on (see install-system list){RESET}"),
            },
            [name, "--yes-erase", code] => {
                let mut last = (usize::MAX, u32::MAX);
                let result = install::install(name, code, &mut |p| {
                    let pct = p.permille / 10;
                    if (p.phase, pct) != last {
                        last = (p.phase, pct);
                        println!("install: {pct}% {} ({} of {})", install::PHASES[p.phase], Size(p.done), Size(p.total));
                    }
                });
                match result {
                    Ok(v) => println!("{OK}justOS {v} is installed on {name}: remove the installation medium and restart{RESET}"),
                    Err(f) => {
                        println!("{ERR}install-system: {}{RESET}", f.message);
                        if f.touched {
                            println!("{WARN}{name} was partly written: what it held is gone and it does not start{RESET}");
                        }
                    }
                }
            }
            _ => println!("usage: install-system [list] | install-system <disk> | install-system <disk> --yes-erase <code>"),
        }
    }

    fn devices(&self, arg: Option<&str>) {
        if arg == Some("scan") {
            let (new, bound) = driver::rescan();
            println!("{new} new devices, {bound} drivers installed");
            return;
        }
        if let Some(id) = arg.and_then(|a| a.parse::<u32>().ok()) {
            let Some(d) = dev::get(id) else {
                println!("{ERR}no device {id}{RESET}");
                return;
            };
            let i = &d.info;
            println!("{BOLD}{}{RESET}  {}", d.description(), d.location());
            if i.bus == justdk::BUS_PCI {
                println!(
                    "  id        {:04x}:{:04x}  (subsystem {:04x}:{:04x}, revision {:02x})",
                    i.vendor, i.device, i.subsystem_vendor, i.subsystem, i.revision
                );
                println!("  class     {:02x}:{:02x}:{:02x}", i.class, i.subclass, i.prog_if);
            }
            if i.irq != justdk::IRQ_NONE {
                println!("  IRQ       {}", i.irq);
            }
            for (n, r) in i.resources.iter().enumerate() {
                match r.kind {
                    justdk::RES_MEMORY => println!("  [{n}]       memory {:#x} ({})", r.base, Size(r.size)),
                    justdk::RES_IO => println!("  [{n}]       ports  {:#x}", r.base),
                    _ => {}
                }
            }
            match (&d.driver, d.builtin, &d.problem) {
                (Some(drv), _, _) => println!("  driver    {OK}{drv}{RESET}"),
                (None, Some(b), _) => println!("  driver    {b}"),
                (None, None, Some(p)) => println!("  driver    {WARN}none: {p}{RESET}"),
                _ if d.owned => println!("  driver    {}, through the device it is plugged into", owner_driver(&d)),
                _ => println!("  driver    {DIM}not needed{RESET}"),
            }
            return;
        }
        let list = dev::list();
        let width = list.iter().map(|d| d.location().chars().count()).max().unwrap_or(0).max(8);
        let names = list.iter().map(|d| d.description().chars().count()).max().unwrap_or(0).clamp(6, 44);
        println!("{DIM}   id  {:<width$}  {:<names$}  driver{RESET}", "location", "device");
        for d in list {
            let driver = match (&d.driver, d.builtin, &d.problem) {
                (Some(drv), _, _) => alloc::format!("{OK}{drv}{RESET}"),
                (None, Some(b), _) => alloc::format!("{DIM}{b}{RESET}"),
                (None, None, Some(p)) => alloc::format!("{WARN}{p}{RESET}"),
                _ if d.owned => alloc::format!("{OK}{}{RESET}", owner_driver(&d)),
                _ => alloc::format!("{DIM}—{RESET}"),
            };
            let desc: String = d.description().chars().take(names).collect();
            println!("  {:>3}  {:<width$}  {:<names$}  {}", d.info.id, d.location(), desc, driver);
        }
    }

    fn drivers(&self, args: &[&str]) {
        match args.first().copied() {
            None | Some("list") => {
                println!("{DIM}  driver        version  class     where              status{RESET}");
                for d in driver::status() {
                    let origin: Vec<&str> = d.origins.iter().map(|o| o.name()).collect();
                    let status = match &d.loaded {
                        Some((v, _, devs, _)) => {
                            alloc::format!("{OK}running{RESET} {v}, {} device{}", devs.len(), if devs.len() == 1 { "" } else { "s" })
                        }
                        None => alloc::format!("{DIM}installed, idle{RESET}"),
                    };
                    println!("  {BOLD}{:<13}{RESET} {:<8} {:<9} {:<18} {}", d.name, d.version, d.class, origin.join(" + "), status);
                }
            }
            Some("info") => {
                let Some(name) = args.get(1) else {
                    println!("usage: drv info <name>");
                    return;
                };
                match driver::details(name) {
                    Some((manifest, paths)) => {
                        println!("{BOLD}{name}{RESET}");
                        for line in manifest.lines().filter(|l| !l.trim().is_empty()) {
                            println!("  {line}");
                        }
                        for (p, o, size) in paths {
                            println!("  {DIM}{} ({}, {}){RESET}", p, o.name(), Size(size as u64));
                        }
                        if let Some(s) = driver::status().into_iter().find(|s| s.name.eq_ignore_ascii_case(name)) {
                            if let Some((v, size, devs, unload)) = s.loaded {
                                println!(
                                    "  {OK}running{RESET} version {v}, {} in memory, {}",
                                    Size(size),
                                    if unload { "can be unloaded" } else { "stays loaded" }
                                );
                                for id in devs {
                                    if let Some(d) = dev::get(id) {
                                        println!("    device {id}: {} ({})", d.description(), d.location());
                                    }
                                }
                            }
                        }
                    }
                    None => println!("{ERR}no driver named {name}{RESET}"),
                }
            }
            Some("add") | Some("install") => {
                let Some(file) = args.get(1) else {
                    println!("usage: drv add <file.jdp>");
                    return;
                };
                if let Some(p) = self.path(file) {
                    match driver::add(&p) {
                        Ok(msg) => println!("{OK}{msg}{RESET}"),
                        Err(e) => println!("{ERR}{e}{RESET}"),
                    }
                }
            }
            Some("remove") | Some("uninstall") => {
                let Some(name) = args.get(1) else {
                    println!("usage: drv remove <name>");
                    return;
                };
                match driver::remove(name) {
                    Ok(msg) => println!("{OK}{name}: {msg}{RESET}"),
                    Err(e) => println!("{ERR}{e}{RESET}"),
                }
            }
            Some("reload") => {
                let Some(name) = args.get(1) else {
                    println!("usage: drv reload <name>");
                    return;
                };
                match driver::reload(name) {
                    Ok(msg) => println!("{OK}{msg}{RESET}"),
                    Err(e) => println!("{ERR}{e}{RESET}"),
                }
            }
            Some("inbox") => {
                let msgs = driver::process_inbox();
                if msgs.is_empty() {
                    println!("{} is empty", driver::INBOX);
                }
            }
            Some("scan") => self.devices(Some("scan")),
            Some(other) => println!("{ERR}drv {other}: unknown action{RESET} {DIM}(help drv){RESET}"),
        }
    }

    fn sysinfo(&self) {
        let mut brand = [0u8; 48];
        println!("{BOLD}justOS{RESET} {}", crate::VERSION);
        println!("  name       {} ({})", crate::host::name(), crate::host::hostname());
        println!("  processor  {} ({} running)", crate::arch::cpu::brand(&mut brand), crate::arch::smp::count());
        let (_, usable) = mem::frame::stats();
        println!("  memory     {}", Size(usable));
        if let Some((w, h)) = crate::console::pixel_size() {
            println!("  screen     {w}x{h}");
        }
        let (oem, tables) = crate::acpi::summary();
        if !tables.is_empty() {
            println!("  ACPI       {oem}: {}", tables.join(" "));
        }
        let devs = dev::list();
        let with = devs.iter().filter(|d| d.has_driver()).count();
        let missing = devs.iter().filter(|d| !d.has_driver() && d.needs_driver()).count();
        println!("  devices    {} ({} with a driver, {} without)", devs.len(), with, missing);
        println!("  disks      {}", dev::block::list().iter().filter(|d| d.parent.is_none()).count());
        if let Some((pct, plugged)) = crate::acpi::power::battery() {
            println!("  battery    {pct}%{}", if plugged { " (on mains)" } else { "" });
        }
        if let Some(t) = crate::acpi::power::hottest() {
            println!("  temperature {}.{} °C", t / 10, t % 10);
        }
        match crate::sleep::available() {
            Ok(()) => match crate::sleep::count() {
                0 => println!("  sleep      ready"),
                n => println!("  sleep      ready, slept {n} time{}", if n == 1 { "" } else { "s" }),
            },
            Err(e) => println!("  sleep      {DIM}not available: {e}{RESET}"),
        }
        println!("  uptime     {} s", time::uptime_ms() / 1000);
    }

    fn acpi(&self, rest: &[&str]) {
        let waiting = || println!("{WARN}the firmware's AML is not running{RESET}");
        match rest.first().copied() {
            None => {
                if !crate::acpi::aml_ready() {
                    waiting();
                    return;
                }
                println!("{BOLD}AML{RESET}  {}", crate::acpi::aml_summary());
                if let Some((data, command, timeouts)) = crate::acpi::ec_info() {
                    println!("{BOLD}embedded controller{RESET}  ports {data:#x}/{command:#x}, {timeouts} timeouts");
                }
                let s = crate::acpi::power::state();
                match s.ac {
                    Some(true) => println!("{BOLD}power{RESET}  on mains"),
                    Some(false) => println!("{BOLD}power{RESET}  on battery"),
                    None => {}
                }
                for b in &s.batteries {
                    if !b.present {
                        println!("{BOLD}battery{RESET}  {DIM}{}: not inserted{RESET}", b.path);
                        continue;
                    }
                    let unit = if b.watts { "mWh" } else { "mAh" };
                    let what = if b.charging() {
                        "charging"
                    } else if b.discharging() {
                        "discharging"
                    } else {
                        "full or idle"
                    };
                    let pct = b.percent().map(|p| alloc::format!("{p}%")).unwrap_or_else(|| String::from("?"));
                    println!("{BOLD}battery{RESET}  {pct}, {what}  {DIM}{} {} {}{RESET}", b.vendor, b.model, b.kind);
                    let n = |v: Option<u32>| v.map(|x| x.to_string()).unwrap_or_else(|| String::from("?"));
                    println!("  now {} of {} {unit} (design {}), rate {}", n(b.remaining), n(b.full), n(b.design), n(b.rate));
                    if let Some(m) = b.minutes_left() {
                        println!("  {} h {:02} min {}", m / 60, m % 60, if b.charging() { "to full" } else { "left" });
                    }
                    if let Some(h) = b.health() {
                        println!("  health {h}%{}", b.cycles.map(|c| alloc::format!(", {c} cycles")).unwrap_or_default());
                    }
                }
                if let Some(open) = s.lid_open {
                    println!("{BOLD}lid{RESET}  {}", if open { "open" } else { "closed" });
                }
                for t in &s.thermal {
                    let crit = t.critical.map(|c| alloc::format!("  {DIM}critical {} °C{RESET}", c / 10)).unwrap_or_default();
                    println!("{BOLD}thermal{RESET}  {} {}.{} °C{crit}", t.path, t.temp / 10, t.temp % 10);
                }
            }
            Some("tree") => match crate::acpi::tree(rest.get(1).copied().unwrap_or("\\_SB")) {
                None => waiting(),
                Some(Err(e)) => println!("{ERR}{e}{RESET}"),
                Some(Ok(lines)) => {
                    for l in lines {
                        println!("{l}");
                    }
                }
            },
            Some("eval") => {
                let Some(path) = rest.get(1) else {
                    println!("usage: acpi eval <path> [numbers...]");
                    return;
                };
                let mut args = Vec::new();
                for a in &rest[2..] {
                    let v = match a.strip_prefix("0x") {
                        Some(h) => u64::from_str_radix(h, 16),
                        None => a.parse(),
                    };
                    match v {
                        Ok(v) => args.push(v),
                        Err(_) => {
                            println!("{ERR}{a}: not a number{RESET}");
                            return;
                        }
                    }
                }
                match crate::acpi::eval(path, args) {
                    None => waiting(),
                    Some(Ok(v)) => println!("{v}"),
                    Some(Err(e)) => println!("{ERR}{e}{RESET}"),
                }
            }
            Some(other) => println!("{ERR}acpi {other}: unknown action{RESET} {DIM}(help acpi){RESET}"),
        }
    }
}

fn hexdump(data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        print!("{DIM}{:08x}{RESET}  ", i * 16);
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => print!("{b:02x} "),
                None => print!("   "),
            }
        }
        print!(" ");
        for &b in chunk {
            print!("{}", if (0x20..0x7F).contains(&b) { b as char } else { '.' });
        }
        println!();
    }
}

/// The driver that handles a device for its parent (a USB hub is run by
/// the controller's driver): "ehci (hub)".
fn owner_driver(d: &dev::Device) -> String {
    let mut parent = d.info.parent;
    while let Some(p) = dev::get(parent) {
        if let Some(drv) = &p.driver {
            return if d.info.class == 0x09 { alloc::format!("{drv} (hub)") } else { drv.clone() };
        }
        if p.info.parent == parent {
            break;
        }
        parent = p.info.parent;
    }
    String::from("—")
}
