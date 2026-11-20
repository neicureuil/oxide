//! Call settings kept between launches that the macOS app keeps in `UserDefaults` (`call.prejoin.*`, `call.layout`,
//! `call.recordings.folder`, `call.echoBotMri`), as JSON next to the app's other preferences.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CallPrefs {
    /// Pre-join screen: camera on, microphone off.
    pub prejoin_camera: bool,
    pub prejoin_muted: bool,
    /// The remembered call layout (`gallery`, `speaker`, `sharing`, `focus`).
    pub layout: String,
    /// Folder of local recordings; empty means `~/Documents/Oxide Recordings`.
    pub recordings_folder: String,
    /// Optional override of the Echo Bot MRI for test calls.
    pub echo_bot_mri: String,
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/oxide-gpui/calls.json"))
}

impl CallPrefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, json);
        }
    }

    /// `TestCall.botOverride`.
    pub fn bot_override(&self) -> Option<String> {
        (!self.echo_bot_mri.is_empty()).then(|| self.echo_bot_mri.clone())
    }
}
