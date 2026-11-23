package dev.justos.connect;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.security.GeneralSecurityException;
import java.security.KeyPair;
import java.security.PublicKey;
import java.security.interfaces.ECPublicKey;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;

/**
 * One connection to a PC: the socket, the handshake, a writer thread that
 * seals frames in order (and pings), and a read loop run by the caller.
 */
final class Link {
    /** Where the tokens of paired PCs are kept. */
    interface TokenStore {
        byte[] token(String pcId);

        void putToken(String pcId, String pcName, byte[] token);

        void forgetToken(String pcId);
    }

    /** Asks the user for the pairing code; null if they gave up. */
    interface CodeAsker {
        String askCode(String pcName) throws InterruptedException;
    }

    /** Receives the frames read after the handshake (on the reading thread). */
    interface Receiver {
        void onMessage(int type, byte[] payload) throws IOException;
    }

    /** The PC refused us: reason "auth" (token not valid) or "code" (wrong code). */
    static final class DeniedException extends IOException {
        final String reason;

        DeniedException(String reason) {
            super("denied: " + reason);
            this.reason = reason;
        }
    }

    /** The user cancelled pairing. */
    static final class CancelledException extends IOException {
        CancelledException() {
            super("cancelled");
        }
    }

    private static final int CONNECT_TIMEOUT_MS = 5000;
    private static final int HANDSHAKE_TIMEOUT_MS = 15000;
    /** The PC answers every ping, so 40 s of silence means the link is dead. */
    private static final int READ_TIMEOUT_MS = 40000;
    private static final long PING_EVERY_MS = 10000;

    final String host;
    final int port;
    String pcId;
    String pcName;
    int screenW;
    int screenH;
    boolean gui;

    private final Socket socket = new Socket();
    private DataInputStream in;
    private OutputStream out;
    private Protocol.Aead seal;
    private Protocol.Aead open;
    private final LinkedBlockingQueue<byte[]> queue = new LinkedBlockingQueue<>();
    private volatile boolean closed;
    private Thread writer;

    private Link(String host, int port) {
        this.host = host;
        this.port = port;
    }

    /** Connects and runs the handshake; on return the link is ready. */
    static Link connect(String host, int port, String phoneId, String phoneName, TokenStore tokens, CodeAsker asker)
            throws IOException, InterruptedException {
        Link l = new Link(host, port);
        try {
            l.handshake(phoneId, phoneName, tokens, asker);
        } catch (IOException | InterruptedException | RuntimeException e) {
            l.close();
            throw e;
        }
        l.startWriter();
        return l;
    }

