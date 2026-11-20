#!/bin/sh
# call_shot.sh <out.png> [OXIDE_SCRIPT] [seconds]: like shot.sh, but captures the call window (main window if none).
# Also writes <out>.main.png with the main window.
set -eu
OUT=$1
SCRIPT=${2:-}
WAIT=${3:-6}
cd "$(dirname "$0")/../../.."
TARGET=${CARGO_TARGET_DIR:-target}
TOOLS=$TARGET/tools
mkdir -p "$TOOLS"
[ "$TOOLS/call_winid" -nt apps/gpui/tools/call_winid.swift ] || swiftc -O apps/gpui/tools/call_winid.swift -o "$TOOLS/call_winid"
OXIDE_SCRIPT="$SCRIPT" "$TARGET/debug/oxide-gpui" --demo > "$OUT.log" 2>&1 &
PID=$!
sleep "$WAIT"
W=$("$TOOLS/call_winid" $PID)
screencapture -x -o -l"$W" "$OUT"
M=$("$TOOLS/call_winid" $PID main)
screencapture -x -o -l"$M" "$OUT.main.png"
kill $PID
wait $PID 2>/dev/null || true
echo "$OUT"
