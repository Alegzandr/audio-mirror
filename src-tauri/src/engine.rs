//! Engine: one capture, N outputs, one supervisor thread.
//!
//! The supervisor thread owns the cpal streams. It rebuilds whatever fails
//! every three seconds (the `RECONNECT_INTERVAL` of OBS's `win-wasapi`) and
//! follows the default output when the source is desktop audio.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, ErrorKind, Sample, SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;
use rtrb::{Producer, RingBuffer};
use serde::Serialize;

use crate::devices;
use crate::dsp::{push_frames, AtomicF32, OutputShared, Pipe, PipeFormat, PlayState};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);
const DEFAULT_POLL: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);
const MAX_OUTPUTS: usize = 64;

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
    pub latency_ms: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    Idle,
    Starting,
    Buffering,
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
    Stop,
    Shutdown,
}

/// Messages for the capture thread, handled without allocating.
enum CaptureCmd {
    Add(u64, Producer<f32>),
    Remove(u64),
}

type Fault = Arc<Mutex<Option<String>>>;
type SharedMap = Arc<Mutex<HashMap<String, Arc<OutputShared>>>>;

pub struct Engine {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
    shared: SharedMap,
    source_peak: Arc<AtomicF32>,
}

impl Engine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let shared = SharedMap::default();
        let source_peak = Arc::new(AtomicF32::new(0.0));

        let sup = Supervisor {
            rx,
            status: status.clone(),
            shared: shared.clone(),
            source_peak: source_peak.clone(),
            cfg: None,
            capture: None,
            outputs: HashMap::new(),
            next_source_try: Instant::now(),
            last_default_poll: Instant::now(),
            next_key: 1,
            source_error: None,
        };
        let thread = std::thread::Builder::new()
            .name("audio-supervisor".into())
            .spawn(move || sup.run())
            .expect("failed to start the audio supervisor");

        Self {
            tx,
            thread: Some(thread),
            status,
            shared,
            source_peak,
        }
    }

    /// Starts or updates mirroring. An empty output list stops it.
    pub fn apply(&self, cfg: EngineConfig) {
        if cfg.outputs.is_empty() {
            self.stop();
            return;
        }
        {
            let mut shared = self.shared.lock();
            for o in &cfg.outputs {
                let s = shared
                    .entry(o.id.clone())
                    .or_insert_with(|| Arc::new(OutputShared::new(o.gain, o.muted)));
                s.gain.store(o.gain);
                s.muted.store(o.muted, Ordering::Relaxed);
            }
        }
        let _ = self.tx.send(Cmd::Apply(cfg));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// Instant volume change, without rebuilding the stream.
    pub fn set_gain(&self, id: &str, gain: f32) {
        if let Some(s) = self.shared.lock().get(id) {
            s.gain.store(gain);
        }
    }

    pub fn set_muted(&self, id: &str, muted: bool) {
        if let Some(s) = self.shared.lock().get(id) {
            s.muted.store(muted, Ordering::Relaxed);
        }
    }

    /// Status snapshot, with the peaks accumulated since the previous call.
    pub fn status(&self) -> Status {
        let mut st = self.status.lock().clone();
        st.source.peak = self.source_peak.take();
        let shared = self.shared.lock();
        for o in &mut st.outputs {
            let Some(s) = shared.get(&o.id) else { continue };
            o.peak = s.peak.take();
            if matches!(o.state, NodeState::Buffering | NodeState::Playing) {
                o.state = match s.play_state() {
                    PlayState::Playing => NodeState::Playing,
                    PlayState::Buffering => NodeState::Buffering,
                };
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

struct Capture {
    _stream: Stream,
    cmds: Producer<CaptureCmd>,
    fault: Fault,
    rate: u32,
    channels: usize,
    loopback_of: Option<String>,
}

struct OutputSlot {
    stream: Option<Stream>,
    key: u64,
    fault: Fault,
    error: Option<String>,
    format: Option<String>,
    blocked: bool,
    next_try: Instant,
}

impl OutputSlot {
    fn new() -> Self {
        Self {
            stream: None,
            key: 0,
            fault: Fault::default(),
            error: None,
            format: None,
            blocked: false,
            next_try: Instant::now(),
        }
    }
}

struct Supervisor {
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    shared: SharedMap,
    source_peak: Arc<AtomicF32>,

    cfg: Option<EngineConfig>,
    capture: Option<Capture>,
    outputs: HashMap<String, OutputSlot>,
    next_source_try: Instant,
    last_default_poll: Instant,
    next_key: u64,
    source_error: Option<String>,
}

impl Supervisor {
    fn run(mut self) {
        let host = cpal::default_host();
        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(Cmd::Apply(cfg)) => self.apply(cfg),
                Ok(Cmd::Stop) => self.stop(),
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick(&host);
            self.publish();
        }
    }

    fn stop(&mut self) {
        self.outputs.clear();
        self.capture = None;
        self.cfg = None;
        self.source_error = None;
    }

    fn apply(&mut self, cfg: EngineConfig) {
        let full_rebuild = match &self.cfg {
            None => true,
            Some(old) => old.source != cfg.source || old.latency_ms != cfg.latency_ms,
        };
        if full_rebuild {
            self.outputs.clear();
            self.capture = None;
            self.source_error = None;
            self.next_source_try = Instant::now();
        } else {
            let removed: Vec<String> = self
                .outputs
                .keys()
                .filter(|id| !cfg.outputs.iter().any(|o| &o.id == *id))
                .cloned()
                .collect();
            for id in removed {
                if let Some(slot) = self.outputs.remove(&id) {
                    self.send_capture(CaptureCmd::Remove(slot.key));
                }
            }
        }
        for o in &cfg.outputs {
            self.outputs
                .entry(o.id.clone())
                .or_insert_with(OutputSlot::new);
        }
        self.cfg = Some(cfg);
    }

    fn send_capture(&mut self, cmd: CaptureCmd) {
        let failed = match self.capture.as_mut() {
            Some(c) => c.cmds.push(cmd).is_err(),
            None => false,
        };
        if failed {
            // Command queue saturated: start over cleanly.
            self.drop_capture(Some("Restarting the stream".into()));
        }
    }

    fn drop_capture(&mut self, reason: Option<String>) {
        self.capture = None;
        for slot in self.outputs.values_mut() {
            slot.stream = None;
            slot.key = 0;
            slot.next_try = Instant::now();
        }
        self.source_error = reason;
        self.next_source_try = Instant::now() + RECONNECT_INTERVAL;
    }

    fn tick(&mut self, host: &cpal::Host) {
        let Some(cfg) = self.cfg.clone() else { return };

        // Capture failure reported by cpal.
        let fault = self.capture.as_ref().and_then(|c| c.fault.lock().take());
        if let Some(msg) = fault {
            self.drop_capture(Some(msg));
        }

        // Desktop audio follows the default output.
        if cfg.source == devices::DESKTOP
            && self.capture.is_some()
            && self.last_default_poll.elapsed() >= DEFAULT_POLL
        {
            self.last_default_poll = Instant::now();
            let current = devices::default_output_id(host);
            let captured = self.capture.as_ref().and_then(|c| c.loopback_of.clone());
            if current.is_some() && current != captured {
                self.drop_capture(None);
                self.next_source_try = Instant::now();
            }
        }

        if self.capture.is_none() {
            if Instant::now() < self.next_source_try {
                return;
            }
            match build_capture(host, &cfg, self.source_peak.clone()) {
                Ok((capture, format)) => {
                    self.capture = Some(capture);
                    self.source_error = None;
                    self.status.lock().source.format = Some(format);
                }
                Err(e) => {
                    log::warn!("capture: {e}");
                    self.source_error = Some(e);
                    self.next_source_try = Instant::now() + RECONNECT_INTERVAL;
                    return;
                }
            }
        }

        let (rate, channels, loopback_of) = match &self.capture {
            Some(c) => (c.rate, c.channels, c.loopback_of.clone()),
            None => return,
        };
        let target_frames = (rate as u64 * cfg.latency_ms as u64 / 1000) as usize;

        let mut cmds = Vec::new();
        for id in cfg.outputs.iter().map(|o| &o.id).take(MAX_OUTPUTS) {
            let Some(shared) = self.shared.lock().get(id).cloned() else {
                continue;
            };
            let key = self.next_key;
            let Some(slot) = self.outputs.get_mut(id) else {
                continue;
            };

            if let Some(msg) = slot.fault.lock().take() {
                slot.stream = None;
                slot.error = Some(msg);
                slot.next_try = Instant::now() + RECONNECT_INTERVAL;
                cmds.push(CaptureCmd::Remove(std::mem::take(&mut slot.key)));
                continue;
            }

            // Never play back into the captured device: instant feedback loop.
            slot.blocked = loopback_of.as_deref() == Some(id.as_str());
            if slot.blocked {
                if slot.stream.take().is_some() {
                    cmds.push(CaptureCmd::Remove(std::mem::take(&mut slot.key)));
                }
                continue;
            }
            if slot.stream.is_some() || Instant::now() < slot.next_try {
                continue;
            }

            let fault = Fault::default();
            match build_output(
                host,
                id,
                rate,
                channels,
                target_frames,
                shared,
                fault.clone(),
            ) {
                Ok((stream, producer, format)) => {
                    slot.stream = Some(stream);
                    slot.key = key;
                    slot.fault = fault;
                    slot.error = None;
                    slot.format = Some(format);
                    self.next_key += 1;
                    cmds.push(CaptureCmd::Add(key, producer));
                }
                Err(e) => {
                    log::warn!("output {id}: {e}");
                    slot.error = Some(e);
                    slot.next_try = Instant::now() + RECONNECT_INTERVAL;
                }
            }
        }
        for cmd in cmds {
            self.send_capture(cmd);
        }
    }

    fn publish(&self) {
        let mut st = self.status.lock();
        let Some(cfg) = &self.cfg else {
            *st = Status::default();
            return;
        };
        st.running = true;

        st.source.state = if self.capture.is_some() {
            NodeState::Playing
        } else if self.source_error.is_some() {
            NodeState::Error
        } else {
            NodeState::Starting
        };
        st.source.message = self.source_error.clone();
        if self.capture.is_none() {
            st.source.format = None;
        }

        st.outputs = cfg
            .outputs
            .iter()
            .map(|o| {
                let slot = self.outputs.get(&o.id);
                let (state, message) = match slot {
                    Some(s) if s.blocked => (
                        NodeState::Blocked,
                        Some("Captured by the source".to_string()),
                    ),
                    Some(s) if s.stream.is_some() => (NodeState::Buffering, None),
                    Some(s) if s.error.is_some() => (NodeState::Error, s.error.clone()),
                    _ if self.capture.is_none() => (NodeState::Idle, None),
                    _ => (NodeState::Starting, None),
                };
                OutputStatus {
                    id: o.id.clone(),
                    state,
                    message,
                    format: slot
                        .filter(|s| s.stream.is_some())
                        .and_then(|s| s.format.clone()),
                    peak: 0.0,
                }
            })
            .collect();
    }
}

fn describe(rate: u32, channels: usize, fmt: SampleFormat) -> String {
    let layout = match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        6 => "5.1".to_string(),
        8 => "7.1".to_string(),
        n => format!("{n} channels"),
    };
    format!("{} kHz, {layout}, {fmt}", rate as f32 / 1000.0)
}

fn error_sink(fault: Fault, what: &'static str) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err: cpal::Error| match err.kind() {
        // Transient incidents: the stream keeps running.
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
            log::debug!("{what}: {err}");
        }
        kind => {
            log::warn!("{what}: {err}");
            let msg = match kind {
                ErrorKind::DeviceNotAvailable => "Device disconnected".to_string(),
                ErrorKind::PermissionDenied => "Access denied by the system".to_string(),
                _ => err.to_string(),
            };
            *fault.lock() = Some(msg);
        }
    }
}

