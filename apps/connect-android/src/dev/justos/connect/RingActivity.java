package dev.justos.connect;

import android.app.Activity;
import android.os.Build;
import android.os.Bundle;
import android.view.Gravity;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.lang.ref.WeakReference;

/** Find my phone: a big Stop button while the phone rings. */
public final class RingActivity extends Activity {
    private static WeakReference<RingActivity> current;

    /** Closes the ring screen if it is open (the ring stopped). */
    static void closeAll() {
        RingActivity a = current == null ? null : current.get();
        if (a != null) a.finish();
    }

    @Override
    @SuppressWarnings("deprecation")
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true);
            setTurnScreenOn(true);
        } else {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED | WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON);
        }
        Ui.setupWindow(this);
        current = new WeakReference<>(this);

        LinearLayout root = Ui.column(this);
        root.setGravity(Gravity.CENTER);
        int pad = Ui.dp(this, 32);
        root.setPadding(pad, pad, pad, pad);
        TextView title = Ui.text(this, getString(R.string.ring_title), 26, Ui.TEXT, true);
        title.setGravity(Gravity.CENTER);
        root.addView(title);
        String name = ConnectService.pcName();
        TextView text = Ui.text(this, getString(R.string.ring_text, name == null ? "" : name), 16, Ui.TEXT_DIM, false);
        text.setGravity(Gravity.CENTER);
        text.setPadding(0, Ui.dp(this, 12), 0, Ui.dp(this, 40));
        root.addView(text);
        Button stop = Ui.button(this, R.string.action_stop, Ui.PRIMARY);
        stop.setTextSize(22);
        stop.setOnClickListener(v -> {
            ConnectService.stopRinging(this);
            finish();
        });
        root.addView(stop, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(this, 88)));
        Ui.padForInsets(root);
        setContentView(root);
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (!ConnectService.isRinging()) finish();
    }

    @Override
    protected void onDestroy() {
        if (current != null && current.get() == this) current = null;
        super.onDestroy();
    }
}
