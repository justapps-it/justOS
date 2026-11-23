import dev.justos.connect.Protocol;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.PublicKey;
import java.security.interfaces.ECPublicKey;
import java.util.Arrays;

/**
 * Standalone check of the JustOS Connect crypto, run on a desktop JDK (not
 * part of the APK). Usage: see run-test.ps1 / run-test.sh next to this file.
 */
public final class ProtocolTest {
    private static int failures;

    public static void main(String[] args) throws Exception {
        gcmKnownAnswer();
        hmacKnownAnswer();
        hkdfKnownAnswer();
        nonceLayout();
        pointEncoding();
        handshakeBothSides();
        framing();
        if (failures > 0) {
            System.out.println("FAILED: " + failures + " check(s)");
            System.exit(1);
        }
        System.out.println("ALL OK");
    }

    /** AES-256-GCM, zero key, zero nonce (counter 0), 16 zero bytes. */
    static void gcmKnownAnswer() throws Exception {
        Protocol.Aead a = new Protocol.Aead(new byte[32]);
        byte[] out = a.seal(new byte[16]);
        check("gcm length", out.length == 32);
        check("gcm ciphertext", Protocol.hex(Arrays.copyOfRange(out, 0, 16)).equals("cea7403d4d606b6e074ec5d3baf39d18"));
        check("gcm tag", Protocol.hex(Arrays.copyOfRange(out, 16, 32)).equals("d0d1c8a799996bf0265b98b5d48ab919"));
        Protocol.Aead b = new Protocol.Aead(new byte[32]);
        check("gcm open", Arrays.equals(b.open(out), new byte[16]));
        // The second frame uses counter 1, so the same plaintext seals differently.
        byte[] second = a.seal(new byte[16]);
        check("gcm counter advances", !Arrays.equals(second, out));
        check("gcm open second", Arrays.equals(b.open(second), new byte[16]));
        // A flipped bit must be rejected.
        Protocol.Aead c = new Protocol.Aead(new byte[32]);
        byte[] bad = out.clone();
        bad[3] ^= 1;
        boolean rejected = false;
        try {
            c.open(bad);
        } catch (java.security.GeneralSecurityException e) {
            rejected = true;
        }
        check("gcm rejects tampering", rejected);
    }

