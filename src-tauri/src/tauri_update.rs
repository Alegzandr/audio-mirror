//! Shows the update steps of `updater.rs` in the Tauri splash window and
//! tells the panel when a version waits for a restart.

use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::updater::{self, Stage, Updater};
use crate::AppState;

pub const SPLASH: &str = "splash";

/// What `ui/splash.js` polls.
#[derive(Serialize)]
pub struct Progress {
    stage: &'static str,
    percent: Option<u8>,
}

#[tauri::command]
pub fn update_progress(state: State<'_, Updater>) -> Progress {
    let p = state.progress();
    Progress {
        stage: match p.stage {
            Stage::Checking => "checking",
            Stage::Downloading => "downloading",
            Stage::Installing => "installing",
            Stage::Restarting => "restarting",
        },
        percent: p.percent,
    }
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
    std::thread::spawn(move || {
        let shown = Instant::now();
        let updater = app.state::<Updater>();
        let wait = match updater.check_and_install(true) {
            Ok(Some(_)) => {
                updater.set_stage(Stage::Restarting);
                updater::relaunch();
                app.exit(0);
                return;
            }
            Ok(None) => updater::CHECK_EVERY,
            Err(e) => e.wait(),
        };
        if let Some(rest) = updater::SPLASH_MIN.checked_sub(shown.elapsed()) {
            std::thread::sleep(rest);
        }
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(splash) = handle.get_webview_window(SPLASH) {
                let _ = splash.destroy();
            }
            crate::start(&handle);
        });
        watch(&app, wait);
    });
    Ok(())
}

/// Checks again while the app runs; a found version waits for a restart.
fn watch(app: &AppHandle, mut wait: std::time::Duration) {
    loop {
        std::thread::sleep(wait);
        wait = match app.state::<Updater>().check_and_install(false) {
            Ok(Some(version)) => {
                *app.state::<AppState>().update_ready.lock() = Some(version.clone());
                let _ = app.emit("update-ready", version);
                return;
            }
            Ok(None) => updater::CHECK_EVERY,
            Err(e) => e.wait(),
        };
    }
}
