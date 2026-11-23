//! Link flags of the macOS build. The call media engine (`teams-media`) brings its own libwebrtc: never link a
//! second one (such as `LiveKitWebRTC.framework`) into this binary, their C++ symbols would cross-bind.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // Without `-ObjC` the linker drops libwebrtc's ObjC codec categories and the factory crashes when a call starts;
    // cargo won't forward it from webrtc-sys.
    println!("cargo:rustc-link-arg=-ObjC");
    // macOS frameworks the app calls directly (CoreAudio's HAL, the device tests, sample buffers).
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
}