    private void handshake(String phoneId, String phoneName, TokenStore tokens, CodeAsker asker)
            throws IOException, InterruptedException {
        socket.connect(new InetSocketAddress(host, port), CONNECT_TIMEOUT_MS);
        socket.setTcpNoDelay(true);
        socket.setSoTimeout(HANDSHAKE_TIMEOUT_MS);
        in = new DataInputStream(new BufferedInputStream(socket.getInputStream(), 64 * 1024));
        out = new BufferedOutputStream(socket.getOutputStream(), 64 * 1024 + 64);
        try {
            // 1. HELLO, in the clear.
            KeyPair kp = Protocol.newKeyPair();
            ECPublicKey myKey = (ECPublicKey) kp.getPublic();
            byte[] myPub = Protocol.encodePoint(myKey);
            JSONObject hello = new JSONObject().put("id", phoneId).put("name", phoneName).put("key", Protocol.hex(myPub));
            Protocol.writeFrame(out, Protocol.body(Protocol.HELLO, Protocol.utf8(hello.toString())));
            out.flush();

            // 2. CHALLENGE, in the clear.
            byte[] body = Protocol.readFrame(in);
            if (body[0] != Protocol.CHALLENGE) throw new IOException("expected CHALLENGE, got " + body[0]);
            JSONObject ch = json(body);
            String id = ch.getString("id");
            if (!id.matches("[0-9A-Za-z-]{1,64}")) throw new IOException("bad PC id");
            pcId = id;
            pcName = ch.optString("name", host);
            boolean known = ch.optBoolean("known", false);
            byte[] pcPub = Protocol.unhex(ch.optString("key"));
            PublicKey pcKey = Protocol.decodePoint(pcPub, myKey.getParams());

            // 3. Keys; everything from here on is sealed.
            byte[] shared = Protocol.sharedSecret(kp.getPrivate(), pcKey);
            Protocol.Keys keys = Protocol.deriveKeys(shared, myPub, pcPub);
            seal = new Protocol.Aead(keys.phoneToPc);
            open = new Protocol.Aead(keys.pcToPhone);

            // 4. AUTH with the token, or PAIR then CODE.
            byte[] token = tokens.token(pcId);
            boolean pairing = token == null || !known;
            if (!pairing) {
                String mac = Protocol.hex(Protocol.authMac(token, keys.th));
                writeSealed(Protocol.body(Protocol.AUTH, Protocol.utf8(new JSONObject().put("mac", mac).toString())));
                out.flush();
            } else {
                writeSealed(Protocol.body(Protocol.PAIR, null));
                out.flush();
                String code = asker.askCode(pcName);
                if (code == null) throw new CancelledException();
                String mac = Protocol.hex(Protocol.pairMac(code.trim(), keys.th));
                writeSealed(Protocol.body(Protocol.CODE, Protocol.utf8(new JSONObject().put("mac", mac).toString())));
                out.flush();
            }

            // 5. WELCOME or DENIED.
            body = open.open(Protocol.readFrame(in));
            if (body.length == 0) throw new IOException("empty frame");
            if (body[0] == Protocol.DENIED) {
                String reason = json(body).optString("reason", "auth");
                if ("auth".equals(reason)) tokens.forgetToken(pcId);
                throw new DeniedException(reason);
            }
            if (body[0] != Protocol.WELCOME) throw new IOException("expected WELCOME, got " + body[0]);
            JSONObject w = json(body);
            if (pairing) {
                token = Protocol.unhex(w.optString("token"));
                if (token == null || token.length != 32) throw new IOException("no token in WELCOME");
            }
            byte[] mac = Protocol.unhex(w.optString("mac"));
            if (!Protocol.equalsConstantTime(mac, Protocol.pcMac(token, keys.th))) {
                throw new IOException("the PC could not prove its identity");
            }
            if (pairing) tokens.putToken(pcId, pcName, token);
            pcName = w.optString("name", pcName);
            screenW = w.optInt("w", 0);
            screenH = w.optInt("h", 0);
            gui = w.optBoolean("gui", false);
            socket.setSoTimeout(READ_TIMEOUT_MS);
        } catch (JSONException e) {
            throw new IOException("bad JSON: " + e.getMessage());
        } catch (GeneralSecurityException e) {
            throw new IOException("crypto: " + e.getMessage());
        }
    }

    private static JSONObject json(byte[] body) throws JSONException {
        return new JSONObject(new String(body, 1, body.length - 1, java.nio.charset.StandardCharsets.UTF_8));
    }

    private void writeSealed(byte[] body) throws IOException {
        try {
            Protocol.writeFrame(out, seal.seal(body));
        } catch (GeneralSecurityException e) {
            throw new IOException("seal: " + e.getMessage());
        }
    }

    private void startWriter() {
        writer = new Thread(this::writeLoop, "connect-writer");
        writer.start();
    }

    private void writeLoop() {
        long lastPing = System.nanoTime();
        try {
            while (!closed) {
                byte[] body = queue.poll(500, TimeUnit.MILLISECONDS);
                long now = System.nanoTime();
                if (now - lastPing >= TimeUnit.MILLISECONDS.toNanos(PING_EVERY_MS)) {
                    lastPing = now;
                    writeSealed(Protocol.body(Protocol.PING, null));
                    if (body == null) out.flush();
                }
                if (body == null) continue;
                writeSealed(body);
                if (queue.isEmpty()) out.flush();
            }
        } catch (IOException | InterruptedException e) {
            // Falls through to close; the reader then fails too.
        }
        close();
    }

    /** Queues a frame; false if the link is closed. Safe from any thread. */
    boolean send(int type, byte[] payload) {
        if (closed) return false;
        queue.add(Protocol.body(type, payload));
        return true;
    }

    boolean sendJson(int type, JSONObject o) {
        return send(type, Protocol.utf8(o.toString()));
    }

    /** Frames waiting to be written. */
    int pending() {
        return queue.size();
    }

    boolean isClosed() {
        return closed;
    }

    /** Reads frames until the link ends; always ends with an exception. */
    void readLoop(Receiver r) throws IOException {
        try {
            while (!closed) {
                byte[] body = open.open(Protocol.readFrame(in));
                if (body.length == 0) throw new IOException("empty frame");
                byte[] payload = new byte[body.length - 1];
                System.arraycopy(body, 1, payload, 0, payload.length);
                r.onMessage(body[0] & 0xff, payload);
            }
            throw new IOException("closed");
        } catch (GeneralSecurityException e) {
            throw new IOException("bad seal");
        } finally {
            close();
        }
    }

    void close() {
        closed = true;
        try {
            socket.close();
        } catch (IOException ignored) {
        }
        Thread w = writer;
        if (w != null && w != Thread.currentThread()) w.interrupt();
    }
}
