//! Updates from the latest GitHub release, the way Discord does them.
//!
//! Every launch opens a small splash window that checks for a new version
//! before anything else starts. A new version is downloaded, its minisign
//! signature verified by the official plugin, installed in place, and the app
//! restarts into it. Without one (or without network) the splash closes and
//! the app starts. While the app runs, it checks again every few hours;
//! a version found then is installed right away and the panel offers a
//! restart. On Windows the installed `.exe` is swapped in place instead of
//! running an installer. AppImage and `.app` bundles use the plugin's own
//! installer.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

pub const SPLASH: &str = "splash";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// No network at login must not hold the app for long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Keeps the splash from flashing when the check is instant.
const SPLASH_MIN: Duration = Duration::from_millis(700);

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Checking,
    Downloading,
    Installing,
    Restarting,
}

/// What the splash window shows, polled by `ui/splash.js`.
#[derive(Clone, Copy, Default, Serialize)]
pub struct Progress {
    stage: Stage,
    /// Download progress, when the size is known.
    percent: Option<u8>,
}

#[derive(Default)]
pub struct SplashState(Mutex<Progress>);

#[tauri::command]
pub fn update_progress(state: State<'_, SplashState>) -> Progress {
    *state.0.lock()
}

fn set_progress(app: &AppHandle, stage: Stage, percent: Option<u8>) {
    *app.state::<SplashState>().0.lock() = Progress { stage, percent };
}

/// Development builds never replace themselves and start right away.
pub fn enabled() -> bool {
    !cfg!(debug_assertions)
}

/// Shows the splash, applies a pending release, then calls `crate::start`.
pub fn launch(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, SPLASH, WebviewUrl::App("splash.html".into()))
        .title("Audio Mirror")
        .inner_size(300.0, 110.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .center()
        .focused(false)
        .build()?;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shown = Instant::now();
        let wait = match check_and_install(&app, true).await {
            Ok(Some(_)) => {
                set_progress(&app, Stage::Restarting, None);
                app.request_restart();
                return;
            }
            Ok(None) => CHECK_EVERY,
            Err(e) => {
                log::warn!("update: {e}");
                RETRY_AFTER
            }
        };
        if let Some(rest) = SPLASH_MIN.checked_sub(shown.elapsed()) {
            sleep(rest).await;
        }
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(splash) = handle.get_webview_window(SPLASH) {
                let _ = splash.destroy();
            }
            crate::start(&handle);
        });
        watch(app, wait).await;
    });
    Ok(())
}

/// Checks again while the app runs; a found version waits for a restart.
async fn watch(app: AppHandle, mut wait: Duration) {
    loop {
        sleep(wait).await;
        wait = match check_and_install(&app, false).await {
            Ok(Some(version)) => {
                *app.state::<AppState>().update_ready.lock() = Some(version.clone());
                let _ = app.emit("update-ready", version);
                // Installed: nothing more to do until the next restart.
                return;
            }
            Ok(None) => CHECK_EVERY,
            Err(e) => {
                log::warn!("update: {e}");
                RETRY_AFTER
            }
        };
    }
}

async fn sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// Installs a newer release if there is one and returns its version.
/// `splash` reports each step to the splash window.
async fn check_and_install(app: &AppHandle, splash: bool) -> Result<Option<String>, String> {
    let update = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(mut update) = update else {
        return Ok(None);
    };
    update.timeout = Some(DOWNLOAD_TIMEOUT);

    let mut received = 0usize;
    let bytes = update
        .download(
            |chunk, total| {
                if !splash {
                    return;
                }
                received += chunk;
                let percent = total
                    .filter(|&t| t > 0)
                    .map(|t| (received as u64 * 100 / t).min(100) as u8);
                set_progress(app, Stage::Downloading, percent);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    if splash {
        set_progress(app, Stage::Installing, None);
    }
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
