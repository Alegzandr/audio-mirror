pub mod audio;
pub mod config;

mod autostart;
mod i18n;
#[cfg(windows)]
mod install;
mod logging;
mod paths;
mod single_instance;
mod state;
mod ui;
mod updater;

use std::sync::Arc;

use i18n::Lang;
use state::AppState;

/// Starts Audio Mirror and returns when the user quits it.
pub fn run() {
    #[cfg(windows)]
    if !install::prepare() {
        return;
    }
    logging::init();

    let (events, incoming) = futures::channel::mpsc::unbounded();
    let restarted = std::env::args().any(|a| a == updater::RESTARTED_ARG);
    let show = events.clone();
    let _instance = match single_instance::acquire(restarted, move || {
        let _ = show.unbounded_send(ui::AppEvent::Show);
    }) {
        single_instance::Outcome::Primary(guard) => guard,
        single_instance::Outcome::Secondary => return,
    };

    let Some(dir) = paths::config_dir() else {
        log::error!("no settings folder");
        return;
    };
    let path = config::config_path(dir);
    // Built once the UI has set this thread up: on Windows the engine
    // registers for device changes on it, which must find COM already set
    // up the way the windows need it (OLE, single-threaded).
    let state = move || Arc::new(AppState::new(path, Lang::detect()));
    ui::run(state, events, incoming);
}
