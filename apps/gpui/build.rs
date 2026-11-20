//! Links LiveKit's libwebrtc build (`LiveKitWebRTC.framework`), the one the macOS app uses for calls.
//!
//! The framework comes with the Swift package's artifacts (`apps/macos/.build`, fetched by `swift build`), or from
//! `OXIDE_WEBRTC_FRAMEWORKS` (the folder holding `LiveKitWebRTC.framework`). It is linked weakly: without it the app
//! still starts, and calls fail with a clear message instead (`media_objc::available`).
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=OXIDE_WEBRTC_FRAMEWORKS");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let folder = std::env::var_os("OXIDE_WEBRTC_FRAMEWORKS").map(PathBuf::from).unwrap_or_else(|| {
        manifest.join(
            "../macos/.build/artifacts/webrtc-xcframework/LiveKitWebRTC/LiveKitWebRTC.xcframework/macos-arm64_x86_64",
        )
    });
    // macOS frameworks the media code calls directly (CoreAudio's HAL, capture, sample buffers).
    for framework in [
        "AVFoundation",
        "CoreAudio",
        "AudioToolbox",
        "CoreMedia",
        "CoreVideo",
        "CoreGraphics",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    if !folder.join("LiveKitWebRTC.framework").exists() {
        println!(
            "cargo:warning=LiveKitWebRTC.framework not found in {}: calls will have no media (run `swift build` in apps/macos, or set OXIDE_WEBRTC_FRAMEWORKS)",
            folder.display()
        );
        return;
    }
    let folder = folder.canonicalize().unwrap_or(folder);
    println!(
        "cargo:rerun-if-changed={}",
        folder.join("LiveKitWebRTC.framework").display()
    );
    println!("cargo:rustc-link-search=framework={}", folder.display());
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,LiveKitWebRTC");
    // From the build folder, and from `Oxide.app/Contents/Frameworks` once bundled.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", folder.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
}
