package dev.justos.connect;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.widget.Toast;

import java.util.ArrayList;
import java.util.List;

/**
 * Sends files to the PC: either picked here (ACTION_OPEN_DOCUMENT) or shared
 * from another app (ACTION_SEND / ACTION_SEND_MULTIPLE). It has no window of
 * its own; the service streams the files.
 */
public final class SendActivity extends Activity {
    private static final int PICK = 1;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent in = getIntent();
        String action = in.getAction();
        if (Intent.ACTION_SEND.equals(action) || Intent.ACTION_SEND_MULTIPLE.equals(action)) {
            deliver(sharedUris(in));
            finish();
        } else if (state == null) {
            Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("*/*")
                    .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
            try {
                startActivityForResult(pick, PICK);
            } catch (RuntimeException e) {
                finish();
            }
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == PICK && result == RESULT_OK && data != null) {
            List<Uri> uris = new ArrayList<>();
            ClipData clip = data.getClipData();
            if (clip != null) {
                for (int i = 0; i < clip.getItemCount(); i++) {
                    Uri u = clip.getItemAt(i).getUri();
                    if (u != null) uris.add(u);
                }
            } else if (data.getData() != null) {
                uris.add(data.getData());
            }
            deliver(uris);
        }
        finish();
    }

    @SuppressWarnings("deprecation")
    private static List<Uri> sharedUris(Intent in) {
        List<Uri> uris = new ArrayList<>();
        if (Intent.ACTION_SEND_MULTIPLE.equals(in.getAction())) {
            ArrayList<Uri> list = in.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (list != null) {
                for (Uri u : list) if (u != null) uris.add(u);
            }
        } else {
            Uri u = in.getParcelableExtra(Intent.EXTRA_STREAM);
            if (u != null) uris.add(u);
        }
        if (uris.isEmpty() && in.getClipData() != null) {
            ClipData clip = in.getClipData();
            for (int i = 0; i < clip.getItemCount(); i++) {
                Uri u = clip.getItemAt(i).getUri();
                if (u != null) uris.add(u);
            }
        }
        return uris;
    }

    private void deliver(List<Uri> uris) {
        if (uris.isEmpty()) {
            Toast.makeText(this, R.string.toast_nothing_to_send, Toast.LENGTH_SHORT).show();
            return;
        }
        if (!ConnectService.isConnected()) {
            Toast.makeText(this, R.string.toast_not_connected, Toast.LENGTH_LONG).show();
            startActivity(new Intent(this, MainActivity.class));
            return;
        }
        ConnectService.sendFiles(this, uris);
        Toast.makeText(this, getString(R.string.toast_sending, uris.size()), Toast.LENGTH_SHORT).show();
    }
}
