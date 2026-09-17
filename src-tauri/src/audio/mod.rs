//! Audio engine: OBS Studio's desktop audio capture and audio monitoring,
//! with one source feeding N monitors instead of one.
//!
//! Each platform module ports the matching OBS code:
//! - Windows: `plugins/win-wasapi` and `libobs/audio-monitoring/win32`
//! - macOS: `plugins/mac-capture` and `libobs/audio-monitoring/osx`
//! - Linux: `plugins/linux-pulseaudio` and `libobs/audio-monitoring/pulse`
//!
//! [`hub`] ports the source side of libobs that sits between the two, and
//! [`swr`] the FFmpeg resampler OBS uses. [`drift`] has no OBS equivalent:
//! it keeps each monitor on its device's clock.

pub mod drift;
pub mod format;
pub mod hub;
pub mod message;
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
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use hub::{AudioCallback, CallbackId, SourceHub};
use message::{self as msg, Message};
use volume::OutputShared;

/// Id of the system default output, captured like OBS's "Desktop Audio".
pub const DESKTOP: &str = "desktop";
pub const DESKTOP_NAME: &str = "Default output";

/// Retry delay for a monitor whose device could not be opened, or that went
/// away while playing. OBS has no equivalent: `obs_reset_audio_monitoring`
/// (`libobs/obs.c`) rebuilds monitors only when the user picks another
/// monitoring device, or, on Windows, when the default render device
/// changes, and nothing ever reads a monitor's device back. A tray app
/// nobody is watching retries on its own, at the pace `win-wasapi`
/// reconnects a capture.
const MONITOR_RETRY: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_millis(250);
/// How many times the supervisor is put back on its feet after a panic. A
/// panic there is a bug, not a device saying no, so the point is to keep the
/// app answering long enough to be reported, not to spin on it forever.
const PANIC_RESTARTS: u32 = 3;

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

/// What a backend calls when the system says something changed.
pub(crate) type EventCallback = Box<dyn Fn(SystemEvent) + Send + Sync>;

/// The registration that keeps [`Platform::watch_system`] alive; dropping it
/// unsubscribes.
pub(crate) type Watcher = Option<Box<dyn std::any::Any + Send + Sync>>;

/// The platform backend, behind a trait so the session and the supervisor
/// can be exercised without audio devices.
///
/// Only the lifecycle crosses it: opening a capture, opening a monitor,
/// listing devices. The audio itself never does, because a monitor is an
/// [`AudioCallback`] the capture thread calls straight through an `Arc`, as
/// `source_signal_audio_data` does in libobs. Nothing is added to the
/// per-packet work.
pub(crate) trait Platform: Send + Sync + 'static {
    /// Prepares the calling thread, where the backend needs it (COM).
    fn init_thread(&self);
    fn enumerate(&self) -> Result<DeviceList, Message>;
    fn start_capture(&self, source: &str, hub: Arc<SourceHub>)
        -> Result<Box<dyn Capture>, Message>;
    fn create_monitor(
        &self,
        source: &str,
        device: &str,
        shared: Arc<OutputShared>,
    ) -> Result<MonitorInit, Message>;
    fn watch_system(&self, callback: EventCallback) -> Watcher;
}

/// The backend of the system this was built for.
pub(crate) struct Native;

impl Platform for Native {
    fn init_thread(&self) {
        platform::init_thread();
    }

    fn enumerate(&self) -> Result<DeviceList, Message> {
        platform::enumerate()
    }

    fn start_capture(
        &self,
        source: &str,
        hub: Arc<SourceHub>,
    ) -> Result<Box<dyn Capture>, Message> {
        platform::start_capture(source, hub)
    }

    fn create_monitor(
        &self,
        source: &str,
        device: &str,
        shared: Arc<OutputShared>,
    ) -> Result<MonitorInit, Message> {
        platform::create_monitor(source, device, shared)
    }

