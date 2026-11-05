pub mod audio;
mod autostart;
pub mod config;
mod glass;
mod i18n;
#[cfg(windows)]
mod install;
mod logging;
mod paths;
mod single_instance;
mod state;
mod tauri_update;
mod tray;
mod updater;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use audio::{DeviceList, Status};
use config::AppConfig;
use i18n::Lang;
pub(crate) use state::AppState;

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
        config: state.config(),
        devices,
        autostart: autostart::is_enabled(),
        update_ready: state.update_ready.lock().clone(),
    })
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Status {
    state.status()
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
    state.set_source(id);
    Ok(())
}

#[tauri::command]
fn set_output_enabled(
    state: State<'_, AppState>,
    id: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    check_len(&[&id, &name])?;
    state.set_output_enabled(&id, &name, enabled);
    Ok(())
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
    state.set_output_volume(&id, &name, fader, persist);
    Ok(())
}

#[tauri::command]
fn set_output_muted(
    state: State<'_, AppState>,
    id: String,
    name: String,
    muted: bool,
) -> Result<(), String> {
    check_len(&[&id, &name])?;
    state.set_output_muted(&id, &name, muted);
    Ok(())
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
fn set_autostart(enabled: bool) -> Result<bool, String> {
    autostart::set(enabled)
}

#[tauri::command]
fn hide_panel(app: AppHandle) {
    tray::hide_panel(&app);
}

#[tauri::command]
fn restart(app: AppHandle) {
    // The new copy waits for this one to release the single-instance lock.
    updater::relaunch();
    app.exit(0);
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Starts the tray, the audio and the panel, once the startup update check
/// is done.
pub(crate) fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.start_engine();

    // An install moved the executable: point the login item at the new path.
    autostart::refresh();

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
    if !std::env::args().any(|a| a == autostart::ARG) {
        tray::show_panel(app);
    }
}

/// The app, once Tauri has built it, for the single-instance listener that
/// starts before it.
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    if !install::prepare() {
        return;
    }

    logging::init();
    let restarted = std::env::args().any(|a| a == updater::RESTARTED_ARG);
    let _instance = match single_instance::acquire(restarted, || {
        if let Some(app) = APP.get() {
            // During the startup update check, the splash is already showing.
            if app.get_webview_window(tauri_update::SPLASH).is_none() {
                tray::show_panel(app);
            }
        }
    }) {
        single_instance::Outcome::Primary(guard) => guard,
        single_instance::Outcome::Secondary => return,
    };

    let lang = Lang::detect();

    tauri::Builder::default()
        .plugin(i18n::plugin(lang))
        .plugin(glass::plugin())
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Start at login is opt-in: the user turns it on in the panel.
            let dir = paths::config_dir().ok_or("no settings folder")?;
            let path = config::config_path(dir);
            app.manage(AppState::new(path, lang));
            app.manage(updater::Updater::default());
            let _ = APP.set(app.handle().clone());

            if updater::enabled() {
                tauri_update::launch(app.handle())?;
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
            tauri_update::update_progress,
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
