package dev.justos.connect;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ServiceInfo;
import android.database.Cursor;
import android.graphics.drawable.Icon;
import android.media.AudioAttributes;
import android.media.AudioManager;
import android.media.MediaPlayer;
import android.media.MediaScannerConnection;
import android.media.RingtoneManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Environment;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;

/**
 * The foreground service that owns the connection to a PC: it connects,
 * pairs, reconnects to the last PC when the Wi-Fi comes back, answers the
 * PC, receives and sends files, sends the battery level and rings the phone.
 * Activities reach it through the static methods (a simple singleton).
 */
public final class ConnectService extends Service {
    static final String ACTION_CONNECT = "dev.justos.connect.CONNECT";
    static final String ACTION_DISCONNECT = "dev.justos.connect.DISCONNECT";
    static final String ACTION_SEND_FILES = "dev.justos.connect.SEND_FILES";
    static final String ACTION_STOP_RING = "dev.justos.connect.STOP_RING";
    static final String EXTRA_HOST = "host";
    static final String EXTRA_PORT = "port";
    static final String EXTRA_ID = "id";
    static final String EXTRA_NAME = "name";

    static final String CH_SERVICE = "service";
    static final String CH_TRANSFER = "transfer";
    static final String CH_FILES = "files";
    static final String CH_RING = "ring";
    private static final int ID_SERVICE = 1;
    private static final int ID_TRANSFER = 2;
    private static final int ID_RING = 3;
    private static int nextFileNotification = 100;

    enum State { IDLE, CONNECTING, PAIRING, CONNECTED, RECONNECTING }

    /** What activities hear about; called on the main thread. */
    interface Listener {
        default void onStateChanged() {}

        default void onNotice(String text) {}

        /** The PC refused: "auth" (it forgot this phone) or "code" (wrong code). */
        default void onDenied(String reason, String pcName) {}
    }

    /** Receives FRAME payloads on the link's reading thread. */
    interface FrameSink {
        void onFrame(byte[] payload);
    }

    private static final CopyOnWriteArrayList<Listener> listeners = new CopyOnWriteArrayList<>();
    private static volatile FrameSink frameSink;
    private static volatile ConnectService instance;
    private static volatile State state = State.IDLE;
    private static volatile Link link;
    private static volatile String host;
    private static volatile int port = Protocol.PORT;
    private static volatile String pcId;
    private static volatile String pcName;
    private static volatile int screenW;
    private static volatile int screenH;
    private static final LinkedBlockingQueue<String> codes = new LinkedBlockingQueue<>();

    private final Handler main = new Handler(Looper.getMainLooper());
    private final Object wake = new Object();
    private Prefs prefs;
    private volatile boolean wanted;
    private volatile int generation;
    private Thread worker;
    private ConnectivityManager.NetworkCallback netCallback;

    // ------------------------------------------------------------------
    // Static API for activities

    static State state() {
        return state;
    }

    static boolean isConnected() {
        return state == State.CONNECTED && link != null;
    }

    static String pcName() {
        return pcName;
    }

    static String pcId() {
        return pcId;
    }

    static String host() {
        return host;
    }

    static int port() {
        return port;
    }

    static int screenW() {
        return screenW;
    }

    static int screenH() {
        return screenH;
    }

    static void addListener(Listener l) {
        listeners.addIfAbsent(l);
    }

    static void removeListener(Listener l) {
        listeners.remove(l);
    }

    static void setFrameSink(FrameSink s) {
        frameSink = s;
    }

    /** Sends a message to the PC; false when not connected. */
    static boolean send(int type, byte[] payload) {
        Link l = link;
        return l != null && state == State.CONNECTED && l.send(type, payload);
    }

    static boolean sendJson(int type, JSONObject o) {
        return send(type, Protocol.utf8(o.toString()));
    }

