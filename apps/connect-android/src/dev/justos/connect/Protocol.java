package dev.justos.connect;

import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.KeyFactory;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.security.PrivateKey;
import java.security.PublicKey;
import java.security.SecureRandom;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECFieldFp;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.ECParameterSpec;
import java.security.spec.ECPoint;
import java.security.spec.ECPublicKeySpec;
import java.security.spec.EllipticCurve;
import java.util.Arrays;

import javax.crypto.Cipher;
import javax.crypto.KeyAgreement;
import javax.crypto.Mac;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * The JustOS Connect wire format: message types, frames, payload encoders and
 * the handshake crypto. Plain Java with no Android dependencies, so the
 * standalone test in test/ can exercise it on a desktop JDK.
 */
public final class Protocol {
    private Protocol() {}

    public static final int DISCOVERY_PORT = 47800;
    public static final int PORT = 47801;
    public static final int VERSION = 1;
    public static final String DISCOVERY_QUERY = "JUSTOS-CONNECT?";
    public static final String DISCOVERY_REPLY = "JUSTOS-CONNECT ";

    /** Largest frame accepted from the PC (it caps its own at 4 MiB). */
    public static final int MAX_FRAME = 8 << 20;
    /** File data goes in pieces of this size. */
    public static final int CHUNK = 64 * 1024;

    // Message types.
    public static final int HELLO = 0x01;
    public static final int CHALLENGE = 0x02;
    public static final int AUTH = 0x03;
    public static final int PAIR = 0x04;
    public static final int CODE = 0x05;
    public static final int WELCOME = 0x06;
    public static final int DENIED = 0x07;
    public static final int MOVE = 0x10;
    public static final int BUTTON = 0x11;
    public static final int SCROLL = 0x12;
    public static final int TEXT = 0x13;
    public static final int KEY = 0x14;
    public static final int TAP = 0x15;
    public static final int FILE_BEGIN = 0x20;
    public static final int FILE_DATA = 0x21;
    public static final int FILE_END = 0x22;
    public static final int FILE_DONE = 0x23;
    public static final int FILE_CANCEL = 0x24;
    public static final int SCREEN = 0x30;
    public static final int FRAME = 0x31;
    public static final int FRAME_ACK = 0x32;
    public static final int NOTIFY = 0x40;
    public static final int BATTERY = 0x41;
    public static final int CLIP = 0x42;
    public static final int PING = 0x50;
    public static final int PONG = 0x51;
    public static final int RING = 0x52;

    // Mouse buttons.
    public static final int BTN_LEFT = 0;
    public static final int BTN_RIGHT = 1;
    public static final int BTN_MIDDLE = 2;

    // Key codes for KEY.
    public static final int K_CHAR = 0;
    public static final int K_ENTER = 1;
    public static final int K_BACKSPACE = 2;
    public static final int K_DELETE = 3;
    public static final int K_TAB = 4;
    public static final int K_ESC = 5;
    public static final int K_UP = 6;
    public static final int K_DOWN = 7;
    public static final int K_LEFT = 8;
    public static final int K_RIGHT = 9;
    public static final int K_HOME = 10;
    public static final int K_END = 11;
    public static final int K_PGUP = 12;
    public static final int K_PGDN = 13;
    public static final int K_VOL_UP = 14;
    public static final int K_VOL_DOWN = 15;
    public static final int K_MUTE = 16;
    public static final int K_OVERVIEW = 17;
    /** F1 is 0x21, F12 is 0x2C. */
    public static final int K_F1 = 0x21;

    // Key modifiers.
    public static final int MOD_CTRL = 1;
    public static final int MOD_ALT = 2;
    public static final int MOD_SHIFT = 4;

    // ------------------------------------------------------------------
    // Frames

    /** A frame body: the type byte followed by the payload. */
    public static byte[] body(int type, byte[] payload) {
        int n = payload == null ? 0 : payload.length;
        byte[] b = new byte[1 + n];
        b[0] = (byte) type;
        if (n > 0) System.arraycopy(payload, 0, b, 1, n);
        return b;
    }

