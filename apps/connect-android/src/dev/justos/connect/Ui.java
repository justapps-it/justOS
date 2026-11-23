package dev.justos.connect;

import android.app.Activity;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.os.Build;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

/** Colours and small builders for the views, which are made in code. */
final class Ui {
    private Ui() {}

    static final int BG = 0xff0f0f10;
    static final int CARD = 0xff1c1c1e;
    static final int CARD_HI = 0xff2c2c2e;
    static final int ACCENT = 0xffff5a1f;
    static final int TEXT = 0xffffffff;
    static final int TEXT_DIM = 0xff9a9aa0;
    static final int DANGER = 0xffff5c4d;
    static final int OK = 0xff34c759;

    static final int PRIMARY = 0;
    static final int SECONDARY = 1;
    static final int DESTRUCTIVE = 2;

    static int dp(Context c, float v) {
        return Math.round(v * c.getResources().getDisplayMetrics().density);
    }

    static GradientDrawable rounded(int color, float radiusPx) {
        GradientDrawable d = new GradientDrawable();
        d.setColor(color);
        d.setCornerRadius(radiusPx);
        return d;
    }

    /** A rounded background with a ripple when pressed. */
    static Drawable pressable(Context c, int color, float radiusDp) {
        float r = dp(c, radiusDp);
        return new RippleDrawable(ColorStateList.valueOf(0x33ffffff), rounded(color, r), rounded(0xffffffff, r));
    }

    static LinearLayout column(Context c) {
        LinearLayout l = new LinearLayout(c);
        l.setOrientation(LinearLayout.VERTICAL);
        return l;
    }

    static LinearLayout row(Context c) {
        LinearLayout l = new LinearLayout(c);
        l.setOrientation(LinearLayout.HORIZONTAL);
        l.setGravity(Gravity.CENTER_VERTICAL);
        return l;
    }

    /** A card: rounded, slightly lighter than the background. */
    static LinearLayout card(Context c) {
        LinearLayout l = column(c);
        l.setBackground(rounded(CARD, dp(c, 18)));
        int p = dp(c, 16);
        l.setPadding(p, p, p, p);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.bottomMargin = dp(c, 12);
        l.setLayoutParams(lp);
        return l;
    }

    static TextView text(Context c, CharSequence s, float sp, int color, boolean bold) {
        TextView t = new TextView(c);
        t.setText(s);
        t.setTextSize(sp);
        t.setTextColor(color);
        if (bold) t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        return t;
    }

    /** A section heading. */
    static TextView heading(Context c, int res) {
        TextView t = text(c, c.getString(res), 13, TEXT_DIM, true);
        t.setAllCaps(true);
        t.setLetterSpacing(0.08f);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.topMargin = dp(c, 12);
        lp.bottomMargin = dp(c, 8);
        lp.leftMargin = dp(c, 4);
        t.setLayoutParams(lp);
        return t;
    }

    static Button button(Context c, CharSequence label, int style) {
        Button b = new Button(c);
        b.setText(label);
        b.setAllCaps(false);
        b.setTextSize(15);
        b.setStateListAnimator(null);
        b.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        int bg = style == PRIMARY ? ACCENT : CARD_HI;
        b.setBackground(pressable(c, bg, 14));
        b.setTextColor(style == DESTRUCTIVE ? DANGER : TEXT);
        int h = dp(c, 12);
        b.setPadding(dp(c, 16), h, dp(c, 16), h);
        b.setMinHeight(dp(c, 48));
        b.setMinimumHeight(dp(c, 48));
        b.setMinWidth(0);
        b.setMinimumWidth(0);
        return b;
    }

    static Button button(Context c, int res, int style) {
        return button(c, c.getString(res), style);
    }

    /** A small key-like button for rows of keys. */
    static Button keyButton(Context c, CharSequence label) {
        Button b = button(c, label, SECONDARY);
        b.setTextSize(14);
        int p = dp(c, 6);
        b.setPadding(dp(c, 12), p, dp(c, 12), p);
        b.setMinHeight(dp(c, 40));
        b.setMinimumHeight(dp(c, 40));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, dp(c, 40));
        lp.rightMargin = dp(c, 6);
        b.setLayoutParams(lp);
        return b;
    }

    /** Marks a key-like button as on (a held modifier) or off. */
    static void setToggled(Context c, Button b, boolean on) {
        b.setBackground(pressable(c, on ? ACCENT : CARD_HI, 14));
    }

    static EditText input(Context c, CharSequence hint, int inputType) {
        EditText e = new EditText(c);
        e.setHint(hint);
        e.setInputType(inputType == 0 ? InputType.TYPE_CLASS_TEXT : inputType);
        e.setTextColor(TEXT);
        e.setHintTextColor(TEXT_DIM);
        e.setBackgroundTintList(ColorStateList.valueOf(ACCENT));
        e.setSingleLine(true);
        return e;
    }

    static LinearLayout.LayoutParams weighted(Context c, float weight, int gapDp) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, weight);
        lp.rightMargin = dp(c, gapDp);
        return lp;
    }

    static LinearLayout.LayoutParams matchWrap() {
        return new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
    }

    /** Dark bars; content laid out edge to edge with padding for the system bars. */
    static void setupWindow(Activity a) {
        Window w = a.getWindow();
        // Edge to edge on every version (Android 15+ does it anyway), with
        // padForInsets keeping the content clear of the bars.
        if (Build.VERSION.SDK_INT >= 30) {
            w.setDecorFitsSystemWindows(false);
        } else {
            w.getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }
        if (Build.VERSION.SDK_INT < 35) {
            w.setStatusBarColor(0x00000000);
            w.setNavigationBarColor(0xcc0f0f10);
        }
        w.getDecorView().setBackgroundColor(BG);
    }

    /**
     * Pads {@code v} by the system bars (and the keyboard) on top of the
     * padding it already has, so nothing hides behind them.
     */
    static void padForInsets(View v) {
        final int l = v.getPaddingLeft(), t = v.getPaddingTop(), r = v.getPaddingRight(), b = v.getPaddingBottom();
        v.setOnApplyWindowInsetsListener((view, insets) -> {
            int il, it, ir, ib;
            if (Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets i = insets.getInsets(WindowInsets.Type.systemBars()
                        | WindowInsets.Type.displayCutout() | WindowInsets.Type.ime());
                il = i.left;
                it = i.top;
                ir = i.right;
                ib = i.bottom;
            } else {
                il = insets.getSystemWindowInsetLeft();
                it = insets.getSystemWindowInsetTop();
                ir = insets.getSystemWindowInsetRight();
                ib = insets.getSystemWindowInsetBottom();
            }
            view.setPadding(l + il, t + it, r + ir, b + ib);
            return insets;
        });
    }
}
