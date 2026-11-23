package dev.justos.connect;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ComponentName;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import java.util.List;

/** Device name, notification mirroring, paired PCs and About. */
public final class SettingsActivity extends Activity {
    private Prefs prefs;
    private EditText name;
    private Switch mirror;
    private TextView accessState;
    private LinearLayout pairedList;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = Prefs.get(this);
        Ui.setupWindow(this);

        LinearLayout root = Ui.column(this);
        int pad = Ui.dp(this, 20);
        root.setPadding(pad, Ui.dp(this, 12), pad, pad);
        TextView title = Ui.text(this, getString(R.string.settings), 28, Ui.TEXT, true);
        title.setPadding(0, Ui.dp(this, 8), 0, Ui.dp(this, 8));
        root.addView(title);

        // Device name.
        root.addView(Ui.heading(this, R.string.settings_device_name));
        LinearLayout nameCard = Ui.card(this);
        name = Ui.input(this, android.os.Build.MODEL, 0);
        name.setText(prefs.customName());
        nameCard.addView(name, Ui.matchWrap());
        TextView nameHint = Ui.text(this, getString(R.string.settings_device_name_hint), 13, Ui.TEXT_DIM, false);
        nameHint.setPadding(0, Ui.dp(this, 6), 0, 0);
        nameCard.addView(nameHint);
        root.addView(nameCard);

        // Notification mirroring.
        root.addView(Ui.heading(this, R.string.settings_notifications));
        LinearLayout notifCard = Ui.card(this);
        mirror = new Switch(this);
        mirror.setText(R.string.settings_mirror);
        mirror.setTextColor(Ui.TEXT);
        mirror.setTextSize(16);
        mirror.setThumbTintList(new ColorStateList(new int[][] {{android.R.attr.state_checked}, {}}, new int[] {Ui.ACCENT, 0xffc7c7cc}));
        mirror.setTrackTintList(new ColorStateList(new int[][] {{android.R.attr.state_checked}, {}}, new int[] {0x88ff5a1f, 0xff48484a}));
        mirror.setChecked(prefs.mirrorNotifications());
        mirror.setOnCheckedChangeListener((b, on) -> {
            prefs.setMirrorNotifications(on);
            if (on && !hasNotificationAccess()) openNotificationAccess();
        });
        notifCard.addView(mirror, Ui.matchWrap());
        accessState = Ui.text(this, "", 13, Ui.TEXT_DIM, false);
        accessState.setPadding(0, Ui.dp(this, 6), 0, Ui.dp(this, 10));
        notifCard.addView(accessState);
        Button access = Ui.button(this, R.string.settings_open_access, Ui.SECONDARY);
        access.setOnClickListener(v -> openNotificationAccess());
        notifCard.addView(access, Ui.matchWrap());
        root.addView(notifCard);

        // Paired computers.
        root.addView(Ui.heading(this, R.string.settings_paired));
        pairedList = Ui.column(this);
        root.addView(pairedList, Ui.matchWrap());

        // About.
        root.addView(Ui.heading(this, R.string.settings_about));
        LinearLayout about = Ui.card(this);
        about.addView(Ui.text(this, getString(R.string.app_name), 17, Ui.TEXT, true));
        TextView version = Ui.text(this, getString(R.string.about_version, "1.0"), 14, Ui.TEXT_DIM, false);
        version.setPadding(0, Ui.dp(this, 2), 0, Ui.dp(this, 6));
        about.addView(version);
        about.addView(Ui.text(this, getString(R.string.about_text), 14, Ui.TEXT_DIM, false));
        Button site = Ui.button(this, R.string.about_site, Ui.SECONDARY);
        site.setOnClickListener(v -> {
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("https://justos.dev")));
            } catch (RuntimeException ignored) {
                // No browser.
            }
        });
        LinearLayout.LayoutParams slp = Ui.matchWrap();
        slp.topMargin = Ui.dp(this, 12);
        about.addView(site, slp);
        root.addView(about);

        ScrollView scroll = new ScrollView(this);
        scroll.addView(root);
        scroll.setClipToPadding(false);
        Ui.padForInsets(scroll);
        setContentView(scroll);
    }

    @Override
    protected void onResume() {
        super.onResume();
        boolean ok = hasNotificationAccess();
        accessState.setText(ok ? R.string.settings_access_on : R.string.settings_access_off);
        renderPaired();
    }

    @Override
    protected void onPause() {
        prefs.setDeviceName(name.getText().toString());
        super.onPause();
    }

    private boolean hasNotificationAccess() {
        String flat = Settings.Secure.getString(getContentResolver(), "enabled_notification_listeners");
        if (flat == null) return false;
        ComponentName me = new ComponentName(this, NotifyListener.class);
        for (String s : flat.split(":")) {
            ComponentName c = ComponentName.unflattenFromString(s);
            if (me.equals(c)) return true;
        }
        return false;
    }

    private void openNotificationAccess() {
        Intent i;
        if (Build.VERSION.SDK_INT >= 30) {
            i = new Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
                    .putExtra(Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME,
                            new ComponentName(this, NotifyListener.class).flattenToString());
        } else {
            i = new Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS);
        }
        try {
            startActivity(i);
        } catch (RuntimeException e) {
            startActivity(new Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS));
        }
    }

    private void renderPaired() {
        pairedList.removeAllViews();
        List<String[]> pcs = prefs.paired();
        if (pcs.isEmpty()) {
            TextView none = Ui.text(this, getString(R.string.settings_paired_none), 14, Ui.TEXT_DIM, false);
            none.setPadding(Ui.dp(this, 4), 0, 0, Ui.dp(this, 12));
            pairedList.addView(none);
            return;
        }
        for (String[] pc : pcs) {
            LinearLayout card = Ui.card(this);
            LinearLayout line = Ui.row(this);
            LinearLayout texts = Ui.column(this);
            texts.addView(Ui.text(this, pc[1], 17, Ui.TEXT, true));
            texts.addView(Ui.text(this, pc[0], 12, Ui.TEXT_DIM, false));
            line.addView(texts, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
            Button forget = Ui.button(this, R.string.action_forget, Ui.DESTRUCTIVE);
            forget.setOnClickListener(v -> new AlertDialog.Builder(this)
                    .setTitle(getString(R.string.forget_title, pc[1]))
                    .setMessage(R.string.forget_message)
                    .setPositiveButton(R.string.action_forget, (d, w) -> {
                        if (pc[0].equals(ConnectService.pcId()) && ConnectService.state() != ConnectService.State.IDLE) {
                            ConnectService.disconnect(this);
                        }
                        prefs.forgetToken(pc[0]);
                        renderPaired();
                    })
                    .setNegativeButton(R.string.cancel, null)
                    .show());
            line.addView(forget);
            card.addView(line);
            pairedList.addView(card);
        }
    }
}
