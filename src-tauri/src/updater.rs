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
//!
//! A version that downloads but fails to install (no write access to the
//! install folder, say) is remembered, also on disk: later launches start
//! without downloading it again, and the running app tries it again only
//! after the usual wait between checks, not after the short network retry.

use std::path::PathBuf;
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
/// Holds the version that last failed to install, next to the settings.
const FAILED_FILE: &str = "update-failed.txt";

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
pub struct SplashState {
    progress: Mutex<Progress>,
    /// The version that failed to install in this process.
    failed: Mutex<Option<String>>,
}

#[tauri::command]
pub fn update_progress(state: State<'_, SplashState>) -> Progress {
    *state.progress.lock()
}

fn set_progress(app: &AppHandle, stage: Stage, percent: Option<u8>) {
    *app.state::<SplashState>().progress.lock() = Progress { stage, percent };
}

/// Why no update was installed.
enum Failure {
    /// Checking or downloading failed: worth trying again soon.
    Fetch(String),
    /// The download is fine but could not be installed: trying again soon
    /// would only download it again for nothing.
    Install(String),
}

impl Failure {
    /// How long to wait before the next check.
    fn wait(&self) -> Duration {
        match self {
            Failure::Fetch(e) => {
                log::warn!("update: {e}");
                RETRY_AFTER
            }
            Failure::Install(e) => {
                log::warn!("update install: {e}");
                CHECK_EVERY
            }
        }
    }
}

fn failed_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join(FAILED_FILE))
}

fn failed_version(app: &AppHandle) -> Option<String> {
    if let Some(v) = app.state::<SplashState>().failed.lock().clone() {
        return Some(v);
    }
    let text = std::fs::read_to_string(failed_path(app)?).ok()?;
    Some(text.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Remembers the version that failed to install, or forgets it with `None`.
fn set_failed(app: &AppHandle, version: Option<&str>) {
    *app.state::<SplashState>().failed.lock() = version.map(str::to_owned);
    let Some(path) = failed_path(app) else {
        return;
    };
    let result = match version {
        Some(v) => path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, v)),
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    };
    if let Err(e) = result {
        log::warn!("update: {}: {e}", path.display());
    }
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
            Err(e) => e.wait(),
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
            Err(e) => e.wait(),
        };
    }
}

/// A timer, not a parked thread. The wait between checks is six hours, and
/// `spawn_blocking` would hold a pool thread and its stack for all of it, in
/// an app whose whole point is to sit in the tray unnoticed.
async fn sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

/// Installs a newer release if there is one and returns its version.
/// `splash` reports each step to the splash window, and skips a version that
/// already failed to install: the launch must not wait on a download that
/// cannot be used. The running app still tries it, every `CHECK_EVERY`.
async fn check_and_install(app: &AppHandle, splash: bool) -> Result<Option<String>, Failure> {
    let update = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| Failure::Fetch(e.to_string()))?
        .check()
        .await
        .map_err(|e| Failure::Fetch(e.to_string()))?;
    let Some(mut update) = update else {
        return Ok(None);
    };
    if splash && failed_version(app).as_deref() == Some(update.version.as_str()) {
        log::info!("update: {} failed to install before", update.version);
        return Ok(None);
    }
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
        .map_err(|e| Failure::Fetch(e.to_string()))?;
    if splash {
        set_progress(app, Stage::Installing, None);
    }
    // Writing tens of megabytes and swapping the executable in is disk work,
    // not async work: left on the runtime's thread it holds up every other
    // task while it runs, for the same reason the six hour wait between
    // checks is a timer rather than a parked thread.
    let version = update.version.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || install(&update, &bytes))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    if let Err(e) = installed {
        set_failed(app, Some(&version));
        return Err(Failure::Install(e));
    }
    if failed_version(app).is_some() {
        set_failed(app, None);
    }
    Ok(Some(version))
}

#[cfg(windows)]
fn install(update: &Update, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;

    // The verified bytes are staged next to the executable, in a file this
    // process creates itself, so nothing can swap them before the replace.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no install folder")?;
    let tmp = dir.join(format!("audio-mirror.update-{}.exe", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        });
    let result = written
        .map_err(|e| e.to_string())
        .and_then(|()| self_replace::self_replace(&tmp).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    if result.is_ok() {
        crate::install::updated(&update.version);
    }
    result
}

#[cfg(not(windows))]
fn install(update: &Update, bytes: &[u8]) -> Result<(), String> {
    update.install(bytes).map_err(|e| e.to_string())
}