    fn watch_system(&self, callback: EventCallback) -> Watcher {
        platform::watch_system(callback)
    }
}

pub fn enumerate() -> Result<DeviceList, Message> {
    let mut list = Native.enumerate()?;
    // Name the device the default output currently points to.
    if let Some(current) = list.outputs.iter().find(|o| o.is_default) {
        let name = format!("{DESKTOP_NAME} ({})", current.name);
        for s in list.sources.iter_mut().filter(|s| s.id == DESKTOP) {
            s.name = name.clone();
        }
    }
    Ok(list)
}

/// State of the running capture, reported by the platform.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureState {
    Starting,
    Active {
        format: String,
    },
    /// The capture retries on its own (WASAPI).
    Retrying(Message),
    /// The capture stopped for good; the session restarts it.
    Failed(Message),
}

/// A running source capture. Dropping it stops the capture.
pub trait Capture: Send {
    fn state(&self) -> CaptureState;
    /// The system default output changed. Returns true when the capture
    /// must be reopened by the session.
    fn default_output_changed(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MonitorState {
    Playing {
        format: String,
    },
    /// The device is being reopened after a failure.
    Reconnecting(Message),
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
    pub message: Option<Message>,
    pub format: Option<String>,
    pub peak: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputStatus {
    pub id: String,
    pub state: NodeState,
    pub message: Option<Message>,
    pub format: Option<String>,
    pub peak: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub source: SourceStatus,
    pub outputs: Vec<OutputStatus>,
    /// Bumped every time the system says the devices changed. The panel
    /// watches it instead of re-enumerating on a timer: the backends already
    /// know, and enumerating is a full COM pass on Windows.
    pub devices_revision: u64,
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
            devices_revision: 0,
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
    _watcher: Watcher,
}

impl Engine {
    pub fn new() -> Self {
        Self::with_platform(Arc::new(Native))
    }

    pub(crate) fn with_platform(platform: Arc<dyn Platform>) -> Self {
        Self::build(platform, MONITOR_RETRY)
    }

    /// Same engine with a shorter retry delay, so the tests do not wait the
    /// three seconds the product does.
    #[cfg(test)]
    pub(crate) fn with_retry(platform: Arc<dyn Platform>, retry: Duration) -> Self {
        Self::build(platform, retry)
    }

    fn build(platform: Arc<dyn Platform>, retry: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = SharedMap::default();

        let sup = Supervisor {
            rx,
            status: status.clone(),
            shared: shared.clone(),
            platform: platform.clone(),
            retry,
            devices_revision: 0,
            cfg: None,
            session: None,
        };
        let thread = std::thread::Builder::new()
            .name("audio-supervisor".into())
            .spawn(move || sup.run_guarded())
            .expect("failed to start the audio supervisor");

        let events = tx.clone();
        let watcher = platform.watch_system(Box::new(move |event| {
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
            // An output that is no longer configured keeps no controls. The
            // audio threads hold their own `Arc`, so a monitor still being
            // torn down is unaffected.
            shared.retain(|id, _| cfg.outputs.iter().any(|o| &o.id == id));
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
        /// Refreshed once per tick and read from there afterwards. Asking a
        /// monitor twice for the same answer costs a device query on macOS,
        /// where the state is read back from the hardware.
        state: MonitorState,
        /// Since when the monitor has been reporting a failed device, so a
        /// backend that cannot reopen it on its own still gets rebuilt.
        failing_since: Option<Instant>,
    },
    Ignored,
    Failed {
        error: Message,
        retry_at: Instant,
    },
}

/// One source and its monitors, like an `obs_source_t` with monitoring on.
struct Session {
    platform: Arc<dyn Platform>,
    retry: Duration,
    source: String,
    hub: Arc<SourceHub>,
    capture: Result<Box<dyn Capture>, Message>,
    /// Refreshed once per tick and read from there afterwards, like a
    /// monitor's, so one pass never asks the same backend twice.
    capture_state: CaptureState,
    capture_retry_at: Instant,
    slots: HashMap<String, Slot>,
}

impl Session {
    fn start(platform: Arc<dyn Platform>, retry: Duration, source: &str) -> Session {
        let hub = Arc::new(SourceHub::new());
        let capture = platform.start_capture(source, hub.clone());
        if let Err(e) = &capture {
            log::warn!("capture: {e}");
        }
        let mut session = Session {
            platform,
            retry,
            source: source.to_string(),
            hub,
            capture,
            capture_state: CaptureState::Starting,
            capture_retry_at: Instant::now() + retry,
            slots: HashMap::new(),
        };
        session.capture_state = session.read_capture_state();
        session
    }

    /// A capture that could not be opened at all reads as one that stopped,
    /// so there is a single shape for the tick and the panel to look at.
    fn read_capture_state(&self) -> CaptureState {
        match &self.capture {
            Ok(capture) => capture.state(),
            Err(e) => CaptureState::Failed(e.clone()),
        }
    }

    fn reopen_capture(&mut self) {
        // Close the device before opening it again. The placeholder is what
        // the panel shows if the new capture is slow to answer.
        self.capture = Err(msg::WAITING_FOR_DEVICE.into());
        self.capture = self.platform.start_capture(&self.source, self.hub.clone());
        if let Err(e) = &self.capture {
            log::warn!("capture: {e}");
        }
        self.capture_state = self.read_capture_state();
        self.capture_retry_at = Instant::now() + self.retry;
    }

    /// `audio_monitor_create`. A monitor already on that output is destroyed
    /// first, like OBS does, so the device is released before it is opened
    /// again: a driver that only accepts one client can still be rebuilt.
    fn create_monitor(&mut self, id: &str, shared: Arc<OutputShared>) {
        self.remove_monitor(id);
        let slot = match self.platform.create_monitor(&self.source, id, shared) {
            Ok(MonitorInit::Active(monitor)) => {
                let callback = self.hub.add_callback(monitor.clone());
                Slot::Active {
                    state: monitor.state(),
                    monitor,
                    callback,
                    failing_since: None,
                }
            }
            Ok(MonitorInit::Ignored) => {
                log::info!("prevented feedback loop on {id}");
                Slot::Ignored
            }
            Err(error) => {
                log::warn!("monitor {id}: {error}");
                Slot::Failed {
                    error,
                    retry_at: Instant::now() + self.retry,
                }
            }
        };
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

/// Whether a slot must be built again now: a device that could not be opened
/// and is due for a retry, or a monitor that has been reporting a failure for
/// a full retry delay.
fn slot_is_due(slot: &Slot, now: Instant, retry: Duration) -> bool {
    match slot {
        Slot::Failed { retry_at, .. } => now >= *retry_at,
        Slot::Active {
            failing_since: Some(since),
            ..
        } => now >= *since + retry,
        _ => false,
    }
}

struct Supervisor {
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    shared: SharedMap,
    platform: Arc<dyn Platform>,
    /// How long a failed capture or monitor waits before being opened again.
    retry: Duration,
    /// Counts the system's device notifications, published for the panel.
    devices_revision: u64,
    cfg: Option<EngineConfig>,
    session: Option<Session>,
}

impl Supervisor {
    /// Runs the loop, and keeps the engine answering if it ever panics.
    ///
    /// The supervisor owns the capture and every monitor, so letting a panic
    /// unwind out of this thread would leave the app silent in the worst
    /// way: the command channel closed, every click on the panel dropped on
    /// the floor, and the status frozen on whatever was published last. The
    /// session is thrown away instead and opened again from the
    /// configuration. The locks around the status and the per-output
    /// controls are `parking_lot`'s, which do not poison, so they are still
    /// usable on the way out of the unwind.
    ///
    /// This needs unwinding to work: under a `panic = "abort"` profile it is
    /// dead code, which is a fair trade rather than a reason to leave the
    /// thread unguarded.
    fn run_guarded(mut self) {
        for _ in 0..=PANIC_RESTARTS {
            let clean = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Whatever a panic left half built is dropped in here, so a
                // device handle that panics on close is caught as well.
                self.session = None;
                self.run();
            }));
            if clean.is_ok() {
                return;
            }
            log::error!("audio supervisor panicked, opening the devices again");
        }
        log::error!("audio supervisor gave up after {PANIC_RESTARTS} panics");
        *self.status.lock() = Status::default();
    }

    fn run(&mut self) {
        self.platform.init_thread();
        // Nothing is open after a panic: build the session again before
        // going back to waiting for commands.
        if self.session.is_none() {
            if let Some(cfg) = self.cfg.clone() {
                self.apply(cfg);
            }
        }
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
                Ok(Cmd::System(event)) => {
                    // Counted even with nothing running, so the panel still
                    // refreshes its list while every output is off.
                    self.devices_revision += 1;
                    self.system(event);
                }
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

        // What the user asked for is recorded before anything is opened, so
        // a backend that panics half way through does not also lose it.
        self.cfg = Some(cfg.clone());

        let restart = self.session.as_ref().is_none_or(|s| s.source != cfg.source);
        if restart {
            self.session = None;
            self.session = Some(Session::start(
                self.platform.clone(),
                self.retry,
                &cfg.source,
            ));
        }

        // `restart` just put one there, so this always binds. Taking the
        // branch instead of unwrapping keeps the thread that owns every
        // device free of reachable panics.
        let Some(session) = self.session.as_mut() else {
            return;
        };
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
    }

    fn system(&mut self, event: SystemEvent) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match event {
            SystemEvent::DefaultOutputChanged => {
                log::info!("default output changed");
                if session
                    .capture
                    .as_ref()
                    .is_ok_and(|c| c.default_output_changed())
                {
                    session.reopen_capture();
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
                // A device that just came back is opened on the next tick
                // rather than at the end of the retry delay, whether the
                // output never opened or is open and reporting a dead
                // device.
                let retry = self.retry;
                let now = Instant::now();
                for slot in session.slots.values_mut() {
                    match slot {
                        Slot::Failed { retry_at, .. } => *retry_at = now,
                        Slot::Active {
                            monitor,
                            state,
                            failing_since,
                            ..
                        } => {
                            // Read here rather than trusting the last tick,
                            // so the event works whichever of the two
                            // noticed the device first.
                            *state = monitor.state();
                            if matches!(state, MonitorState::Reconnecting(_)) {
                                *failing_since = now.checked_sub(retry).or(*failing_since);
                            }
                        }
                        Slot::Ignored => {}
                    }
                }
                session.capture_retry_at = now;
            }
        }
    }

    fn tick(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let now = Instant::now();

        let state = session.read_capture_state();
        session.capture_state = state;
        let failed = matches!(session.capture_state, CaptureState::Failed(_));
        if !failed {
            session.capture_retry_at = now + self.retry;
        } else if now >= session.capture_retry_at {
            session.reopen_capture();
        }

        // A monitor reporting a failed device is rebuilt once it has said so
        // for a full retry delay. Backends that reopen the device themselves
        // (WASAPI, on the next packet) clear the flag long before that, so
        // this only fires when nothing else would.
        for (id, slot) in session.slots.iter_mut() {
            if let Slot::Active {
                monitor,
                state,
                failing_since,
                ..
            } = slot
            {
                *state = monitor.state();
                match state {
                    // Once on the way down and once on the way back, so a
                    // mirror left running for days leaves a trace of every
                    // time an output dropped without anyone watching.
                    MonitorState::Reconnecting(why) => {
                        if failing_since.is_none() {
                            log::warn!("output {id}: {why}");
                        }
                        failing_since.get_or_insert(now);
                    }
                    MonitorState::Playing { .. } => {
                        if failing_since.take().is_some() {
                            log::info!("output {id}: playing again");
                        }
                    }
                }
            }
        }

        let due: Vec<String> = session
            .slots
            .iter()
            .filter(|(_, slot)| slot_is_due(slot, now, self.retry))
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
        st.devices_revision = self.devices_revision;
        let (Some(cfg), Some(session)) = (&self.cfg, &self.session) else {
            *st = Status {
                devices_revision: self.devices_revision,
                ..Status::default()
            };
            return;
        };
        st.running = true;

        // Read as of this tick, so the backend is not queried again just to
        // fill the panel.
        let (state, message, format) = match &session.capture_state {
            CaptureState::Starting => (NodeState::Starting, None, None),
            CaptureState::Active { format } => (NodeState::Playing, None, Some(format.clone())),
            CaptureState::Retrying(msg) | CaptureState::Failed(msg) => {
                (NodeState::Error, Some(msg.clone()), None)
            }
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
                    // Read as of this tick, so the backend is not queried
                    // again just to fill the panel.
                    Some(Slot::Active { state, .. }) => match state {
                        MonitorState::Playing { format } => {
                            (NodeState::Playing, None, Some(format.clone()))
                        }
                        MonitorState::Reconnecting(why) => {
                            (NodeState::Error, Some(why.clone()), None)
                        }
                    },
                    Some(Slot::Ignored) => (
                        NodeState::Blocked,
                        Some(msg::CAPTURED_BY_SOURCE.into()),
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

    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A monitor whose reported state the test drives.
    struct FakeMonitor(Mutex<MonitorState>);

    impl FakeMonitor {
        fn playing() -> Arc<FakeMonitor> {
            Arc::new(FakeMonitor(Mutex::new(MonitorState::Playing {
                format: "48 kHz, stereo".into(),
            })))
        }

        fn lose_device(&self) {
            *self.0.lock() = MonitorState::Reconnecting(msg::DEVICE_DISCONNECTED.into());
        }
    }

    impl hub::AudioCallback for FakeMonitor {
        fn on_audio(&self, _audio: &format::ObsAudio) {}
    }

    impl Monitor for FakeMonitor {
        fn state(&self) -> MonitorState {
            self.0.lock().clone()
        }
    }

    fn active_slot(failing_since: Option<Instant>) -> Slot {
        let monitor = FakeMonitor::playing();
        monitor.lose_device();
        Slot::Active {
            state: monitor.state(),
            monitor,
            callback: 1,
            failing_since,
        }
    }

    /// A capture whose reported state the test drives.
    struct FakeCapture {
        state: Mutex<CaptureState>,
        follows_default: bool,
    }

    struct FakeCaptureHandle(Arc<FakeCapture>);

    impl Capture for FakeCaptureHandle {
        fn state(&self) -> CaptureState {
            self.0.state.lock().clone()
        }

        fn default_output_changed(&self) -> bool {
            self.0.follows_default
        }
    }

    /// A backend with no audio hardware, scripted by the test.
    #[derive(Default)]
    struct FakePlatform {
        /// Devices whose monitor refuses to open, and why.
        refusing: Mutex<HashMap<String, Message>>,
        /// Devices the source records, so they come back `Ignored`.
        captured: Mutex<HashSet<String>>,
        /// Every monitor handed out, in order, so rebuilds can be counted.
        built: Mutex<Vec<String>>,
        live: Mutex<HashMap<String, Arc<FakeMonitor>>>,
        capture: Mutex<Option<Arc<FakeCapture>>>,
        captures_started: AtomicUsize,
        /// Monitors left to blow up on, to stand in for a bug in a backend.
        panics_left: AtomicUsize,
        follows_default: Mutex<bool>,
        events: Mutex<Option<EventCallback>>,
    }

    impl FakePlatform {
        fn refuse(&self, device: &str, why: msg::Kind) {
            self.refusing.lock().insert(device.into(), why.into());
        }

        fn accept(&self, device: &str) {
            self.refusing.lock().remove(device);
        }

        fn built(&self, device: &str) -> usize {
            self.built.lock().iter().filter(|d| *d == device).count()
        }

        /// The monitor currently playing on that device.
        fn monitor(&self, device: &str) -> Arc<FakeMonitor> {
            self.live.lock()[device].clone()
        }

        fn captures_started(&self) -> usize {
            self.captures_started.load(Ordering::Relaxed)
        }

        fn fire(&self, event: SystemEvent) {
            if let Some(callback) = self.events.lock().as_ref() {
                callback(event);
            }
        }
    }

    impl Platform for FakePlatform {
        fn init_thread(&self) {}

        fn enumerate(&self) -> Result<DeviceList, Message> {
            Ok(DeviceList::default())
        }

        fn start_capture(
            &self,
            _source: &str,
            _hub: Arc<SourceHub>,
        ) -> Result<Box<dyn Capture>, Message> {
            self.captures_started.fetch_add(1, Ordering::Relaxed);
            let capture = Arc::new(FakeCapture {
                state: Mutex::new(CaptureState::Active {
                    format: "48 kHz, stereo".into(),
                }),
                follows_default: *self.follows_default.lock(),
            });
            *self.capture.lock() = Some(capture.clone());
            Ok(Box::new(FakeCaptureHandle(capture)))
        }

        fn create_monitor(
            &self,
            _source: &str,
            device: &str,
            _shared: Arc<OutputShared>,
        ) -> Result<MonitorInit, Message> {
            self.built.lock().push(device.to_string());
            if self
                .panics_left
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                panic!("a backend blew up");
            }
            if self.captured.lock().contains(device) {
                return Ok(MonitorInit::Ignored);
            }
            if let Some(why) = self.refusing.lock().get(device) {
                return Err(why.clone());
            }
            let monitor = FakeMonitor::playing();
            self.live.lock().insert(device.to_string(), monitor.clone());
            Ok(MonitorInit::Active(monitor))
        }

        fn watch_system(&self, callback: EventCallback) -> Watcher {
            *self.events.lock() = Some(callback);
            None
        }
    }

    /// Retry delay for the tests: long enough not to race the 250 ms tick
    /// being irrelevant here, short enough to keep the suite quick.
    const TEST_RETRY: Duration = Duration::from_millis(60);

    /// Polls until `done` holds, so the tests do not depend on how long a
    /// supervisor tick takes.
    #[track_caller]
    fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        // Only reached when something is actually wrong, so a generous
        // deadline costs nothing and keeps a loaded runner from failing a
        // test that would have passed.
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn one_output(id: &str) -> EngineConfig {
        EngineConfig {
            source: "input:fake".into(),
            outputs: vec![OutputSpec {
                id: id.into(),
                gain: 1.0,
                muted: false,
            }],
        }
    }

    fn output_state(engine: &Engine, id: &str) -> Option<NodeState> {
        engine
            .status()
            .outputs
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.state.clone())
    }

    #[test]
    fn a_monitor_whose_device_dies_is_rebuilt_and_plays_again() {
        let fake = Arc::new(FakePlatform::default());
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));

        eventually("the output to play", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
        assert_eq!(fake.built("out"), 1);

        // The device goes away under a monitor that was playing: the panel
        // has to say so, and the session has to open it again.
        fake.monitor("out").lose_device();
        eventually("the loss to show", || {
            output_state(&engine, "out") == Some(NodeState::Error)
        });
        eventually("the monitor to be rebuilt", || fake.built("out") >= 2);
        eventually("the output to play again", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
    }

    #[test]
    fn an_output_that_cannot_be_opened_is_retried_until_it_works() {
        let fake = Arc::new(FakePlatform::default());
        fake.refuse("out", msg::DEVICE_UNAVAILABLE);
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));

        eventually("the failure to show", || {
            output_state(&engine, "out") == Some(NodeState::Error)
        });
        let message = engine.status().outputs[0].message.clone();
        assert_eq!(message, Some(msg::DEVICE_UNAVAILABLE.into()));

        eventually("a retry", || fake.built("out") >= 2);
        fake.accept("out");
        eventually("the output to come back", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
    }

    #[test]
    fn the_output_the_source_records_is_left_alone() {
        let fake = Arc::new(FakePlatform::default());
        fake.captured.lock().insert("out".into());
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));

        eventually("the output to be skipped", || {
            output_state(&engine, "out") == Some(NodeState::Blocked)
        });
        // `OBS_SOURCE_DO_NOT_SELF_MONITOR` is not a failure: never retried.
        std::thread::sleep(TEST_RETRY * 5);
        assert_eq!(fake.built("out"), 1);
        assert_eq!(output_state(&engine, "out"), Some(NodeState::Blocked));
    }

    #[test]
    fn a_capture_that_stopped_is_opened_again() {
        let fake = Arc::new(FakePlatform::default());
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));

        eventually("the capture to start", || fake.captures_started() == 1);
        eventually("the source to play", || {
            engine.status().source.state == NodeState::Playing
        });

        *fake.capture.lock().as_ref().unwrap().state.lock() =
            CaptureState::Failed(msg::DEVICE_DISCONNECTED.into());
        eventually("the capture to be reopened", || {
            fake.captures_started() >= 2
        });
        eventually("the source to play again", || {
            engine.status().source.state == NodeState::Playing
        });
    }

    #[test]
    fn a_device_notification_tells_the_panel_to_look_again() {
        let fake = Arc::new(FakePlatform::default());
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);

        // Counted with nothing running, so the list refreshes while every
        // output is off.
        let before = engine.status().devices_revision;
        fake.fire(SystemEvent::DevicesChanged);
        eventually("the revision to move", || {
            engine.status().devices_revision > before
        });

        engine.apply(one_output("out"));
        eventually("the output to play", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
        let running = engine.status().devices_revision;
        fake.fire(SystemEvent::DefaultOutputChanged);
        eventually("a default change to count too", || {
            engine.status().devices_revision > running
        });
    }

    #[test]
    fn a_device_coming_back_is_opened_without_waiting_for_the_retry() {
        // Long enough that a rebuild inside it can only come from the event.
        const SLOW: Duration = Duration::from_secs(30);

        let fake = Arc::new(FakePlatform::default());
        fake.refuse("out", msg::DEVICE_UNAVAILABLE);
        let engine = Engine::with_retry(fake.clone(), SLOW);
        engine.apply(one_output("out"));
        eventually("the failure to show", || {
            output_state(&engine, "out") == Some(NodeState::Error)
        });
        assert_eq!(fake.built("out"), 1);

        fake.accept("out");
        fake.fire(SystemEvent::DevicesChanged);
        eventually("the output to come back", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
    }

    #[test]
    fn a_device_coming_back_under_a_failing_monitor_is_opened_at_once() {
        const SLOW: Duration = Duration::from_secs(30);

        let fake = Arc::new(FakePlatform::default());
        let engine = Engine::with_retry(fake.clone(), SLOW);
        engine.apply(one_output("out"));
        eventually("the output to play", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });

        // Unplugged: the monitor is still there, saying the device is gone.
        fake.monitor("out").lose_device();
        eventually("the loss to show", || {
            output_state(&engine, "out") == Some(NodeState::Error)
        });
        assert_eq!(fake.built("out"), 1, "the retry delay has not elapsed");

        // Plugged back in: the panel should not sit on an error for the rest
        // of the delay when the system already said the devices changed.
        fake.fire(SystemEvent::DevicesChanged);
        eventually("the monitor to be rebuilt", || fake.built("out") >= 2);
        eventually("the output to play again", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });
    }

