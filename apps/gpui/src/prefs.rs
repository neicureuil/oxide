//! Settings kept on this Mac between launches (the macOS app's `@AppStorage`), as JSON in Application Support.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePref {
    #[default]
    System,
    French,
    English,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshPref {
    #[default]
    Push,
    Poll,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub appearance: Appearance,
    pub language: LanguagePref,
    pub refresh: RefreshPref,
    /// Chosen devices: CoreAudio UIDs and the camera's unique ID, as Swift keeps them; empty means the system default.
    pub microphone: String,
    pub speaker: String,
    pub camera: String,
    /// Spell checking in the composer.
    pub spelling: bool,
    /// The calendar's view: "agenda", "day", "week" or "month".
    pub calendar_mode: String,
}

fn path() -> Option<PathBuf> {
    Some(crate::paths::support()?.join("prefs.json"))
}

impl Prefs {
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
    pub fn core_language(&self) -> Option<teams_core::Language> {
        match self.language {
            LanguagePref::French => Some(teams_core::Language::French),
            LanguagePref::English => Some(teams_core::Language::English),
            LanguagePref::System => None,
        }
    }
    pub fn core_refresh(&self) -> teams_core::RefreshMode {
        match self.refresh {
            RefreshPref::Push => teams_core::RefreshMode::Push,
            RefreshPref::Poll => teams_core::RefreshMode::Polling,
        }
    }
}