    /** Writes a u32 big-endian length then the (already sealed, if needed) body. */
    public static void writeFrame(OutputStream out, byte[] body) throws IOException {
        int n = body.length;
        out.write(new byte[] {(byte) (n >>> 24), (byte) (n >>> 16), (byte) (n >>> 8), (byte) n});
        out.write(body);
    }

    /** Reads one frame's body (still sealed, if the link is encrypted). */
    public static byte[] readFrame(DataInputStream in) throws IOException {
        int n = in.readInt();
        if (n <= 0 || n > MAX_FRAME) throw new IOException("bad frame length " + n);
        byte[] b = new byte[n];
        in.readFully(b);
        return b;
    }

    // ------------------------------------------------------------------
    // Payload encoders (all integers big-endian)

    public static byte[] move(int dx, int dy) {
        dx = clamp(dx, Short.MIN_VALUE, Short.MAX_VALUE);
        dy = clamp(dy, Short.MIN_VALUE, Short.MAX_VALUE);
        return new byte[] {(byte) (dx >> 8), (byte) dx, (byte) (dy >> 8), (byte) dy};
    }

    public static byte[] button(int button, boolean down) {
        return new byte[] {(byte) button, (byte) (down ? 1 : 0)};
    }

    public static byte[] scroll(int amount) {
        amount = clamp(amount, Short.MIN_VALUE, Short.MAX_VALUE);
        return new byte[] {(byte) (amount >> 8), (byte) amount};
    }

    /** KEY: code, modifiers and, for code 0, the character as UTF-8. */
    public static byte[] key(int code, int mods, String ch) {
        byte[] c = code == K_CHAR && ch != null ? utf8(ch) : new byte[0];
        byte[] b = new byte[2 + c.length];
        b[0] = (byte) code;
        b[1] = (byte) mods;
        System.arraycopy(c, 0, b, 2, c.length);
        return b;
    }

    /** TAP: x and y as fractions of the screen, 0..1 mapped to 0..65535. */
    public static byte[] tap(float fx, float fy) {
        int x = clamp(Math.round(fx * 65535f), 0, 65535);
        int y = clamp(Math.round(fy * 65535f), 0, 65535);
        return new byte[] {(byte) (x >> 8), (byte) x, (byte) (y >> 8), (byte) y};
    }

    public static int u16(byte[] b, int off) {
        return ((b[off] & 0xff) << 8) | (b[off + 1] & 0xff);
    }

    public static byte[] utf8(String s) {
        return s.getBytes(StandardCharsets.UTF_8);
    }

    public static String utf8(byte[] b) {
        return new String(b, StandardCharsets.UTF_8);
    }

    static int clamp(int v, int lo, int hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }

    // ------------------------------------------------------------------
    // Elliptic curve (P-256)

    public static KeyPair newKeyPair() throws GeneralSecurityException {
        KeyPairGenerator g = KeyPairGenerator.getInstance("EC");
        g.initialize(new ECGenParameterSpec("secp256r1"), new SecureRandom());
        return g.generateKeyPair();
    }

    /** The uncompressed point: 0x04, then x and y padded to 32 bytes each. */
    public static byte[] encodePoint(ECPublicKey key) {
        byte[] out = new byte[65];
        out[0] = 4;
        fixed32(key.getW().getAffineX(), out, 1);
        fixed32(key.getW().getAffineY(), out, 33);
        return out;
    }

    private static void fixed32(BigInteger v, byte[] out, int off) {
        byte[] b = v.toByteArray();
        int start = 0;
        while (start < b.length - 1 && b[start] == 0) start++;
        int n = b.length - start;
        if (n > 32) throw new IllegalArgumentException("coordinate too large");
        System.arraycopy(b, start, out, off + 32 - n, n);
    }

