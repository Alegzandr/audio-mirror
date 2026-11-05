//! Where the app keeps its files.
//!
//! The folders are the ones Tauri resolved for the bundle identifier
//! (`app_config_dir`), so the settings of an install that predates the GPUI
//! shell are found where they already are.

use std::path::PathBuf;

/// Bundle identifier, also the name of the app's folders.
pub const IDENTIFIER: &str = "io.github.alegzandr.audiomirror";

/// Folder of the settings: `%APPDATA%`, `~/Library/Application Support` or
/// `$XDG_CONFIG_HOME`, then the identifier.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(IDENTIFIER))
}
