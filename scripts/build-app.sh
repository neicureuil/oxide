#!/bin/sh
# Builds target/Oxide.app: Rust core, Swift shell, fonts and Info.plist.
set -eu
cd "$(dirname "$0")/.."
./scripts/build-core.sh
swift build --package-path apps/macos -c release
BIN="$(swift build --package-path apps/macos -c release --show-bin-path)/Oxide"
APP="target/Oxide.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Fonts"
cp "$BIN" "$APP/Contents/MacOS/Oxide"
# libwebrtc is linked statically into the core (crates/teams-media): no framework to embed.
cp apps/macos/Resources/Fonts/* "$APP/Contents/Resources/Fonts/"
cp apps/macos/Resources/AppIcon.icns "$APP/Contents/Resources/"
# The version lives in Cargo.toml's [workspace.package]; the build number counts commits (override with BUILD_NUMBER).
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
BUILD="${BUILD_NUMBER:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>fr.neicureuil.oxide</string>
    <key>CFBundleName</key><string>Oxide</string>
    <key>CFBundleDisplayName</key><string>Oxide</string>
    <key>CFBundleExecutable</key><string>Oxide</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$BUILD</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>ATSApplicationFontsPath</key><string>Fonts</string>
    <key>CFBundleDevelopmentRegion</key><string>fr</string>
    <key>CFBundleLocalizations</key><array><string>fr</string><string>en</string></array>
    <key>NSMicrophoneUsageDescription</key><string>Oxide utilise le micro pendant les appels et réunions, et pour le tester dans les réglages.</string>
    <key>NSCameraUsageDescription</key><string>Oxide utilise la caméra pendant les appels vidéo et réunions, et pour l’aperçu dans les réglages.</string>
</dict>
</plist>
PLIST
# The usage descriptions in the system's (or the app's chosen) language.
mkdir -p "$APP/Contents/Resources/fr.lproj" "$APP/Contents/Resources/en.lproj"
cat > "$APP/Contents/Resources/fr.lproj/InfoPlist.strings" <<STRINGS
"NSMicrophoneUsageDescription" = "Oxide utilise le micro pendant les appels et réunions, et pour le tester dans les réglages.";
"NSCameraUsageDescription" = "Oxide utilise la caméra pendant les appels vidéo et réunions, et pour l’aperçu dans les réglages.";
STRINGS
cat > "$APP/Contents/Resources/en.lproj/InfoPlist.strings" <<STRINGS
"NSMicrophoneUsageDescription" = "Oxide uses the microphone during calls and meetings, and to test it in Settings.";
"NSCameraUsageDescription" = "Oxide uses the camera during video calls and meetings, and for the preview in Settings.";
STRINGS
# Ad hoc by default: no certificate or team in the app. SIGN_IDENTITY names one, whose stable
# identity keeps the Keychain's "Always Allow" across rebuilds (ad hoc changes each build).
ENTITLEMENTS="apps/macos/Oxide.entitlements"
IDENTITY="${SIGN_IDENTITY:--}"
if [ "$IDENTITY" = - ]; then
    echo "Signature ad hoc : le Trousseau redemandera après chaque build (SIGN_IDENTITY=\"…\" pour un certificat)." >&2
fi
codesign --force --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP" >/dev/null
echo "$APP ($VERSION, build $BUILD)"
