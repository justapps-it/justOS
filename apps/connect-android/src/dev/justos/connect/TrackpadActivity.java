package dev.justos.connect;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.os.Bundle;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;
import android.widget.TextView;

/** The phone as a trackpad and keyboard for the PC. */
public final class TrackpadActivity extends Activity implements ConnectService.Listener {
    static final String EXTRA_KEYBOARD = "keyboard";

    private int mods;
    private Button ctrlKey;
    private Button altKey;
    private Button shiftKey;
    private HorizontalScrollView fRow;
    private KeyInputView keyInput;
    private TextView title;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Ui.setupWindow(this);

        LinearLayout root = Ui.column(this);
        int pad = Ui.dp(this, 12);
        root.setPadding(pad, pad, pad, pad);

        // Top: the PC's name and the keyboard.
        LinearLayout top = Ui.row(this);
        title = Ui.text(this, "", 18, Ui.TEXT, true);
        top.addView(title, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        Button kb = Ui.button(this, R.string.action_keyboard, Ui.PRIMARY);
        kb.setOnClickListener(v -> keyInput.showKeyboard());
        top.addView(kb);
        LinearLayout.LayoutParams tlp = Ui.matchWrap();
        tlp.bottomMargin = Ui.dp(this, 10);
        root.addView(top, tlp);

        // Special keys.
        LinearLayout keys = Ui.row(this);
        addKey(keys, R.string.key_esc, Protocol.K_ESC);
        addKey(keys, R.string.key_tab, Protocol.K_TAB);
        ctrlKey = Ui.keyButton(this, getString(R.string.key_ctrl));
        ctrlKey.setOnClickListener(v -> toggleMod(Protocol.MOD_CTRL));
        keys.addView(ctrlKey);
        altKey = Ui.keyButton(this, getString(R.string.key_alt));
        altKey.setOnClickListener(v -> toggleMod(Protocol.MOD_ALT));
        keys.addView(altKey);
        shiftKey = Ui.keyButton(this, getString(R.string.key_shift));
        shiftKey.setOnClickListener(v -> toggleMod(Protocol.MOD_SHIFT));
        keys.addView(shiftKey);
        addKey(keys, R.string.key_left, Protocol.K_LEFT);
        addKey(keys, R.string.key_up, Protocol.K_UP);
        addKey(keys, R.string.key_down, Protocol.K_DOWN);
        addKey(keys, R.string.key_right, Protocol.K_RIGHT);
        Button fn = Ui.keyButton(this, getString(R.string.key_fn));
        fn.setOnClickListener(v -> {
            boolean show = fRow.getVisibility() != View.VISIBLE;
            fRow.setVisibility(show ? View.VISIBLE : View.GONE);
            Ui.setToggled(this, fn, show);
        });
        keys.addView(fn);
        addKey(keys, R.string.key_home, Protocol.K_HOME);
        addKey(keys, R.string.key_end, Protocol.K_END);
        addKey(keys, R.string.key_pgup, Protocol.K_PGUP);
        addKey(keys, R.string.key_pgdn, Protocol.K_PGDN);
        addKey(keys, R.string.key_delete, Protocol.K_DELETE);
        addKey(keys, R.string.key_overview, Protocol.K_OVERVIEW);
        addKey(keys, R.string.key_vol_down, Protocol.K_VOL_DOWN);
        addKey(keys, R.string.key_vol_up, Protocol.K_VOL_UP);
        addKey(keys, R.string.key_mute, Protocol.K_MUTE);
        root.addView(scroller(keys), Ui.matchWrap());

        // F1..F12, shown with Fn.
        LinearLayout fkeys = Ui.row(this);
        for (int i = 0; i < 12; i++) {
            Button b = Ui.keyButton(this, getString(R.string.key_f, i + 1));
            int code = Protocol.K_F1 + i;
            b.setOnClickListener(v -> sendKey(code));
            fkeys.addView(b);
        }
        fRow = scroller(fkeys);
        fRow.setVisibility(View.GONE);
        LinearLayout.LayoutParams flp = Ui.matchWrap();
        flp.topMargin = Ui.dp(this, 6);
        root.addView(fRow, flp);

        // The surface.
        TrackpadView pad1 = new TrackpadView(this, new TrackpadView.Sink() {
            @Override
            public void move(int dx, int dy) {
                ConnectService.send(Protocol.MOVE, Protocol.move(dx, dy));
            }

            @Override
            public void button(int button, boolean down) {
                ConnectService.send(Protocol.BUTTON, Protocol.button(button, down));
            }

            @Override
            public void scroll(int amount) {
                ConnectService.send(Protocol.SCROLL, Protocol.scroll(amount));
            }
        });
        LinearLayout.LayoutParams plp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1);
        plp.topMargin = Ui.dp(this, 10);
        plp.bottomMargin = Ui.dp(this, 10);
        root.addView(pad1, plp);

