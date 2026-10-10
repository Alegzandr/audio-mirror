pub mod audio;
pub mod config;
mod glass;
mod i18n;
#[cfg(windows)]
mod install;
mod tray;
mod updater;

use std::path::PathBuf;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use audio::{DeviceList, Engine, Status};
use config::AppConfig;
use i18n::Lang;

/// Argument passed by the login item: start in the tray, panel hidden.
const AUTOSTART_ARG: &str = "--autostart";

pub(crate) struct AppState {
    config: Mutex<AppConfig>,
    /// Held while the settings are written, see [`AppState::change`].
    writing: Mutex<()>,
    path: PathBuf,
    engine: Engine,
    pub(crate) lang: Lang,
    /// Version already installed and waiting for a restart.
    pub(crate) update_ready: Mutex<Option<String>>,
}

impl AppState {
    /// Changes the settings, writes them, and hands `apply` the result.
    ///
    /// The file is written with the settings unlocked, so the panel reading
    /// them, a volume going to the audio thread, and the next command do not
    /// queue behind a disk that answers slowly, which a roaming or networked
    /// profile directory does. `writing` takes the place of that lock for
    /// the writers alone: it is taken first and held across the write, so
    /// two changes reach the file in the order they were made.
    fn change<T>(
        &self,
        f: impl FnOnce(&mut AppConfig) -> T,
        apply: impl FnOnce(&AppConfig, T),
    ) -> Result<(), String> {
        let _writing = self.writing.lock();
        let (cfg, value) = {
            let mut cfg = self.config.lock();
            let value = f(&mut cfg);
            (cfg.clone(), value)
        };
        // The change is already in memory, so it reaches the engine even when
        // the file cannot be written: otherwise the panel would show a mute
        // or an output the audio never got. The next change saves it again.
        let saved = cfg.save(&self.path);
        apply(&cfg, value);
        if let Err(e) = saved {
            log::error!("settings not saved: {e}");
        }
        Ok(())
    }

    /// Applies a change, saves it and pushes it to the engine.
    fn update(&self, f: impl FnOnce(&mut AppConfig)) -> Result<(), String> {
        self.change(f, |cfg, ()| self.engine.apply(cfg.engine_config()))
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
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
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

/// Longest id or name a command accepts. Device ids are far shorter; the
/// bound keeps a misbehaving page from growing the settings file without end.
const MAX_ARG_LEN: usize = 1024;

fn check_len(args: &[&str]) -> Result<(), String> {
    if args.iter().any(|a| a.len() > MAX_ARG_LEN) {
        return Err("argument too long".into());
    }
    Ok(())
}

#[tauri::command]
fn set_source(state: State<'_, AppState>, id: String) -> Result<(), String> {
    check_len(&[&id])?;
    state.update(|c| c.source = id)
}

#[tauri::command]
fn set_output_enabled(
    state: State<'_, AppState>,
    id: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    check_len(&[&id, &name])?;
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
    check_len(&[&id, &name])?;
    let fader = if fader.is_finite() {
        fader.clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Volume goes straight to the audio thread, the stream is not rebuilt.
    // While the slider moves, only the final position is written to disk.
    let gain = audio::volume::fader_to_gain(fader);
    if !persist {
        state.config.lock().output_mut(&id, &name).fader = fader;
        state.engine.set_gain(&id, gain);
        return Ok(());
    }
    state.change(
        |c| c.output_mut(&id, &name).fader = fader,
        |_, ()| state.engine.set_gain(&id, gain),
    )
}

#[tauri::command]
fn set_output_muted(
    state: State<'_, AppState>,
    id: String,
    name: String,
    muted: bool,
) -> Result<(), String> {
    check_len(&[&id, &name])?;
    state.change(
        |c| c.output_mut(&id, &name).muted = muted,
        |_, ()| state.engine.set_muted(&id, muted),
    )
}

/// One line of the source menu: a source, or a group header when `id` is None.
#[derive(serde::Deserialize)]
struct MenuEntry {
    id: Option<String>,
    label: String,
    checked: bool,
}

/// Menu ids of the source menu carry this prefix before the source id.
const SOURCE_MENU_PREFIX: &str = "source:";

/// Shows the source list as a native menu over the pop-up button, at `x`,
/// `y` in the panel (logical pixels). The panel only asks for it on macOS,
/// where the native menu is the one the look is drawn after; the choice
/// comes back as a `source-picked` event, handled like the page's own menu.
#[tauri::command]
fn source_menu(
    window: tauri::WebviewWindow,
    entries: Vec<MenuEntry>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    use tauri::menu::{CheckMenuItem, Menu, MenuItem};
    let app = window.app_handle();
    let menu = Menu::new(app).map_err(|e| e.to_string())?;
    for (i, entry) in entries.iter().enumerate() {
        let added = match &entry.id {
            Some(id) => {
                let item = CheckMenuItem::with_id(
                    app,
                    format!("{SOURCE_MENU_PREFIX}{id}"),
                    &entry.label,
                    true,
                    entry.checked,
                    None::<&str>,
                )
                .map_err(|e| e.to_string())?;
                menu.append(&item)
            }
            None => {
                let item = MenuItem::with_id(
                    app,
                    format!("header:{i}"),
                    &entry.label,
                    false,
                    None::<&str>,
                )
                .map_err(|e| e.to_string())?;
                menu.append(&item)
            }
        };
        added.map_err(|e| e.to_string())?;
    }
    window
        .popup_menu_at(&menu, tauri::LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// Renames the output in the system; the panel re-reads the devices after.
#[tauri::command]
async fn rename_output(id: String, description: String) -> Result<(), String> {
    check_len(&[&id, &description])?;
    tauri::async_runtime::spawn_blocking(move || audio::rename_output(&id, &description))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
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

    if let Some(panel) = app.get_webview_window(tray::PANEL) {
        glass::apply(&panel);
        // A source picked in the native menu goes back to the page, which
        // applies it the way its own menu does.
        let target = panel.clone();
        panel.on_menu_event(move |_, event| {
            if let Some(id) = event.id().as_ref().strip_prefix(SOURCE_MENU_PREFIX) {
                let _ = target.emit("source-picked", id);
            }
        });
    }
    if let Err(e) = tray::setup(app) {
        log::error!("tray: {e}");
    }
    if !std::env::args().any(|a| a == AUTOSTART_ARG) {
        tray::show_panel(app);
    }
}

/// Writes the engine's messages to a rotating file in the app log folder, so
/// a device that refuses to open leaves a trace to report. Without this the
/// `log` calls spread over the audio backends go nowhere.
fn logging<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let mut builder = tauri_plugin_log::Builder::new()
        .level(log::LevelFilter::Info)
        .max_file_size(512 * 1024)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
        .target(tauri_plugin_log::Target::new(
            tauri_plugin_log::TargetKind::LogDir {
                file_name: Some("audio-mirror".into()),
            },
        ));
    if cfg!(debug_assertions) {
        builder = builder.target(tauri_plugin_log::Target::new(
            tauri_plugin_log::TargetKind::Stdout,
        ));
    }
    builder.build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    if !install::prepare() {
        return;
    }

    let lang = Lang::detect();

    tauri::Builder::default()
        .plugin(logging())
        .plugin(i18n::plugin(lang))
        .plugin(glass::plugin())
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
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Start at login is opt-in: the user turns it on in the panel.
            let path = config::config_path(app.path().app_config_dir()?);
            let config = AppConfig::load(&path);

            app.manage(AppState {
                config: Mutex::new(config),
                writing: Mutex::new(()),
                path,
                engine: Engine::new(),
                lang,
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
            rename_output,
            source_menu,
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
