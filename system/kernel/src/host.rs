//! The computer's name: what the user called it in the first-start setup
//! or in Impostazioni → Generale ("justOS-PC", "Anna-PC"), kept with the
//! other settings in `sys:/system/settings` (`device-name=`), and the host
//! name made from it in lower case ("anna-pc"). The host name is what DHCP
//! sends to the network (option 12), what Linux programs see (uname,
//! gethostname, /etc/hostname, /proc/sys/kernel/hostname) and what the
//! shell's `hostname` and `sysinfo` show.
//!
//! A name is a valid host name label: 1 to 63 letters (without accents),
//! digits and hyphens, not starting or ending with a hyphen.

use crate::fs;
use crate::sync::IrqLock;
use alloc::string::String;

/// The name until the user gives one.
pub const DEFAULT: &str = "justOS-PC";
/// The longest name (a host name label).
pub const MAX: usize = 63;

static NAME: IrqLock<String> = IrqLock::new(String::new());

/// The computer's name as the user wrote it.
pub fn name() -> String {
    let n = NAME.lock();
    if n.is_empty() {
        String::from(DEFAULT)
    } else {
        n.clone()
    }
}

/// The host name: the name in lower case.
pub fn hostname() -> String {
    name().to_ascii_lowercase()
}

/// Whether `name` can be the computer's name.
pub fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Why `name` cannot be the computer's name (None: it can).
pub fn problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Scrivi un nome")
    } else if name.len() > MAX {
        Some("Al massimo 63 caratteri")
    } else if name.starts_with('-') || name.ends_with('-') {
        Some("Non può iniziare o finire con un trattino")
    } else if !valid(name) {
        Some("Solo lettere senza accenti, cifre e trattini")
    } else {
        None
    }
}

/// Changes the computer's name (the settings call it when they load or
/// change); an invalid name is ignored. When it changes while the network
/// is up, DHCP is told the new one.
pub fn set(name: &str) {
    let before = hostname();
    *NAME.lock() = String::from(if valid(name) { name } else { "" });
    if hostname() != before {
        crate::net::announce_name();
    }
}

/// The letter a typed character stands for in a name: letters without
/// accents, digits and hyphens; a space (or an underscore) is a hyphen.
pub fn name_char(ch: char) -> Option<char> {
    let plain = match ch {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'A',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'È' | 'É' | 'Ê' | 'Ë' => 'E',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
        'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => 'O',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        ' ' | '_' | '.' => '-',
        ch => ch,
    };
    (plain.is_ascii_alphanumeric() || plain == '-').then_some(plain)
}

/// A name for the computer of `user` ("Anna Rossi" → "Anna-Rossi-PC");
/// [`DEFAULT`] when the user's name has no letters to make one from.
pub fn suggest(user: &str) -> String {
    let mut base = String::new();
    for ch in user.trim().chars().filter_map(name_char) {
        if !(ch == '-' && (base.is_empty() || base.ends_with('-'))) {
            base.push(ch);
        }
    }
    let base = base.trim_end_matches('-');
    if base.is_empty() {
        return String::from(DEFAULT);
    }
    let mut out: String = base.chars().take(MAX - 3).collect();
    while out.ends_with('-') {
        out.pop();
    }
    out.push_str("-PC");
    out
}

/// Reads the name from the settings (at start, before the network: DHCP
/// sends it from its first message).
pub fn load() {
    let Ok(bytes) = fs::abs("sys:/system/settings").and_then(|p| fs::read_file(&p)) else { return };
    for line in String::from_utf8_lossy(&bytes).lines() {
        if let Some(v) = line.strip_prefix("device-name=") {
            let v = v.trim();
            if valid(v) {
                *NAME.lock() = String::from(v);
            }
        }
    }
}