    /**
     * The PC's public key from its uncompressed point, using the curve
     * parameters of our own key. Rejects points not on the curve.
     */
    public static PublicKey decodePoint(byte[] p, ECParameterSpec params) throws GeneralSecurityException {
        if (p == null || p.length != 65 || p[0] != 4) throw new GeneralSecurityException("bad public key");
        BigInteger x = new BigInteger(1, Arrays.copyOfRange(p, 1, 33));
        BigInteger y = new BigInteger(1, Arrays.copyOfRange(p, 33, 65));
        EllipticCurve curve = params.getCurve();
        BigInteger prime = ((ECFieldFp) curve.getField()).getP();
        if (x.compareTo(prime) >= 0 || y.compareTo(prime) >= 0) throw new GeneralSecurityException("bad public key");
        BigInteger lhs = y.multiply(y).mod(prime);
        BigInteger rhs = x.pow(3).add(curve.getA().multiply(x)).add(curve.getB()).mod(prime);
        if (!lhs.equals(rhs)) throw new GeneralSecurityException("public key not on the curve");
        return KeyFactory.getInstance("EC").generatePublic(new ECPublicKeySpec(new ECPoint(x, y), params));
    }

    /** ECDH: the 32-byte x coordinate of the shared point. */
    public static byte[] sharedSecret(PrivateKey mine, PublicKey theirs) throws GeneralSecurityException {
        KeyAgreement ka = KeyAgreement.getInstance("ECDH");
        ka.init(mine);
        ka.doPhase(theirs, true);
        byte[] s = ka.generateSecret();
        if (s.length != 32) throw new GeneralSecurityException("bad shared secret length " + s.length);
        return s;
    }

    // ------------------------------------------------------------------
    // Hashes and key derivation

    public static final byte[] SALT = "justos-connect 1".getBytes(StandardCharsets.US_ASCII);

    public static byte[] sha256(byte[]... parts) {
        try {
            MessageDigest d = MessageDigest.getInstance("SHA-256");
            for (byte[] p : parts) d.update(p);
            return d.digest();
        } catch (GeneralSecurityException e) {
            throw new IllegalStateException(e);
        }
    }

    /** HMAC-SHA256 of the concatenation of {@code parts}. */
    public static byte[] hmac(byte[] key, byte[]... parts) {
        try {
            Mac m = Mac.getInstance("HmacSHA256");
            m.init(new SecretKeySpec(key, "HmacSHA256"));
            for (byte[] p : parts) m.update(p);
            return m.doFinal();
        } catch (GeneralSecurityException e) {
            throw new IllegalStateException(e);
        }
    }

    /** HKDF-Extract (RFC 5869): HMAC(salt, ikm). */
    public static byte[] hkdfExtract(byte[] salt, byte[] ikm) {
        return hmac(salt, ikm);
    }

    /** HKDF-Expand (RFC 5869): T(i) = HMAC(prk, T(i-1) || info || i). */
    public static byte[] hkdfExpand(byte[] prk, byte[] info, int length) {
        if (length > 255 * 32) throw new IllegalArgumentException("too long");
        byte[] out = new byte[length];
        byte[] t = new byte[0];
        int done = 0;
        for (int i = 1; done < length; i++) {
            t = hmac(prk, t, info, new byte[] {(byte) i});
            int n = Math.min(t.length, length - done);
            System.arraycopy(t, 0, out, done, n);
            done += n;
        }
        return out;
    }

    /** The keys of a session. */
    public static final class Keys {
        /** SHA-256(phone public point || PC public point). */
        public final byte[] th;
        public final byte[] phoneToPc;
        public final byte[] pcToPhone;

        Keys(byte[] th, byte[] phoneToPc, byte[] pcToPhone) {
            this.th = th;
            this.phoneToPc = phoneToPc;
            this.pcToPhone = pcToPhone;
        }
    }

    public static Keys deriveKeys(byte[] shared, byte[] phonePub, byte[] pcPub) {
        byte[] th = sha256(phonePub, pcPub);
        byte[] prk = hkdfExtract(SALT, shared);
        byte[] okm = hkdfExpand(prk, th, 64);
        return new Keys(th, Arrays.copyOfRange(okm, 0, 32), Arrays.copyOfRange(okm, 32, 64));
    }