    #[test]
    fn a_default_output_change_rebuilds_every_monitor() {
        let fake = Arc::new(FakePlatform::default());
        *fake.follows_default.lock() = true;
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));
        eventually("the output to play", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });

        // `obs_reset_audio_monitoring`: the capture follows the new default
        // and every monitor is built again, which re-runs the feedback rule.
        fake.fire(SystemEvent::DefaultOutputChanged);
        eventually("the capture to follow", || fake.captures_started() >= 2);
        eventually("the monitors to be rebuilt", || fake.built("out") >= 2);
    }

    /// Prints a panic and its backtrace on stderr, which is expected here.
    #[test]
    fn a_panic_in_the_supervisor_does_not_take_the_engine_down() {
        let fake = Arc::new(FakePlatform::default());
        fake.panics_left.store(1, Ordering::Relaxed);
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        engine.apply(one_output("out"));

        // The devices are opened again and the output ends up playing.
        eventually("the engine to come back", || {
            output_state(&engine, "out") == Some(NodeState::Playing)
        });

        // And the engine still takes commands, which is what would be lost
        // if the panic had ended the thread.
        engine.apply(EngineConfig {
            source: "input:fake".into(),
            outputs: vec![],
        });
        eventually("the engine to stop", || !engine.status().running);
    }

    #[test]
    fn an_output_switched_off_is_dropped_but_the_others_keep_playing() {
        let fake = Arc::new(FakePlatform::default());
        let engine = Engine::with_retry(fake.clone(), TEST_RETRY);
        let mut cfg = one_output("keep");
        cfg.outputs.push(OutputSpec {
            id: "drop".into(),
            gain: 1.0,
            muted: false,
        });
        engine.apply(cfg);
        eventually("both outputs to play", || {
            output_state(&engine, "keep") == Some(NodeState::Playing)
                && output_state(&engine, "drop") == Some(NodeState::Playing)
        });

        engine.apply(one_output("keep"));
        eventually("the output to be dropped", || {
            output_state(&engine, "drop").is_none()
        });
        assert_eq!(output_state(&engine, "keep"), Some(NodeState::Playing));
        assert_eq!(
            fake.built("keep"),
            1,
            "the output that stayed on is not rebuilt"
        );
        assert!(!engine.shared.lock().contains_key("drop"));
    }

    #[test]
    fn a_monitor_stuck_on_a_failed_device_is_rebuilt() {
        let now = Instant::now();

        // A device that could not be opened keeps its own retry schedule.
        let failed = Slot::Failed {
            error: msg::DEVICE_UNAVAILABLE.into(),
            retry_at: now + MONITOR_RETRY,
        };
        assert!(!slot_is_due(&failed, now, MONITOR_RETRY));
        assert!(slot_is_due(&failed, now + MONITOR_RETRY, MONITOR_RETRY));

        // A monitor that reports a failure is left alone until the retry
        // delay has passed: backends that heal on their own get their chance.
        assert!(!slot_is_due(
            &active_slot(None),
            now + MONITOR_RETRY * 10,
            MONITOR_RETRY
        ));
        assert!(!slot_is_due(&active_slot(Some(now)), now, MONITOR_RETRY));
        assert!(!slot_is_due(
            &active_slot(Some(now)),
            now + MONITOR_RETRY - Duration::from_millis(1),
            MONITOR_RETRY
        ));
        assert!(slot_is_due(
            &active_slot(Some(now)),
            now + MONITOR_RETRY,
            MONITOR_RETRY
        ));

        // An output left out on purpose is never rebuilt.
        assert!(!slot_is_due(
            &Slot::Ignored,
            now + MONITOR_RETRY * 10,
            MONITOR_RETRY
        ));
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
        assert!(
            engine.shared.lock().is_empty(),
            "an output that is gone leaves no controls behind"
        );
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
