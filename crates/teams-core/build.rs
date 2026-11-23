// `-ObjC` keeps libwebrtc's ObjC codec categories in the dylib; the app passes it for the static library.
fn main() {
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if macos && std::env::var_os("CARGO_FEATURE_MEDIA").is_some() {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
