//! Apps and their sandbox: who a program runs as, running a program as an
//! app, and a self-test of what the sandbox lets an app do.
//!
//! ```text
//! sandbox                         the app this program runs as
//! sandbox run <app_id> <program>  run a program as that app, in its sandbox
//! sandbox test                    check the sandbox's rules and its popup
//! sandbox ask <perm>              ask for a permission as this app
//! ```

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use justrt::console::color::*;
use justrt::ipc::Port;
use justrt::{app, eprintln, fs, println, process, Error};

justrt::main!(main);

/// The app the test runs as.
const TEST_ID: &str = "dev.justos.prova";
/// The app the popup's checks run as: it starts with no answers at all.
const FRESH_ID: &str = "dev.justos.nuova";
/// An app from the Negozio whose manifest lists a permission
/// (`permissions=files:Immagini`); the session installs it first.
const STORE_ID: &str = "dev.justos.disegno";
/// The Linux program the checks ask as, and the app the sandbox makes it.
const FIREFOX: &str = "sys:/usr/lib/firefox-esr/firefox-esr";
const FIREFOX_ID: &str = "linux.firefox-esr";

fn main(args: &str) -> i32 {
    let words = justrt::split_args(args);
    let w = |i: usize| words.get(i).map(String::as_str).unwrap_or("");
    match w(0) {
        "" | "whoami" => {
            match app::id() {
                Some(id) => println!("{id} {DIM}({}){RESET}", app::data_dir()),
                None => println!("{DIM}no app: a system program, without a sandbox{RESET}"),
            }
            0
        }
        "run" if !w(2).is_empty() => {
            if let Err(e) = app::enter(w(1), w(1)) {
                eprintln!("sandbox: cannot run as {}: {}", w(1), e.as_str());
                return 1;
            }
            let rest = words.get(3..).map(|r| r.join(" ")).unwrap_or_default();
            match process::spawn(w(2), &rest).and_then(process::wait) {
                Ok(code) => code as i32,
                Err(e) => {
                    eprintln!("sandbox: {}: {}", w(2), e.as_str());
                    1
                }
            }
        }
        "test" => test(),
        "probe" => probe(),
        // For the test: ask twice for a permission as the fresh app (or as
        // this one). The exit code has a bit for each answer, 1 and 2.
        "ask" if !w(1).is_empty() => ask_twice(w(1)),
        // ...or list a folder of the home as it (1: it could).
        "list" if !w(1).is_empty() => {
            if app::enter(FRESH_ID, "Nuova").is_err() {
                return 9;
            }
            i32::from(fs::list_dir(w(1)).is_ok())
        }
        // ...or as a Linux program, the way the PulseAudio server asks.
        "ask-as" if !w(2).is_empty() => {
            let req = format!("{{\"request\":\"{}\",\"program\":\"{}\"}}", w(2), w(1));
            i32::from(permissions(&req).is_ok_and(|a| a.contains("\"granted\":true")))
        }
        // ...or send a notification as the fresh app.
        "notify" => {
            if app::enter(FRESH_ID, "Nuova").is_err() {
                return 9;
            }
            let sent = Port::lookup("just.notify").and_then(|p| p.call(b"{\"title\":\"Nuova\",\"text\":\"ciao\"}"));
            i32::from(sent.is_ok_and(|a| String::from_utf8_lossy(&a).contains("\"ok\":true")))
        }
        // What an app was told about a permission, without asking: 0 not
        // asked, 1 allowed, 2 denied.
        "state" if !w(1).is_empty() => match app::permission(w(1)) {
            Ok(None) => 0,
            Ok(Some(true)) => 1,
            Ok(Some(false)) => 2,
            Err(_) => 3,
        },
        // For the test: does the program run as this app?
        "is" => i32::from(app::id().as_deref() != Some(w(1))),
        // For `updates test`: an app that stays open for a while.
        "wait" => {
            justrt::time::sleep_ms(w(1).parse().unwrap_or(1000));
            0
        }
        _ => {
            println!("usage: sandbox | sandbox run <app_id> <program> [args] | sandbox test");
            2
        }
    }
}

