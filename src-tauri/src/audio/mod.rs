//! Audio engine: OBS Studio's desktop audio capture and audio monitoring,
//! with one source feeding N monitors instead of one.
//!
//! Each platform module ports the matching OBS code:
//! - Windows: `plugins/win-wasapi` and `libobs/audio-monitoring/win32`
//! - macOS: `plugins/mac-capture` and `libobs/audio-monitoring/osx`
//! - Linux: `plugins/linux-pulseaudio` and `libobs/audio-monitoring/pulse`
//!
//! [`hub`] ports the source side of libobs that sits between the two, and
//! [`swr`] the FFmpeg resampler OBS uses.

pub mod format;
pub mod hub;
pub mod swr;
pub mod volume;

#[cfg(target_os = "macos")]
mod coreaudio;
#[cfg(target_os = "linux")]
mod pulse;
#[cfg(windows)]
mod wasapi;

#[cfg(target_os = "macos")]
use coreaudio as platform;
#[cfg(target_os = "linux")]
use pulse as platform;
#[cfg(windows)]
use wasapi as platform;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use hub::{AudioCallback, CallbackId, SourceHub};
use volume::OutputShared;

/// Id of the system default output, captured like OBS's "Desktop Audio".
pub const DESKTOP: &str = "desktop";

/// Retry delay for a monitor whose device could not be opened. OBS only
/// retries on a settings change; a background service retries on its own,
/// at the same pace as `win-wasapi` reconnects.
const MONITOR_RETRY: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Desktop,
    Loopback,
    Capture,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub is_default: bool,
    /// Output this source records, which therefore cannot be a destination.
    pub captures_output: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeviceList {
    pub sources: Vec<SourceInfo>,
    pub outputs: Vec<OutputInfo>,
}

pub fn enumerate() -> Result<DeviceList, String> {
    platform::enumerate()
}

/// State of the running capture, reported by the platform.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureState {
    Starting,
    Active {
        format: String,
    },
    /// The capture retries on its own (WASAPI).
    Retrying(String),
    /// The capture stopped for good; the session restarts it.
    Failed(String),
}

/// A running source capture. Dropping it stops the capture.
pub trait Capture: Send {
    fn state(&self) -> CaptureState;
    /// The system default output changed.
    fn default_output_changed(&self) {}
}

#[derive(Debug, Clone, PartialEq)]
pub enum MonitorState {
    Playing {
        format: String,
    },
    /// The device is being reopened after a failure.
    Reconnecting(String),
}

/// A monitor is a capture callback of the source, like in libobs.
pub trait Monitor: AudioCallback {
    fn state(&self) -> MonitorState;
}

pub enum MonitorInit {
    Active(Arc<dyn Monitor>),
    /// `OBS_SOURCE_DO_NOT_SELF_MONITOR`: the output is the captured device.
    Ignored,
}

/// Notifications from the operating system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SystemEvent {
    DefaultOutputChanged,
    DevicesChanged,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputSpec {
    pub id: String,
    pub gain: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub source: String,
    pub outputs: Vec<OutputSpec>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    Idle,
    Starting,
    Playing,
    Error,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceStatus {
    pub state: NodeState,
    pub message: Option<String>,
    pub format: Option<String>,
    pub peak: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputStatus {
    pub id: String,
    pub state: NodeState,
    pub message: Option<String>,
    pub format: Option<String>,
    pub peak: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub source: SourceStatus,
    pub outputs: Vec<OutputStatus>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            running: false,
            source: SourceStatus {
                state: NodeState::Idle,
                message: None,
                format: None,
                peak: 0.0,
            },
            outputs: Vec::new(),
        }
    }
}

enum Cmd {
    Apply(EngineConfig),
    Restart,
    System(SystemEvent),
    Shutdown,
}

type SharedMap = Arc<Mutex<HashMap<String, Arc<OutputShared>>>>;

pub struct Engine {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
    shared: SharedMap,
    _watcher: Option<Box<dyn std::any::Any + Send + Sync>>,
}

