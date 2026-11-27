# Oxide (GPUI)

The same `teams-core` as the macOS app, drawn with [GPUI](https://gpui.rs) and called straight from Rust, with no UniFFI layer. It covers the same screens as the SwiftUI app.

```sh
cargo run -p oxide-gpui --features demo -- --demo   # fake services
./scripts/build-gpui-app.sh                          # target/OxideGPUI.app, release, ad hoc signed
DEMO=1 PROFILE=dev ./scripts/build-gpui-app.sh       # quick bundle with the demo
```

## Layout
- `oxide.rs`: the store. It keeps every part the core sends, as `Store.swift` does.
- `shell.rs`: the window frame (top bar, rail, banners, router).
- `screens.rs`: each screen's local state, the hooks the store calls, and the `OXIDE_SCRIPT` driver.
- One module per area, each named after the Swift files it ports:
  - `sidebar`, `activity`, `search`, `new_chat`, `chat_admin`
  - `conversation`, `feed_*`, `image_viewer`
  - `composer_*`, `input`
  - `teams*`, `members`
  - `calendar_*`
  - `files*`
  - `settings_*`, `calls_screen`
  - `call_*`, `media_*`
  - `platform_*`
- Shared pieces: `widgets.rs`, `icons.rs` (line icons drawn for this app, since SF Symbols are not available to GPUI), `theme.rs` and `prefs.rs`. Settings are stored in `~/Library/Application Support/oxide-gpui/`.

## Differences from the SwiftUI app
- **Sign-in** opens Microsoft's page in the app's own window, as the SwiftUI app does, through [wry](https://github.com/tauri-apps/wry) (WKWebView, WebView2 or WebKitGTK). If that window cannot be created, a link signs in through the default browser instead: copy the address of the Teams page it ends on, then click "Paste the return address".
- **Pickers:** menus and pickers inside dialogs open in place instead of floating, because GPUI cannot draw a deferred element inside another one. Date and time pickers are custom.
- **Spell checking:** spelling mistakes are underlined, but automatic correction is not done.
- **Updates:** the update checker does not run at launch. The published releases are the SwiftUI app's.
- **Calls** run on our own media engine, `crates/teams-media` (libwebrtc), called directly from Rust: the same engine as the SwiftUI app, on macOS, Windows and Linux. Local recordings are `.m4a` on macOS and `.wav` elsewhere.

## Releases
The workflow `.github/workflows/release-gpui.yml` ("Release GPUI" in the Actions tab) builds the app for a version and attaches it to that version's GitHub release, next to the SwiftUI app's `Oxide-<version>.zip`: Run workflow, give the version, tick Windows ARM64 and/or macOS ARM64.
- If the tag is new, the version is bumped on a `release/vX.Y.Z` branch, tagged, and merged back into the branch the workflow ran on (as `release.yml` does with main). If it exists, this branch is built at that version.
- The release is created as a draft when missing; otherwise the files are added to it. `release.yml` does the same, so either can run first.
- Files, named like the SwiftUI app's `Oxide-<version>.zip`: `Oxide-gpui-<version>-windows-arm64.zip` holding `Oxide.exe`, and `Oxide-gpui-<version>-macos-arm64.zip` holding `Oxide.app` (signed ad hoc, bundle id `fr.neicureuil.oxide.gpui`). The SwiftUI app's updater, in versions already installed, takes a release's first `.zip`: `release.yml` uploads the GPUI zips again after its own so they're listed after it, and that updater refuses an app with another bundle id anyway.
- Settings › Check for updates offers a version only when its release has this system's file, and downloads it; the app doesn't replace itself yet.

Windows is built on Windows (GPUI compiles its DirectX shaders with the SDK's `fxc.exe`), macOS on a Mac.

## Windows
On Windows:
- **Calls** use the same engine as on macOS. The Settings page has no microphone test, chime or camera preview yet, and incoming calls ring without a sound.
- **Banners** are Windows notifications (a click opens the conversation, a call's buttons answer it). **No badge or spell checking.** Downloaded files open in their default app.
- The session is kept in the Windows Credential Manager. Settings go in `%APPDATA%\oxide-gpui`, caches in `%LOCALAPPDATA%\oxide-gpui`, logs in `%LOCALAPPDATA%\Oxide\Logs`.
- The exe is not signed, so SmartScreen asks for confirmation: More info, then Run anyway.

## Checking screens without clicking
`OXIDE_SCRIPT` runs a step every 1.5 s after sign-in. The built-in steps are listed in `screens.rs`, and each module adds its own in its `debug_step`. `apps/gpui/tools/shot.sh` captures only the app's window:

```sh
cargo build -p oxide-gpui --features demo
apps/gpui/tools/shot.sh /tmp/teams.png "wait;nav:teams;wait;teams:open 0 0" 9
```

GPUI stops drawing a covered window, so the driver brings the window to the front before each step.

GPUI's shaders are compiled at launch (`runtime_shaders`), so building does not need Xcode's optional Metal Toolchain.