fn permissions(request: &str) -> Result<String, Error> {
    let answer = Port::lookup("just.permissions")?.call(request.as_bytes())?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Gives the test app some answers, lets `sandbox probe` run as it, then
/// forgets them.
fn test() -> i32 {
    // Answers given ahead, so the probe never waits for a dialog.
    for (perm, allow) in [("files:Musica", true), ("files:Scaricati", false), ("files:ram:", false), ("camera", false)] {
        let req = format!("{{\"set\":\"{perm}\",\"app\":\"{TEST_ID}\",\"name\":\"Prova\",\"allow\":{allow}}}");
        if !permissions(&req).is_ok_and(|a| a.contains("\"ok\":true")) {
            eprintln!("sandbox: just.permissions did not take {perm}");
            return 1;
        }
    }
    let failed = match process::spawn("sandbox", "probe").and_then(process::wait) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("sandbox: cannot start the probe: {}", e.as_str());
            return 1;
        }
    };
    let forgot = permissions(&format!("{{\"forget\":\"{TEST_ID}\"}}")).is_ok_and(|a| a.contains("true"));
    let listed = permissions("{\"list\":true}").map(|a| a.contains(TEST_ID));
    let mut failed = failed + i64::from(popup());
    if forgot && listed == Ok(false) {
        println!("  {GREEN}ok{RESET}    answers forgotten");
    } else {
        println!("  {RED}fail{RESET}  answers forgotten {listed:?}");
        failed += 1;
    }
    if failed == 0 {
        println!("{ACCENT}sandbox: all checks passed{RESET}");
        0
    } else {
        println!("{RED}sandbox: {failed} check(s) failed{RESET}");
        1
    }
}

/// Runs as the test app and tries what an app may and may not do. The exit
/// code is the number of failures.
fn probe() -> i32 {
    let mut failed = 0;
    let mut check = |what: &str, ok: bool, detail: String| {
        if ok {
            println!("  {GREEN}ok{RESET}    {what} {DIM}{detail}{RESET}");
        } else {
            println!("  {RED}fail{RESET}  {what} {detail}");
            failed += 1;
        }
    };
    let denied = |r: Result<(), Error>| r == Err(Error::Denied);
    let entered = app::enter(TEST_ID, "Prova");
    check("enter", entered.is_ok(), format!("{entered:?}"));
    if entered.is_err() {
        return 99;
    }
    let id = app::id();
    check("app id", id.as_deref() == Some(TEST_ID), format!("{id:?}"));
    let home = format!("sys:/home/.apps/{TEST_ID}");
    let cwd = fs::current_dir();
    check("starts in its folder", cwd == home, cwd.clone());

    // Its own folder is all its.
    let own = fs::write("nota.txt", b"ciao").and_then(|_| fs::read_to_string("nota.txt"));
    check("own folder: write, read", own.as_deref() == Ok("ciao"), String::new());
    let sub = fs::make_dir("cartella").or_else(|e| if e == Error::Exists { Ok(()) } else { Err(e) });
    check("own folder: make a folder", sub.is_ok() && fs::write("cartella/a", b"1").is_ok(), format!("{sub:?}"));
    check("own folder: list", fs::list_dir(".").is_ok_and(|l| l.iter().any(|e| e.name == "nota.txt")), String::new());
    check("own folder, any case", fs::read_to_string(&format!("SYS:/HOME/.APPS/{}/NOTA.TXT", TEST_ID.to_uppercase())).is_ok(), String::new());

    // The system: programs and fonts to read, never to change; nothing else.
    check("read the programs", fs::File::open("sys:/apps/sandbox.app").is_ok(), String::new());
    check("not change them", denied(fs::write("sys:/apps/sandbox.app", b"x")), String::new());
    check("not read the settings", denied(fs::read("sys:/system/settings").map(|_| ())), String::new());
    check("not read the permissions", denied(fs::read("sys:/system/permissions.json").map(|_| ())), String::new());
    check("not list the home", denied(fs::list_dir("sys:/home").map(|_| ())), String::new());
    check("not read a file in the home", denied(fs::read("sys:/home/welcome.txt").map(|_| ())), String::new());
    check("not climb out", denied(fs::read("../../welcome.txt").map(|_| ())), String::new());
    check("not another app's folder", denied(fs::write("sys:/home/.apps/dev.justos.altra/x", b"x")), String::new());
    check("not other disks", denied(fs::list_dir("ram:/").map(|_| ())), String::new());

    // Folders of the home, as the user said.
    let music = fs::list_dir("sys:/home/Musica").map(|_| ());
    check("Musica: allowed", music.is_ok(), format!("{music:?}"));
    check("Scaricati: denied", denied(fs::write("sys:/home/Scaricati/x.txt", b"x")), String::new());
    check("what the user said", app::permission("files:Musica") == Ok(Some(true)) && app::permission("files:Scaricati") == Ok(Some(false)), String::new());
    check("not asked yet", app::permission("files:Documenti") == Ok(None), String::new());
    check("camera: denied", app::request("camera") == Ok(false), String::new());

    // Ports carry the app's name, and the table is the system's.
    check("own port", Port::create(&format!("{TEST_ID}.eco")).is_ok(), String::new());
    check("not a system port", Port::create("just.finto").err() == Some(Error::Denied), String::new());
    check("not another app's port", Port::create("org.altra.app").err() == Some(Error::Denied), String::new());
    check("not the whole table", permissions("{\"list\":true}").is_ok_and(|a| a.contains("error")), String::new());

    // Once an app, always that app; what it starts is that app too.
    check("not become another app", app::enter("org.altra.app", "Altra") == Err(Error::Denied), String::new());
    let child = process::spawn("sandbox", &format!("is {TEST_ID}")).and_then(process::wait);
    check("programs it starts", child == Ok(0), format!("{child:?}"));
    let _ = fs::remove("cartella/a");
    let _ = fs::remove("cartella");
    let _ = fs::remove("nota.txt");
    failed
}

