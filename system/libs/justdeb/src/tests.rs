//! Versions (dpkg's own test cases), relations, the index, signatures
//! (Debian's real ones, and a test key's) and the resolver on a small
//! made-up archive.

use crate::control::{self, Dep};
use crate::index::{self, Index, Pkg};
use crate::resolve::{self, Problem, LOCAL};
use crate::version::{compare, Op};
use crate::{pgp, release};
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering::{self, Equal, Greater, Less};

#[test]
fn versions() {
    let cases: &[(&str, &str, Ordering)] = &[
        ("1.0", "1.0", Equal),
        ("1.0", "1.0-0", Equal),
        ("0:1.0", "1.0", Equal),
        ("1.0-1", "1.0-2", Less),
        ("1.2.3", "1.2.10", Less),
        ("1.10", "1.9", Greater),
        ("1:0.1", "2.0", Greater),
        ("2:1", "10:0", Less),
        ("1.0~rc1", "1.0", Less),
        ("1.0~~", "1.0~", Less),
        ("1.0~", "1.0", Less),
        ("1.0", "1.0+b1", Less),
        ("1.0+b1", "1.0.1", Less),
        ("1.0a", "1.0", Greater),
        ("1.0a", "1.0+", Less),
        ("1.0", "1.0.0", Less),
        ("1.001", "1.1", Equal),
        ("2.41-12", "2.41-12+deb13u1", Less),
        ("2.41-12+deb13u1", "2.41-12+deb13u2", Less),
        ("7.4.0-1", "7.4.0-1~bpo13+1", Greater),
        ("1.2-3-4", "1.2-3-5", Less),
        ("1:2.3-1", "1:2.10-1", Less),
        ("0.9.9", "1", Less),
        ("a", "b", Less),
        ("1.0-1", "1.0-1.1", Less),
        ("3.0.0~beta2", "3.0.0~beta10", Less),
    ];
    for &(a, b, want) in cases {
        assert_eq!(compare(a, b), want, "{a} vs {b}");
        assert_eq!(compare(b, a), want.reverse(), "{b} vs {a}");
    }
    assert!(Op::GreaterEq.holds("2.41-12", "2.34"));
    assert!(!Op::Less.holds("2.0", "2.0"));
    assert!(Op::LessEq.holds("2.0", "2.0"));
    assert!(Op::Greater.holds("2.0.1", "2.0"));
    assert_eq!(Op::parse("<"), Some(Op::LessEq));
    assert_eq!(Op::parse(">"), Some(Op::GreaterEq));
}

fn dep(name: &str, arch: Option<&str>, rule: Option<(Op, &str)>) -> Dep {
    Dep { name: name.into(), arch: arch.map(String::from), rule: rule.map(|(o, v)| (o, String::from(v))) }
}

#[test]
fn relations() {
    let r = control::relations("libc6 (>= 2.34), perl:any, a (<< 1:2~) | b (= 3) | c, d [amd64], e [!amd64], f [i386 arm64] | g, h (>=1.0)<!nocheck>, ");
    assert_eq!(r.len(), 6);
    assert_eq!(r[0], [dep("libc6", None, Some((Op::GreaterEq, "2.34")))]);
    assert_eq!(r[1], [dep("perl", Some("any"), None)]);
    assert_eq!(r[2], [dep("a", None, Some((Op::Less, "1:2~"))), dep("b", None, Some((Op::Eq, "3"))), dep("c", None, None)]);
    assert_eq!(r[3], [dep("d", None, None)]);
    // e is not for amd64 (the whole group goes), f neither (only g stays).
    assert_eq!(r[4], [dep("g", None, None)]);
    assert_eq!(r[5], [dep("h", None, Some((Op::GreaterEq, "1.0")))]);
    // A multi-line field (as dpkg's status has it).
    let r = control::relations("a,\n b (<= 2),\n c");
    assert_eq!(r.len(), 3);
    assert_eq!(control::group_text(&control::relations("x (>= 1) | y:any")[0]), "x (>= 1) | y:any");
}

