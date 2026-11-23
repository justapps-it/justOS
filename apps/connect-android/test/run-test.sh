#!/usr/bin/env bash
# Compiles Protocol.java with the test and runs it on the desktop JDK.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
app="$(dirname "$here")"
out="$app/build/test-classes"
rm -rf "$out"
mkdir -p "$out"
javac --release 17 -d "$out" "$app/src/dev/justos/connect/Protocol.java" "$here/ProtocolTest.java"
java -cp "$out" ProtocolTest
