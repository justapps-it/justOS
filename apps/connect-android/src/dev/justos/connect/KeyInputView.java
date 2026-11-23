package dev.justos.connect;

import android.content.Context;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

/**
 * An invisible view that takes the soft keyboard's input and turns it into
 * TEXT and KEY messages instead of editing anything locally. Composing text
 * (predictions) is followed by diffing, so corrections become backspaces.
 */
final class KeyInputView extends View {
    interface Sink {
        void text(String s);

        void key(int code);
    }

    private final Sink sink;

    KeyInputView(Context c, Sink sink) {
        super(c);
        this.sink = sink;
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    void showKeyboard() {
        requestFocus();
        InputMethodManager imm = getContext().getSystemService(InputMethodManager.class);
        imm.restartInput(this);
        imm.showSoftInput(this, 0);
    }

    void hideKeyboard() {
        InputMethodManager imm = getContext().getSystemService(InputMethodManager.class);
        imm.hideSoftInputFromWindow(getWindowToken(), 0);
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        out.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        out.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_ACTION_NONE;
        out.initialSelStart = 0;
        out.initialSelEnd = 0;
        return new Connection();
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        return handleKey(keyCode, event) || super.onKeyDown(keyCode, event);
    }

    /** A key from a hardware keyboard or the IME. */
    private boolean handleKey(int keyCode, KeyEvent e) {
        int code;
        switch (keyCode) {
            case KeyEvent.KEYCODE_DEL: code = Protocol.K_BACKSPACE; break;
            case KeyEvent.KEYCODE_FORWARD_DEL: code = Protocol.K_DELETE; break;
            case KeyEvent.KEYCODE_ENTER:
            case KeyEvent.KEYCODE_NUMPAD_ENTER: code = Protocol.K_ENTER; break;
            case KeyEvent.KEYCODE_TAB: code = Protocol.K_TAB; break;
            case KeyEvent.KEYCODE_ESCAPE: code = Protocol.K_ESC; break;
            case KeyEvent.KEYCODE_DPAD_UP: code = Protocol.K_UP; break;
            case KeyEvent.KEYCODE_DPAD_DOWN: code = Protocol.K_DOWN; break;
            case KeyEvent.KEYCODE_DPAD_LEFT: code = Protocol.K_LEFT; break;
            case KeyEvent.KEYCODE_DPAD_RIGHT: code = Protocol.K_RIGHT; break;
            case KeyEvent.KEYCODE_MOVE_HOME: code = Protocol.K_HOME; break;
            case KeyEvent.KEYCODE_MOVE_END: code = Protocol.K_END; break;
            case KeyEvent.KEYCODE_PAGE_UP: code = Protocol.K_PGUP; break;
            case KeyEvent.KEYCODE_PAGE_DOWN: code = Protocol.K_PGDN; break;
            default: code = -1; break;
        }
        if (code >= 0) {
            sink.key(code);
            return true;
        }
        if (e == null) return false;
        int ch = e.getUnicodeChar();
        if (ch > 0) {
            sink.text(new String(Character.toChars(ch)));
            return true;
        }
        return false;
    }

    /** Sends text, turning new lines into Enter keys. */
    private void emit(String s) {
        int start = 0;
        for (int i = 0; i < s.length(); i++) {
            if (s.charAt(i) == '\n') {
                if (i > start) sink.text(s.substring(start, i));
                sink.key(Protocol.K_ENTER);
                start = i + 1;
            }
        }
        if (start < s.length()) sink.text(s.substring(start));
    }

    private final class Connection extends BaseInputConnection {
        /** The composing text as the PC has it. */
        private String composing = "";

        Connection() {
            super(KeyInputView.this, false);
        }

        /** Replaces the composing text on the PC with {@code next}. */
        private void replace(String next) {
            int common = 0;
            int max = Math.min(composing.length(), next.length());
            while (common < max && composing.charAt(common) == next.charAt(common)) common++;
            for (int i = composing.length() - 1; i >= common; i--) sink.key(Protocol.K_BACKSPACE);
            if (common < next.length()) emit(next.substring(common));
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            replace(text.toString());
            composing = "";
            return true;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            String next = text.toString();
            replace(next);
            composing = next;
            return true;
        }

        @Override
        public boolean finishComposingText() {
            composing = "";
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            for (int i = 0; i < beforeLength; i++) sink.key(Protocol.K_BACKSPACE);
            for (int i = 0; i < afterLength; i++) sink.key(Protocol.K_DELETE);
            return true;
        }

        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            if (event.getAction() == KeyEvent.ACTION_DOWN) handleKey(event.getKeyCode(), event);
            return true;
        }

        @Override
        public boolean performEditorAction(int actionCode) {
            sink.key(Protocol.K_ENTER);
            return true;
        }

        @Override
        public CharSequence getTextBeforeCursor(int n, int flags) {
            return "";
        }

        @Override
        public CharSequence getTextAfterCursor(int n, int flags) {
            return "";
        }
    }
}
