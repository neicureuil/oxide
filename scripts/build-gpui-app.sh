#!/bin/sh
# Builds target/OxideGPUI.app: the GPUI client (fonts are compiled in), its icon and Info.plist.
# PROFILE=dev for a quick unoptimised bundle, DEMO=1 to keep the fake services behind `--demo`,
# CARGO_TARGET_DIR to pick the build directory, SIGN_IDENTITY to sign with a certificate.
# Never set MACOSX_DEPLOYMENT_TARGET here: it breaks proc-macro dylibs on this toolchain; the minimum
# version is LSMinimumSystemVersion below.
set -eu
cd "$(dirname "$0")/.."
PROFILE="${PROFILE:-release}"
TARGET="${CARGO_TARGET_DIR:-target}"
case "$PROFILE" in
    dev) OUT_DIR="$TARGET/debug" ;;
    *) OUT_DIR="$TARGET/$PROFILE" ;;
esac
FEATURES=""
if [ "${DEMO:-0}" = 1 ]; then FEATURES="--features demo"; fi

# Panic messages embed source paths: keep the builder's home and checkout out of the binary.
REMAP="--remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
export RUSTFLAGS="${RUSTFLAGS:-} $REMAP"
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$HOME=~ -ffile-prefix-map=$PWD=."
CARGO_TARGET_DIR="$TARGET" cargo build -p oxide-gpui --profile "$PROFILE" $FEATURES
BIN="$OUT_DIR/oxide-gpui"

APP="${APP_PATH:-target/OxideGPUI.app}"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/oxide-gpui"
cp apps/macos/Resources/AppIcon.icns "$APP/Contents/Resources/"

# Dynamic frameworks the binary loads through @rpath (LiveKitWebRTC, linked weakly by apps/gpui/build.rs
# when present): embed them, from OXIDE_WEBRTC_FRAMEWORKS or the Swift package's artifacts, as build.rs finds them.
FRAMEWORKS=$(otool -L "$BIN" | sed -n 's|^[[:space:]]*@rpath/\([^/]*\.framework\)/.*|\1|p' | sort -u)
for FRAMEWORK in $FRAMEWORKS; do
    FOUND=""
    if [ -n "${OXIDE_WEBRTC_FRAMEWORKS:-}" ] && [ -d "$OXIDE_WEBRTC_FRAMEWORKS/$FRAMEWORK" ]; then
        FOUND="$OXIDE_WEBRTC_FRAMEWORKS/$FRAMEWORK"
    fi
    [ -n "$FOUND" ] || FOUND="$(find apps/macos/.build/artifacts -maxdepth 6 -type d -name "$FRAMEWORK" \
        -path "*macos-arm64_x86_64*" 2>/dev/null | head -1)"
    if [ -z "$FOUND" ]; then
        echo "Introuvable : $FRAMEWORK, requis par oxide-gpui (swift build dans apps/macos, ou OXIDE_WEBRTC_FRAMEWORKS)." >&2
        exit 1
    fi
    mkdir -p "$APP/Contents/Frameworks"
    cp -R "$FOUND" "$APP/Contents/Frameworks/"
done
# Only the bundle's own Frameworks folder: build.rs also points at the build machine's copy.
for RPATH in $(otool -l "$APP/Contents/MacOS/oxide-gpui" | sed -n 's|^ *path \(/[^ ]*\) (offset.*|\1|p'); do
    install_name_tool -delete_rpath "$RPATH" "$APP/Contents/MacOS/oxide-gpui"
done
if [ -n "$FRAMEWORKS" ] && ! otool -l "$APP/Contents/MacOS/oxide-gpui" | grep -q "@executable_path/../Frameworks"; then
    install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/oxide-gpui"
fi

# The version lives in Cargo.toml's [workspace.package]; the build number counts commits (override with BUILD_NUMBER).
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
BUILD="${BUILD_NUMBER:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}"
MIC_FR="Oxide utilise le micro pendant les appels et réunions, et pour le tester dans les réglages."
CAM_FR="Oxide utilise la caméra pendant les appels vidéo et réunions, et pour l’aperçu dans les réglages."
SCREEN_FR="Oxide partage votre écran ou une fenêtre pendant les appels et réunions, quand vous le demandez."
MIC_EN="Oxide uses the microphone during calls and meetings, and to test it in Settings."
CAM_EN="Oxide uses the camera during video calls and meetings, and for the preview in Settings."
SCREEN_EN="Oxide shares your screen or a window during calls and meetings, when you ask it to."
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>fr.neicureuil.oxide.gpui</string>
    <key>CFBundleName</key><string>Oxide GPUI</string>
    <key>CFBundleDisplayName</key><string>Oxide GPUI</string>
    <key>CFBundleExecutable</key><string>oxide-gpui</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>CFBundleDevelopmentRegion</key><string>fr</string>
    <key>CFBundleLocalizations</key><array><string>fr</string><string>en</string></array>
    <key>NSMicrophoneUsageDescription</key><string>$MIC_FR</string>
    <key>NSCameraUsageDescription</key><string>$CAM_FR</string>
    <key>NSScreenCaptureUsageDescription</key><string>$SCREEN_FR</string>
</dict>
</plist>
PLIST
# The usage descriptions in the system's language.
mkdir -p "$APP/Contents/Resources/fr.lproj" "$APP/Contents/Resources/en.lproj"
cat > "$APP/Contents/Resources/fr.lproj/InfoPlist.strings" <<STRINGS
"NSMicrophoneUsageDescription" = "$MIC_FR";
"NSCameraUsageDescription" = "$CAM_FR";
"NSScreenCaptureUsageDescription" = "$SCREEN_FR";
STRINGS
cat > "$APP/Contents/Resources/en.lproj/InfoPlist.strings" <<STRINGS
"NSMicrophoneUsageDescription" = "$MIC_EN";
"NSCameraUsageDescription" = "$CAM_EN";
"NSScreenCaptureUsageDescription" = "$SCREEN_EN";
STRINGS
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# Ad hoc by default: no certificate or team in the app. SIGN_IDENTITY names one, whose stable
# identity keeps the Keychain's "Always Allow" across rebuilds (ad hoc changes each build).
ENTITLEMENTS="apps/macos/Oxide.entitlements"
IDENTITY="${SIGN_IDENTITY:--}"
if [ "$IDENTITY" = - ]; then
    echo "Signature ad hoc : le Trousseau redemandera après chaque build (SIGN_IDENTITY=\"…\" pour un certificat)." >&2
fi
if [ -d "$APP/Contents/Frameworks" ]; then
    for FRAMEWORK in "$APP"/Contents/Frameworks/*.framework; do
        codesign --force --sign "$IDENTITY" "$FRAMEWORK" >/dev/null
    done
fi
codesign --force --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP" >/dev/null
echo "$APP ($VERSION, build $BUILD)"
