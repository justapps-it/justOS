#!/usr/bin/env bash
# Builds build/justos-connect.apk without Gradle: aapt2, javac, d8, zipalign, apksigner.
# Env: ANDROID_HOME (required), BUILD_TOOLS (default 36.0.0), PLATFORM (default android-36),
#      CONNECT_KEYSTORE (default ~/.justos-connect.keystore), CONNECT_KEYSTORE_PASS (default justos-connect).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
BUILD_TOOLS="${BUILD_TOOLS:-36.0.0}"
PLATFORM="${PLATFORM:-android-36}"
BT="$ANDROID_HOME/build-tools/$BUILD_TOOLS"
ANDROID_JAR="$ANDROID_HOME/platforms/$PLATFORM/android.jar"
KEYSTORE="${CONNECT_KEYSTORE:-$HOME/.justos-connect.keystore}"
export CONNECT_KEYSTORE_PASS="${CONNECT_KEYSTORE_PASS:-justos-connect}"

for p in "$BT/aapt2" "$BT/d8" "$BT/zipalign" "$BT/apksigner" "$ANDROID_JAR"; do
  [ -e "$p" ] || { echo "Missing: $p" >&2; exit 1; }
done

OUT="$ROOT/build"
GEN="$OUT/gen"; CLASSES="$OUT/classes"; DEX="$OUT/dex"
RES_ZIP="$OUT/res.zip"; UNSIGNED="$OUT/unsigned.apk"; ALIGNED="$OUT/aligned.apk"
APK="$OUT/justos-connect.apk"

rm -rf "$GEN" "$CLASSES" "$DEX" "$RES_ZIP" "$UNSIGNED" "$ALIGNED" "$APK" "$APK.idsig" "$OUT/classes.jar"
mkdir -p "$GEN" "$CLASSES" "$DEX"

echo "==> aapt2 compile"
"$BT/aapt2" compile --dir "$ROOT/res" -o "$RES_ZIP"
echo "==> aapt2 link"
"$BT/aapt2" link -o "$UNSIGNED" -I "$ANDROID_JAR" \
  --manifest "$ROOT/AndroidManifest.xml" \
  --java "$GEN" --min-sdk-version 26 --target-sdk-version 36 \
  --version-code 1 --version-name 1.0 "$RES_ZIP"

echo "==> javac"
find "$ROOT/src" "$GEN" -name '*.java' > "$OUT/sources.txt"
javac --release 17 -encoding UTF-8 -Xlint:-options -classpath "$ANDROID_JAR" -d "$CLASSES" @"$OUT/sources.txt"

echo "==> d8"
jar --create --file "$OUT/classes.jar" -C "$CLASSES" .
"$BT/d8" --release --min-api 26 --lib "$ANDROID_JAR" --output "$DEX" "$OUT/classes.jar"

echo "==> add dex"
(cd "$DEX" && "$BT/aapt" add "$UNSIGNED" classes.dex >/dev/null)

echo "==> zipalign"
"$BT/zipalign" -p -f 4 "$UNSIGNED" "$ALIGNED"

if [ ! -f "$KEYSTORE" ]; then
  echo "==> Creating signing key at $KEYSTORE"
  keytool -genkeypair -keystore "$KEYSTORE" -alias justos -keyalg RSA -keysize 2048 -validity 10000 \
    -storepass "$CONNECT_KEYSTORE_PASS" -keypass "$CONNECT_KEYSTORE_PASS" -dname 'CN=JustOS Connect' -noprompt
fi

echo "==> apksigner"
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-key-alias justos \
  --ks-pass env:CONNECT_KEYSTORE_PASS --key-pass env:CONNECT_KEYSTORE_PASS \
  --out "$APK" "$ALIGNED"
"$BT/apksigner" verify --verbose "$APK"

rm -f "$UNSIGNED" "$ALIGNED"
echo "BUILD OK: $APK ($(wc -c < "$APK") bytes)"
