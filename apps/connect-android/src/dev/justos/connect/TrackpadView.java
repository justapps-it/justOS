package dev.justos.connect;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.MotionEvent;
import android.view.View;

/**
 * The touch surface of the trackpad. One finger moves the pointer, a tap
 * clicks, two fingers scroll, a two-finger tap right-clicks, and
 * double-tap-and-hold drags with the left button held.
 */
final class TrackpadView extends View {
    interface Sink {
        void move(int dx, int dy);

        void button(int button, boolean down);

        void scroll(int amount);
    }

    private static final long TAP_MS = 250;
    /** How long a tap waits for a second tap (that would start a drag). */
    private static final long DOUBLE_TAP_MS = 220;
    private static final long HOLD_MS = 180;
    private static final long FRAME_MS = 16;
    private static final float GAIN = 1.5f;

    private final Sink sink;
    private final float density;
    private final float slop;
    private final float scrollStep;
    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint bg = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final String hint;

    private static final int NONE = 0, ONE = 1, TWO = 2, DONE = 3;
    private int mode = NONE;
    private float startX, startY, lastX, lastY;
    private long downTime, lastMoveTime;
    private boolean moved;

    // Pointer motion waiting to be sent (fractions carry over).
    private float pendingX, pendingY;
    private boolean flushPosted;
    private long lastFlush;

    // Taps and drags.
    private Runnable pendingClick;
    private boolean dragCandidate;
    private boolean dragging;

    // Two fingers.
    private long twoDownTime;
    private float twoStartY, twoLastY;
    private float scrollAcc;
    private boolean twoMoved;

    TrackpadView(Context c, Sink sink) {
        super(c);
        this.sink = sink;
        density = c.getResources().getDisplayMetrics().density;
        slop = 8 * density;
        scrollStep = 18 * density;
        hint = c.getString(R.string.trackpad_hint);
        paint.setTextAlign(Paint.Align.CENTER);
        paint.setTextSize(14 * c.getResources().getDisplayMetrics().scaledDensity);
        paint.setColor(Ui.TEXT_DIM);
        bg.setColor(Ui.CARD);
    }

    @Override
    protected void onDraw(Canvas c) {
        rect.set(0, 0, getWidth(), getHeight());
        float r = 22 * density;
        c.drawRoundRect(rect, r, r, bg);
        c.drawText(hint, getWidth() / 2f, getHeight() / 2f, paint);
    }

    private final Runnable flush = this::sendPending;

    private void sendPending() {
        flushPosted = false;
        lastFlush = android.os.SystemClock.uptimeMillis();
        int dx = (int) pendingX, dy = (int) pendingY;
        if (dx != 0 || dy != 0) {
            pendingX -= dx;
            pendingY -= dy;
            sink.move(dx, dy);
        }
    }

    private void flushNow() {
        removeCallbacks(flush);
        flush.run();
    }

    private void scheduleFlush() {
        if (flushPosted) return;
        flushPosted = true;
        long wait = Math.max(0, FRAME_MS - (android.os.SystemClock.uptimeMillis() - lastFlush));
        postDelayed(flush, wait);
    }

    private void click(int button) {
        flushNow();
        sink.button(button, true);
        sink.button(button, false);
    }

    private final Runnable holdCheck = () -> {
        if (mode == ONE && dragCandidate && !dragging) startDrag();
    };

    private void startDrag() {
        dragging = true;
        flushNow();
        sink.button(Protocol.BTN_LEFT, true);
    }

