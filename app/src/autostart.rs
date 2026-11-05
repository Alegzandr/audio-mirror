//! Start at login, opt-in from the panel.
//!
//! The entry has the name, arguments and form Tauri's autostart plugin gave
//! it (a `Run` value on Windows, a LaunchAgent on macOS, an XDG autostart
//! file on Linux), so a login item made by an earlier version is the same
//! one this code reads and changes.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

/// Argument passed by the login item: start in the tray, panel hidden.
pub const ARG: &str = "--autostart";
const NAME: &str = "Audio Mirror";

fn launcher() -> Result<AutoLaunch, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let exe = exe.canonicalize().map_err(|e| e.to_string())?;
    // An AppImage runs from a temporary mount: the login item must start
    // the AppImage file itself.
    #[cfg(target_os = "linux")]
    let exe = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or(exe);
    let path = exe.to_string_lossy().into_owned();
    AutoLaunchBuilder::new()
        .set_app_name(NAME)
        .set_app_path(&path)
        .set_args(&[ARG])
        .set_use_launch_agent(true)
        .build()
        .map_err(|e| e.to_string())
}

pub fn is_enabled() -> bool {
    launcher().and_then(|l| l.is_enabled().map_err(|e| e.to_string())) == Ok(true)
}

/// Turns the login item on or off and returns the state it ended in.
pub fn set(enabled: bool) -> Result<bool, String> {
    let launcher = launcher()?;
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| e.to_string())?;
    launcher.is_enabled().map_err(|e| e.to_string())
}

/// An install or an update can move the executable: points an enabled
/// login item at the current path.
pub fn refresh() {
    if is_enabled() {
        let _ = set(true);
    }
}
