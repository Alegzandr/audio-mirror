//! What the panel shows, worked out from the devices, the settings and the
//! engine status alone: no window, no state of its own, so all of it can be
//! read and tested without a display.

use crate::audio::volume;
use crate::audio::{
    DeviceList, NodeState, OutputInfo, OutputStatus, SourceInfo, SourceKind, Status,
};
use crate::config::AppConfig;
use crate::i18n::Lang;

/// The engine names the desktop source "Default output (<device>)".
const DESKTOP_PREFIX: &str = "Default output";
/// Bottom of the meters, in dB.
const FLOOR_DB: f32 = -60.0;
/// How far a meter falls per frame, once the peak has passed.
const FALL: f32 = 0.02;
/// Bottom of the OBS fader range, `LOG_RANGE_DB` in `volume.rs`.
const LOG_RANGE_DB: f32 = 96.0;
const MINUS: char = '\u{2212}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    On,
    Muted,
    Error,
}

pub fn format_db(lang: Lang, db: f32) -> String {
    if !db.is_finite() || db <= -LOG_RANGE_DB {
        return format!("{MINUS}\u{221E} dB");
    }
    let abs = lang.number(db.abs());
    if db < -0.05 {
        format!("{MINUS}{abs} dB")
    } else {
        format!("{abs} dB")
    }
}

pub fn fader_db(lang: Lang, fader: f32) -> String {
    format_db(lang, volume::fader_to_db(fader))
}

pub fn peak_to_db(peak: f32) -> f32 {
    if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Where a meter should stand for that peak, 0 to 1 across the scale.
pub fn meter_target(peak: f32) -> f32 {
    let db = peak_to_db(peak);
    if !db.is_finite() {
        return 0.0;
    }
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// The next position of a meter: peaks are taken at once, the fall is
/// smoothed so a meter does not flicker between two frames.
pub fn meter_fall(level: f32, target: f32) -> f32 {
    if target > level {
        target
    } else {
        target.max(level - FALL)
    }
}

pub fn meter_db(level: f32) -> f32 {
    if level > 0.0 {
        level * -FLOOR_DB + FLOOR_DB
    } else {
        f32::NEG_INFINITY
    }
}

pub fn source_name(lang: Lang, source: &SourceInfo) -> String {
    let mut name = source.name.clone();
    if source.kind == SourceKind::Desktop {
        if let Some(rest) = name.strip_prefix(DESKTOP_PREFIX) {
            name = format!("{}{rest}", lang.t("source.desktop"));
        }
    }
    if source.is_default {
        lang.tf("device.default", &[("name", &name)])
    } else {
        name
    }
}

pub fn device_name(lang: Lang, dev: &OutputInfo) -> String {
    if dev.is_default {
        lang.tf("device.default", &[("name", &dev.name)])
    } else {
        dev.name.clone()
    }
}

/// Output the current source records: playing into it would feed back, so
/// the engine skips it and the panel leaves it out.
pub fn captured_output<'a>(devices: &'a DeviceList, config: &AppConfig) -> Option<&'a str> {
    devices
        .sources
        .iter()
        .find(|s| s.id == config.source)
        .and_then(|s| s.captures_output.as_deref())
}

/// Every row of the output list: the devices that are here, in the order
/// the system gave them, except the one the source records. An unplugged
/// output is not listed; its settings wait for it to come back.
pub fn output_rows(devices: &DeviceList, config: &AppConfig) -> Vec<OutputInfo> {
    let captured = captured_output(devices, config);
    devices
        .outputs
        .iter()
        .filter(|d| Some(d.id.as_str()) != captured)
        .cloned()
        .collect()
}

/// The rows split the way the panel shows them: the outputs that are on,
/// each with its controls, and the others, waiting to be added.
pub fn split_outputs(
    rows: &[OutputInfo],
    config: &AppConfig,
) -> (Vec<OutputInfo>, Vec<OutputInfo>) {
    let on = |id: &str| config.outputs.iter().any(|o| o.enabled && o.id == id);
    rows.iter().cloned().partition(|r| on(&r.id))
}

/// The line in the header: what the engine is doing right now, counting
/// only the outputs the list shows.
pub fn run_state(lang: Lang, status: Option<&Status>, rows: &[OutputInfo]) -> (String, Tone) {
    let off = || (lang.t("run.off").to_owned(), Tone::Muted);
    let Some(status) = status.filter(|s| s.running) else {
        return off();
    };
    if status.source.state == NodeState::Error {
        return (lang.t("run.sourceError").to_owned(), Tone::Error);
    }
    let shown: Vec<&OutputStatus> = status
        .outputs
        .iter()
        .filter(|o| rows.iter().any(|r| r.id == o.id))
        .collect();
    let total = shown.len();
    if total == 0 {
        return off();
    }
    let playing = shown
        .iter()
        .filter(|o| o.state == NodeState::Playing)
        .count();
    let text = if playing == total {
        lang.plural("run.mirroring", total, &[])
    } else {
        lang.tf(
            "run.partial",
            &[
                ("playing", &playing.to_string()),
                ("total", &total.to_string()),
            ],
        )
    };
    (text, Tone::On)
}