    @Override
    public boolean onTouchEvent(MotionEvent e) {
        switch (e.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                mode = ONE;
                moved = false;
                startX = lastX = e.getX();
                startY = lastY = e.getY();
                downTime = lastMoveTime = e.getEventTime();
                if (pendingClick != null) {
                    // A second tap soon after the first: a drag if held or moved, else a double click.
                    removeCallbacks(pendingClick);
                    pendingClick = null;
                    dragCandidate = true;
                    postDelayed(holdCheck, HOLD_MS);
                }
                return true;

            case MotionEvent.ACTION_POINTER_DOWN:
                if (mode == ONE && e.getPointerCount() == 2 && !dragging) {
                    if (dragCandidate) {
                        // The first tap of a would-be drag was a click after all.
                        removeCallbacks(holdCheck);
                        dragCandidate = false;
                        click(Protocol.BTN_LEFT);
                    }
                    mode = TWO;
                    twoDownTime = e.getEventTime();
                    twoStartY = twoLastY = avgY(e);
                    scrollAcc = 0;
                    twoMoved = false;
                }
                return true;

            case MotionEvent.ACTION_MOVE:
                if (mode == ONE) {
                    float x = e.getX(), y = e.getY();
                    if (!moved && Math.hypot(x - startX, y - startY) > slop) {
                        moved = true;
                        if (dragCandidate && !dragging) {
                            removeCallbacks(holdCheck);
                            startDrag();
                        }
                    }
                    if (moved) {
                        float dx = x - lastX, dy = y - lastY;
                        long dt = Math.max(1, e.getEventTime() - lastMoveTime);
                        // Mild acceleration: faster strokes go further.
                        float speed = (float) Math.hypot(dx, dy) / density / dt;
                        float gain = GAIN * (1f + 0.5f * Math.min(speed, 1.5f));
                        pendingX += dx * gain;
                        pendingY += dy * gain;
                        scheduleFlush();
                    }
                    lastX = x;
                    lastY = y;
                    lastMoveTime = e.getEventTime();
                } else if (mode == TWO && e.getPointerCount() >= 2) {
                    float y = avgY(e);
                    if (Math.abs(y - twoStartY) > slop) twoMoved = true;
                    // Fingers up = content up = scroll down (positive).
                    scrollAcc += (twoLastY - y) / scrollStep;
                    twoLastY = y;
                    int n = (int) scrollAcc;
                    if (n != 0 && twoMoved) {
                        scrollAcc -= n;
                        sink.scroll(n);
                    }
                }
                return true;

            case MotionEvent.ACTION_POINTER_UP:
                if (mode == TWO) {
                    if (!twoMoved && e.getEventTime() - twoDownTime < TAP_MS) click(Protocol.BTN_RIGHT);
                    mode = DONE;
                }
                return true;

            case MotionEvent.ACTION_UP:
                if (mode == ONE) {
                    removeCallbacks(holdCheck);
                    if (dragging) {
                        flushNow();
                        sink.button(Protocol.BTN_LEFT, false);
                    } else if (dragCandidate) {
                        // Two quick taps: a double click.
                        click(Protocol.BTN_LEFT);
                        click(Protocol.BTN_LEFT);
                    } else if (!moved && e.getEventTime() - downTime < TAP_MS) {
                        pendingClick = () -> {
                            pendingClick = null;
                            click(Protocol.BTN_LEFT);
                        };
                        postDelayed(pendingClick, DOUBLE_TAP_MS);
                    }
                }
                dragging = false;
                dragCandidate = false;
                mode = NONE;
                flushNow();
                return true;

            case MotionEvent.ACTION_CANCEL:
                removeCallbacks(holdCheck);
                if (dragging) sink.button(Protocol.BTN_LEFT, false);
                dragging = false;
                dragCandidate = false;
                mode = NONE;
                return true;
        }
        return true;
    }

    private static float avgY(MotionEvent e) {
        float s = 0;
        int n = e.getPointerCount();
        for (int i = 0; i < n; i++) s += e.getY(i);
        return s / n;
    }

    @Override
    protected void onDetachedFromWindow() {
        removeCallbacks(flush);
        removeCallbacks(holdCheck);
        if (pendingClick != null) removeCallbacks(pendingClick);
        if (dragging) sink.button(Protocol.BTN_LEFT, false);
        super.onDetachedFromWindow();
    }
}