    /** RFC 4231 test case 2. */
    static void hmacKnownAnswer() {
        byte[] mac = Protocol.hmac("Jefe".getBytes(StandardCharsets.US_ASCII),
                "what do ya want ".getBytes(StandardCharsets.US_ASCII),
                "for nothing?".getBytes(StandardCharsets.US_ASCII));
        check("hmac rfc4231 #2", Protocol.hex(mac).equals("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    }

    /** RFC 5869 test case 1 (SHA-256); also our 64-byte layout of T1 || T2. */
    static void hkdfKnownAnswer() {
        byte[] ikm = new byte[22];
        Arrays.fill(ikm, (byte) 0x0b);
        byte[] salt = Protocol.unhex("000102030405060708090a0b0c");
        byte[] info = Protocol.unhex("f0f1f2f3f4f5f6f7f8f9");
        byte[] prk = Protocol.hkdfExtract(salt, ikm);
        check("hkdf prk", Protocol.hex(prk).equals("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"));
        byte[] okm = Protocol.hkdfExpand(prk, info, 42);
        check("hkdf okm", Protocol.hex(okm).equals(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"));
        // The spec spelled out: T1 = HMAC(prk, th || 1), T2 = HMAC(prk, T1 || th || 2).
        byte[] th = Protocol.sha256("transcript".getBytes(StandardCharsets.US_ASCII));
        byte[] t1 = Protocol.hmac(prk, th, new byte[] {1});
        byte[] t2 = Protocol.hmac(prk, t1, th, new byte[] {2});
        byte[] ours = Protocol.hkdfExpand(prk, th, 64);
        check("hkdf T1", Arrays.equals(Arrays.copyOfRange(ours, 0, 32), t1));
        check("hkdf T2", Arrays.equals(Arrays.copyOfRange(ours, 32, 64), t2));
    }

    static void nonceLayout() {
        check("nonce 0", Protocol.hex(Protocol.Aead.nonce(0)).equals("000000000000000000000000"));
        check("nonce 1", Protocol.hex(Protocol.Aead.nonce(1)).equals("000000000000000000000001"));
        check("nonce big", Protocol.hex(Protocol.Aead.nonce(0x0102030405060708L)).equals("000000000102030405060708"));
    }

    static void pointEncoding() throws Exception {
        // Many keys, so some have coordinates with leading zero bytes.
        boolean lengths = true, trips = true;
        for (int i = 0; i < 200; i++) {
            KeyPair kp = Protocol.newKeyPair();
            ECPublicKey pub = (ECPublicKey) kp.getPublic();
            byte[] p = Protocol.encodePoint(pub);
            lengths &= p.length == 65 && p[0] == 4;
            PublicKey back = Protocol.decodePoint(p, pub.getParams());
            trips &= ((ECPublicKey) back).getW().equals(pub.getW());
        }
        check("point length (200 keys)", lengths);
        check("point round trip (200 keys)", trips);
        KeyPair kp = Protocol.newKeyPair();
        byte[] p = Protocol.encodePoint((ECPublicKey) kp.getPublic());
        p[64] ^= 1;
        boolean rejected = false;
        try {
            Protocol.decodePoint(p, ((ECPublicKey) kp.getPublic()).getParams());
        } catch (java.security.GeneralSecurityException e) {
            rejected = true;
        }
        check("point off curve rejected", rejected);
    }

    /** Both ends derive the same keys; the PC opens with okm[0..32] and seals with okm[32..64]. */
    static void handshakeBothSides() throws Exception {
        KeyPair phone = Protocol.newKeyPair();
        KeyPair pc = Protocol.newKeyPair();
        byte[] phonePub = Protocol.encodePoint((ECPublicKey) phone.getPublic());
        byte[] pcPub = Protocol.encodePoint((ECPublicKey) pc.getPublic());
        java.security.spec.ECParameterSpec params = ((ECPublicKey) phone.getPublic()).getParams();

        byte[] s1 = Protocol.sharedSecret(phone.getPrivate(), Protocol.decodePoint(pcPub, params));
        byte[] s2 = Protocol.sharedSecret(pc.getPrivate(), Protocol.decodePoint(phonePub, params));
        check("ecdh agrees", Arrays.equals(s1, s2) && s1.length == 32);

        Protocol.Keys k = Protocol.deriveKeys(s1, phonePub, pcPub);
        check("th", Arrays.equals(k.th, Protocol.sha256(phonePub, pcPub)));
        byte[] prk = Protocol.hmac("justos-connect 1".getBytes(StandardCharsets.US_ASCII), s1);
        byte[] okm = Protocol.hkdfExpand(prk, k.th, 64);
        check("phone->pc key", Arrays.equals(k.phoneToPc, Arrays.copyOfRange(okm, 0, 32)));
        check("pc->phone key", Arrays.equals(k.pcToPhone, Arrays.copyOfRange(okm, 32, 64)));

        // PC side: open = keys[..32], seal = keys[32..].
        Protocol.Aead phoneSeal = new Protocol.Aead(k.phoneToPc);
        Protocol.Aead pcOpen = new Protocol.Aead(Arrays.copyOfRange(okm, 0, 32));
        byte[] body = Protocol.body(Protocol.PAIR, null);
        check("pc opens phone frame", Arrays.equals(pcOpen.open(phoneSeal.seal(body)), body));
        Protocol.Aead pcSeal = new Protocol.Aead(Arrays.copyOfRange(okm, 32, 64));
        Protocol.Aead phoneOpen = new Protocol.Aead(k.pcToPhone);
        byte[] w = Protocol.body(Protocol.WELCOME, "{}".getBytes(StandardCharsets.UTF_8));
        check("phone opens pc frame", Arrays.equals(phoneOpen.open(pcSeal.seal(w)), w));

        // Pairing and auth proofs.
        byte[] code = Protocol.pairMac("012345", k.th);
        byte[] pair = new byte[4 + 32];
        System.arraycopy("pair".getBytes(StandardCharsets.US_ASCII), 0, pair, 0, 4);
        System.arraycopy(k.th, 0, pair, 4, 32);
        check("pair mac", Arrays.equals(code, Protocol.hmac("012345".getBytes(StandardCharsets.US_ASCII), pair)));
        byte[] token = new byte[32];
        Arrays.fill(token, (byte) 7);
        check("auth mac", Arrays.equals(Protocol.authMac(token, k.th),
                Protocol.hmac(token, "auth".getBytes(StandardCharsets.US_ASCII), k.th)));
        check("pc mac", Arrays.equals(Protocol.pcMac(token, k.th),
                Protocol.hmac(token, "pc".getBytes(StandardCharsets.US_ASCII), k.th)));
        check("hex round trip", Arrays.equals(Protocol.unhex(Protocol.hex(token)), token));
        check("unhex rejects", Protocol.unhex("zz") == null && Protocol.unhex("abc") == null);
    }

    static void framing() throws Exception {
        ByteArrayOutputStream bo = new ByteArrayOutputStream();
        Protocol.writeFrame(bo, Protocol.body(Protocol.MOVE, Protocol.move(-3, 70000)));
        byte[] raw = bo.toByteArray();
        check("frame bytes", Protocol.hex(raw).equals("0000000510fffd7fff"));
        byte[] back = Protocol.readFrame(new DataInputStream(new ByteArrayInputStream(raw)));
        check("frame read", back.length == 5 && back[0] == Protocol.MOVE);
        check("tap", Protocol.hex(Protocol.tap(1f, 0f)).equals("ffff0000"));
        check("key char", Protocol.hex(Protocol.key(0, 1, "c")).equals("000163"));
        check("scroll", Protocol.hex(Protocol.scroll(-1)).equals("ffff"));
    }

    static void check(String what, boolean ok) {
        System.out.println((ok ? "ok    " : "FAIL  ") + what);
        if (!ok) failures++;
    }
}