/// The state and the explanation of one output row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowState {
    pub label: String,
    pub tone: Tone,
    pub detail: String,
}

/// A muted output that is playing reads as muted: that is what the user
/// did, and it is what they are hearing. `status` is absent while the
/// engine opens the output.
pub fn output_state(
    lang: Lang,
    status: Option<&OutputStatus>,
    enabled: bool,
    muted: bool,
) -> RowState {
    let Some(status) = status.filter(|_| enabled) else {
        return RowState {
            label: String::new(),
            tone: Tone::Muted,
            detail: String::new(),
        };
    };
    let mut label = lang
        .t(match status.state {
            NodeState::Idle => "state.idle",
            NodeState::Starting => "state.starting",
            NodeState::Playing => "state.playing",
            NodeState::Error => "state.error",
            NodeState::Blocked => "state.blocked",
        })
        .to_owned();
    let mut tone = match status.state {
        NodeState::Playing => Tone::On,
        NodeState::Error => Tone::Error,
        _ => Tone::Muted,
    };
    if muted && status.state == NodeState::Playing {
        label = lang.t("output.muted").to_owned();
        tone = Tone::Muted;
    }
    let detail = match status.state {
        NodeState::Error => retrying(lang, status.message.as_ref()),
        NodeState::Blocked => lang.t("output.echo").to_owned(),
        _ => String::new(),
    };
    RowState {
        label,
        tone,
        detail,
    }
}

