// Without `-ObjC` the linker drops libwebrtc's ObjC codec categories; cargo won't forward it from webrtc-sys.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