#[test]
fn paragraphs_and_fields() {
    let text = "Package: a\nDescription: first\n second line\n .\n third\n\n\n \nPackage: b\r\nDepends: x,\r\n y\r\n\r\nPackage: c";
    let pars: Vec<&str> = control::paragraphs(text).collect();
    assert_eq!(pars.len(), 3);
    assert_eq!(control::field(pars[0], "description").unwrap(), "first\nsecond line\n.\nthird");
    assert_eq!(control::field(pars[1], "Depends").unwrap(), "x,\ny");
    assert_eq!(control::field(pars[2], "Package").unwrap(), "c");
    assert!(control::field(pars[2], "Version").is_none());
}

const ARCHIVE: &str = "\
Package: app
Version: 1.0
Architecture: amd64
Pre-Depends: helper
Depends: libfoo (>= 1.2) | libbar, mail-transport-agent, perl:any, libc6 (>= 2.34)
Filename: pool/main/a/app/app_1.0_amd64.deb
Size: 1000
SHA256: aa
Installed-Size: 10
Description: an application
 with a longer description
 that compact() drops.

Package: libfoo
Version: 1.0
Architecture: amd64
Filename: pool/main/libf/libfoo_1.0_amd64.deb
Size: 100

Package: libfoo
Version: 1.3
Architecture: amd64
Depends: libc6 (>= 2.34)
Filename: pool/main/libf/libfoo_1.3_amd64.deb
Size: 130

Package: libbar
Version: 9
Architecture: amd64

Package: postfix
Version: 3.10
Architecture: amd64
Provides: mail-transport-agent
Conflicts: mail-transport-agent
Filename: pool/main/p/postfix.deb
Size: 500

Package: exim4-daemon-light
Version: 4.98
Architecture: amd64
Provides: mail-transport-agent
Conflicts: mail-transport-agent
Filename: pool/main/e/exim.deb
Size: 400

Package: perl
Version: 5.40.1-6
Architecture: amd64
Multi-Arch: allowed
Pre-Depends: perl-base (= 5.40.1-6)
Filename: pool/main/p/perl.deb
Size: 300

Package: perl-base
Version: 5.40.1-6
Architecture: amd64
Essential: yes
Filename: pool/main/p/perl-base.deb
Size: 1700

Package: helper
Version: 2
Architecture: all
Depends: perl
Filename: pool/main/h/helper.deb
Size: 20

Package: i386only
Version: 1
Architecture: i386

Package: bad
Version: 1
Architecture: amd64
Conflicts: oldthing (<< 2)

Package: newlib
Version: 1.5
Architecture: amd64

Package: libz
Version: 2.0
Architecture: amd64
Size: 50

Package: needsz
Version: 1
Architecture: amd64
Depends: libz (>= 2)

Package: virtuser
Version: 1
Architecture: amd64
Depends: virt (>= 2)

Package: virtgiver
Version: 1
Architecture: amd64
Provides: virt (= 3)

Package: plainuser
Version: 1
Architecture: amd64
Depends: notallowed:any

Package: notallowed
Version: 1
Architecture: amd64

Package: missinguser
Version: 1
Architecture: amd64
Depends: nothere (>= 1) | alsonot
";

const STATUS: &str = "\
Package: libc6
Status: install ok installed
Version: 2.41-12
Architecture: amd64
Multi-Arch: same

Package: oldthing
Status: install ok installed
Version: 1
Architecture: amd64

Package: legacy
Status: install ok installed
Version: 1
Architecture: all
Breaks: newlib (<< 2)

Package: libz
Status: install ok installed
Version: 1.0
Architecture: amd64

Package: removed
Status: deinstall ok config-files
Version: 1
Architecture: amd64
";

fn index() -> Index {
    let mut i = Index::new();
    i.add(index::compact(ARCHIVE), 0);
    i
}

