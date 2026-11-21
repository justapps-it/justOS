//! Known-answer tests: IEEE 802.11-2020 Annex J (PSK, CCMP, SAE), the RFCs
//! (HMAC, PBKDF2, CMAC, key wrap) and FIPS (SHA-1, AES), and the
//! handshakes against an authenticator written here from the standard.

use crate::crypto::*;
use crate::eapol::{self, Akm, Key};
use crate::frame;
use crate::sae::{self, Sae};
use alloc::vec::Vec;
use justtls::bigint;

fn hex(s: &str) -> Vec<u8> {
    let s: alloc::string::String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn sha1_and_hmac() {
    assert_eq!(sha1(b"abc").to_vec(), hex("a9993e364706816aba3e25717850c26c9cd0d89d"));
    assert_eq!(
        sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_vec(),
        hex("84983e441c3bd26ebaae4aa1f95129e5e54670f1")
    );
    // RFC 2202 cases 1 and 2.
    assert_eq!(hmac_sha1(&[0x0b; 20], &[b"Hi There"]).to_vec(), hex("b617318655057264e28bc0b6fb378c8ef146be00"));
    assert_eq!(hmac_sha1(b"Jefe", &[b"what do ya ", b"want for nothing?"]).to_vec(), hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"));
}

#[test]
fn pbkdf2_and_psk() {
    // RFC 6070.
    let mut out = [0u8; 20];
    pbkdf2_sha1(b"password", b"salt", 1, &mut out);
    assert_eq!(out.to_vec(), hex("0c60c80f961f0e71f3a9b524af6012062fe037a6"));
    pbkdf2_sha1(b"password", b"salt", 4096, &mut out);
    assert_eq!(out.to_vec(), hex("4b007901b765489abead49d926f721d065a429c1"));
    // IEEE 802.11-2020 J.4.2.
    assert_eq!(psk(b"password", b"IEEE").to_vec(), hex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"));
    assert_eq!(
        psk(b"ThisIsAPassword", b"ThisIsASSID").to_vec(),
        hex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af")
    );
}

#[test]
fn prf() {
    // IEEE 802.11 J.3.2 (the vectors hostapd checks).
    assert_eq!(
        prf_sha1(&[0x0b; 20], b"prefix", b"Hi There", 64),
        hex("bcd4c650b30b9684951829e0d75f9d54b862175ed9f00606e17d8da35402ffee75df78c3d31e0f889f012120c0862beb67753e7439ae242edb8373698356cf5a")
    );
    assert_eq!(
        prf_sha1(b"Jefe", b"prefix", b"what do ya want for nothing?", 64),
        hex("51f4de5b33f249adf81aeb713a3c20f4fe631446fabdfa58244759ae58ef9009a99abf4eac2ca5fa87e692c440eb40023e7babb206d61de7b92f41529092b8fc")
    );
}

#[test]
fn aes_decrypt() {
    // FIPS 197 C.1 and C.3.
    let aes = justtls::aes::Aes::new(&hex("000102030405060708090a0b0c0d0e0f"));
    let mut b: [u8; 16] = hex("69c4e0d86a7b0430d8cdb78070b4c55a").try_into().unwrap();
    aes.decrypt(&mut b);
    assert_eq!(b.to_vec(), hex("00112233445566778899aabbccddeeff"));
    let aes = justtls::aes::Aes::new(&hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"));
    let mut b: [u8; 16] = hex("8ea2b7ca516745bfeafc49904b496089").try_into().unwrap();
    aes.decrypt(&mut b);
    assert_eq!(b.to_vec(), hex("00112233445566778899aabbccddeeff"));
}

#[test]
fn aes_cmac() {
    // RFC 4493.
    let k = hex("2b7e151628aed2a6abf7158809cf4f3c");
    let m = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710");
    assert_eq!(cmac(&k, &[]).to_vec(), hex("bb1d6929e95937287fa37d129b756746"));
    assert_eq!(cmac(&k, &[&m[..16]]).to_vec(), hex("070a16b46b4d4144f79bdd9dd04a287c"));
    assert_eq!(cmac(&k, &[&m[..20], &m[20..40]]).to_vec(), hex("dfa66747de9ae63030ca32611497c827"));
    assert_eq!(cmac(&k, &[&m]).to_vec(), hex("51f0bebf7e3b9d92fc49741779363cfe"));
}

#[test]
fn aes_key_wrap() {
    // RFC 3394 4.1 and 4.6.
    let kek = hex("000102030405060708090a0b0c0d0e0f");
    let plain = hex("00112233445566778899aabbccddeeff");
    let w = key_wrap(&kek, &plain);
    assert_eq!(w, hex("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5"));
    assert_eq!(key_unwrap(&kek, &w), Some(plain));
    let kek = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    let plain = hex("00112233445566778899aabbccddeeff000102030405060708090a0b0c0d0e0f");
    let w = key_wrap(&kek, &plain);
    assert_eq!(w, hex("28c9f404c4b810f4cbccb35cfb87f8263f5786e2d80ed326cbc7f0e71a99f43bfb988b9b7a02dd21"));
    let mut bad = w.clone();
    bad[5] ^= 1;
    assert_eq!(key_unwrap(&kek, &bad), None);
}

#[test]
fn ccmp_vector() {
    // IEEE 802.11-2012 M.6.4 (2020: J.6.4).
    let tk = hex("c97c1f67ce371185514a8a19f2bdd52f");
    let frame = hex("08 48 c3 2c 0f d2 e1 28 a5 7c 50 30 f1 84 44 08 ab ae a5 b8 fc ba 80 33 f8 ba 1a 55 d0 2f 85 ae 96 7b b6 2f b6 cd a8 eb 7e 78 a0 50");
    let pn = 0xB5039776E70C;
    let enc = ccmp_encrypt(&tk, &frame[..24], pn, 0, &frame[24..]);
    let want = hex(
        "0c e7 00 20 76 97 03 b5 f3 d0 a2 fe 9a 3d bf 23 42 a6 43 e4 32 46 e8 0c 3c 04 d0 19 78 45 ce 0b 16 f9 76 23",
    );
    assert_eq!(enc, want);
    let (got_pn, plain) = ccmp_decrypt(&tk, &frame[..24], &enc).unwrap();
    assert_eq!(got_pn, pn);
    assert_eq!(plain, frame[24..].to_vec());
    let mut bad = enc.clone();
    bad[12] ^= 0x80;
    assert!(ccmp_decrypt(&tk, &frame[..24], &bad).is_none());
}

#[test]
fn sae_hunting_and_pecking() {
    // IEEE 802.11-2020 J.10, as hostapd's module tests check it.
    let own = [0x4d, 0x3f, 0x2f, 0xff, 0xe3, 0x87];
    let peer = [0xa5, 0xd8, 0xaa, 0x95, 0x8e, 0x3c];
    let pwe = sae::pwe_hunt_and_peck(&own, &peer, b"mekmitasdigoat").unwrap();
    let rand = hex("992465fd3daa3c60aa6565b7f62a2a7f2e12dd12f198faf4fbed89d7ff1ace94");
    let mask = hex("9507a90f777a044d6a0830b91ea3d5dd70bece44e1acffb86983b5e1bf9fb322");
    let mut s = Sae::with_values(&pwe, &rand, &mask).unwrap();
    let commit = hex(
        "13002e2c0f0db52440ad146d967114ce005ce1eab0aa2c2e5c2871b774f6c2575c65d5ad9e00829707aa36ba8b859738fc961d08243505f47c035376d7ac4bc8\
         d7b95083bf43827d0fc31ed778dd3671fd21a46d1091d64b6f9a1e1272621325dbe1",
    );
    assert_eq!(s.commit(), commit);
    let peer_commit = hex(
        "1300591b96f3397fb945100848e7b550543b6720d88337ee93fc49fd6df7e08b5223e71b9bb048d3873f20556953a96c91536fd8ee6ca9b4a68a148b056a909b\
         e03e83ae208f60f8ef5537858074db066870323998629 99b511e0a1552a5fea317c2",
    );
    s.process_commit(&peer_commit).unwrap();
    assert_eq!(s.kck.to_vec(), hex("1e733f6d9bd5325628730433883 1b09a39406d121017073a5c30db36f36cb81a"));
    assert_eq!(s.pmk.to_vec(), hex("4e4dfab1a2dd8ac1a91790f953faaa452ae5c6873ab75b63605ba663f8a7fe59"));
    assert_eq!(s.pmkid.to_vec(), hex("8747a600eea3f9f22475df58ca1e5498"));
    // Our own commit sent back is refused.
    assert_eq!(s.process_commit(&commit), Err(sae::SaeError::Reflected));
}

#[test]
fn sae_hash_to_element() {
    // IEEE 802.11-2020 J.10: PT for "byteme", "mekmitasdigoat" with the
    // identifier "psk4internet", and the PWE for two addresses.
    let pt = sae::pt(b"byteme", b"mekmitasdigoat", b"psk4internet");
    let a = [0x00, 0x09, 0x5b, 0x66, 0xec, 0x1e];
    let b = [0x00, 0x0b, 0x6b, 0xd9, 0x02, 0x46];
    let pwe = sae::pwe_from_pt(&pt, &a, &b).unwrap();
    assert_eq!(bigint::to_be(&pwe.0, 32), hex("c93049b9e64000f848201649e999f2b5c22dea69b5632c9df4d633b8aa1f6c1e"));
    assert_eq!(bigint::to_be(&pwe.1, 32), hex("73634e94b53d82e7383a8d258199d9dc1a5ee8269d060382ccbf33e614ff59a0"));
    // The same from either side.
    assert_eq!(sae::pwe_from_pt(&pt, &b, &a).unwrap(), pwe);
}

/// Both sides of SAE agree, and a wrong password does not.
#[test]
fn sae_exchange() {
    let sta = [2, 0, 0, 0, 0, 1];
    let ap = [2, 0, 0, 0, 0, 2];
    let pw = b"correct horse";
    let pwe_s = sae::pwe_hunt_and_peck(&sta, &ap, pw).unwrap();
    let pwe_a = sae::pwe_hunt_and_peck(&ap, &sta, pw).unwrap();
    assert_eq!(pwe_s, pwe_a);
    let mut s = Sae::new(&pwe_s, &[7; 40], &[9; 40]).unwrap();
    let mut a = Sae::new(&pwe_a, &[3; 40], &[5; 40]).unwrap();
    s.process_commit(&a.commit()).unwrap();
    a.process_commit(&s.commit()).unwrap();
    assert_eq!(s.pmk, a.pmk);
    let mut conf = 1u16.to_le_bytes().to_vec();
    conf.extend_from_slice(&s.confirm(1));
    assert!(a.check_confirm(&conf));
    // The wrong password.
    let pwe_w = sae::pwe_hunt_and_peck(&ap, &sta, b"wrong horse").unwrap();
    let mut w = Sae::new(&pwe_w, &[3; 40], &[5; 40]).unwrap();
    w.process_commit(&s.commit()).unwrap();
    assert!(!w.check_confirm(&conf));
}

#[test]
fn rsn_element() {
    // A WPA2/WPA3 transition network with PMF capable.
    let d = hex("0100 000fac04 0100 000fac04 0200 000fac02 000fac08 8000");
    let r = frame::Rsn::parse(&d).unwrap();
    assert_eq!(r.akms, alloc::vec![frame::AKM_PSK, frame::AKM_SAE]);
    assert_eq!(r.caps, frame::RSN_CAP_MFPC);
    assert_eq!(r.build(), d);
}

#[test]
fn ethernet_conversion() {
    let bssid = [2, 0, 0, 0, 0, 9];
    let sta = [2, 0, 0, 0, 0, 1];
    let host = [2, 0, 0, 0, 0, 5];
    // To the access point.
    let h = frame::data_header(&bssid, &sta, &host, 7, true);
    let body = frame::llc(0x0800, b"payload");
    let f = [h, body].concat();
    let p = frame::parse(&f).unwrap();
    assert_eq!(p.kind, frame::QOS_DATA);
    assert_eq!(p.len, 26);
    assert_eq!(p.seq, 7);
    // From it: A1 us, A2 the BSSID, A3 the source.
    let mut back = f.clone();
    back[1] = frame::FLAG_FROM_DS;
    back[4..10].copy_from_slice(&sta);
    back[10..16].copy_from_slice(&bssid);
    back[16..22].copy_from_slice(&host);
    let p = frame::parse(&back).unwrap();
    let e = frame::to_ethernet(&p, &back[p.len..]);
    assert_eq!(e.len(), 1);
    assert_eq!(&e[0][..6], &sta);
    assert_eq!(&e[0][6..12], &host);
    assert_eq!(&e[0][12..14], &[8, 0]);
    assert_eq!(&e[0][14..], b"payload");
}

/// The 4-way handshake between our supplicant's pieces and an
/// authenticator built here from the standard, for every AKM.
#[test]
fn four_way_keys() {
    let aa = [2, 0, 0, 0, 0, 9];
    let spa = [2, 0, 0, 0, 0, 1];
    let pmk = psk(b"password", b"IEEE");
    for akm in [Akm::Psk, Akm::PskSha256, Akm::Sae] {
        let anonce = [0x11; 32];
        let snonce = [0x22; 32];
        let a = eapol::derive_ptk(akm, &pmk, &aa, &spa, &anonce, &snonce);
        let s = eapol::derive_ptk(akm, &pmk, &spa, &aa, &snonce, &anonce);
        assert_eq!(a, s);
        let gtk = [0x33; 16];
        let mut kde = alloc::vec![0xDD, 22, 0x00, 0x0F, 0xAC, 1, 1, 0];
        kde.extend_from_slice(&gtk);
        let mut m3 = Key {
            eapol_version: 2,
            info: akm.version() | eapol::KI_PAIRWISE | eapol::KI_ACK | eapol::KI_MIC | eapol::KI_INSTALL | eapol::KI_SECURE | eapol::KI_ENCRYPTED,
            key_len: 16,
            replay: 2,
            nonce: anonce,
            data: eapol::encrypt_data(&a.kek, &kde),
            ..Key::default()
        };
        let raw = m3.build_signed(akm, &a.kck);
        assert!(Key::check_mic(&raw, akm, &s.kck));
        let parsed = Key::parse(&raw).unwrap();
        let data = eapol::decrypt_data(&s.kek, &parsed.data).unwrap();
        let kd = eapol::parse_key_data(&data);
        assert_eq!(kd.gtk.unwrap().key, gtk.to_vec());
        let mut bad = raw.clone();
        bad[20] ^= 1;
        assert!(!Key::check_mic(&bad, akm, &s.kck));
    }
}
