//! Where the app keeps its files: `~/Library/Application Support` and `~/Library/Caches` on macOS,
//! `%APPDATA%` and `%LOCALAPPDATA%` on Windows, the XDG folders elsewhere.
use std::path::PathBuf;

const APP: &str = "oxide-gpui";

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from)
}

/// Settings and small state kept between launches.
pub fn support() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support").join(APP));
    }
    if cfg!(windows) {
        return Some(PathBuf::from(std::env::var_os("APPDATA")?).join(APP));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home()?.join(".config")))?;
    Some(base.join(APP))
}

/// Files that can be fetched again.
pub fn caches() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Caches").join(APP));
    }
    if cfg!(windows) {
        return Some(
            PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                .join(APP)
                .join("Cache"),
        );
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(home()?.join(".cache")))?;
    Some(base.join(APP))
}

/// Where save dialogs open first.
pub fn downloads() -> PathBuf {
    home()
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}
