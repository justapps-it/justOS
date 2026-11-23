package dev.justos.connect;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/** Everything the app remembers: its id, its name, the PCs it paired with. */
final class Prefs implements Link.TokenStore {
    private static final String FILE = "connect";
    private static final String TOKEN = "token.";
    private static final String NAME = "pcname.";

    private static Prefs instance;
    private final SharedPreferences sp;

    private Prefs(Context c) {
        sp = c.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    static synchronized Prefs get(Context c) {
        if (instance == null) instance = new Prefs(c);
        return instance;
    }

    /** This phone's id: 16 random hex characters, made once. */
    synchronized String phoneId() {
        String id = sp.getString("phone_id", null);
        if (id == null || id.length() != 16) {
            id = Protocol.randomHex(8);
            sp.edit().putString("phone_id", id).apply();
        }
        return id;
    }

    String deviceName() {
        String n = sp.getString("device_name", "");
        return n.trim().isEmpty() ? Build.MODEL : n.trim();
    }

    String customName() {
        return sp.getString("device_name", "");
    }

    void setDeviceName(String name) {
        sp.edit().putString("device_name", name == null ? "" : name.trim()).apply();
    }

    boolean mirrorNotifications() {
        return sp.getBoolean("mirror", false);
    }

    void setMirrorNotifications(boolean on) {
        sp.edit().putBoolean("mirror", on).apply();
    }

    /** 0 low, 1 normal, 2 high. */
    int screenQuality() {
        return sp.getInt("screen_quality", 1);
    }

    void setScreenQuality(int q) {
        sp.edit().putInt("screen_quality", q).apply();
    }

    // The last PC, for reconnecting.

    String lastPcId() {
        return sp.getString("last_id", null);
    }

    String lastHost() {
        return sp.getString("last_host", null);
    }

    int lastPort() {
        return sp.getInt("last_port", Protocol.PORT);
    }

    String lastName() {
        return sp.getString("last_name", null);
    }

    void setLast(String id, String name, String host, int port) {
        sp.edit().putString("last_id", id).putString("last_name", name).putString("last_host", host)
                .putInt("last_port", port).apply();
    }

    /** Whether to reconnect by itself (cleared when the user disconnects). */
    boolean autoConnect() {
        return sp.getBoolean("auto", false);
    }

    void setAutoConnect(boolean on) {
        sp.edit().putBoolean("auto", on).apply();
    }

    // Tokens, one per PC id.

    @Override
    public byte[] token(String pcId) {
        byte[] t = Protocol.unhex(sp.getString(TOKEN + pcId, null));
        return t != null && t.length == 32 ? t : null;
    }

    @Override
    public void putToken(String pcId, String pcName, byte[] token) {
        sp.edit().putString(TOKEN + pcId, Protocol.hex(token)).putString(NAME + pcId, pcName).commit();
    }

    @Override
    public void forgetToken(String pcId) {
        SharedPreferences.Editor e = sp.edit().remove(TOKEN + pcId).remove(NAME + pcId);
        if (pcId.equals(lastPcId())) e.putBoolean("auto", false);
        e.commit();
    }

    boolean isPaired(String pcId) {
        return pcId != null && token(pcId) != null;
    }

    /** The paired PCs as {id, name}. */
    List<String[]> paired() {
        List<String[]> out = new ArrayList<>();
        for (Map.Entry<String, ?> e : sp.getAll().entrySet()) {
            if (e.getKey().startsWith(TOKEN)) {
                String id = e.getKey().substring(TOKEN.length());
                out.add(new String[] {id, sp.getString(NAME + id, id)});
            }
        }
        return out;
    }
}
