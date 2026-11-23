//! Lists what the engine can use on this machine: `cargo run -p teams-media --example devices`.
//! A quick check of device access on Windows and Linux, where there's no app yet.
use teams_media::{ScreenSourceKind, audio_devices, cameras, screen_sources};

fn main() {
    for device in audio_devices() {
        let kind = if device.input { "microphone" } else { "speaker" };
        println!("{kind:10} {} [{}]", device.name, device.id);
    }
    for camera in cameras() {
        println!("camera     {} [{}]", camera.name, camera.id);
    }
    for kind in [ScreenSourceKind::Screen, ScreenSourceKind::Window] {
        for source in screen_sources(kind) {
            println!("{:10} {} [{}]", format!("{kind:?}").to_lowercase(), source.title, source.id);
        }
    }
}