fn build_capture(
    host: &cpal::Host,
    cfg: &EngineConfig,
    peak: Arc<AtomicF32>,
) -> Result<(Capture, String), String> {
    let src = devices::resolve_source(host, &cfg.source)?;
    let rate = src.config.sample_rate();
    let channels = src.config.channels() as usize;
    let format = src.config.sample_format();
    let stream_cfg: StreamConfig = src.config.config();

    let (cmds, mut cmd_rx) = RingBuffer::<CaptureCmd>::new(MAX_OUTPUTS * 2);
    let mut sinks: Vec<(u64, Producer<f32>)> = Vec::with_capacity(MAX_OUTPUTS);
    let mut scratch: Vec<f32> = vec![0.0; 1 << 16];
    let fault = Fault::default();

    let stream = src
        .device
        .build_input_stream_raw(
            stream_cfg,
            format,
            move |data: &Data, _| {
                while let Ok(cmd) = cmd_rx.pop() {
                    match cmd {
                        CaptureCmd::Add(k, p) => {
                            sinks.retain(|(key, _)| *key != k);
                            if sinks.len() < sinks.capacity() {
                                sinks.push((k, p));
                            }
                        }
                        CaptureCmd::Remove(k) => sinks.retain(|(key, _)| *key != k),
                    }
                }
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buf = &mut scratch[..data.len()];
                if !read_f32(data, buf) {
                    return;
                }
                peak.raise(buf.iter().fold(0f32, |m, s| m.max(s.abs())));
                for (_, p) in sinks.iter_mut() {
                    push_frames(p, buf, channels);
                }
            },
            error_sink(fault.clone(), "capture"),
            Some(Duration::from_secs(5)),
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok((
        Capture {
            _stream: stream,
            cmds,
            fault,
            rate,
            channels,
            loopback_of: src.loopback_of,
        },
        describe(rate, channels, format),
    ))
}

fn build_output(
    host: &cpal::Host,
    id: &str,
    in_rate: u32,
    in_channels: usize,
    target_frames: usize,
    shared: Arc<OutputShared>,
    fault: Fault,
) -> Result<(Stream, Producer<f32>, String), String> {
    let (device, config) = devices::resolve_output(host, id)?;
    let out_rate = config.sample_rate();
    let out_channels = config.channels() as usize;
    let format = config.sample_format();

    let fmt = PipeFormat {
        in_rate,
        in_channels,
        out_rate,
        out_channels,
        target_frames: target_frames.max(64),
    };
    let (producer, consumer) = RingBuffer::new(fmt.ring_capacity());
    shared
        .state
        .store(PlayState::Buffering as u8, Ordering::Relaxed);
    let mut pipe = Pipe::new(fmt, consumer, shared)?;
    let mut scratch: Vec<f32> = vec![0.0; 1 << 16];

    let stream = device
        .build_output_stream_raw(
            config.config(),
            format,
            move |data: &mut Data, _| {
                let n = data.len();
                if scratch.len() < n {
                    scratch.resize(n, 0.0);
                }
                pipe.render(&mut scratch[..n]);
                write_f32(&scratch[..n], data);
            },
            error_sink(fault, "output"),
            Some(Duration::from_secs(5)),
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok((stream, producer, describe(out_rate, out_channels, format)))
}

macro_rules! convert_formats {
    ($($variant:ident => $ty:ty),* $(,)?) => {
        /// Converts a cpal buffer to `f32`. False for an unknown format.
        fn read_f32(data: &Data, out: &mut [f32]) -> bool {
            match data.sample_format() {
                $(SampleFormat::$variant => match data.as_slice::<$ty>() {
                    Some(src) => {
                        for (o, s) in out.iter_mut().zip(src) {
                            *o = s.to_sample::<f32>();
                        }
                        true
                    }
                    None => false,
                },)*
                _ => false,
            }
        }

        /// Writes `f32` samples into a cpal buffer, silence for an unknown format.
        fn write_f32(src: &[f32], data: &mut Data) {
            match data.sample_format() {
                $(SampleFormat::$variant => {
                    if let Some(dst) = data.as_slice_mut::<$ty>() {
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s.clamp(-1.0, 1.0).to_sample::<$ty>();
                        }
                    }
                })*
                _ => data.bytes_mut().fill(0),
            }
        }
    };
}

convert_formats! {
    F32 => f32,
    F64 => f64,
    I8 => i8,
    I16 => i16,
    I24 => cpal::I24,
    I32 => i32,
    I64 => i64,
    U8 => u8,
    U16 => u16,
    U24 => cpal::U24,
    U32 => u32,
    U64 => u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_common_layouts() {
        assert_eq!(
            describe(48_000, 2, SampleFormat::F32),
            "48 kHz, stereo, f32"
        );
        assert_eq!(describe(44_100, 6, SampleFormat::I16), "44.1 kHz, 5.1, i16");
    }

    #[test]
    fn engine_starts_and_stops_without_devices() {
        let engine = Engine::new();
        assert!(!engine.status().running);
        engine.apply(EngineConfig {
            source: "wasapi:missing".into(),
            outputs: vec![OutputSpec {
                id: "wasapi:absent".into(),
                gain: 0.5,
                muted: false,
            }],
            latency_ms: 40,
        });
        std::thread::sleep(Duration::from_millis(600));
        let st = engine.status();
        assert!(st.running);
        assert_ne!(st.source.state, NodeState::Playing);
        assert_eq!(st.outputs.len(), 1);

        engine.set_gain("wasapi:absent", 0.1);
        assert_eq!(engine.shared.lock()["wasapi:absent"].gain.load(), 0.1);

        engine.apply(EngineConfig {
            source: "wasapi:missing".into(),
            outputs: vec![],
            latency_ms: 40,
        });
        std::thread::sleep(Duration::from_millis(400));
        assert!(!engine.status().running);
    }

    /// Needs real audio hardware: `cargo test -- --ignored`. The output is
    /// muted, so nothing is heard.
    #[test]
    #[ignore]
    fn mirrors_desktop_audio_on_real_devices() {
        let host = cpal::default_host();
        let list = devices::enumerate(&host).expect("device list");
        let output = list
            .outputs
            .iter()
            .find(|o| !o.is_default)
            .expect("a second output device");

        let engine = Engine::new();
        engine.apply(EngineConfig {
            source: devices::DESKTOP.into(),
            outputs: vec![OutputSpec {
                id: output.id.clone(),
                gain: 1.0,
                muted: true,
            }],
            latency_ms: 40,
        });
        std::thread::sleep(Duration::from_secs(3));
        let st = engine.status();
        println!("{st:#?}");
        assert_eq!(st.source.state, NodeState::Playing);
        assert!(matches!(
            st.outputs[0].state,
            NodeState::Playing | NodeState::Buffering
        ));
    }
}
