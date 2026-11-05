//! The app's state and every change the panel can make to it, free of any
//! UI toolkit: the shell (Tauri today) only forwards to these methods.

use std::path::PathBuf;

use parking_lot::Mutex;

use crate::audio::{self, Engine, Status};
use crate::config::AppConfig;
use crate::i18n::Lang;

pub struct AppState {
    config: Mutex<AppConfig>,
    /// Held while the settings are written, see [`AppState::change`].
    writing: Mutex<()>,
    path: PathBuf,
    engine: Engine,
    pub lang: Lang,
    /// Version already installed and waiting for a restart.
    pub update_ready: Mutex<Option<String>>,
}

impl AppState {
    /// Loads the settings at `path` and creates a stopped engine.
    pub fn new(path: PathBuf, lang: Lang) -> Self {
        AppState {
            config: Mutex::new(AppConfig::load(&path)),
            writing: Mutex::new(()),
            path,
            engine: Engine::new(),
            lang,
            update_ready: Mutex::new(None),
        }
    }

    /// Changes the settings, writes them, and hands `apply` the result.
    ///
    /// The file is written with the settings unlocked, so the panel reading
    /// them, a volume going to the audio thread, and the next command do not
    /// queue behind a disk that answers slowly, which a roaming or networked
    /// profile directory does. `writing` takes the place of that lock for
    /// the writers alone: it is taken first and held across the write, so
    /// two changes reach the file in the order they were made.
    fn change<T>(&self, f: impl FnOnce(&mut AppConfig) -> T, apply: impl FnOnce(&AppConfig, T)) {
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
    }

    /// Applies a change, saves it and pushes it to the engine.
    fn update(&self, f: impl FnOnce(&mut AppConfig)) {
        self.change(f, |cfg, ()| self.engine.apply(cfg.engine_config()))
    }

    pub fn config(&self) -> AppConfig {
        self.config.lock().clone()
    }

    pub fn status(&self) -> Status {
        self.engine.status()
    }

    /// Hands the saved settings to the engine, which starts the audio.
    pub fn start_engine(&self) {
        self.engine.apply(self.config.lock().engine_config());
    }

    /// Reopens the capture and every output.
    pub fn restart_engine(&self) {
        self.engine.restart();
    }

    pub fn set_source(&self, id: String) {
        self.update(|c| c.source = id)
    }

    pub fn set_output_enabled(&self, id: &str, name: &str, enabled: bool) {
        self.update(|c| c.output_mut(id, name).enabled = enabled)
    }

    /// Moves an output's fader (0..1). Volume goes straight to the audio
    /// thread, the stream is not rebuilt; while the slider moves (`persist`
    /// false) the position is kept in memory only, and the final one is
    /// written to disk.
    pub fn set_output_volume(&self, id: &str, name: &str, fader: f32, persist: bool) {
        let fader = if fader.is_finite() {
            fader.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let gain = audio::volume::fader_to_gain(fader);
        if !persist {
            self.config.lock().output_mut(id, name).fader = fader;
            self.engine.set_gain(id, gain);
            return;
        }
        self.change(
            |c| c.output_mut(id, name).fader = fader,
            |_, ()| self.engine.set_gain(id, gain),
        )
    }

    pub fn set_output_muted(&self, id: &str, name: &str, muted: bool) {
        self.change(
            |c| c.output_mut(id, name).muted = muted,
            |_, ()| self.engine.set_muted(id, muted),
        )
    }
}
