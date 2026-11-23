#!/bin/sh
# Builds the Rust core into apps/macos/OxideCore.xcframework and its Swift bindings into apps/macos/Sources/OxideCore.
set -eu
cd "$(dirname "$0")/.."
PROFILE="${PROFILE:-release}"
TARGET_DIR="target/$PROFILE"
APP="apps/macos"
# Must match the platform in apps/macos/Package.swift.
export MACOSX_DEPLOYMENT_TARGET=14.0
GEN="target/uniffi"
# DEMO=1 adds the fake services behind `--demo`; release apps never ship them.
# MEDIA=0 leaves out the libwebrtc call engine (`CallMedia`).
FEATURE_LIST=""
if [ "${MEDIA:-1}" = 1 ]; then FEATURE_LIST="media"; fi
if [ "${DEMO:-0}" = 1 ]; then FEATURE_LIST="${FEATURE_LIST:+$FEATURE_LIST,}demo"; fi
if [ -z "$FEATURE_LIST" ]; then FEATURES=""; else FEATURES="--features $FEATURE_LIST"; fi

# Panic messages embed source paths: keep the builder's home and checkout out of the binary.
REMAP="--remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
export RUSTFLAGS="${RUSTFLAGS:-} $REMAP"
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$HOME=~ -ffile-prefix-map=$PWD=."

cargo build -p teams-core --lib --profile "$PROFILE" $FEATURES
rm -rf "$GEN"
cargo run -q -p teams-core --bin uniffi-bindgen -- generate \
    --library "$TARGET_DIR/libteams_core.dylib" \
    --language swift --out-dir "$GEN"

mkdir -p "$GEN/include" "$APP/Sources/OxideCore"
cp "$GEN/teams_coreFFI.h" "$GEN/include/"
# SwiftPM finds the C module through the xcframework's modulemap; the video frame callback is plain C.
sed 's/^module teams_coreFFI/module OxideCoreFFI/' "$GEN/teams_coreFFI.modulemap" \
    > "$GEN/include/module.modulemap"
if [ "${MEDIA:-1}" = 1 ]; then
    cp crates/teams-core/include/teams_media.h "$GEN/include/"
    awk '{ print } /header "teams_coreFFI.h"/ { print "    header \"teams_media.h\"" }' \
        "$GEN/include/module.modulemap" > "$GEN/include/module.modulemap.new"
    mv "$GEN/include/module.modulemap.new" "$GEN/include/module.modulemap"
fi
sed 's/import teams_coreFFI/import OxideCoreFFI/; s/canImport(teams_coreFFI)/canImport(OxideCoreFFI)/' \
    "$GEN/teams_core.swift" > "$APP/Sources/OxideCore/oxide_core.swift"

rm -rf "$APP/OxideCore.xcframework"
xcodebuild -create-xcframework \
    -library "$TARGET_DIR/libteams_core.a" -headers "$GEN/include" \
    -output "$APP/OxideCore.xcframework" >/dev/null
echo "OxideCore prêt ($PROFILE)."