    private static final byte[] L_AUTH = {'a', 'u', 't', 'h'};
    private static final byte[] L_PAIR = {'p', 'a', 'i', 'r'};
    private static final byte[] L_PC = {'p', 'c'};

    /** AUTH proof: HMAC(token, "auth" || th). */
    public static byte[] authMac(byte[] token, byte[] th) {
        return hmac(token, L_AUTH, th);
    }

    /** CODE proof: HMAC(ASCII code digits, "pair" || th). */
    public static byte[] pairMac(String code, byte[] th) {
        return hmac(code.getBytes(StandardCharsets.US_ASCII), L_PAIR, th);
    }

    /** The PC's proof in WELCOME: HMAC(token, "pc" || th). */
    public static byte[] pcMac(byte[] token, byte[] th) {
        return hmac(token, L_PC, th);
    }

    public static boolean equalsConstantTime(byte[] a, byte[] b) {
        return a != null && b != null && MessageDigest.isEqual(a, b);
    }

    // ------------------------------------------------------------------
    // AES-256-GCM, one direction of a link

    /**
     * Seals or opens the frames of one direction. The nonce is 4 zero bytes
     * then a u64 big-endian counter that starts at 0; tag 128 bits, no AAD.
     * Not thread-safe: each direction is used by one thread.
     */
    public static final class Aead {
        private final SecretKeySpec key;
        private final Cipher cipher;
        private long counter;

        public Aead(byte[] key) throws GeneralSecurityException {
            if (key.length != 32) throw new GeneralSecurityException("AES-256 needs a 32-byte key");
            this.key = new SecretKeySpec(key, "AES");
            this.cipher = Cipher.getInstance("AES/GCM/NoPadding");
        }

        public static byte[] nonce(long n) {
            byte[] b = new byte[12];
            for (int i = 0; i < 8; i++) b[4 + i] = (byte) (n >>> (56 - 8 * i));
            return b;
        }

        public long counter() {
            return counter;
        }

        /** Ciphertext followed by the 16-byte tag. */
        public byte[] seal(byte[] plain) throws GeneralSecurityException {
            cipher.init(Cipher.ENCRYPT_MODE, key, new GCMParameterSpec(128, nonce(counter)));
            byte[] out = cipher.doFinal(plain);
            counter++;
            return out;
        }

        public byte[] open(byte[] sealed) throws GeneralSecurityException {
            if (sealed.length < 16) throw new GeneralSecurityException("frame too short");
            cipher.init(Cipher.DECRYPT_MODE, key, new GCMParameterSpec(128, nonce(counter)));
            byte[] out = cipher.doFinal(sealed);
            counter++;
            return out;
        }
    }

    // ------------------------------------------------------------------
    // Hex

    private static final char[] HEX = "0123456789abcdef".toCharArray();

    public static String hex(byte[] b) {
        char[] c = new char[b.length * 2];
        for (int i = 0; i < b.length; i++) {
            c[2 * i] = HEX[(b[i] >> 4) & 15];
            c[2 * i + 1] = HEX[b[i] & 15];
        }
        return new String(c);
    }

    /** Parses hex, or returns null if it is not valid hex. */
    public static byte[] unhex(String s) {
        if (s == null || (s.length() & 1) != 0) return null;
        byte[] b = new byte[s.length() / 2];
        for (int i = 0; i < b.length; i++) {
            int hi = Character.digit(s.charAt(2 * i), 16);
            int lo = Character.digit(s.charAt(2 * i + 1), 16);
            if (hi < 0 || lo < 0) return null;
            b[i] = (byte) ((hi << 4) | lo);
        }
        return b;
    }

    public static String randomHex(int bytes) {
        byte[] b = new byte[bytes];
        new SecureRandom().nextBytes(b);
        return hex(b);
    }
}