impl Engine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = SharedMap::default();

        let sup = Supervisor {
            rx,
            status: status.clone(),
            shared: shared.clone(),
            cfg: None,
            session: None,
        };
        let thread = std::thread::Builder::new()
            .name("audio-supervisor".into())
            .spawn(move || sup.run())
            .expect("failed to start the audio supervisor");

        let events = tx.clone();
        let watcher = platform::watch_system(Box::new(move |event| {
            let _ = events.send(Cmd::System(event));
        }));

        Self {
            tx,
            thread: Some(thread),
            status,
            shared,
            _watcher: watcher,
        }
    }

    /// Starts, updates or stops mirroring. No enabled output means stopped.
    pub fn apply(&self, cfg: EngineConfig) {
        {
            let mut shared = self.shared.lock();
            for o in &cfg.outputs {
                let s = shared
                    .entry(o.id.clone())
                    .or_insert_with(|| Arc::new(OutputShared::new(o.gain, o.muted)));
                s.gain.store(o.gain);
                s.muted.store(o.muted, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let _ = self.tx.send(Cmd::Apply(cfg));
    }

    /// Closes the capture and every output, then reopens them with the
    /// current settings, like `obs_reset_audio_monitoring` plus a source
    /// restart.
    pub fn restart(&self) {
        let _ = self.tx.send(Cmd::Restart);
    }

    /// Volume changes apply on the next packet, like `user_volume` in OBS.
    pub fn set_gain(&self, id: &str, gain: f32) {
        if let Some(s) = self.shared.lock().get(id) {
            s.gain.store(gain);
        }
    }

    pub fn set_muted(&self, id: &str, muted: bool) {
        if let Some(s) = self.shared.lock().get(id) {
            s.muted.store(muted, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Status snapshot, with the peaks accumulated since the previous call.
    pub fn status(&self) -> Status {
        let mut st = self.status.lock().clone();
        let shared = self.shared.lock();
        for o in &mut st.outputs {
            if let Some(s) = shared.get(&o.id) {
                o.peak = s.peak.take();
            }
        }
        st
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

enum Slot {
    Active {
        monitor: Arc<dyn Monitor>,
        callback: CallbackId,
    },
    Ignored,
    Failed {
        error: String,
        retry_at: Instant,
    },
}

/// One source and its monitors, like an `obs_source_t` with monitoring on.
struct Session {
    source: String,
    hub: Arc<SourceHub>,
    capture: Result<Box<dyn Capture>, String>,
    capture_retry_at: Instant,
    slots: HashMap<String, Slot>,
}

impl Session {
    fn start(source: &str) -> Session {
        let hub = Arc::new(SourceHub::new());
        let capture = platform::start_capture(source, hub.clone());
        if let Err(e) = &capture {
            log::warn!("capture: {e}");
        }
        Session {
            source: source.to_string(),
            hub,
            capture,
            capture_retry_at: Instant::now() + MONITOR_RETRY,
            slots: HashMap::new(),
        }
    }

    /// `audio_monitor_create`.
    fn create_monitor(&mut self, id: &str, shared: Arc<OutputShared>) {
        let slot = match platform::create_monitor(&self.source, id, shared) {
            Ok(MonitorInit::Active(monitor)) => {
                let callback = self.hub.add_callback(monitor.clone());
                Slot::Active { monitor, callback }
            }
            Ok(MonitorInit::Ignored) => {
                log::info!("prevented feedback loop on {id}");
                Slot::Ignored
            }
            Err(error) => {
                log::warn!("monitor {id}: {error}");
                Slot::Failed {
                    error,
                    retry_at: Instant::now() + MONITOR_RETRY,
                }
            }
        };
        self.remove_monitor(id);
        self.slots.insert(id.to_string(), slot);
    }

    /// `audio_monitor_destroy`.
    fn remove_monitor(&mut self, id: &str) {
        if let Some(Slot::Active { callback, .. }) = self.slots.remove(id) {
            self.hub.remove_callback(callback);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let ids: Vec<String> = self.slots.keys().cloned().collect();
        for id in ids {
            self.remove_monitor(&id);
        }
    }
}

struct Supervisor {
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    shared: SharedMap,
    cfg: Option<EngineConfig>,
    session: Option<Session>,
}

impl Supervisor {
    fn run(mut self) {
        platform::init_thread();
        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(Cmd::Apply(cfg)) => self.apply(cfg),
                Ok(Cmd::Restart) => {
                    if let Some(cfg) = self.cfg.take() {
                        log::info!("restarting audio");
                        self.session = None;
                        self.apply(cfg);
                    }
                }
                Ok(Cmd::System(event)) => self.system(event),
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    self.session = None;
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
            self.publish();
        }
    }

    fn shared(&self, id: &str) -> Option<Arc<OutputShared>> {
        self.shared.lock().get(id).cloned()
    }

    fn apply(&mut self, cfg: EngineConfig) {
        if cfg.outputs.is_empty() {
            self.session = None;
            self.cfg = None;
            return;
        }

        let restart = self.session.as_ref().is_none_or(|s| s.source != cfg.source);
        if restart {
            self.session = None;
            self.session = Some(Session::start(&cfg.source));
        }

        let session = self.session.as_mut().expect("session");
        let stale: Vec<String> = session
            .slots
            .keys()
            .filter(|id| !cfg.outputs.iter().any(|o| &o.id == *id))
            .cloned()
            .collect();
        for id in stale {
            session.remove_monitor(&id);
        }
        for o in &cfg.outputs {
            if session.slots.contains_key(&o.id) {
                continue;
            }
            if let Some(shared) = self.shared.lock().get(&o.id).cloned() {
                session.create_monitor(&o.id, shared);
            }
        }
        self.cfg = Some(cfg);
    }

    fn system(&mut self, event: SystemEvent) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match event {
            SystemEvent::DefaultOutputChanged => {
                log::info!("default output changed");
                if let Ok(capture) = &session.capture {
                    capture.default_output_changed();
                }
                // `obs_reset_audio_monitoring`: rebuild every monitor, which
                // also re-evaluates the feedback-loop rule.
                let ids: Vec<String> = session.slots.keys().cloned().collect();
                for id in ids {
                    if let Some(shared) = self.shared.lock().get(&id).cloned() {
                        session.create_monitor(&id, shared);
                    }
                }
            }
            SystemEvent::DevicesChanged => {
                for slot in session.slots.values_mut() {
                    if let Slot::Failed { retry_at, .. } = slot {
                        *retry_at = Instant::now();
                    }
                }
                session.capture_retry_at = Instant::now();
            }
        }
    }

    fn tick(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let now = Instant::now();

        let failed = match &session.capture {
            Ok(capture) => matches!(capture.state(), CaptureState::Failed(_)),
            Err(_) => true,
        };
        if !failed {
            session.capture_retry_at = now + MONITOR_RETRY;
        } else if now >= session.capture_retry_at {
            // Drop the old capture before opening the device again.
            session.capture = Err(String::new());
            session.capture = platform::start_capture(&session.source, session.hub.clone());
            if let Err(e) = &session.capture {
                log::warn!("capture: {e}");
            }
            session.capture_retry_at = now + MONITOR_RETRY;
        }

        let due: Vec<String> = session
            .slots
            .iter()
            .filter(|(_, s)| matches!(s, Slot::Failed { retry_at, .. } if now >= *retry_at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(shared) = self.shared(&id) {
                if let Some(session) = self.session.as_mut() {
                    session.create_monitor(&id, shared);
                }
            }
        }
    }

    fn publish(&self) {
        let mut st = self.status.lock();
        let (Some(cfg), Some(session)) = (&self.cfg, &self.session) else {
            *st = Status::default();
            return;
        };
        st.running = true;

        let (state, message, format) = match &session.capture {
            Ok(capture) => match capture.state() {
                CaptureState::Starting => (NodeState::Starting, None, None),
                CaptureState::Active { format } => (NodeState::Playing, None, Some(format)),
                CaptureState::Retrying(msg) | CaptureState::Failed(msg) => {
                    (NodeState::Error, Some(msg), None)
                }
            },
            Err(e) => (NodeState::Error, Some(e.clone()), None),
        };
        st.source = SourceStatus {
            state,
            message,
            format,
            peak: session.hub.take_peak(),
        };

        st.outputs = cfg
            .outputs
            .iter()
            .map(|o| {
                let (state, message, format) = match session.slots.get(&o.id) {
                    Some(Slot::Active { monitor, .. }) => match monitor.state() {
                        MonitorState::Playing { format } => {
                            (NodeState::Playing, None, Some(format))
                        }
                        MonitorState::Reconnecting(msg) => (NodeState::Error, Some(msg), None),
                    },
                    Some(Slot::Ignored) => (
                        NodeState::Blocked,
                        Some("Captured by the source".to_string()),
                        None,
                    ),
                    Some(Slot::Failed { error, .. }) => {
                        (NodeState::Error, Some(error.clone()), None)
                    }
                    None => (NodeState::Starting, None, None),
                };
                OutputStatus {
                    id: o.id.clone(),
                    state,
                    message,
                    format,
                    peak: 0.0,
                }
            })
            .collect();
    }
}

/// Human readable format, shared by the platform modules.
pub fn describe(rate: u32, channels: usize) -> String {
    let layout = match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        3 => "2.1".to_string(),
        4 => "4.0".to_string(),
        5 => "4.1".to_string(),
        6 => "5.1".to_string(),
        8 => "7.1".to_string(),
        n => format!("{n} channels"),
    };
    format!("{} kHz, {layout}", rate as f32 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_common_layouts() {
        assert_eq!(describe(48_000, 2), "48 kHz, stereo");
        assert_eq!(describe(44_100, 6), "44.1 kHz, 5.1");
    }

    #[test]
    fn engine_starts_and_stops_without_devices() {
        let engine = Engine::new();
        assert!(!engine.status().running);
        engine.apply(EngineConfig {
            source: "input:missing".into(),
            outputs: vec![OutputSpec {
                id: "missing-output".into(),
                gain: 0.5,
                muted: false,
            }],
        });
        std::thread::sleep(Duration::from_millis(600));
        let st = engine.status();
        assert!(st.running);
        assert_eq!(st.outputs.len(), 1);
        assert_eq!(st.outputs[0].state, NodeState::Error);

        engine.restart();
        std::thread::sleep(Duration::from_millis(600));
        let st = engine.status();
        assert!(st.running, "restart keeps the configuration");
        assert_eq!(st.outputs.len(), 1);

        engine.set_gain("missing-output", 0.1);
        assert_eq!(engine.shared.lock()["missing-output"].gain.load(), 0.1);

        engine.apply(EngineConfig {
            source: "input:missing".into(),
            outputs: vec![],
        });
        std::thread::sleep(Duration::from_millis(400));
        assert!(!engine.status().running);
    }

    /// Needs real audio devices: `cargo test -- --ignored`. The output is
    /// muted unless `AUDIO_MIRROR_AUDIBLE=1`, which also checks that audio
    /// actually reaches it (use it with virtual or silent outputs).
    #[test]
    #[ignore]
    fn mirrors_desktop_audio_on_real_devices() {
        let audible = std::env::var("AUDIO_MIRROR_AUDIBLE").is_ok_and(|v| v == "1");
        let list = enumerate().expect("device list");
        let desktop = list
            .sources
            .iter()
            .find(|s| s.id == DESKTOP)
            .cloned()
            .unwrap();
        let output = list
            .outputs
            .iter()
            .find(|o| Some(&o.id) != desktop.captures_output.as_ref())
            .expect("an output that is not captured");

        let engine = Engine::new();
        let mut outputs = vec![OutputSpec {
            id: output.id.clone(),
            gain: 1.0,
            muted: !audible,
        }];
        if let Some(captured) = &desktop.captures_output {
            outputs.push(OutputSpec {
                id: captured.clone(),
                gain: 1.0,
                muted: true,
            });
        }
        engine.apply(EngineConfig {
            source: DESKTOP.into(),
            outputs,
        });

        let mut peak = 0f32;
        let mut st = engine.status();
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            st = engine.status();
            peak = peak.max(st.outputs[0].peak);
        }
        println!(
            "{st:#?}
output peak over 3 s: {peak}"
        );
        assert_eq!(st.source.state, NodeState::Playing);
        assert_eq!(st.outputs[0].state, NodeState::Playing);
        if desktop.captures_output.is_some() {
            assert_eq!(st.outputs[1].state, NodeState::Blocked);
        }
        if audible {
            assert!(peak > 0.01, "no audio reached the output");
        }

        engine.restart();
        std::thread::sleep(Duration::from_millis(1500));
        let st = engine.status();
        assert_eq!(st.source.state, NodeState::Playing, "after restart");
        assert_eq!(st.outputs[0].state, NodeState::Playing, "after restart");
    }
}