    /** A JSON object from key, value pairs. */
    static JSONObject json(Object... kv) {
        JSONObject o = new JSONObject();
        try {
            for (int i = 0; i + 1 < kv.length; i += 2) o.put((String) kv[i], kv[i + 1]);
        } catch (JSONException e) {
            throw new IllegalArgumentException(e);
        }
        return o;
    }

    static void connect(Context c, String host, int port, String id, String name) {
        Intent i = new Intent(c, ConnectService.class).setAction(ACTION_CONNECT)
                .putExtra(EXTRA_HOST, host).putExtra(EXTRA_PORT, port)
                .putExtra(EXTRA_ID, id).putExtra(EXTRA_NAME, name);
        c.startForegroundService(i);
    }

    /** The user asked to disconnect: stop and do not reconnect by itself. */
    static void disconnect(Context c) {
        Prefs.get(c).setAutoConnect(false);
        ConnectService s = instance;
        if (s != null) s.shutdown();
    }

    static void submitCode(String code) {
        codes.offer(code);
    }

    static void cancelPairing() {
        codes.offer("");
    }

    /** Hands files to the service, granting it read access to the URIs. */
    static void sendFiles(Context c, List<Uri> uris) {
        if (uris.isEmpty()) return;
        ClipData clip = ClipData.newRawUri("files", uris.get(0));
        for (int i = 1; i < uris.size(); i++) clip.addItem(new ClipData.Item(uris.get(i)));
        Intent i = new Intent(c, ConnectService.class).setAction(ACTION_SEND_FILES);
        i.setClipData(clip);
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        c.startService(i);
    }

    static void stopRinging(Context c) {
        ConnectService s = instance;
        if (s != null) s.stopRing();
    }

    static boolean isRinging() {
        ConnectService s = instance;
        return s != null && s.ringPlayer != null;
    }

    // ------------------------------------------------------------------
    // Service lifecycle

