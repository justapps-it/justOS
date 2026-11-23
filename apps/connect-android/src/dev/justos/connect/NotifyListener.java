package dev.justos.connect;

import android.app.Notification;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.service.notification.NotificationListenerService;
import android.service.notification.StatusBarNotification;

import java.util.ArrayDeque;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;

/**
 * Mirrors the phone's notifications to the PC as NOTIFY, when the user
 * turned it on. Skips ongoing ones, our own, group summaries and repeats,
 * and sends at most a few per second.
 */
public final class NotifyListener extends NotificationListenerService {
    private static final long WINDOW_MS = 10_000;
    private static final int MAX_PER_WINDOW = 8;
    private static final long REPEAT_MS = 15_000;

    private final ArrayDeque<Long> sent = new ArrayDeque<>();
    private final Map<String, Long> recent = new HashMap<>();

    @Override
    public void onNotificationPosted(StatusBarNotification sbn) {
        if (!Prefs.get(this).mirrorNotifications() || !ConnectService.isConnected()) return;
        if (sbn.isOngoing() || getPackageName().equals(sbn.getPackageName())) return;
        Notification n = sbn.getNotification();
        if ((n.flags & (Notification.FLAG_GROUP_SUMMARY | Notification.FLAG_FOREGROUND_SERVICE)) != 0) return;
        Bundle x = n.extras;
        if (x == null) return;
        String title = str(x.getCharSequence(Notification.EXTRA_TITLE));
        CharSequence big = x.getCharSequence(Notification.EXTRA_BIG_TEXT);
        String text = str(big != null ? big : x.getCharSequence(Notification.EXTRA_TEXT));
        if (title.isEmpty() && text.isEmpty()) return;
        if (text.length() > 500) text = text.substring(0, 500);

        long now = System.currentTimeMillis();
        // The same notification again (updates, re-posts): once is enough.
        String key = sbn.getPackageName() + '\n' + title + '\n' + text;
        for (Iterator<Map.Entry<String, Long>> it = recent.entrySet().iterator(); it.hasNext(); ) {
            if (now - it.next().getValue() > REPEAT_MS) it.remove();
        }
        if (recent.containsKey(key)) return;
        while (!sent.isEmpty() && now - sent.peekFirst() > WINDOW_MS) sent.pollFirst();
        if (sent.size() >= MAX_PER_WINDOW) return;

        if (ConnectService.sendJson(Protocol.NOTIFY,
                ConnectService.json("app", appLabel(sbn.getPackageName()), "title", title, "text", text))) {
            recent.put(key, now);
            sent.addLast(now);
        }
    }

    private String appLabel(String pkg) {
        PackageManager pm = getPackageManager();
        try {
            ApplicationInfo ai = pm.getApplicationInfo(pkg, 0);
            return pm.getApplicationLabel(ai).toString();
        } catch (PackageManager.NameNotFoundException | RuntimeException e) {
            return pkg;
        }
    }

    private static String str(CharSequence s) {
        return s == null ? "" : s.toString().trim();
    }
}
