pub mod audio;
pub mod config;
#[cfg(windows)]
mod install;
mod tray;
mod updater;

use std::path::PathBuf;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use audio::{DeviceList, Engine, Status};
use config::AppConfig;

/// Argument passed by the login item: start in the tray, panel hidden.
const AUTOSTART_ARG: &str = "--autostart";

pub(crate) struct AppState {
    config: Mutex<AppConfig>,
    path: PathBuf,
    engine: Engine,
    /// Version already installed and waiting for a restart.
    pub(crate) update_ready: Mutex<Option<String>>,
}

impl AppState {
    fn save(&self, cfg: &AppConfig) -> Result<(), String> {
        cfg.save(&self.path).map_err(|e| e.to_string())
    }

    /// Applies a change, saves it and pushes it to the engine.
    fn update(&self, f: impl FnOnce(&mut AppConfig)) -> Result<(), String> {
        let mut cfg = self.config.lock();
        f(&mut cfg);
        self.save(&cfg)?;
        self.engine.apply(cfg.engine_config());
        Ok(())
    }
}

#[derive(Serialize)]
struct Snapshot {
    version: String,
    config: AppConfig,
    devices: DeviceList,
    autostart: bool,
    update_ready: Option<String>,
}

#[tauri::command]
async fn snapshot(app: AppHandle, state: State<'_, AppState>) -> Result<Snapshot, String> {
    let devices = tauri::async_runtime::spawn_blocking(audio::enumerate)
        .await
        .map_err(|e| e.to_string())??;
    Ok(Snapshot {
        version: app.package_info().version.to_string(),
        config: state.config.lock().clone(),
        devices,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        update_ready: state.update_ready.lock().clone(),
    })
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Status {
    state.engine.status()
}

#[tauri::command]
fn set_source(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.update(|c| c.source = id)
}

#[tauri::command]
fn set_output_enabled(
    state: State<'_, AppState>,
    id: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    state.update(|c| c.output_mut(&id, &name).enabled = enabled)
}

#[tauri::command]
fn set_output_volume(
    state: State<'_, AppState>,
    id: String,
    name: String,
    fader: f32,
    persist: bool,
) -> Result<(), String> {
    let fader = if fader.is_finite() {
        fader.clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Volume goes straight to the audio thread, the stream is not rebuilt.
    // While the slider moves, only the final position is written to disk.
    let mut cfg = state.config.lock();
    cfg.output_mut(&id, &name).fader = fader;
    if persist {
        state.save(&cfg)?;
    }
    state
        .engine
        .set_gain(&id, audio::volume::fader_to_gain(fader));
    Ok(())
}

#[tauri::command]
fn set_output_muted(
    state: State<'_, AppState>,
    id: String,
    name: String,
    muted: bool,
) -> Result<(), String> {
    let mut cfg = state.config.lock();
    cfg.output_mut(&id, &name).muted = muted;
    state.save(&cfg)?;
    state.engine.set_muted(&id, muted);
    Ok(())
}

#[tauri::command]
fn forget_output(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.update(|c| c.outputs.retain(|o| o.id != id))
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let launcher = app.autolaunch();
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|e| e.to_string())?;
    launcher.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn hide_panel(app: AppHandle) {
    tray::hide_panel(&app);
}

#[tauri::command]
fn restart(app: AppHandle) {
    // Goes through RunEvent::Exit so the single-instance lock is released
    // before the new process starts; `restart()` would skip it.
    app.request_restart();
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Starts the tray, the audio and the panel, once the startup update check
/// is done.
pub(crate) fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.engine.apply(state.config.lock().engine_config());

    // An install moved the executable: point the login item at the new path.
    let launcher = app.autolaunch();
    if launcher.is_enabled().unwrap_or(false) {
        let _ = launcher.enable();
    }

    if let Err(e) = tray::setup(app) {
        log::error!("tray: {e}");
    }
    if !std::env::args().any(|a| a == AUTOSTART_ARG) {
        tray::show_panel(app);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    if !install::prepare() {
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // During the startup update check, the splash is already showing.
            if app.get_webview_window(updater::SPLASH).is_none() {
                tray::show_panel(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Start at login is opt-in: the user turns it on in the panel.
            let path = config::config_path(app.path().app_config_dir()?);
            let config = AppConfig::load(&path);

            app.manage(AppState {
                config: Mutex::new(config),
                path,
                engine: Engine::new(),
                update_ready: Mutex::new(None),
            });
            app.manage(updater::SplashState::default());

            if updater::enabled() {
                updater::launch(app.handle())?;
            } else {
                start(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            status,
            set_source,
            set_output_enabled,
            set_output_volume,
            set_output_muted,
            forget_output,
            set_autostart,
            hide_panel,
            restart,
            quit,
            updater::update_progress,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start Audio Mirror")
        .run(|_, event| {
            // The panel hides instead of closing; only Quit ends the app.
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