    @Override
    public void onCreate() {
        super.onCreate();
        instance = this;
        prefs = Prefs.get(this);
        createChannels(this);
        IntentFilter f = new IntentFilter(Intent.ACTION_BATTERY_CHANGED);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(batteryReceiver, f, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(batteryReceiver, f);
        }
        ConnectivityManager cm = getSystemService(ConnectivityManager.class);
        netCallback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                synchronized (wake) {
                    wake.notifyAll();
                }
            }
        };
        try {
            cm.registerDefaultNetworkCallback(netCallback);
        } catch (RuntimeException e) {
            netCallback = null;
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (!goForeground()) {
            stopSelf();
            return START_NOT_STICKY;
        }
        String action = intent == null ? null : intent.getAction();
        if (ACTION_CONNECT.equals(action)) {
            String h = intent.getStringExtra(EXTRA_HOST);
            if (h != null && !h.isEmpty()) {
                startWorker(h, intent.getIntExtra(EXTRA_PORT, Protocol.PORT),
                        intent.getStringExtra(EXTRA_ID), intent.getStringExtra(EXTRA_NAME));
            }
        } else if (ACTION_DISCONNECT.equals(action)) {
            prefs.setAutoConnect(false);
            shutdown();
        } else if (ACTION_SEND_FILES.equals(action)) {
            ClipData clip = intent.getClipData();
            List<Uri> uris = new ArrayList<>();
            if (clip != null) {
                for (int i = 0; i < clip.getItemCount(); i++) {
                    Uri u = clip.getItemAt(i).getUri();
                    if (u != null) uris.add(u);
                }
            }
            if (!uris.isEmpty()) fileExec.execute(() -> sendFilesNow(uris));
        } else if (ACTION_STOP_RING.equals(action)) {
            stopRing();
        }
        if (state == State.IDLE && worker == null) {
            // Started without a target (for example to stop a ring): nothing to keep alive.
            main.post(() -> {
                if (state == State.IDLE && worker == null && ringPlayer == null) shutdown();
            });
        }
        return START_NOT_STICKY;
    }

    private boolean goForeground() {
        try {
            Notification n = serviceNotification();
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(ID_SERVICE, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
            } else {
                startForeground(ID_SERVICE, n);
            }
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public void onDestroy() {
        wanted = false;
        generation++;
        Link l = link;
        if (l != null) l.close();
        Thread w = worker;
        if (w != null) w.interrupt();
        worker = null;
        stopRing();
        try {
            unregisterReceiver(batteryReceiver);
        } catch (RuntimeException ignored) {
        }
        if (netCallback != null) {
            try {
                getSystemService(ConnectivityManager.class).unregisterNetworkCallback(netCallback);
            } catch (RuntimeException ignored) {
            }
        }
        fileExec.shutdownNow();
        instance = null;
        state = State.IDLE;
        notifyState();
        super.onDestroy();
    }

    /** Ends everything and stops the service. Main thread. */
    private void shutdown() {
        wanted = false;
        generation++;
        Link l = link;
        if (l != null) l.close();
        Thread w = worker;
        if (w != null) w.interrupt();
        worker = null;
        codes.offer("");
        state = State.IDLE;
        notifyState();
        if (ringPlayer == null) {
            stopForeground(STOP_FOREGROUND_REMOVE);
            stopSelf();
        }
    }

    // ------------------------------------------------------------------
    // Connecting

    private void startWorker(String h, int p, String id, String name) {
        Link old = link;
        if (old != null) old.close();
        Thread oldWorker = worker;
        if (oldWorker != null) oldWorker.interrupt();
        final int gen = ++generation;
        host = h;
        port = p;
        pcId = id;
        pcName = name != null && !name.isEmpty() ? name : h;
        wanted = true;
        codes.clear();
        Thread t = new Thread(() -> runWorker(gen), "connect-worker");
        worker = t;
        t.start();
    }

    private boolean alive(int gen) {
        return wanted && gen == generation;
    }

    private void runWorker(int gen) {
        boolean everConnected = false;
        long backoff = 2000;
        boolean first = true;
        while (alive(gen)) {
            if (!first) {
                // Lost the PC: wait, then look for it again (its address may have changed).
                setState(gen, State.RECONNECTING);
                synchronized (wake) {
                    try {
                        wake.wait(backoff);
                    } catch (InterruptedException e) {
                        break;
                    }
                }
                backoff = Math.min(backoff * 2, 30000);
                if (!alive(gen)) break;
                if (!onLocalNetwork()) continue;
                String id = pcId;
                if (id != null) {
                    for (Discovery.Found f : Discovery.scanBlocking(this, 2500, id, null)) {
                        if (f.id.equals(id)) {
                            host = f.host;
                            port = f.port;
                        }
                    }
                }
                if (!alive(gen)) break;
            }
            first = false;
            setState(gen, State.CONNECTING);
            Link l = null;
            try {
                l = Link.connect(host, port, prefs.phoneId(), prefs.deviceName(), prefs, n -> askCode(gen, n));
                if (!alive(gen)) break;
                link = l;
                pcId = l.pcId;
                pcName = l.pcName;
                screenW = l.screenW;
                screenH = l.screenH;
                everConnected = true;
                backoff = 2000;
                prefs.setLast(l.pcId, l.pcName, host, port);
                prefs.setAutoConnect(true);
                setState(gen, State.CONNECTED);
                onLinkUp();
                notice(getString(R.string.toast_connected, l.pcName));
                l.readLoop(this::onMessage);
            } catch (Link.DeniedException e) {
                if (alive(gen)) {
                    String name = pcName;
                    main.post(() -> {
                        for (Listener x : listeners) x.onDenied(e.reason, name);
                    });
                    stopFromWorker(gen);
                }
                return;
            } catch (Link.CancelledException e) {
                if (!everConnected) {
                    stopFromWorker(gen);
                    return;
                }
            } catch (InterruptedException e) {
                break;
            } catch (IOException | RuntimeException e) {
                if (alive(gen) && !everConnected) {
                    String why = e.getMessage() == null ? e.getClass().getSimpleName() : e.getMessage();
                    notice(getString(R.string.toast_connect_failed, why));
                    stopFromWorker(gen);
                    return;
                }
            } finally {
                if (l != null) l.close();
                if (l != null && link == l) {
                    link = null;
                    onLinkDown();
                }
            }
        }
    }

    private void stopFromWorker(int gen) {
        main.post(() -> {
            if (gen == generation) shutdown();
        });
    }

    private String askCode(int gen, String name) throws InterruptedException {
        codes.clear();
        pcName = name;
        setState(gen, State.PAIRING);
        String c = codes.poll(120, TimeUnit.SECONDS);
        if (c == null || c.isEmpty() || !alive(gen)) return null;
        setState(gen, State.CONNECTING);
        return c;
    }

    private boolean onLocalNetwork() {
        ConnectivityManager cm = getSystemService(ConnectivityManager.class);
        Network n = cm.getActiveNetwork();
        NetworkCapabilities caps = n == null ? null : cm.getNetworkCapabilities(n);
        return caps != null && (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET));
    }

    private void setState(int gen, State s) {
        if (gen != generation) return;
        state = s;
        notifyState();
    }

    private void notifyState() {
        main.post(() -> {
            if (instance == this && state != State.IDLE) {
                NotificationManager nm = getSystemService(NotificationManager.class);
                nm.notify(ID_SERVICE, serviceNotification());
            }
            for (Listener x : listeners) x.onStateChanged();
        });
    }

    private void notice(String text) {
        main.post(() -> {
            for (Listener x : listeners) x.onNotice(text);
        });
    }

    private void onLinkUp() {
        lastLevel = -1;
        Intent sticky = registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED));
        if (sticky != null) onBattery(sticky);
    }

    private void onLinkDown() {
        abortIncoming();
        cancelOutgoing = true;
        fileDoneOk = false;
        fileDone.release();
    }

    // ------------------------------------------------------------------
    // Messages from the PC (on the link's reading thread)

    private void onMessage(int type, byte[] p) throws IOException {
        switch (type) {
            case Protocol.PONG:
                break;
            case Protocol.FRAME: {
                FrameSink s = frameSink;
                if (s != null) s.onFrame(p);
                break;
            }
            case Protocol.CLIP: {
                String text = Protocol.utf8(p);
                main.post(() -> {
                    ClipboardManager cm = getSystemService(ClipboardManager.class);
                    cm.setPrimaryClip(ClipData.newPlainText("JustOS", text));
                    notice(getString(R.string.toast_clip_received));
                });
                break;
            }
            case Protocol.RING:
                main.post(this::startRing);
                break;
            case Protocol.FILE_BEGIN:
                fileBegin(p);
                break;
            case Protocol.FILE_DATA:
                fileData(p);
                break;
            case Protocol.FILE_END:
                fileEnd();
                break;
            case Protocol.FILE_CANCEL:
                abortIncoming();
                cancelOutgoing = true;
                break;
            case Protocol.FILE_DONE: {
                JSONObject o = parse(p);
                fileDoneOk = o.optBoolean("ok", false);
                fileDone.release();
                break;
            }
            default:
                // Unknown or phone-to-PC types: ignore.
                break;
        }
    }

    private static JSONObject parse(byte[] p) {
        try {
            return new JSONObject(Protocol.utf8(p));
        } catch (JSONException e) {
            return new JSONObject();
        }
    }

    // ------------------------------------------------------------------
    // Files from the PC, to Downloads/JustOS

    private static final class Incoming {
        String name;
        long size;
        long got;
        OutputStream out;
        Uri uri;
        File file;
        int shownPercent = -1;
        int notification;
    }

    private Incoming incoming;

    private void fileBegin(byte[] p) {
        abortIncoming();
        JSONObject o = parse(p);
        Incoming in = new Incoming();
        in.name = o.optString("name", "file");
        in.size = o.optLong("size", -1);
        in.notification = nextFileNotification++;
        String clean = cleanName(in.name);
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                ContentValues v = new ContentValues();
                v.put(MediaStore.MediaColumns.DISPLAY_NAME, clean);
                v.put(MediaStore.MediaColumns.MIME_TYPE, mimeOf(clean));
                v.put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/JustOS");
                v.put(MediaStore.MediaColumns.IS_PENDING, 1);
                in.uri = getContentResolver().insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, v);
                if (in.uri == null) throw new IOException("MediaStore refused");
                in.out = getContentResolver().openOutputStream(in.uri);
                if (in.out == null) throw new IOException("no stream");
            } else {
                File dir = new File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "JustOS");
                if (!dir.isDirectory() && !dir.mkdirs() || !dir.canWrite()) {
                    // No storage permission: the app's own Downloads folder.
                    dir = new File(getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "JustOS");
                    if (!dir.isDirectory() && !dir.mkdirs()) throw new IOException("no folder");
                }
                in.file = uniqueFile(dir, clean);
                in.out = new FileOutputStream(in.file);
            }
        } catch (IOException | RuntimeException e) {
            discard(in);
            send(Protocol.FILE_DONE, Protocol.utf8(json("name", in.name, "ok", false).toString()));
            fileNotification(in.notification, getString(R.string.notif_receive_failed, in.name), false);
            return;
        }
        incoming = in;
        progress(getString(R.string.notif_receiving, in.name), 0, in.size);
    }

    private void fileData(byte[] p) {
        Incoming in = incoming;
        if (in == null) return;
        try {
            in.out.write(p);
        } catch (IOException e) {
            incoming = null;
            discard(in);
            send(Protocol.FILE_DONE, Protocol.utf8(json("name", in.name, "ok", false).toString()));
            cancelProgress();
            fileNotification(in.notification, getString(R.string.notif_receive_failed, in.name), false);
            return;
        }
        in.got += p.length;
        if (in.size > 0) {
            int pct = (int) (in.got * 100 / in.size);
            if (pct != in.shownPercent) {
                in.shownPercent = pct;
                progress(getString(R.string.notif_receiving, in.name), in.got, in.size);
            }
        }
    }

    private void fileEnd() {
        Incoming in = incoming;
        if (in == null) return;
        incoming = null;
        boolean ok = in.size < 0 || in.got == in.size;
        try {
            in.out.close();
        } catch (IOException e) {
            ok = false;
        }
        if (ok) {
            if (in.uri != null) {
                ContentValues v = new ContentValues();
                v.put(MediaStore.MediaColumns.IS_PENDING, 0);
                getContentResolver().update(in.uri, v, null, null);
            } else if (in.file != null) {
                MediaScannerConnection.scanFile(this, new String[] {in.file.getPath()}, null, null);
            }
        } else {
            discard(in);
        }
        send(Protocol.FILE_DONE, Protocol.utf8(json("name", in.name, "ok", ok).toString()));
        cancelProgress();
        fileNotification(in.notification,
                getString(ok ? R.string.notif_received : R.string.notif_receive_failed, in.name), ok);
    }

    private void abortIncoming() {
        Incoming in = incoming;
        incoming = null;
        if (in != null) {
            discard(in);
            cancelProgress();
        }
    }

    private void discard(Incoming in) {
        try {
            if (in.out != null) in.out.close();
        } catch (IOException ignored) {
        }
        try {
            if (in.uri != null) getContentResolver().delete(in.uri, null, null);
            if (in.file != null) in.file.delete();
        } catch (RuntimeException ignored) {
        }
    }

    private static String cleanName(String name) {
        StringBuilder b = new StringBuilder();
        for (char c : name.toCharArray()) {
            if (c < 32 || c == '/' || c == '\\' || c == ':' || c == '*' || c == '?' || c == '"' || c == '<'
                    || c == '>' || c == '|') continue;
            b.append(c);
        }
        String s = b.toString().trim();
        while (s.startsWith(".")) s = s.substring(1);
        if (s.length() > 120) s = s.substring(s.length() - 120);
        return s.isEmpty() ? "file" : s;
    }

    private static String mimeOf(String name) {
        int dot = name.lastIndexOf('.');
        String type = dot < 0 ? null
                : MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT));
        return type == null ? "application/octet-stream" : type;
    }

    private static File uniqueFile(File dir, String name) {
        File f = new File(dir, name);
        int dot = name.lastIndexOf('.');
        String stem = dot > 0 ? name.substring(0, dot) : name;
        String ext = dot > 0 ? name.substring(dot) : "";
        for (int n = 2; f.exists() && n < 1000; n++) f = new File(dir, stem + " (" + n + ")" + ext);
        return f;
    }

    // ------------------------------------------------------------------
    // Files to the PC (on the file thread)

    private final ExecutorService fileExec = Executors.newSingleThreadExecutor();
    private final Semaphore fileDone = new Semaphore(0);
    private volatile boolean fileDoneOk;
    private volatile boolean cancelOutgoing;

    private void sendFilesNow(List<Uri> uris) {
        int sent = 0;
        int failed = 0;
        String lastFailed = null;
        for (Uri u : uris) {
            Link l = link;
            if (l == null || state != State.CONNECTED) {
                failed += uris.size() - sent - failed;
                break;
            }
            String[] name = {"file"};
            boolean ok;
            try {
                ok = sendOne(l, u, name);
            } catch (Exception e) {
                ok = false;
            }
            if (ok) {
                sent++;
            } else {
                failed++;
                lastFailed = name[0];
            }
        }
        cancelProgress();
        if (failed == 0) {
            fileNotification(nextFileNotification++, getString(R.string.notif_sent_files, sent), true);
        } else {
            String what = lastFailed != null ? lastFailed : getString(R.string.notif_files);
            fileNotification(nextFileNotification++, getString(R.string.notif_send_failed, what), false);
        }
    }

    private boolean sendOne(Link l, Uri u, String[] nameOut) throws Exception {
        ContentResolver cr = getContentResolver();
        String name = u.getLastPathSegment() != null ? u.getLastPathSegment() : "file";
        long size = -1;
        try (Cursor c = cr.query(u, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int ni = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                int si = c.getColumnIndex(OpenableColumns.SIZE);
                if (ni >= 0 && !c.isNull(ni)) name = c.getString(ni);
                if (si >= 0 && !c.isNull(si)) size = c.getLong(si);
            }
        } catch (RuntimeException ignored) {
            // Some providers do not answer queries; the name and size come from elsewhere.
        }
        name = cleanName(name);
        nameOut[0] = name;
        File temp = null;
        InputStream in;
        if (size < 0) {
            // Size unknown: copy it first, the PC needs the size up front.
            temp = File.createTempFile("send", ".tmp", getCacheDir());
            try (InputStream src = cr.openInputStream(u); OutputStream dst = new FileOutputStream(temp)) {
                if (src == null) throw new IOException("cannot open");
                byte[] b = new byte[Protocol.CHUNK];
                int n;
                while ((n = src.read(b)) > 0) dst.write(b, 0, n);
            }
            size = temp.length();
            in = new FileInputStream(temp);
        } else {
            in = cr.openInputStream(u);
            if (in == null) throw new IOException("cannot open");
        }
        try {
            fileDone.drainPermits();
            cancelOutgoing = false;
            fileDoneOk = false;
            l.send(Protocol.FILE_BEGIN, Protocol.utf8(json("name", name, "size", size).toString()));
            progress(getString(R.string.notif_sending, name), 0, size);
            byte[] buf = new byte[Protocol.CHUNK];
            long done = 0;
            int shown = -1;
            while (true) {
                int n = readFull(in, buf);
                if (n <= 0) break;
                while (l.pending() > 16 && !l.isClosed() && !cancelOutgoing) Thread.sleep(4);
                if (l.isClosed()) return false;
                if (cancelOutgoing) {
                    l.send(Protocol.FILE_CANCEL, null);
                    return false;
                }
                l.send(Protocol.FILE_DATA, Arrays.copyOf(buf, n));
                done += n;
                int pct = size > 0 ? (int) (done * 100 / size) : 0;
                if (pct != shown) {
                    shown = pct;
                    progress(getString(R.string.notif_sending, name), done, size);
                }
            }
            l.send(Protocol.FILE_END, null);
            return fileDone.tryAcquire(120, TimeUnit.SECONDS) && fileDoneOk;
        } finally {
            in.close();
            if (temp != null) temp.delete();
        }
    }

    private static int readFull(InputStream in, byte[] buf) throws IOException {
        int got = 0;
        while (got < buf.length) {
            int n = in.read(buf, got, buf.length - got);
            if (n < 0) break;
            got += n;
        }
        return got;
    }

    // ------------------------------------------------------------------
    // Battery

    private int lastLevel = -1;
    private boolean lastCharging;

    private final BroadcastReceiver batteryReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            onBattery(intent);
        }
    };

    private void onBattery(Intent i) {
        int level = i.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
        int scale = i.getIntExtra(BatteryManager.EXTRA_SCALE, -1);
        if (level < 0 || scale <= 0) return;
        int pct = Math.max(0, Math.min(100, level * 100 / scale));
        int status = i.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
        boolean charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL;
        if (!isConnected()) return;
        if (pct == lastLevel && charging == lastCharging) return;
        if (sendJson(Protocol.BATTERY, json("level", pct, "charging", charging))) {
            lastLevel = pct;
            lastCharging = charging;
        }
    }

    // ------------------------------------------------------------------
    // Find my phone

    private MediaPlayer ringPlayer;
    private int savedAlarmVolume = -1;

    private void startRing() {
        if (ringPlayer != null) return;
        AudioManager am = getSystemService(AudioManager.class);
        try {
            savedAlarmVolume = am.getStreamVolume(AudioManager.STREAM_ALARM);
            am.setStreamVolume(AudioManager.STREAM_ALARM, am.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0);
        } catch (RuntimeException e) {
            savedAlarmVolume = -1;
        }
        AudioAttributes attrs = new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build();
        MediaPlayer mp = new MediaPlayer();
        try {
            Uri uri = RingtoneManager.getActualDefaultRingtoneUri(this, RingtoneManager.TYPE_ALARM);
            if (uri == null) uri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE);
            mp.setAudioAttributes(attrs);
            mp.setDataSource(this, uri);
            mp.setLooping(true);
            mp.prepare();
            mp.start();
        } catch (IOException | RuntimeException e) {
            // No playable tone: vibration and the notification still work.
        }
        ringPlayer = mp;
        Vibrator v = getSystemService(Vibrator.class);
        if (v != null && v.hasVibrator()) {
            v.vibrate(VibrationEffect.createWaveform(new long[] {0, 800, 400}, 0), attrs);
        }
        Intent stop = new Intent(this, ConnectService.class).setAction(ACTION_STOP_RING);
        PendingIntent stopPi = PendingIntent.getService(this, 3, stop, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Intent show = new Intent(this, RingActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        PendingIntent showPi = PendingIntent.getActivity(this, 4, show, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification n = new Notification.Builder(this, CH_RING)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(Ui.ACCENT)
                .setContentTitle(getString(R.string.ring_title))
                .setContentText(getString(R.string.ring_text, pcName == null ? "" : pcName))
                .setCategory(Notification.CATEGORY_ALARM)
                .setOngoing(true)
                .setContentIntent(showPi)
                .setFullScreenIntent(showPi, true)
                .addAction(new Notification.Action.Builder(Icon.createWithResource(this, R.drawable.ic_notify),
                        getString(R.string.action_stop), stopPi).build())
                .build();
        getSystemService(NotificationManager.class).notify(ID_RING, n);
        try {
            startActivity(show);
        } catch (RuntimeException ignored) {
            // In the background the notification is the way in.
        }
    }

    private void stopRing() {
        MediaPlayer mp = ringPlayer;
        if (mp == null) return;
        ringPlayer = null;
        try {
            mp.stop();
        } catch (RuntimeException ignored) {
        }
        mp.release();
        Vibrator v = getSystemService(Vibrator.class);
        if (v != null) v.cancel();
        if (savedAlarmVolume >= 0) {
            try {
                getSystemService(AudioManager.class).setStreamVolume(AudioManager.STREAM_ALARM, savedAlarmVolume, 0);
            } catch (RuntimeException ignored) {
            }
            savedAlarmVolume = -1;
        }
        getSystemService(NotificationManager.class).cancel(ID_RING);
        RingActivity.closeAll();
        if (state == State.IDLE) {
            stopForeground(STOP_FOREGROUND_REMOVE);
            stopSelf();
        }
    }

    // ------------------------------------------------------------------
    // Notifications

    static void createChannels(Context c) {
        NotificationManager nm = c.getSystemService(NotificationManager.class);
        NotificationChannel service = new NotificationChannel(CH_SERVICE, c.getString(R.string.channel_service), NotificationManager.IMPORTANCE_LOW);
        service.setShowBadge(false);
        NotificationChannel transfer = new NotificationChannel(CH_TRANSFER, c.getString(R.string.channel_transfer), NotificationManager.IMPORTANCE_LOW);
        transfer.setShowBadge(false);
        NotificationChannel files = new NotificationChannel(CH_FILES, c.getString(R.string.channel_files), NotificationManager.IMPORTANCE_DEFAULT);
        NotificationChannel ring = new NotificationChannel(CH_RING, c.getString(R.string.channel_ring), NotificationManager.IMPORTANCE_HIGH);
        ring.setSound(null, null);
        nm.createNotificationChannels(Arrays.asList(service, transfer, files, ring));
    }

    private Notification serviceNotification() {
        String name = pcName == null ? "" : pcName;
        String text;
        switch (state) {
            case CONNECTED:
                text = getString(R.string.notif_connected, name);
                break;
            case PAIRING:
                text = getString(R.string.pair_message, name);
                break;
            case RECONNECTING:
                text = getString(R.string.notif_reconnecting, name);
                break;
            default:
                text = getString(R.string.notif_connecting, name);
                break;
        }
        PendingIntent open = PendingIntent.getActivity(this, 0,
                new Intent(this, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent disc = PendingIntent.getService(this, 1,
                new Intent(this, ConnectService.class).setAction(ACTION_DISCONNECT),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(this, CH_SERVICE)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(Ui.ACCENT)
                .setContentTitle(getString(R.string.app_name))
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_SERVICE)
                .setContentIntent(open)
                .addAction(new Notification.Action.Builder(Icon.createWithResource(this, R.drawable.ic_notify),
                        getString(R.string.action_disconnect), disc).build())
                .build();
    }

    private void progress(String title, long done, long size) {
        int max = 1000;
        int at = size > 0 ? (int) Math.min(max, done * max / size) : 0;
        Notification n = new Notification.Builder(this, CH_TRANSFER)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(Ui.ACCENT)
                .setContentTitle(title)
                .setProgress(max, at, size <= 0)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .build();
        getSystemService(NotificationManager.class).notify(ID_TRANSFER, n);
    }

    private void cancelProgress() {
        getSystemService(NotificationManager.class).cancel(ID_TRANSFER);
    }

    private void fileNotification(int id, String text, boolean openDownloads) {
        Notification.Builder b = new Notification.Builder(this, CH_FILES)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(Ui.ACCENT)
                .setContentTitle(getString(R.string.app_name))
                .setContentText(text)
                .setAutoCancel(true);
        if (openDownloads) {
            Intent i = new Intent(android.app.DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            b.setContentIntent(PendingIntent.getActivity(this, id, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
        }
        getSystemService(NotificationManager.class).notify(id, b.build());
    }
}
