package dev.justos.connect;

import android.content.Context;
import android.net.wifi.WifiManager;
import android.os.Handler;
import android.os.Looper;

import org.json.JSONObject;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.net.SocketTimeoutException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Finds justOS computers on the local network: a UDP broadcast of
 * "JUSTOS-CONNECT?" to port 47800, answered with "JUSTOS-CONNECT {json}".
 */
final class Discovery {
    /** A computer that answered. */
    static final class Found {
        final String id;
        final String name;
        final String host;
        final int port;

        Found(String id, String name, String host, int port) {
            this.id = id;
            this.name = name;
            this.host = host;
            this.port = port;
        }
    }

    interface Callback {
        /** Called on the main thread for each new computer. */
        void onFound(Found f);

        /** Called on the main thread when the scan ends. */
        void onDone();
    }

    private Discovery() {}

    /** Scans in the background for {@code ms} milliseconds. */
    static void scan(Context ctx, long ms, Callback cb) {
        Context app = ctx.getApplicationContext();
        Handler main = new Handler(Looper.getMainLooper());
        new Thread(() -> {
            scanBlocking(app, ms, null, f -> main.post(() -> cb.onFound(f)));
            main.post(cb::onDone);
        }, "connect-discovery").start();
    }

    interface Sink {
        void found(Found f);
    }

    /**
     * Scans on this thread. If {@code wantedId} is given, returns as soon as
     * that computer answers. Returns everything found.
     */
    static List<Found> scanBlocking(Context ctx, long ms, String wantedId, Sink sink) {
        List<Found> found = new ArrayList<>();
        WifiManager wifi = (WifiManager) ctx.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        WifiManager.MulticastLock lock = null;
        try {
            if (wifi != null) {
                lock = wifi.createMulticastLock("justos-connect");
                lock.setReferenceCounted(false);
                lock.acquire();
            }
        } catch (RuntimeException e) {
            lock = null;
        }
        Set<String> seen = new HashSet<>();
        try (DatagramSocket s = new DatagramSocket()) {
            s.setBroadcast(true);
            s.setSoTimeout(200);
            byte[] query = Protocol.utf8(Protocol.DISCOVERY_QUERY);
            Set<InetAddress> targets = broadcastAddresses();
            long start = System.currentTimeMillis();
            long nextSend = start;
            int sends = 0;
            byte[] buf = new byte[2048];
            while (System.currentTimeMillis() - start < ms) {
                long now = System.currentTimeMillis();
                if (sends < 3 && now >= nextSend) {
                    for (InetAddress a : targets) {
                        try {
                            s.send(new DatagramPacket(query, query.length, a, Protocol.DISCOVERY_PORT));
                        } catch (Exception ignored) {
                            // One interface failing must not stop the others.
                        }
                    }
                    sends++;
                    nextSend = now + 600;
                }
                DatagramPacket p = new DatagramPacket(buf, buf.length);
                try {
                    s.receive(p);
                } catch (SocketTimeoutException e) {
                    continue;
                }
                Found f = parse(p);
                if (f == null || !seen.add(f.id)) continue;
                found.add(f);
                if (sink != null) sink.found(f);
                if (f.id.equals(wantedId)) break;
            }
        } catch (Exception ignored) {
            // No network: nothing found.
        } finally {
            if (lock != null && lock.isHeld()) lock.release();
        }
        return found;
    }

    private static Found parse(DatagramPacket p) {
        try {
            String s = new String(p.getData(), p.getOffset(), p.getLength(), java.nio.charset.StandardCharsets.UTF_8);
            if (!s.startsWith(Protocol.DISCOVERY_REPLY)) return null;
            JSONObject o = new JSONObject(s.substring(Protocol.DISCOVERY_REPLY.length()));
            String id = o.getString("id");
            if (id.isEmpty()) return null;
            String host = p.getAddress().getHostAddress();
            int port = o.optInt("port", Protocol.PORT);
            if (port <= 0 || port > 65535) port = Protocol.PORT;
            return new Found(id, o.optString("name", host), host, port);
        } catch (Exception e) {
            return null;
        }
    }

    /** 255.255.255.255 plus the broadcast address of every IPv4 interface that is up. */
    private static Set<InetAddress> broadcastAddresses() {
        Set<InetAddress> out = new LinkedHashSet<>();
        try {
            out.add(InetAddress.getByName("255.255.255.255"));
        } catch (Exception ignored) {
        }
        try {
            for (NetworkInterface ni : Collections.list(NetworkInterface.getNetworkInterfaces())) {
                if (!ni.isUp() || ni.isLoopback()) continue;
                for (InterfaceAddress ia : ni.getInterfaceAddresses()) {
                    if (ia.getAddress() instanceof Inet4Address && ia.getBroadcast() != null) out.add(ia.getBroadcast());
                }
            }
        } catch (Exception ignored) {
        }
        return out;
    }
}
