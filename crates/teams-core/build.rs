// With the `media` feature the core links libwebrtc, whose ObjC codec factories live in categories the
// linker drops without `-ObjC` (cargo doesn't forward it from webrtc-sys). This covers the dylib used for
// the bindings; the app adds it for the static library in Package.swift.
fn main() {
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if macos && std::env::var_os("CARGO_FEATURE_MEDIA").is_some() {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