fn names(p: &resolve::Plan) -> Vec<&str> {
    p.packages.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn index_and_compact() {
    let c = index::compact(ARCHIVE);
    assert!(!c.contains("that compact() drops"));
    assert!(!c.contains("i386only"));
    let i = index();
    assert_eq!(i.versions("libfoo").iter().map(|p| p.version.as_str()).collect::<Vec<_>>(), ["1.3", "1.0"]);
    assert_eq!(i.candidate("app").unwrap().summary, "an application");
    assert_eq!(i.providers("mail-transport-agent").len(), 2);
    assert!(i.knows("virt") && !i.knows("nothere"));
    let installed = index::installed(STATUS);
    assert_eq!(installed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["libc6", "oldthing", "legacy", "libz"]);
}

#[test]
fn resolver_plan() {
    let (i, installed) = (index(), index::installed(STATUS));
    let mut app = i.candidate("app").unwrap();
    app.source = LOCAL;
    let plan = resolve::plan(&installed, &i, alloc::vec![app]).unwrap();
    let n = names(&plan);
    // The newest libfoo, the first provider by name, perl through ":any"
    // (Multi-Arch: allowed); libc6 is installed.
    assert!(n.contains(&"libfoo") && plan.packages.iter().any(|p| p.name == "libfoo" && p.version == "1.3"));
    assert!(n.contains(&"exim4-daemon-light") && !n.contains(&"postfix"));
    assert!(n.contains(&"perl") && n.contains(&"perl-base") && n.contains(&"helper"));
    assert!(!n.contains(&"libc6") && !n.contains(&"libbar"));
    // Dependencies first.
    let at = |name: &str| n.iter().position(|x| *x == name).unwrap();
    assert!(at("perl-base") < at("perl") && at("perl") < at("helper") && at("helper") < at("app"));
    // perl pre-depends on perl-base, app on helper: three batches.
    let batch_of = |name: &str| plan.batches.iter().position(|b| b.contains(&at(name))).unwrap();
    assert!(batch_of("perl-base") < batch_of("perl"));
    assert!(batch_of("helper") < batch_of("app"));
    assert_eq!(plan.batches.iter().map(Vec::len).sum::<usize>(), plan.packages.len());
    // The local package is not downloaded.
    assert_eq!(plan.download_size(), 130 + 400 + 300 + 1700 + 20);
    assert!(plan.upgrades.is_empty());
}

#[test]
fn resolver_problems() {
    let (i, installed) = (index(), index::installed(STATUS));
    let want = |name: &str| resolve::plan(&installed, &i, alloc::vec![i.candidate(name).unwrap()]);
    assert_eq!(want("bad").unwrap_err(), Problem::Conflict { new: "bad".into(), other: "oldthing".into(), breaks: false });
    // An installed package's Breaks counts too.
    assert_eq!(want("newlib").unwrap_err(), Problem::Conflict { new: "newlib".into(), other: "legacy".into(), breaks: true });
    assert!(matches!(want("missinguser").unwrap_err(), Problem::Missing { what, .. } if what == "nothere (>= 1) | alsonot"));
    // ":any" needs Multi-Arch: allowed.
    assert!(matches!(want("plainuser").unwrap_err(), Problem::Missing { .. }));
    let i386 = Pkg { name: "x".into(), version: "1".into(), arch: "i386".into(), ..Pkg::default() };
    assert!(matches!(resolve::plan(&installed, &i, alloc::vec![i386]), Err(Problem::Arch { .. })));
    // A versioned dependency met by a versioned provide.
    assert_eq!(names(&want("virtuser").unwrap()), ["virtgiver", "virtuser"]);
}

#[test]
fn resolver_upgrades() {
    let i = index();
    let mut installed = index::installed(STATUS);
    let plan = resolve::plan(&installed, &i, alloc::vec![i.candidate("needsz").unwrap()]).unwrap();
    assert_eq!(names(&plan), ["libz", "needsz"]);
    assert_eq!(plan.upgrades, [(String::from("libz"), String::from("1.0"))]);
    // Not when an installed package needs the old one.
    installed.extend(index::installed("Package: zuser\nStatus: install ok installed\nVersion: 1\nArchitecture: amd64\nDepends: libz (<< 2)\n"));
    let e = resolve::plan(&installed, &i, alloc::vec![i.candidate("needsz").unwrap()]).unwrap_err();
    assert_eq!(e, Problem::Upgrade { upgraded: "libz".into(), installed: "zuser".into(), what: "libz (<< 2)".into() });
    assert_eq!(resolve::by_name(&i, "virt").unwrap().name, "virtgiver");
    assert!(resolve::by_name(&i, "mail-transport-agent").is_err());
    assert!(resolve::by_name(&i, "nothere").is_err());
}

#[test]
fn release_and_sources() {
    let text = "Origin: Debian\nSuite: stable\nCodename: trixie\nDate: Sat, 12 Sep 2026 07:55:41 UTC\nValid-Until: Sat, 19 Sep 2026 07:55:41 +0100\nAcquire-By-Hash: yes\nSHA256:\n 42c23aaca08e8225ed0449360724b92531952cbe4ce3bbf189298a7108f40a85 13332733 main/binary-amd64/Packages.gz\n 4f2c68d67001d595fbd343f6dbad44468953a51c4d9dec3a4803249f6e295940 56620099 main/binary-amd64/Packages\n 7778d3e3f303b7ddb8ce0fe7c8d57473a076c6bf2e8f241f75421d2396352498  9678380 main/binary-amd64/Packages.xz\n";
    let r = release::Release::parse(text);
    assert_eq!(r.codename, "trixie");
    assert!(r.by_hash);
    assert_eq!(r.date, 1_789_199_741);
    assert_eq!(r.valid_until, 1_789_199_741 + 7 * 86_400 - 3600);
    assert_eq!(r.packages_index("main").unwrap().path, "main/binary-amd64/Packages.gz");
    assert_eq!(r.packages_index("main").unwrap().size, 13_332_733);
    let plain = release::Release::parse("SHA256:\n 7f1dd99bbd1a0416d39499892ec12ff38d80637029114d234c38972bba8ae041     2351 main/binary-amd64/Packages\n");
    assert_eq!(plain.packages_index("main").unwrap().path, "main/binary-amd64/Packages");
    assert!(plain.packages_index("contrib").is_none());
    assert_eq!(release::parse_date("Thu, 01 Jan 1970 00:00:00 UTC"), Some(0));
    assert_eq!(release::parse_date("Sun, 29 Feb 2032 12:00:00 UTC"), Some(1_961_668_800));

    let s = release::sources_list("# comment\ndeb https://deb.debian.org/debian trixie main contrib\ndeb-src https://x y main\ndeb [signed-by=/k.pgp arch=amd64] http://10.0.2.2:8000/ test main # end\n");
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].components, ["main", "contrib"]);
    assert_eq!(s[1].uri, "http://10.0.2.2:8000/");
    assert_eq!(s[1].dists(), "http://10.0.2.2:8000/dists/test");
    assert_eq!(s[1].pool("pool/main/a.deb"), "http://10.0.2.2:8000/pool/main/a.deb");
    let s = release::sources_deb822("Types: deb\nURIs: https://deb.debian.org/debian\nSuites: trixie trixie-updates\nComponents: main\n\nTypes: deb-src\nURIs: x\nSuites: y\nComponents: main\n");
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].suite, "trixie-updates");
}