/// As the fresh app, asks for `perm` twice in a row. Bit 1 of the exit
/// code is the first answer, bit 2 the second.
fn ask_twice(perm: &str) -> i32 {
    if app::enter(FRESH_ID, "Nuova").is_err() {
        return 9;
    }
    let first = app::request(perm) == Ok(true);
    let second = app::request(perm) == Ok(true);
    i32::from(first) + 2 * i32::from(second)
}

/// The question the popup asks now, if one: (app id, name, permission).
fn pending() -> Option<(String, String, String)> {
    let a = permissions("{\"pending\":true}").ok()?;
    let v = justjson::parse(&a)?;
    let p = v.get("pending");
    p.get("perm").as_str().map(|perm| (String::from(p.get("app").str()), String::from(p.get("name").str()), String::from(perm)))
}

/// Waits up to `ms` for the popup to ask something.
fn wait_pending(ms: u64) -> Option<(String, String, String)> {
    let until = justrt::time::uptime_ms() + ms;
    loop {
        if let Some(p) = pending() {
            return Some(p);
        }
        if justrt::time::uptime_ms() > until {
            return None;
        }
        justrt::time::sleep_ms(50);
    }
}

/// Answers the popup as a click on its button would.
fn answer(choice: &str) -> bool {
    permissions(&format!("{{\"answer\":\"{choice}\"}}")).is_ok_and(|a| a.contains("\"ok\":true"))
}

/// What the table keeps for `perm` of app `id`: "allow", "denied_once",
/// "deny", or "" (nothing).
fn kept(id: &str, perm: &str) -> String {
    let Some(v) = permissions("{\"list\":true}").ok().and_then(|a| justjson::parse(&a)) else { return String::from("?") };
    let found = v.get("apps").as_array().iter().find(|a| a.get("id").str() == id).map(|a| String::from(a.get("perms").get(perm).str()));
    found.unwrap_or_default()
}

/// Starts `sandbox <args>`; if the popup is to ask (`asks`: the app's name
/// and the permission), answers it with `choice`. Returns the program's
/// exit code, and whether the popup asked as expected (or did not ask, if
/// nothing was expected).
fn run_and_answer(args: &str, asks: Option<(&str, &str)>, choice: &str) -> (i64, bool) {
    let Ok(pid) = process::spawn("sandbox", args) else { return (-1, false) };
    let as_expected = match asks {
        Some((name, perm)) => match wait_pending(5000) {
            Some((_, n, p)) => n == name && p == perm && answer(choice),
            None => false,
        },
        // Nothing to ask: the program ends without a popup.
        None => wait_pending(1500).is_none() || !answer("dismiss"),
    };
    (process::wait(pid).unwrap_or(-1), as_expected)
}

