// libwebrtc's ObjC codec factories live in categories that the linker drops without `-ObjC`. Cargo
// doesn't forward link args from webrtc-sys, so every final binary on macOS must pass it: this covers
// this crate's tests and examples; apps add it in their own build script.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