        // Mouse buttons.
        LinearLayout buttons = Ui.row(this);
        Button left = Ui.button(this, R.string.button_left, Ui.SECONDARY);
        holdButton(left, Protocol.BTN_LEFT);
        LinearLayout.LayoutParams llp = new LinearLayout.LayoutParams(0, Ui.dp(this, 64), 1);
        llp.rightMargin = Ui.dp(this, 8);
        buttons.addView(left, llp);
        Button right = Ui.button(this, R.string.button_right, Ui.SECONDARY);
        holdButton(right, Protocol.BTN_RIGHT);
        buttons.addView(right, new LinearLayout.LayoutParams(0, Ui.dp(this, 64), 1));
        root.addView(buttons, Ui.matchWrap());

        // The invisible keyboard target.
        keyInput = new KeyInputView(this, new KeyInputView.Sink() {
            @Override
            public void text(String s) {
                sendText(s);
            }

            @Override
            public void key(int code) {
                sendKey(code);
            }
        });
        root.addView(keyInput, new LinearLayout.LayoutParams(1, 1));

        Ui.padForInsets(root);
        setContentView(root);

        if (getIntent().getBooleanExtra(EXTRA_KEYBOARD, false)) {
            keyInput.post(keyInput::showKeyboard);
        }
    }

    private HorizontalScrollView scroller(View content) {
        HorizontalScrollView h = new HorizontalScrollView(this);
        h.setHorizontalScrollBarEnabled(false);
        h.addView(content);
        return h;
    }

    private void addKey(LinearLayout row, int label, int code) {
        Button b = Ui.keyButton(this, getString(label));
        b.setOnClickListener(v -> sendKey(code));
        row.addView(b);
    }

    @SuppressLint("ClickableViewAccessibility")
    private void holdButton(Button b, int button) {
        b.setOnTouchListener((v, e) -> {
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    v.setPressed(true);
                    ConnectService.send(Protocol.BUTTON, Protocol.button(button, true));
                    return true;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    v.setPressed(false);
                    ConnectService.send(Protocol.BUTTON, Protocol.button(button, false));
                    return true;
                default:
                    return true;
            }
        });
    }

    private void toggleMod(int m) {
        mods ^= m;
        showMods();
    }

    private void showMods() {
        Ui.setToggled(this, ctrlKey, (mods & Protocol.MOD_CTRL) != 0);
        Ui.setToggled(this, altKey, (mods & Protocol.MOD_ALT) != 0);
        Ui.setToggled(this, shiftKey, (mods & Protocol.MOD_SHIFT) != 0);
    }

    /** Text goes as TEXT, unless a modifier is held: then each character is a KEY. */
    private void sendText(String s) {
        if (mods == 0) {
            ConnectService.send(Protocol.TEXT, Protocol.utf8(s));
            return;
        }
        int i = 0;
        while (i < s.length()) {
            int cp = s.codePointAt(i);
            ConnectService.send(Protocol.KEY, Protocol.key(Protocol.K_CHAR, mods, new String(Character.toChars(cp))));
            i += Character.charCount(cp);
        }
        mods = 0;
        showMods();
    }

    private void sendKey(int code) {
        ConnectService.send(Protocol.KEY, Protocol.key(code, mods, null));
        if (mods != 0) {
            mods = 0;
            showMods();
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        ConnectService.addListener(this);
        onStateChanged();
    }

    @Override
    protected void onStop() {
        ConnectService.removeListener(this);
        super.onStop();
    }

    @Override
    public void onStateChanged() {
        ConnectService.State s = ConnectService.state();
        if (s == ConnectService.State.IDLE) {
            finish();
            return;
        }
        String name = ConnectService.pcName();
        title.setText(s == ConnectService.State.CONNECTED ? (name == null ? "" : name) : getString(R.string.status_reconnecting));
    }
}
