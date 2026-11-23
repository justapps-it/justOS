package dev.justos.connect;

import android.app.Activity;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.Gravity;
import android.view.GestureDetector;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

/** Watches the PC's screen; taps on it click there. */
public final class ScreenActivity extends Activity implements ConnectService.FrameSink, ConnectService.Listener {
    /** Low, Normal, High: {max width, JPEG quality}. */
    private static final int[][] PRESETS = {{800, 50}, {1280, 70}, {1920, 85}};

    private ScreenView screen;
    private TextView waiting;
    private final Button[] presetButtons = new Button[3];
    private HandlerThread decoder;
    private Handler decodeHandler;
    private Prefs prefs;
    private boolean watching;
    private boolean wasConnected;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = Prefs.get(this);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        Ui.setupWindow(this);
        getWindow().getDecorView().setBackgroundColor(0xff000000);

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(0xff000000);
        screen = new ScreenView(this);
        root.addView(screen, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        waiting = Ui.text(this, getString(R.string.screen_waiting), 15, Ui.TEXT_DIM, false);
        root.addView(waiting, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER));

        // Quality buttons, top right, over the picture.
        LinearLayout bar = Ui.row(this);
        int[] labels = {R.string.quality_low, R.string.quality_normal, R.string.quality_high};
        for (int i = 0; i < 3; i++) {
            Button b = Ui.keyButton(this, getString(labels[i]));
            b.setAlpha(0.85f);
            int q = i;
            b.setOnClickListener(v -> setQuality(q));
            presetButtons[i] = b;
            bar.addView(b);
        }
        FrameLayout.LayoutParams blp = new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP | Gravity.END);
        int m = Ui.dp(this, 10);
        blp.setMargins(m, m, m, m);
        FrameLayout barHolder = new FrameLayout(this);
        barHolder.addView(bar, blp);
        Ui.padForInsets(barHolder);
        root.addView(barHolder, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP));
        setContentView(root);
        showPreset();

        decoder = new HandlerThread("connect-decoder");
        decoder.start();
        decodeHandler = new Handler(decoder.getLooper());
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) immersive();
    }

    @SuppressWarnings("deprecation")
    private void immersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController c = getWindow().getInsetsController();
            if (c != null) {
                c.hide(WindowInsets.Type.systemBars());
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        ConnectService.addListener(this);
        ConnectService.setFrameSink(this);
        wasConnected = ConnectService.isConnected();
        requestScreen(true);
    }

    @Override
    protected void onPause() {
        requestScreen(false);
        ConnectService.setFrameSink(null);
        ConnectService.removeListener(this);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        decoder.quitSafely();
        super.onDestroy();
    }

    private void requestScreen(boolean on) {
        int[] p = PRESETS[prefs.screenQuality()];
        if (on) {
            ConnectService.sendJson(Protocol.SCREEN, ConnectService.json("on", true, "w", p[0], "q", p[1]));
        } else {
            ConnectService.sendJson(Protocol.SCREEN, ConnectService.json("on", false));
        }
        watching = on;
    }

    private void setQuality(int q) {
        if (q == prefs.screenQuality()) return;
        prefs.setScreenQuality(q);
        showPreset();
        // The PC takes the width only when watching starts: off, then on.
        requestScreen(false);
        requestScreen(true);
    }

    private void showPreset() {
        int q = prefs.screenQuality();
        for (int i = 0; i < 3; i++) Ui.setToggled(this, presetButtons[i], i == q);
    }

    // ------------------------------------------------------------------
    // Frames: u16 w, u16 h, u16 screen width, u16 screen height, then JPEG.

    @Override
    public void onFrame(byte[] p) {
        decodeHandler.post(() -> {
            Bitmap bmp = null;
            if (p.length > 8) {
                try {
                    bmp = BitmapFactory.decodeByteArray(p, 8, p.length - 8);
                } catch (OutOfMemoryError e) {
                    bmp = null;
                }
            }
            Bitmap shown = bmp;
            runOnUiThread(() -> {
                if (shown == null) {
                    // Nothing to show, but the PC still waits for the answer.
                    ConnectService.send(Protocol.FRAME_ACK, null);
                    return;
                }
                waiting.setVisibility(View.GONE);
                screen.show(shown);
            });
        });
    }

    @Override
    public void onStateChanged() {
        ConnectService.State s = ConnectService.state();
        if (s == ConnectService.State.IDLE) {
            finish();
            return;
        }
        boolean now = ConnectService.isConnected();
        if (now && !wasConnected && watching) requestScreen(true);
        wasConnected = now;
    }

    /** The picture, letterboxed, and the touch handling on it. */
    private final class ScreenView extends View {
        private Bitmap frame;
        private boolean ackPending;
        private final RectF dst = new RectF();
        private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
        private final GestureDetector gestures;
        private boolean two;
        private float lastY;
        private float scrollAcc;
        private final float step;

        ScreenView(Context c) {
            super(c);
            step = 18 * c.getResources().getDisplayMetrics().density;
            gestures = new GestureDetector(c, new GestureDetector.SimpleOnGestureListener() {
                @Override
                public boolean onDown(MotionEvent e) {
                    return true;
                }

                @Override
                public boolean onSingleTapUp(MotionEvent e) {
                    click(e.getX(), e.getY(), Protocol.BTN_LEFT);
                    return true;
                }

                @Override
                public void onLongPress(MotionEvent e) {
                    click(e.getX(), e.getY(), Protocol.BTN_RIGHT);
                }
            });
            gestures.setIsLongpressEnabled(true);
        }

        void show(Bitmap b) {
            frame = b;
            ackPending = true;
            invalidate();
        }

        @Override
        protected void onDraw(Canvas c) {
            Bitmap b = frame;
            if (b == null) return;
            float vw = getWidth(), vh = getHeight();
            float scale = Math.min(vw / b.getWidth(), vh / b.getHeight());
            float w = b.getWidth() * scale, h = b.getHeight() * scale;
            dst.set((vw - w) / 2, (vh - h) / 2, (vw + w) / 2, (vh + h) / 2);
            c.drawBitmap(b, null, dst, paint);
            if (ackPending) {
                // Shown: the PC may send the next one.
                ackPending = false;
                ConnectService.send(Protocol.FRAME_ACK, null);
            }
        }

        private void click(float x, float y, int button) {
            if (frame == null || dst.width() <= 0) return;
            float fx = (x - dst.left) / dst.width();
            float fy = (y - dst.top) / dst.height();
            if (fx < 0 || fx > 1 || fy < 0 || fy > 1) return;
            ConnectService.send(Protocol.TAP, Protocol.tap(fx, fy));
            ConnectService.send(Protocol.BUTTON, Protocol.button(button, true));
            ConnectService.send(Protocol.BUTTON, Protocol.button(button, false));
            performHapticFeedback(android.view.HapticFeedbackConstants.VIRTUAL_KEY);
        }

        @Override
        public boolean onTouchEvent(MotionEvent e) {
            int action = e.getActionMasked();
            if (action == MotionEvent.ACTION_POINTER_DOWN && e.getPointerCount() == 2) {
                // Two fingers: scrolling, not a tap or a long press.
                two = true;
                lastY = avgY(e);
                scrollAcc = 0;
                MotionEvent cancel = MotionEvent.obtain(e);
                cancel.setAction(MotionEvent.ACTION_CANCEL);
                gestures.onTouchEvent(cancel);
                cancel.recycle();
                return true;
            }
            if (two) {
                if (action == MotionEvent.ACTION_MOVE && e.getPointerCount() >= 2) {
                    float y = avgY(e);
                    scrollAcc += (lastY - y) / step;
                    lastY = y;
                    int n = (int) scrollAcc;
                    if (n != 0) {
                        scrollAcc -= n;
                        ConnectService.send(Protocol.SCROLL, Protocol.scroll(n));
                    }
                } else if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
                    two = false;
                }
                return true;
            }
            gestures.onTouchEvent(e);
            return true;
        }

        private float avgY(MotionEvent e) {
            float s = 0;
            for (int i = 0; i < e.getPointerCount(); i++) s += e.getY(i);
            return s / e.getPointerCount();
        }
    }
}