static UPDATES: &str = include_str!("../tests/data/updates-InRelease");

#[test]
fn debian_signatures() {
    let rsa = pgp::keyring(include_bytes!("../tests/data/trixie-automatic.pgp"));
    let ed = pgp::keyring(include_bytes!("../tests/data/trixie-stable.pgp"));
    assert!(rsa.iter().all(|k| matches!(k, pgp::Key::Rsa { n, .. } if n.len() == 512)) && rsa.len() == 2);
    assert!(matches!(&ed[..], [pgp::Key::Ed25519(_)]));
    // The armored file holds the same key.
    assert_eq!(pgp::keyring(include_bytes!("../tests/data/trixie-stable.asc")), ed);
    // trixie-updates is signed by the archive keys of 12 and 13 (RSA).
    let body = pgp::verify_cleartext(UPDATES, &rsa).unwrap();
    assert!(body.starts_with("Origin: Debian\n"));
    let r = release::Release::parse(&body);
    assert_eq!(r.codename, "trixie-updates");
    assert!(r.valid_until > r.date && r.packages_index("main").is_some());
    assert_eq!(pgp::verify_cleartext(UPDATES, &ed), Err(pgp::Error::NoGoodSignature));
    // One changed byte in the signed text.
    let changed = UPDATES.replacen("Suite: stable-updates", "Suite: stable-updatez", 1);
    assert_ne!(changed, UPDATES);
    assert_eq!(pgp::verify_cleartext(&changed, &rsa), Err(pgp::Error::NoGoodSignature));
    // An added line.
    let added = UPDATES.replacen("Acquire-By-Hash: yes\n", "Acquire-By-Hash: yes\nX: y\n", 1);
    assert_eq!(pgp::verify_cleartext(&added, &rsa), Err(pgp::Error::NoGoodSignature));
    assert_eq!(pgp::verify_cleartext("Origin: Debian\n", &rsa), Err(pgp::Error::NotSigned));
}

