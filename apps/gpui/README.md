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
- **Calls:** real calls use the same LiveKitWebRTC framework as the SwiftUI app, called through the Objective-C runtime. They have only been tried in the demo.

## Windows
The workflow `.github/workflows/gpui-windows.yml` builds `Oxide.exe` for Windows on ARM on each push to `feature/gpui` (Actions tab, artifact "Oxide-GPUI-windows-arm64"). It is built on Windows because GPUI compiles its DirectX shaders with the SDK's `fxc.exe`. On Windows:
- **Calls are not available.** They need LiveKit's WebRTC build for macOS, which is called through Objective-C.
- **No banners, badge or spell checking.** Downloaded files open in their default app.
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
