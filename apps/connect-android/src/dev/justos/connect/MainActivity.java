package dev.justos.connect;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.text.InputFilter;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.util.LinkedHashMap;
import java.util.Map;

/** Home: the computers nearby, the connected one and what to do with it. */
public final class MainActivity extends Activity implements ConnectService.Listener {
    private Prefs prefs;
    private LinearLayout statusBox;
    private LinearLayout foundList;
    private TextView emptyText;
    private ProgressBar scanning;
    private Button rescan;
    private final Map<String, Discovery.Found> found = new LinkedHashMap<>();
    private AlertDialog pairDialog;
    private boolean autoTried;
    private boolean visible;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = Prefs.get(this);
        Ui.setupWindow(this);

        LinearLayout root = Ui.column(this);
        int pad = Ui.dp(this, 20);
        root.setPadding(pad, Ui.dp(this, 12), pad, pad);

        // Header: title and settings.
        LinearLayout header = Ui.row(this);
        LinearLayout titles = Ui.column(this);
        TextView title = Ui.text(this, getString(R.string.app_name), 28, Ui.TEXT, true);
        TextView sub = Ui.text(this, getString(R.string.home_subtitle), 14, Ui.TEXT_DIM, false);
        titles.addView(title);
        titles.addView(sub);
        header.addView(titles, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        Button settings = Ui.button(this, R.string.settings, Ui.SECONDARY);
        settings.setOnClickListener(v -> startActivity(new Intent(this, SettingsActivity.class)));
        header.addView(settings);
        LinearLayout.LayoutParams hlp = Ui.matchWrap();
        hlp.bottomMargin = Ui.dp(this, 16);
        hlp.topMargin = Ui.dp(this, 8);
        root.addView(header, hlp);

        statusBox = Ui.column(this);
        root.addView(statusBox, Ui.matchWrap());

        // Computers on the network.
        LinearLayout foundHeader = Ui.row(this);
        foundHeader.addView(Ui.heading(this, R.string.home_found), new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        scanning = new ProgressBar(this);
        scanning.setIndeterminateTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
        foundHeader.addView(scanning, new LinearLayout.LayoutParams(Ui.dp(this, 20), Ui.dp(this, 20)));
        root.addView(foundHeader, Ui.matchWrap());

        foundList = Ui.column(this);
        root.addView(foundList, Ui.matchWrap());
        emptyText = Ui.text(this, getString(R.string.home_none_found), 14, Ui.TEXT_DIM, false);
        emptyText.setPadding(Ui.dp(this, 4), 0, Ui.dp(this, 4), Ui.dp(this, 12));
        root.addView(emptyText);

        LinearLayout actions = Ui.row(this);
        rescan = Ui.button(this, R.string.home_rescan, Ui.SECONDARY);
        rescan.setOnClickListener(v -> scan());
        actions.addView(rescan, Ui.weighted(this, 1, 8));
        Button byIp = Ui.button(this, R.string.home_connect_ip, Ui.SECONDARY);
        byIp.setOnClickListener(v -> askIp());
        actions.addView(byIp, Ui.weighted(this, 1, 0));
        root.addView(actions, Ui.matchWrap());

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.addView(root);
        scroll.setClipToPadding(false);
        Ui.padForInsets(scroll);
        setContentView(scroll);

        askPermissions();
        scan();
    }

    private void askPermissions() {
        if (Build.VERSION.SDK_INT >= 33
                && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 1);
        } else if (Build.VERSION.SDK_INT < 29
                && checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.WRITE_EXTERNAL_STORAGE}, 2);
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        visible = true;
        ConnectService.addListener(this);
        render();
    }

    @Override
    protected void onStop() {
        visible = false;
        ConnectService.removeListener(this);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
        if (pairDialog != null) pairDialog.dismiss();
        super.onDestroy();
    }

    // ------------------------------------------------------------------
    // Discovery

    private void scan() {
        scanning.setVisibility(View.VISIBLE);
        rescan.setEnabled(false);
        found.clear();
        renderFound();
        Discovery.scan(this, 3000, new Discovery.Callback() {
            @Override
            public void onFound(Discovery.Found f) {
                found.put(f.id, f);
                renderFound();
                maybeAutoConnect(f);
            }

            @Override
            public void onDone() {
                if (isDestroyed()) return;
                scanning.setVisibility(View.INVISIBLE);
                rescan.setEnabled(true);
                renderFound();
            }
        });
    }

    /** Reconnects to the last computer by itself when it shows up. */
    private void maybeAutoConnect(Discovery.Found f) {
        if (autoTried || ConnectService.state() != ConnectService.State.IDLE) return;
        if (!prefs.autoConnect() || !f.id.equals(prefs.lastPcId()) || !prefs.isPaired(f.id)) return;
        autoTried = true;
        ConnectService.connect(this, f.host, f.port, f.id, f.name);
    }

    private void renderFound() {
        foundList.removeAllViews();
        for (Discovery.Found f : found.values()) {
            LinearLayout card = Ui.card(this);
            card.setBackground(Ui.pressable(this, Ui.CARD, 18));
            LinearLayout line = Ui.row(this);
            LinearLayout texts = Ui.column(this);
            texts.addView(Ui.text(this, f.name, 17, Ui.TEXT, true));
            texts.addView(Ui.text(this, f.host, 13, Ui.TEXT_DIM, false));
            line.addView(texts, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
            if (prefs.isPaired(f.id)) {
                TextView badge = Ui.text(this, getString(R.string.badge_paired), 12, Ui.ACCENT, true);
                line.addView(badge);
            }
            card.addView(line);
            card.setOnClickListener(v -> ConnectService.connect(this, f.host, f.port, f.id, f.name));
            foundList.addView(card);
        }
        emptyText.setVisibility(found.isEmpty() && scanning.getVisibility() != View.VISIBLE ? View.VISIBLE : View.GONE);
    }

    private void askIp() {
        EditText ip = Ui.input(this, getString(R.string.ip_hint), InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        String last = prefs.lastHost();
        if (last != null) ip.setText(last);
        new AlertDialog.Builder(this)
                .setTitle(R.string.home_connect_ip)
                .setView(padded(ip))
                .setPositiveButton(R.string.connect, (d, w) -> {
                    String s = ip.getText().toString().trim();
                    int port = Protocol.PORT;
                    int colon = s.lastIndexOf(':');
                    if (colon > 0 && s.indexOf(':') == colon) {
                        try {
                            port = Integer.parseInt(s.substring(colon + 1));
                        } catch (NumberFormatException ignored) {
                        }
                        s = s.substring(0, colon);
                    }
                    if (!s.isEmpty()) ConnectService.connect(this, s, port, null, null);
                })
                .setNegativeButton(R.string.cancel, null)
                .show();
    }

    private View padded(View v) {
        FrameLayout f = new FrameLayout(this);
        int p = Ui.dp(this, 20);
        f.setPadding(p, Ui.dp(this, 8), p, 0);
        f.addView(v);
        return f;
    }

    // ------------------------------------------------------------------
    // The connected computer

    private void render() {
        statusBox.removeAllViews();
        ConnectService.State s = ConnectService.state();
        updatePairDialog(s);
        if (s == ConnectService.State.IDLE) return;

        LinearLayout card = Ui.card(this);
        LinearLayout line = Ui.row(this);
        View dot = new View(this);
        int dotColor = s == ConnectService.State.CONNECTED ? Ui.OK : Ui.ACCENT;
        dot.setBackground(Ui.rounded(dotColor, Ui.dp(this, 5)));
        LinearLayout.LayoutParams dlp = new LinearLayout.LayoutParams(Ui.dp(this, 10), Ui.dp(this, 10));
        dlp.rightMargin = Ui.dp(this, 10);
        line.addView(dot, dlp);
        String name = ConnectService.pcName();
        line.addView(Ui.text(this, name == null ? "" : name, 20, Ui.TEXT, true));
        card.addView(line);

        String status;
        switch (s) {
            case CONNECTED:
                int w = ConnectService.screenW(), h = ConnectService.screenH();
                status = w > 0 ? getString(R.string.status_connected_size, w, h) : getString(R.string.status_connected);
                break;
            case PAIRING:
                status = getString(R.string.status_pairing);
                break;
            case RECONNECTING:
                status = getString(R.string.status_reconnecting);
                break;
            default:
                status = getString(R.string.status_connecting);
                break;
        }
        String host = ConnectService.host();
        TextView info = Ui.text(this, (host == null ? "" : host + " · ") + status, 14, Ui.TEXT_DIM, false);
        info.setPadding(0, Ui.dp(this, 4), 0, Ui.dp(this, 12));
        card.addView(info);

        if (s == ConnectService.State.CONNECTED) {
            addPair(card,
                    action(R.string.action_trackpad, Ui.PRIMARY, v -> startActivity(new Intent(this, TrackpadActivity.class))),
                    action(R.string.action_screen, Ui.PRIMARY, v -> startActivity(new Intent(this, ScreenActivity.class))));
            addPair(card,
                    action(R.string.action_send_files, Ui.SECONDARY, v -> startActivity(new Intent(this, SendActivity.class))),
                    action(R.string.action_keyboard, Ui.SECONDARY, v -> startActivity(
                            new Intent(this, TrackpadActivity.class).putExtra(TrackpadActivity.EXTRA_KEYBOARD, true))));
            addPair(card,
                    action(R.string.action_send_clipboard, Ui.SECONDARY, v -> sendClipboard()),
                    action(R.string.action_disconnect, Ui.SECONDARY, v -> ConnectService.disconnect(this)));
            Button forget = action(R.string.action_forget, Ui.DESTRUCTIVE, v -> confirmForget());
            card.addView(forget, Ui.matchWrap());
        } else {
            Button cancel = action(R.string.action_disconnect, Ui.SECONDARY, v -> ConnectService.disconnect(this));
            card.addView(cancel, Ui.matchWrap());
        }
        statusBox.addView(card);
    }

    private Button action(int label, int style, View.OnClickListener l) {
        Button b = Ui.button(this, label, style);
        b.setOnClickListener(l);
        return b;
    }

    private void addPair(LinearLayout card, Button a, Button b) {
        LinearLayout r = Ui.row(this);
        r.addView(a, Ui.weighted(this, 1, 8));
        r.addView(b, Ui.weighted(this, 1, 0));
        LinearLayout.LayoutParams lp = Ui.matchWrap();
        lp.bottomMargin = Ui.dp(this, 8);
        card.addView(r, lp);
    }

    /** Sends the phone's clipboard (only readable while we are in the foreground). */
    private void sendClipboard() {
        ClipboardManager cm = getSystemService(ClipboardManager.class);
        ClipData clip = cm.getPrimaryClip();
        CharSequence text = clip != null && clip.getItemCount() > 0 ? clip.getItemAt(0).coerceToText(this) : null;
        if (text == null || text.length() == 0) {
            Toast.makeText(this, R.string.toast_clipboard_empty, Toast.LENGTH_SHORT).show();
            return;
        }
        boolean ok = ConnectService.send(Protocol.CLIP, Protocol.utf8(text.toString()));
        Toast.makeText(this, ok ? R.string.toast_clipboard_sent : R.string.toast_not_connected, Toast.LENGTH_SHORT).show();
    }

    private void confirmForget() {
        String id = ConnectService.pcId();
        String name = ConnectService.pcName();
        new AlertDialog.Builder(this)
                .setTitle(getString(R.string.forget_title, name))
                .setMessage(R.string.forget_message)
                .setPositiveButton(R.string.action_forget, (d, w) -> {
                    ConnectService.disconnect(this);
                    if (id != null) prefs.forgetToken(id);
                    renderFound();
                })
                .setNegativeButton(R.string.cancel, null)
                .show();
    }

    // ------------------------------------------------------------------
    // Pairing

    private void updatePairDialog(ConnectService.State s) {
        if (s == ConnectService.State.PAIRING) {
            if (pairDialog == null || !pairDialog.isShowing()) showPairDialog();
        } else if (pairDialog != null) {
            pairDialog.dismiss();
            pairDialog = null;
        }
    }

    private void showPairDialog() {
        EditText code = Ui.input(this, getString(R.string.pair_hint), InputType.TYPE_CLASS_NUMBER);
        code.setFilters(new InputFilter[] {new InputFilter.LengthFilter(6)});
        code.setGravity(Gravity.CENTER);
        code.setTextSize(28);
        code.setLetterSpacing(0.3f);
        String name = ConnectService.pcName();
        pairDialog = new AlertDialog.Builder(this)
                .setTitle(R.string.pair_title)
                .setMessage(getString(R.string.pair_message, name == null ? "" : name))
                .setView(padded(code))
                .setCancelable(false)
                .setPositiveButton(R.string.pair, (d, w) -> {
                    String c = code.getText().toString().trim();
                    if (c.isEmpty()) ConnectService.cancelPairing();
                    else ConnectService.submitCode(c);
                    pairDialog = null;
                })
                .setNegativeButton(R.string.cancel, (d, w) -> {
                    ConnectService.cancelPairing();
                    pairDialog = null;
                })
                .create();
        pairDialog.show();
        code.requestFocus();
        if (pairDialog.getWindow() != null) {
            pairDialog.getWindow().setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        }
    }

    // ------------------------------------------------------------------
    // ConnectService.Listener

    @Override
    public void onStateChanged() {
        if (visible) render();
    }

    @Override
    public void onNotice(String text) {
        Toast.makeText(this, text, Toast.LENGTH_SHORT).show();
    }

    @Override
    public void onDenied(String reason, String pcName) {
        if (isFinishing() || isDestroyed()) return;
        boolean code = "code".equals(reason);
        String name = pcName == null ? "" : pcName;
        new AlertDialog.Builder(this)
                .setTitle(code ? R.string.denied_code_title : R.string.denied_auth_title)
                .setMessage(code ? getString(R.string.denied_code_message) : getString(R.string.denied_auth_message, name))
                .setPositiveButton(code ? R.string.try_again : R.string.pair_again, (d, w) -> reconnectAfterDenied())
                .setNegativeButton(R.string.cancel, null)
                .show();
    }

    private void reconnectAfterDenied() {
        String h = ConnectService.host();
        if (h != null) ConnectService.connect(this, h, ConnectService.port(), ConnectService.pcId(), ConnectService.pcName());
    }
}
