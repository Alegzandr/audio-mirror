//! Settings persisted as JSON in the app config directory.
//!
//! Only what the user actually chooses is stored: the source and, per
//! output, whether it is on, its volume and its mute state. Everything else
//! is decided by the app.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::devices::DESKTOP;
use crate::dsp::fader_to_gain;
use crate::engine::{EngineConfig, OutputSpec};

/// Queue latency. Safe on every backend while staying well under lip-sync
/// tolerance for a live stream.
pub const LATENCY_MS: u32 = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutputConfig {
    pub id: String,
    /// Last known name, to show an unplugged device.
    pub name: String,
    pub enabled: bool,
    /// Slider position from 0 to 1 (OBS logarithmic curve).
    pub fader: f32,
    pub muted: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: false,
            fader: 1.0,
            muted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub source: String,
    pub outputs: Vec<OutputConfig>,
    /// Set once the first launch has enabled start at login.
    pub onboarded: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            source: DESKTOP.into(),
            outputs: Vec::new(),
            onboarded: false,
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Self {
        let mut cfg: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if cfg.source.is_empty() {
            cfg.source = DESKTOP.into();
        }
        cfg.outputs.retain(|o| !o.id.is_empty());
        for o in &mut cfg.outputs {
            o.fader = if o.fader.is_finite() {
                o.fader.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        cfg
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn output_mut(&mut self, id: &str, name: &str) -> &mut OutputConfig {
        let idx = match self.outputs.iter().position(|o| o.id == id) {
            Some(i) => i,
            None => {
                self.outputs.push(OutputConfig {
                    id: id.into(),
                    ..Default::default()
                });
                self.outputs.len() - 1
            }
        };
        let o = &mut self.outputs[idx];
        if !name.is_empty() {
            o.name = name.into();
        }
        o
    }

    pub fn enabled_count(&self) -> usize {
        self.outputs.iter().filter(|o| o.enabled).count()
    }

    pub fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            source: self.source.clone(),
            latency_ms: LATENCY_MS,
            outputs: self
                .outputs
                .iter()
                .filter(|o| o.enabled)
                .map(|o| OutputSpec {
                    id: o.id.clone(),
                    gain: fader_to_gain(o.fader),
                    muted: o.muted,
                })
                .collect(),
        }
    }
}

pub fn config_path(dir: PathBuf) -> PathBuf {
    dir.join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_sanitize() {
        let dir = std::env::temp_dir().join(format!("am-cfg-{}", std::process::id()));
        let path = config_path(dir.clone());

        let mut cfg = AppConfig::default();
        cfg.output_mut("wasapi:a", "Headphones").enabled = true;
        cfg.output_mut("wasapi:a", "").fader = 0.5;
        cfg.output_mut("wasapi:b", "Speakers");
        cfg.save(&path).unwrap();

        let loaded = AppConfig::load(&path);
        assert_eq!(loaded, cfg);
        assert_eq!(loaded.outputs[0].name, "Headphones");
        assert_eq!(loaded.enabled_count(), 1);

        let ec = loaded.engine_config();
        assert_eq!(ec.outputs.len(), 1);
        assert_eq!(ec.latency_ms, LATENCY_MS);
        assert!(ec.outputs[0].gain > 0.0 && ec.outputs[0].gain < 1.0);

        std::fs::write(
            &path,
            r#"{"source": "", "latency_ms": 7, "outputs":[{"id":"x","fader":4},{"id":""}]}"#,
        )
        .unwrap();
        let bad = AppConfig::load(&path);
        assert_eq!(bad.outputs.len(), 1);
        assert_eq!(bad.outputs[0].fader, 1.0);
        assert_eq!(bad.source, DESKTOP);

        let _ = std::fs::remove_dir_all(dir);
    }
}
