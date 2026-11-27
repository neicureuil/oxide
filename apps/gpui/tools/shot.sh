#!/bin/sh
# shot.sh <out.png> [OXIDE_SCRIPT] [seconds]: runs the demo, plays the script, captures only the app's window.
# Build first: cargo build -p oxide-gpui --features demo (set CARGO_TARGET_DIR to pick the binary).
set -eu
OUT=$1
SCRIPT=${2:-}
WAIT=${3:-6}
cd "$(dirname "$0")/../../.."
TARGET=${CARGO_TARGET_DIR:-target}
TOOLS=$TARGET/tools
mkdir -p "$TOOLS"
[ -x "$TOOLS/winid" ] || swiftc -O apps/gpui/tools/winid.swift -o "$TOOLS/winid"
OXIDE_SCRIPT="$SCRIPT" "$TARGET/debug/oxide-gpui" --demo > "$OUT.log" 2>&1 &
PID=$!
sleep "$WAIT"
W=$("$TOOLS/winid" $PID)
screencapture -x -o -l"$W" "$OUT"
kill $PID
wait $PID 2>/dev/null || true
echo "$OUT"
