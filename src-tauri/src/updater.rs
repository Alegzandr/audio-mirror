//! Silent updates from the latest GitHub release.
//!
//! The app checks shortly after launch and then every few hours. A new
//! version is downloaded, its minisign signature verified by the official
//! plugin, and installed in place right away; the UI only offers a restart.
//! On Windows the app is a portable `.exe`, so the executable is swapped in
//! place instead of running an installer. AppImage and `.app` bundles use the
//! plugin's own installer.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub fn spawn(app: AppHandle) {
    // Development builds never replace themselves.
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        sleep(FIRST_CHECK).await;
        loop {
            match check_and_install(&app).await {
                Ok(Some(version)) => {
                    *app.state::<AppState>().update_ready.lock() = Some(version.clone());
                    let _ = app.emit("update-ready", version);
                    // Installed: nothing more to do until the next restart.
                    return;
                }
                Ok(None) => {}
                Err(e) => log::warn!("update: {e}"),
            }
            sleep(CHECK_EVERY).await;
        }
    });
}

async fn sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

async fn check_and_install(app: &AppHandle) -> Result<Option<String>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    install(&update, &bytes)?;
    Ok(Some(update.version.clone()))
}

#[cfg(windows)]
fn install(update: &Update, bytes: &[u8]) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("audio-mirror-{}.exe", update.version));
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    let result = self_replace::self_replace(&tmp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(not(windows))]
fn install(update: &Update, bytes: &[u8]) -> Result<(), String> {
    update.install(bytes).map_err(|e| e.to_string())
}