pub fn retrying(lang: Lang, message: Option<&crate::audio::message::Message>) -> String {
    let text = message.map(|m| lang.engine(m)).unwrap_or_default();
    lang.tf("error.retrying", &[("message", &text)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::message;
    use crate::audio::SourceStatus;
    use crate::config::OutputConfig;

    const EN: Lang = Lang::En;
    const FR: Lang = Lang::Fr;

    fn out(id: &str, state: NodeState) -> OutputStatus {
        OutputStatus {
            id: id.into(),
            state,
            message: None,
            format: None,
            peak: 0.0,
        }
    }

    fn status(source: NodeState, outputs: Vec<OutputStatus>) -> Status {
        Status {
            running: true,
            source: SourceStatus {
                state: source,
                message: None,
                format: None,
                peak: 0.0,
            },
            outputs,
            ..Default::default()
        }
    }

    fn info(id: &str) -> OutputInfo {
        OutputInfo {
            id: id.into(),
            name: id.to_uppercase(),
            is_default: false,
            description: None,
        }
    }

    fn source(id: &str, kind: SourceKind, captures: Option<&str>) -> SourceInfo {
        SourceInfo {
            id: id.into(),
            name: id.into(),
            kind,
            is_default: false,
            captures_output: captures.map(Into::into),
        }
    }

    fn devices() -> DeviceList {
        DeviceList {
            sources: vec![
                source("desktop", SourceKind::Desktop, Some("a")),
                source("output:b", SourceKind::Loopback, Some("b")),
            ],
            outputs: vec![info("a"), info("b")],
        }
    }

    fn config(source: &str, outputs: &[(&str, bool)]) -> AppConfig {
        AppConfig {
            source: source.into(),
            outputs: outputs
                .iter()
                .map(|(id, enabled)| OutputConfig {
                    id: (*id).into(),
                    name: String::new(),
                    enabled: *enabled,
                    fader: 1.0,
                    muted: false,
                })
                .collect(),
        }
    }

    fn ids(rows: &[OutputInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn decibels_are_written_for_the_reader() {
        assert_eq!(format_db(EN, 0.0), "0.0 dB");
        assert_eq!(format_db(EN, -18.74), "\u{2212}18.7 dB");
        assert_eq!(format_db(FR, -18.74), "\u{2212}18,7 dB");
        assert_eq!(format_db(EN, f32::NEG_INFINITY), "\u{2212}\u{221E} dB");
        assert_eq!(format_db(EN, -120.0), "\u{2212}\u{221E} dB");
    }

    #[test]
    fn meters_follow_peaks() {
        assert_eq!(meter_target(1.0), 1.0);
        assert_eq!(meter_target(0.0), 0.0);
        assert!((meter_target(0.5) - 0.9).abs() < 0.01);
        assert_eq!(meter_target(1e-9), 0.0);
        assert_eq!(meter_fall(0.2, 0.9), 0.9);
        assert!(meter_fall(0.9, 0.0) < 0.9 && meter_fall(0.9, 0.0) > 0.8);
        assert_eq!(meter_fall(0.005, 0.0), 0.0);
        assert_eq!(meter_db(0.0), f32::NEG_INFINITY);
        assert_eq!(meter_db(1.0), 0.0);
        assert!((meter_db(meter_target(0.5)) - peak_to_db(0.5)).abs() < 0.001);
    }

    #[test]
    fn sources_are_named_in_the_reader_language() {
        let mut desktop = source("desktop", SourceKind::Desktop, Some("out"));
        desktop.name = "Default output (Speakers)".into();
        assert_eq!(source_name(FR, &desktop), "Sortie par défaut (Speakers)");
        assert_eq!(source_name(EN, &desktop), "Default output (Speakers)");
        let mut mic = source("input:mic", SourceKind::Capture, None);
        mic.name = "Mic".into();
        mic.is_default = true;
        assert_eq!(source_name(EN, &mic), "Mic (default)");
        assert_eq!(source_name(FR, &mic), "Mic (par défaut)");
    }

    #[test]
    fn the_output_list() {
        let d = devices();
        assert_eq!(ids(&output_rows(&d, &config("input:mic", &[]))), ["a", "b"]);
        assert_eq!(ids(&output_rows(&d, &config("desktop", &[]))), ["b"]);
        assert_eq!(ids(&output_rows(&d, &config("output:b", &[]))), ["a"]);
        let gone = config("input:mic", &[("gone", true)]);
        assert_eq!(ids(&output_rows(&d, &gone)), ["a", "b"]);

        let cfg = config("input:mic", &[("b", true), ("a", false)]);
        let (active, others) = split_outputs(&output_rows(&d, &cfg), &cfg);
        assert_eq!(ids(&active), ["b"]);
        assert_eq!(ids(&others), ["a"]);

        assert_eq!(captured_output(&d, &config("desktop", &[])), Some("a"));
        assert_eq!(captured_output(&d, &config("input:mic", &[])), None);
    }

    #[test]
    fn the_header_says_what_the_engine_does() {
        let rows = [info("out"), info("b")];
        assert_eq!(run_state(EN, None, &rows), ("Off".into(), Tone::Muted));
        let failing = status(NodeState::Error, vec![]);
        assert_eq!(
            run_state(EN, Some(&failing), &rows),
            ("Source unavailable".into(), Tone::Error)
        );
        let two = status(
            NodeState::Playing,
            vec![out("out", NodeState::Playing), out("b", NodeState::Playing)],
        );
        assert_eq!(
            run_state(EN, Some(&two), &rows),
            ("Mirroring to 2".into(), Tone::On)
        );
        let half = status(
            NodeState::Playing,
            vec![out("out", NodeState::Playing), out("b", NodeState::Error)],
        );
        assert_eq!(
            run_state(EN, Some(&half), &rows),
            ("1 of 2 playing".into(), Tone::On)
        );
        let gone = status(
            NodeState::Playing,
            vec![
                out("out", NodeState::Playing),
                out("gone", NodeState::Error),
            ],
        );
        assert_eq!(
            run_state(EN, Some(&gone), &[info("out")]),
            ("Mirroring to 1".into(), Tone::On)
        );
        assert_eq!(run_state(EN, Some(&gone), &[]).0, "Off");
    }

    #[test]
    fn one_output_row() {
        let playing = out("out", NodeState::Playing);
        assert_eq!(output_state(EN, Some(&playing), false, false).label, "");
        assert_eq!(output_state(EN, None, true, false).label, "");
        assert_eq!(
            output_state(EN, Some(&playing), true, false).label,
            "Playing"
        );
        let muted = output_state(EN, Some(&playing), true, true);
        assert_eq!((muted.label.as_str(), muted.tone), ("Muted", Tone::Muted));
        let starting = out("out", NodeState::Starting);
        assert_eq!(
            output_state(EN, Some(&starting), true, true).label,
            "Starting"
        );

        let mut failed = out("out", NodeState::Error);
        failed.message = Some(message::DEVICE_DISCONNECTED.into());
        let state = output_state(FR, Some(&failed), true, false);
        assert_eq!(state.label, "Indisponible");
        assert_eq!(state.tone, Tone::Error);
        assert_eq!(state.detail, "Périphérique déconnecté. Nouvelle tentative.");

        let mut blocked = out("out", NodeState::Blocked);
        blocked.message = Some(message::CAPTURED_BY_SOURCE.into());
        let state = output_state(EN, Some(&blocked), true, false);
        assert_eq!(state.tone, Tone::Muted);
        assert_eq!(
            state.detail,
            "This is the source device. Playing into it would echo endlessly."
        );
    }
}