#[test]
fn ed25519_signature() {
    // Made with gpg (an Ed25519 key, SHA-512, dash-escaped lines).
    let text = include_str!("../tests/data/test-InRelease");
    let keys = pgp::keyring(include_bytes!("../tests/data/test-ed25519.pgp"));
    let body = pgp::verify_cleartext(text, &keys).unwrap();
    assert!(body.contains("\n- a line that starts with a dash\n-----a line that looks like armor\n"));
    assert_eq!(release::Release::parse(&body).suite, "test");
    let changed = text.replacen("Suite: test", "Suite: tesT", 1);
    assert!(pgp::verify_cleartext(&changed, &keys).is_err());
    // Trailing spaces are not signed: adding some changes nothing.
    let spaced = text.replacen("Suite: test\n", "Suite: test  \n", 1);
    assert!(pgp::verify_cleartext(&spaced, &keys).is_ok());
    assert!(pgp::verify_cleartext(text, &pgp::keyring(include_bytes!("../tests/data/trixie-stable.pgp"))).is_err());
}

#[test]
fn armor() {
    assert_eq!(pgp::base64("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
    assert_eq!(pgp::base64("aGVs\nbG8=").unwrap(), b"hello");
    assert!(pgp::base64("a$b").is_none());
    let asc = include_str!("../tests/data/trixie-stable.asc");
    // A wrong checksum line.
    let at = asc.rfind("\n=").unwrap() + 2;
    let mut bad = String::from(&asc[..at]);
    bad.push_str(if &asc[at..at + 4] == "AAAA" { "BBBB" } else { "AAAA" });
    bad.push_str(&asc[at + 4..]);
    assert!(pgp::dearmor(&bad).is_none());
}

/// On Debian's real index and a real dpkg status (by hand: `JUSTDEB_PACKAGES=<Packages>
/// JUSTDEB_STATUS=<status> cargo test -p justdeb -- --ignored --nocapture`).
#[test]
#[ignore]
fn real_archive() {
    let (Ok(p), Ok(s)) = (std::env::var("JUSTDEB_PACKAGES"), std::env::var("JUSTDEB_STATUS")) else { return };
    let text = std::fs::read_to_string(p).unwrap();
    let t = std::time::Instant::now();
    let compact = index::compact(&text);
    let mut i = Index::new();
    i.add(compact.clone(), 0);
    std::println!("{} packages, compact {} of {} bytes, {:?}", i.len(), compact.len(), text.len(), t.elapsed());
    let installed = index::installed(&std::fs::read_to_string(s).unwrap());
    for name in ["cowsay", "sl", "figlet", "fortune-mod", "vim", "gimp"] {
        let t = std::time::Instant::now();
        match resolve::plan(&installed, &i, alloc::vec![resolve::by_name(&i, name).unwrap()]) {
            Ok(plan) => std::println!(
                "{name}: {} packages in {} batches, {} KiB down, {} KiB installed, upgrades {:?}, {:?}: {}",
                plan.packages.len(),
                plan.batches.len(),
                plan.download_size() / 1024,
                plan.installed_kib(),
                plan.upgrades,
                t.elapsed(),
                names(&plan).join(" ")
            ),
            Err(e) => std::println!("{name}: {e}"),
        }
    }
}