/// The phone-style rules: nothing at first, the popup, "Non consentire"
/// once and twice, "Solo questa volta", the settings, Linux programs and
/// notifications. Returns the number of failures.
fn popup() -> i32 {
    let mut failed = 0;
    let mut check = |what: &str, ok: bool, detail: String| {
        if ok {
            println!("  {GREEN}ok{RESET}    {what} {DIM}{detail}{RESET}");
        } else {
            println!("  {RED}fail{RESET}  {what} {detail}");
            failed += 1;
        }
    };
    let _ = permissions(&format!("{{\"forget\":\"{FRESH_ID}\"}}"));
    let _ = permissions(&format!("{{\"forget\":\"{FIREFOX_ID}\"}}"));

    // A fresh app has nothing, and neither has an app from the Negozio
    // whose manifest lists a permission: the manifest only says what it
    // may ask for.
    let fresh = ["camera", "microphone", "location", "network", "notifications", "files:Immagini", "files:ram:"]
        .iter()
        .map(|p| process::spawn("sandbox", &format!("run {FRESH_ID} sandbox state {p}")).and_then(process::wait))
        .all(|c| c == Ok(0));
    check("a fresh app has no permission", fresh, String::new());
    let store = process::spawn("sandbox", &format!("run {STORE_ID} sandbox state files:Immagini")).and_then(process::wait);
    check("an installed app has none either", store == Ok(0) && kept(STORE_ID, "files:Immagini").is_empty(), format!("{store:?}"));

    // The first request shows the popup and waits; "Non consentire"
    // denies it, and the same run is not asked again.
    let (code, asked) = run_and_answer("ask microphone", Some(("Nuova", "microphone")), "deny");
    check("the popup asks, the app waits", asked, String::new());
    check("Non consentire: denied, no second popup", code == 0, format!("{code}"));
    check("denied once", kept(FRESH_ID, "microphone") == "denied_once", kept(FRESH_ID, "microphone"));

    // The next run asks once more; a second "Non consentire" is for good.
    let (code, asked) = run_and_answer("ask microphone", Some(("Nuova", "microphone")), "deny");
    check("the next run asks again", asked && code == 0, format!("{code}"));
    check("denied twice: for good", kept(FRESH_ID, "microphone") == "deny", kept(FRESH_ID, "microphone"));
    let (code, quiet) = run_and_answer("ask microphone", None, "");
    check("then no more popups", quiet && code == 0, format!("{code}"));

    // The settings switch it on, and off again (what Impostazioni does).
    let set = |allow: bool| {
        let req = format!("{{\"set\":\"microphone\",\"app\":\"{FRESH_ID}\",\"name\":\"Nuova\",\"allow\":{allow}}}");
        permissions(&req).is_ok_and(|a| a.contains("\"ok\":true"))
    };
    let on = set(true);
    let (code, quiet) = run_and_answer("ask microphone", None, "");
    check("allowed in the settings", on && quiet && code == 3, format!("{code}"));
    let off = set(false);
    let (code, quiet) = run_and_answer("ask microphone", None, "");
    check("revoked in the settings", off && quiet && code == 0, format!("{code}"));

    // "Solo questa volta": for this run only; a closed popup counts as no
    // answer, and the next run asks again.
    let (code, asked) = run_and_answer("ask camera", Some(("Nuova", "camera")), "once");
    check("Solo questa volta: this run", asked && code == 3, format!("{code}"));
    let (code, asked) = run_and_answer("ask camera", Some(("Nuova", "camera")), "dismiss");
    check("closed popup: denied, not kept", asked && code == 0 && kept(FRESH_ID, "camera").is_empty(), format!("{code}"));
    let (code, asked) = run_and_answer("ask camera", Some(("Nuova", "camera")), "allow");
    check("Consenti: for good", asked && code == 3 && kept(FRESH_ID, "camera") == "allow", format!("{code}"));

    // A folder of the home asks in the same popup, from the file system.
    let (code, asked) = run_and_answer("list sys:/home/Documenti", Some(("Nuova", "files:Documenti")), "deny");
    check("a folder asks too", asked && code == 0, format!("{code}"));

    // A Linux program asks under the name its .desktop file gives it.
    let (code, asked) = run_and_answer(&format!("ask-as {FIREFOX} microphone"), Some(("Firefox", "microphone")), "deny");
    check("Firefox asks for the microphone", asked && code == 0, format!("{code}"));
    check("as linux.firefox-esr", kept(FIREFOX_ID, "microphone") == "denied_once", kept(FIREFOX_ID, "microphone"));

    // Notifications need the permission too.
    let (code, asked) = run_and_answer("notify", Some(("Nuova", "notifications")), "allow");
    check("notifications ask first", asked && code == 1, format!("{code}"));

    let _ = permissions(&format!("{{\"forget\":\"{FRESH_ID}\"}}"));
    let _ = permissions(&format!("{{\"forget\":\"{FIREFOX_ID}\"}}"));
    failed
}
