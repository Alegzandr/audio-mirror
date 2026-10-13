//! Linux backend, on PulseAudio's C API (PipeWire serves it through
//! pipewire-pulse).
//!
//! - Capture: port of `plugins/linux-pulseaudio/pulse-input.c`: desktop audio
//!   is the default sink's `.monitor`, reopened when the default sink
//!   changes; 25 ms fragments; the first 500 ms are dropped.
//! - Monitoring: port of `libobs/audio-monitoring/pulse/pulseaudio-output.c`:
//!   a corked stream with a 25 ms target, uncorked once that much audio is
//!   queued, written from the capture thread, with the Pulse buffer grown
//!   when audio piles up. Two deviations, both for a mirror that runs for
//!   days: the stream is pinned to its sink (`PA_STREAM_DONT_MOVE`), so a
//!   sink that goes away fails the stream the supervisor watches instead of
//!   quietly taking this output's audio to another device, and the backlog
//!   is capped, so a device that cannot keep up loses audio rather than
//!   gaining latency without end. The resampler also follows the sink's
//!   clock (`drift.rs`), read from the stream latency, so that cap is only
//!   reached when a device stalls.
//!
//! Each side owns its threaded mainloop and context, like the two OBS
//! wrappers (`pulse-wrapper.c` and `pulseaudio-wrapper.c`). One more
//! deviation: OBS connects its contexts once, so a sound server that
//! restarts (or is not up yet when the app autostarts) leaves them failed
//! until OBS is relaunched. Here a context that is no longer good is
//! replaced on the next request, captures and monitors keep the loop their
//! stream was made on, and the watcher reconnects on its own thread.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use libpulse_sys::*;
use parking_lot::Mutex;

use super::drift::DriftControl;
use super::format::{
    AudioSpec, ObsAudio, SampleFormat, SourceAudio, Speakers, OBS_SAMPLE_RATE, OBS_SPEAKERS,
};
use super::hub::{AudioCallback, SourceHub};
use super::message::{self as msg, Message};
use super::swr::Resampler;
use super::volume::{scale_s16, scale_s32, scale_u8, OutputShared};
use super::{
    describe, Capture, CaptureState, DeviceList, Monitor, MonitorInit, MonitorState, OutputInfo,
    SourceInfo, SourceKind, SystemEvent, DESKTOP, DESKTOP_NAME,
};

const OUTPUT_PREFIX: &str = "output:";
const INPUT_PREFIX: &str = "input:";
const MONITOR_SUFFIX: &str = ".monitor";

/// 25 ms, the `fragsize` of the capture and `tlength` of the monitor.
const BUFFER_USEC: u64 = 25_000;
/// `STARTUP_TIMEOUT_NS`.
const STARTUP_TIMEOUT_NS: u64 = 500_000_000;
/// Largest backlog a monitor keeps. Past this the device is not draining
/// what the source produces (two clocks apart, or a device gone to sleep),
/// and keeping the audio would only raise the latency for good.
const MAX_BACKLOG_USEC: u64 = 400_000;
/// How often the watcher checks its connection, and retries a lost one:
/// the retry delay of the supervisor.
const WATCH_RETRY: Duration = Duration::from_secs(3);

pub fn init_thread() {}

/// Follows the default sink, which OBS resolves only once: the desktop
/// capture is reopened on the new sink's monitor, like `win-wasapi` does.
///
/// The subscription lives on its own thread, which connects again when the
/// server goes away or was not up yet, and then reports the devices as
/// changed so the supervisor retries what failed in the meantime.
pub fn watch_system(
    callback: Box<dyn Fn(SystemEvent) + Send + Sync>,
) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    let data = Arc::new(WatchData {
        callback,
        default_sink: Mutex::new(None),
    });
    let (stop, stopped) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("pulse-watcher".into())
        .spawn(move || watch_loop(&data, &stopped))
        .map_err(|e| log::error!("failed to start the PulseAudio watcher: {e}"))
        .ok()?;
    Some(Box::new(Watcher {
        stop: Some(stop),
        thread: Some(thread),
    }))
}

/// Waits one retry delay. True when the watcher was dropped meanwhile.
fn watch_stopped(stopped: &mpsc::Receiver<()>) -> bool {
    !matches!(
        stopped.recv_timeout(WATCH_RETRY),
        Err(RecvTimeoutError::Timeout)
    )
}

fn watch_loop(data: &Arc<WatchData>, stopped: &mpsc::Receiver<()>) {
    let mut lost = false;
    loop {
        if let Some(subscription) = Subscription::new(data) {
            if lost {
                log::info!("PulseAudio watcher connected again");
                (data.callback)(SystemEvent::DevicesChanged);
            }
            while subscription.pulse.is_good() {
                if watch_stopped(stopped) {
                    return;
                }
            }
            log::warn!("PulseAudio watcher lost its connection");
        }
        lost = true;
        if watch_stopped(stopped) {
            return;
        }
    }
}

struct WatchData {
    callback: Box<dyn Fn(SystemEvent) + Send + Sync>,
    /// `None` until the first connection.
    default_sink: Mutex<Option<String>>,
}

impl WatchData {
    /// Records the default sink, reporting a change from a known one.
    fn default_sink(&self, sink: String) {
        let previous = self.default_sink.lock().replace(sink.clone());
        if previous.is_some_and(|p| p != sink) {
            (self.callback)(SystemEvent::DefaultOutputChanged);
        }
    }
}

/// What [`watch_system`] hands back: dropping it stops the watcher thread,
/// which unsubscribes.
struct Watcher {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One connection of the watcher, subscribed to server, sink and source
/// events.
struct Subscription {
    pulse: PulseLoop,
}

impl Subscription {
    /// `data` outlives the subscription: the watcher thread holds it.
    fn new(data: &Arc<WatchData>) -> Option<Subscription> {
        let pulse = PulseLoop::new(c"Audio Mirror Watcher");
        let defaults = pulse.server_info()?;
        data.default_sink(defaults.default_sink);

        let userdata = Arc::as_ptr(data) as *mut c_void;
        let guard = pulse.lock();
        // SAFETY: context used under the lock; `data` outlives the
        // subscription, whose callback `Subscription::drop` removes first.
        unsafe {
            let context = pulse.context;
            pa_context_set_subscribe_callback(context, Some(watch_event), userdata);
            let op = pa_context_subscribe(
                context,
                PA_SUBSCRIPTION_MASK_SERVER
                    | PA_SUBSCRIPTION_MASK_SINK
                    | PA_SUBSCRIPTION_MASK_SOURCE,
                None,
                ptr::null_mut(),
            );
            if !op.is_null() {
                pa_operation_unref(op);
            }
        }
        drop(guard);
        Some(Subscription { pulse })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _g = self.pulse.lock();
        // SAFETY: context used under the lock, then released by `PulseLoop`.
        unsafe { pa_context_set_subscribe_callback(self.pulse.context, None, ptr::null_mut()) };
    }
}

extern "C" fn watch_event(
    c: *mut pa_context,
    t: pa_subscription_event_type_t,
    _idx: u32,
    userdata: *mut c_void,
) {
    // SAFETY: userdata is the watcher's `WatchData`.
    let data = unsafe { &*(userdata as *const WatchData) };
    let kind = t & PA_SUBSCRIPTION_EVENT_TYPE_MASK;
    match t & PA_SUBSCRIPTION_EVENT_FACILITY_MASK {
        PA_SUBSCRIPTION_EVENT_SERVER => {
            // Runs on the loop thread: ask without waiting for the answer.
            // SAFETY: called by the mainloop with its lock held.
            unsafe {
                let op = pa_context_get_server_info(c, Some(watch_server_info), userdata);
                if !op.is_null() {
                    pa_operation_unref(op);
                }
            }
        }
        PA_SUBSCRIPTION_EVENT_SINK | PA_SUBSCRIPTION_EVENT_SOURCE
            if kind != PA_SUBSCRIPTION_EVENT_CHANGE =>
        {
            (data.callback)(SystemEvent::DevicesChanged);
        }
        _ => {}
    }
}

extern "C" fn watch_server_info(
    _c: *mut pa_context,
    i: *const pa_server_info,
    userdata: *mut c_void,
) {
    if i.is_null() {
        return;
    }
    // SAFETY: userdata is the watcher's `WatchData`, `i` is valid here.
    let (data, sink) = unsafe {
        (
            &*(userdata as *const WatchData),
            cstr((*i).default_sink_name),
        )
    };
    data.default_sink(sink);
}

fn os_gettime_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: PulseAudio hands out NUL terminated strings.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

// ---------------------------------------------------------------------------
// Wrapper: one threaded mainloop and context
// ---------------------------------------------------------------------------

struct PulseLoop {
    mainloop: *mut pa_threaded_mainloop,
    context: *mut pa_context,
    /// Held around every wait on the loop, so one request at a time is in
    /// flight on this context. `pa_threaded_mainloop_signal` wakes every
    /// waiter and each one rechecks its own condition, so this does not
    /// fix a lost wakeup, it makes sure there can be none: the enumeration
    /// (a blocking task of the panel) and the supervisor share the capture
    /// loop. Never taken on the loop thread, and never while holding the
    /// mainloop lock, so callbacks cannot deadlock on it.
    requests: Mutex<()>,
}

// SAFETY: every use goes through the threaded mainloop lock, as in OBS.
unsafe impl Send for PulseLoop {}
unsafe impl Sync for PulseLoop {}

struct LoopGuard<'a>(&'a PulseLoop);

impl Drop for LoopGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: locked in `PulseLoop::lock`.
        unsafe { pa_threaded_mainloop_unlock(self.0.mainloop) };
    }
}

extern "C" fn context_state_changed(_c: *mut pa_context, userdata: *mut c_void) {
    // SAFETY: userdata is the mainloop pointer.
    unsafe { pa_threaded_mainloop_signal(userdata as *mut pa_threaded_mainloop, 0) };
}

impl PulseLoop {
    /// `pulse_init` / `pulseaudio_init`.
    fn new(context_name: &CStr) -> PulseLoop {
        // SAFETY: straight port of the OBS wrapper initialization.
        unsafe {
            let mainloop = pa_threaded_mainloop_new();
            pa_threaded_mainloop_start(mainloop);

            pa_threaded_mainloop_lock(mainloop);
            let props = pa_proplist_new();
            pa_proplist_sets(
                props,
                c"application.name".as_ptr(),
                c"Audio Mirror".as_ptr(),
            );
            pa_proplist_sets(
                props,
                c"application.icon_name".as_ptr(),
                c"audio-mirror".as_ptr(),
            );
            pa_proplist_sets(props, c"media.role".as_ptr(), c"production".as_ptr());

            let context = pa_context_new_with_proplist(
                pa_threaded_mainloop_get_api(mainloop),
                context_name.as_ptr(),
                props,
            );
            pa_context_set_state_callback(
                context,
                Some(context_state_changed),
                mainloop as *mut c_void,
            );
            pa_context_connect(context, ptr::null(), PA_CONTEXT_NOAUTOSPAWN, ptr::null());
            pa_proplist_free(props);
            pa_threaded_mainloop_unlock(mainloop);

            PulseLoop {
                mainloop,
                context,
                requests: Mutex::new(()),
            }
        }
    }

    /// False once the context failed or was terminated, which it never
    /// comes back from: the server went away, or was not there to begin
    /// with.
    fn is_good(&self) -> bool {
        let _g = self.lock();
        // SAFETY: context used under the lock.
        unsafe { pa_context_is_good(pa_context_get_state(self.context)) }
    }

    fn lock(&self) -> LoopGuard<'_> {
        // SAFETY: valid mainloop; never called from the loop thread itself.
        unsafe { pa_threaded_mainloop_lock(self.mainloop) };
        LoopGuard(self)
    }

    fn wait(&self) {
        // SAFETY: called with the lock held.
        unsafe { pa_threaded_mainloop_wait(self.mainloop) };
    }

    fn signal(&self) {
        // SAFETY: valid mainloop.
        unsafe { pa_threaded_mainloop_signal(self.mainloop, 0) };
    }

    /// `pulse_context_ready`.
    fn context_ready(&self) -> bool {
        let _r = self.requests.lock();
        self.wait_ready()
    }

    /// [`Self::context_ready`] for a caller that holds `requests`.
    fn wait_ready(&self) -> bool {
        let _g = self.lock();
        // SAFETY: context used under the lock.
        unsafe {
            if !pa_context_is_good(pa_context_get_state(self.context)) {
                return false;
            }
            while pa_context_get_state(self.context) != PA_CONTEXT_READY {
                self.wait();
                if !pa_context_is_good(pa_context_get_state(self.context)) {
                    return false;
                }
            }
        }
        true
    }

    /// Runs an operation to completion, like the `pulse_get_*` helpers.
    fn run(&self, start: impl FnOnce(*mut pa_context) -> *mut pa_operation) -> bool {
        let _r = self.requests.lock();
        if !self.wait_ready() {
            return false;
        }
        let _g = self.lock();
        let op = start(self.context);
        if op.is_null() {
            return false;
        }
        // SAFETY: operation used under the lock and released once.
        unsafe {
            while pa_operation_get_state(op) == PA_OPERATION_RUNNING {
                self.wait();
            }
            pa_operation_unref(op);
        }
        true
    }

    fn server_info(&self) -> Option<ServerDefaults> {
        let mut out = CallbackData::<ServerDefaults> {
            pulse: self,
            value: None,
        };
        let ok = self.run(|c| unsafe {
            pa_context_get_server_info(c, Some(server_info_cb), &mut out as *mut _ as *mut c_void)
        });
        if ok {
            out.value
        } else {
            None
        }
    }

    fn sink_info(&self, name: &str) -> Option<StreamInfo> {
        let name = CString::new(name).ok()?;
        let mut out = CallbackData::<StreamInfo> {
            pulse: self,
            value: None,
        };
        let ok = self.run(|c| unsafe {
            pa_context_get_sink_info_by_name(
                c,
                name.as_ptr(),
                Some(sink_info_cb),
                &mut out as *mut _ as *mut c_void,
            )
        });
        if ok {
            out.value
        } else {
            None
        }
    }

    fn source_info(&self, name: &str) -> Option<StreamInfo> {
        let name = CString::new(name).ok()?;
        let mut out = CallbackData::<StreamInfo> {
            pulse: self,
            value: None,
        };
        let ok = self.run(|c| unsafe {
            pa_context_get_source_info_by_name(
                c,
                name.as_ptr(),
                Some(source_info_cb),
                &mut out as *mut _ as *mut c_void,
            )
        });
        if ok {
            out.value
        } else {
            None
        }
    }

    fn sink_list(&self) -> Vec<DeviceEntry> {
        let mut out = CallbackData::<Vec<DeviceEntry>> {
            pulse: self,
            value: Some(Vec::new()),
        };
        self.run(|c| unsafe {
            pa_context_get_sink_info_list(c, Some(sink_list_cb), &mut out as *mut _ as *mut c_void)
        });
        out.value.unwrap_or_default()
    }

    fn source_list(&self) -> Vec<DeviceEntry> {
        let mut out = CallbackData::<Vec<DeviceEntry>> {
            pulse: self,
            value: Some(Vec::new()),
        };
        self.run(|c| unsafe {
            pa_context_get_source_info_list(
                c,
                Some(source_list_cb),
                &mut out as *mut _ as *mut c_void,
            )
        });
        out.value.unwrap_or_default()
    }

    /// `pulse_stream_new`.
    fn stream_new(
        &self,
        name: &CStr,
        spec: &pa_sample_spec,
        map: &pa_channel_map,
    ) -> *mut pa_stream {
        let _r = self.requests.lock();
        if !self.wait_ready() {
            return ptr::null_mut();
        }
        let _g = self.lock();
        // SAFETY: context used under the lock.
        unsafe {
            let props = pa_proplist_new();
            let s = pa_stream_new_with_proplist(self.context, name.as_ptr(), spec, map, props);
            pa_proplist_free(props);
            s
        }
    }
}

impl Drop for PulseLoop {
    /// `pulse_unref` / `pulseaudio_unref`, once the last stream made on
    /// this loop is gone.
    fn drop(&mut self) {
        // SAFETY: nothing else holds the loop any more.
        unsafe {
            pa_threaded_mainloop_lock(self.mainloop);
            pa_context_set_state_callback(self.context, None, ptr::null_mut());
            pa_context_disconnect(self.context);
            pa_context_unref(self.context);
            pa_threaded_mainloop_unlock(self.mainloop);
            pa_threaded_mainloop_stop(self.mainloop);
            pa_threaded_mainloop_free(self.mainloop);
        }
    }
}

/// The loop in `slot`, replaced by a new connection when its context is
/// no longer good. The streams of the old one keep it until they close.
fn shared_loop(slot: &Mutex<Option<Arc<PulseLoop>>>, name: &CStr) -> Arc<PulseLoop> {
    let mut slot = slot.lock();
    if let Some(pulse) = slot.as_ref().filter(|p| p.is_good()) {
        return pulse.clone();
    }
    if slot.take().is_some() {
        log::warn!("PulseAudio connection lost, connecting again");
    }
    let pulse = Arc::new(PulseLoop::new(name));
    *slot = Some(pulse.clone());
    pulse
}

/// Capture side, `pulse-wrapper.c`.
fn capture_loop() -> Arc<PulseLoop> {
    static LOOP: Mutex<Option<Arc<PulseLoop>>> = Mutex::new(None);
    shared_loop(&LOOP, c"Audio Mirror")
}

/// Monitoring side, `pulseaudio-wrapper.c`.
fn monitor_loop() -> Arc<PulseLoop> {
    static LOOP: Mutex<Option<Arc<PulseLoop>>> = Mutex::new(None);
    shared_loop(&LOOP, c"Audio Mirror Monitor")
}

struct CallbackData<'a, T> {
    pulse: &'a PulseLoop,
    value: Option<T>,
}

struct ServerDefaults {
    default_sink: String,
    default_source: String,
}

#[derive(Clone, Copy)]
struct StreamInfo {
    format: pa_sample_format_t,
    rate: u32,
    channels: u8,
}

struct DeviceEntry {
    name: String,
    description: String,
    /// For sources: whether it monitors a sink. For sinks: its monitor.
    monitor: Option<String>,
}

extern "C" fn server_info_cb(_c: *mut pa_context, i: *const pa_server_info, userdata: *mut c_void) {
    // SAFETY: userdata points at a live `CallbackData<ServerDefaults>`.
    unsafe {
        let data = &mut *(userdata as *mut CallbackData<ServerDefaults>);
        if !i.is_null() {
            data.value = Some(ServerDefaults {
                default_sink: cstr((*i).default_sink_name),
                default_source: cstr((*i).default_source_name),
            });
        }
        data.pulse.signal();
    }
}

fn obs_format(format: pa_sample_format_t) -> Option<SampleFormat> {
    match format {
        PA_SAMPLE_U8 => Some(SampleFormat::U8),
        PA_SAMPLE_S16LE => Some(SampleFormat::S16),
        PA_SAMPLE_S32LE => Some(SampleFormat::S32),
        PA_SAMPLE_FLOAT32LE => Some(SampleFormat::Float),
        _ => None,
    }
}

/// The format checks of `pulse_source_info` / `pulseaudio_sink_info`.
fn stream_info(spec: &pa_sample_spec) -> StreamInfo {
    let mut format = spec.format;
    if obs_format(format).is_none() {
        log::info!("Sample format not supported, using float32le instead");
        format = PA_SAMPLE_FLOAT32LE;
    }
    let mut channels = spec.channels;
    if Speakers::from_channels(channels as usize) == Speakers::Unknown {
        log::info!("{channels} channels not supported, using 2 instead");
        channels = 2;
    }
    StreamInfo {
        format,
        rate: spec.rate,
        channels,
    }
}

extern "C" fn sink_info_cb(
    _c: *mut pa_context,
    i: *const pa_sink_info,
    eol: i32,
    userdata: *mut c_void,
) {
    // SAFETY: userdata points at a live `CallbackData<StreamInfo>`.
    unsafe {
        let data = &mut *(userdata as *mut CallbackData<StreamInfo>);
        if eol == 0 && !i.is_null() {
            data.value = Some(stream_info(&(*i).sample_spec));
        }
        data.pulse.signal();
    }
}

extern "C" fn source_info_cb(
    _c: *mut pa_context,
    i: *const pa_source_info,
    eol: i32,
    userdata: *mut c_void,
) {
    // SAFETY: userdata points at a live `CallbackData<StreamInfo>`.
    unsafe {
        let data = &mut *(userdata as *mut CallbackData<StreamInfo>);
        if eol == 0 && !i.is_null() {
            data.value = Some(stream_info(&(*i).sample_spec));
        }
        data.pulse.signal();
    }
}

extern "C" fn sink_list_cb(
    _c: *mut pa_context,
    i: *const pa_sink_info,
    eol: i32,
    userdata: *mut c_void,
) {
    // SAFETY: userdata points at a live `CallbackData<Vec<DeviceEntry>>`.
    unsafe {
        let data = &mut *(userdata as *mut CallbackData<Vec<DeviceEntry>>);
        if eol == 0 && !i.is_null() {
            let i = &*i;
            if let Some(list) = data.value.as_mut() {
                list.push(DeviceEntry {
                    name: cstr(i.name),
                    description: cstr(i.description),
                    monitor: (i.monitor_source != PA_INVALID_INDEX)
                        .then(|| cstr(i.monitor_source_name)),
                });
            }
        }
        data.pulse.signal();
    }
}

extern "C" fn source_list_cb(
    _c: *mut pa_context,
    i: *const pa_source_info,
    eol: i32,
    userdata: *mut c_void,
) {
    // SAFETY: userdata points at a live `CallbackData<Vec<DeviceEntry>>`.
    unsafe {
        let data = &mut *(userdata as *mut CallbackData<Vec<DeviceEntry>>);
        if eol == 0 && !i.is_null() {
            let i = &*i;
            if let Some(list) = data.value.as_mut() {
                list.push(DeviceEntry {
                    name: cstr(i.name),
                    description: cstr(i.description),
                    monitor: (i.monitor_of_sink != PA_INVALID_INDEX)
                        .then(|| cstr(i.monitor_of_sink_name)),
                });
            }
        }
        data.pulse.signal();
    }
}

/// `pulse_channel_map` / `pulseaudio_channel_map`.
fn channel_map(speakers: Speakers) -> pa_channel_map {
    let mut map = pa_channel_map::default();
    map.map[0] = PA_CHANNEL_POSITION_FRONT_LEFT;
    map.map[1] = PA_CHANNEL_POSITION_FRONT_RIGHT;
    map.map[2] = PA_CHANNEL_POSITION_FRONT_CENTER;
    map.map[3] = PA_CHANNEL_POSITION_LFE;
    map.map[4] = PA_CHANNEL_POSITION_REAR_LEFT;
    map.map[5] = PA_CHANNEL_POSITION_REAR_RIGHT;
    map.map[6] = PA_CHANNEL_POSITION_SIDE_LEFT;
    map.map[7] = PA_CHANNEL_POSITION_SIDE_RIGHT;
    match speakers {
        Speakers::Mono => {
            map.channels = 1;
            map.map[0] = PA_CHANNEL_POSITION_MONO;
        }
        Speakers::Stereo => map.channels = 2,
        Speakers::TwoPointOne => {
            map.channels = 3;
            map.map[2] = PA_CHANNEL_POSITION_LFE;
        }
        Speakers::FourPointZero => {
            map.channels = 4;
            map.map[3] = PA_CHANNEL_POSITION_REAR_CENTER;
        }
        Speakers::FourPointOne => {
            map.channels = 5;
            map.map[4] = PA_CHANNEL_POSITION_REAR_CENTER;
        }
        Speakers::FivePointOne => map.channels = 6,
        Speakers::SevenPointOne => map.channels = 8,
        Speakers::Unknown => map.channels = 0,
    }
    map
}

fn sample_spec(info: &StreamInfo) -> pa_sample_spec {
    pa_sample_spec {
        format: info.format,
        rate: info.rate,
        channels: info.channels,
    }
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

pub fn enumerate() -> Result<DeviceList, Message> {
    let pulse = capture_loop();
    let defaults = pulse.server_info().ok_or(msg::PULSE_UNAVAILABLE)?;
    let desktop_monitor = format!("{}{MONITOR_SUFFIX}", defaults.default_sink);

    let mut list = DeviceList::default();
    list.sources.push(SourceInfo {
        id: DESKTOP.into(),
        name: DESKTOP_NAME.into(),
        kind: SourceKind::Desktop,
        is_default: false,
        captures_output: Some(defaults.default_sink.clone()),
    });

    for sink in pulse.sink_list() {
        let is_default = sink.name == defaults.default_sink;
        if let Some(monitor) = &sink.monitor {
            list.sources.push(SourceInfo {
                id: format!("{OUTPUT_PREFIX}{monitor}"),
                name: sink.description.clone(),
                kind: SourceKind::Loopback,
                is_default: *monitor == desktop_monitor,
                captures_output: Some(sink.name.clone()),
            });
        }
        list.outputs.push(OutputInfo {
            id: sink.name,
            name: sink.description,
            is_default,
            description: None,
        });
    }
    for source in pulse.source_list() {
        if source.monitor.is_some() {
            continue;
        }
        list.sources.push(SourceInfo {
            is_default: source.name == defaults.default_source,
            id: format!("{INPUT_PREFIX}{}", source.name),
            name: source.description,
            kind: SourceKind::Capture,
            captures_output: None,
        });
    }
    Ok(list)
}

/// Pulse source name recorded by a source id, and whether that source is
/// an output capture (`OBS_SOURCE_DO_NOT_SELF_MONITOR`).
fn resolve_source(source: &str, pulse: &PulseLoop) -> Result<(String, bool), Message> {
    if source == DESKTOP {
        let defaults = pulse.server_info().ok_or(msg::SERVER_INFO)?;
        Ok((format!("{}{MONITOR_SUFFIX}", defaults.default_sink), true))
    } else if let Some(name) = source.strip_prefix(OUTPUT_PREFIX) {
        Ok((name.to_string(), true))
    } else if let Some(name) = source.strip_prefix(INPUT_PREFIX) {
        Ok((name.to_string(), false))
    } else {
        Err(msg::UNKNOWN_SOURCE.detail(source))
    }
}

/// A device name on its way to the C API. Names reach us from the settings
/// file, so a malformed one is an error the panel can show, never a panic on
/// the thread that owns every device.
fn device_name(name: &str) -> Result<CString, Message> {
    CString::new(name).map_err(|_| msg::INVALID_DEVICE_NAME.detail(name))
}

/// `devices_match` of the Pulse monitoring backend.
fn devices_match(source_name: &str, sink: &str) -> bool {
    source_name == format!("{sink}{MONITOR_SUFFIX}")
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

struct CaptureData {
    /// The loop the stream was made on.
    pulse: Arc<PulseLoop>,
    hub: Arc<SourceHub>,
    stream: *mut pa_stream,
    spec: AudioSpec,
    bytes_per_frame: usize,
    /// Only touched on the mainloop thread; atomic because the supervisor
    /// reads `state` through a shared reference at the same time, so the
    /// callback cannot hold a `&mut` to the whole struct.
    first_ts: AtomicU64,
    state: Mutex<CaptureState>,
}

pub struct PulseCapture {
    data: Box<CaptureData>,
    is_default: bool,
}

// SAFETY: the stream is only touched under the capture mainloop lock.
unsafe impl Send for PulseCapture {}

/// `pulse_stream_read`.
extern "C" fn stream_read(_p: *mut pa_stream, _nbytes: usize, userdata: *mut c_void) {
    // SAFETY: userdata is the boxed `CaptureData`, alive until disconnect.
    unsafe {
        let data = &*(userdata as *const CaptureData);
        if data.stream.is_null() {
            data.pulse.signal();
            return;
        }

        let mut frames: *const c_void = ptr::null();
        let mut bytes = 0usize;
        pa_stream_peek(data.stream, &mut frames, &mut bytes);

        if bytes == 0 {
            data.pulse.signal();
            return;
        }
        if frames.is_null() {
            log::error!("Got audio hole of {bytes} bytes");
            pa_stream_drop(data.stream);
            data.pulse.signal();
            return;
        }

        let count = (bytes / data.bytes_per_frame) as u32;
        let timestamp =
            os_gettime_ns().saturating_sub(count as u64 * 1_000_000_000 / data.spec.rate as u64);
        if data.first_ts.load(Ordering::Relaxed) == 0 {
            data.first_ts
                .store(timestamp + STARTUP_TIMEOUT_NS, Ordering::Relaxed);
        }
        if timestamp > data.first_ts.load(Ordering::Relaxed) {
            let slice = std::slice::from_raw_parts(frames as *const u8, bytes);
            data.hub.output_audio(&SourceAudio {
                planes: &[slice],
                frames: count,
                spec: data.spec,
            });
        }

        pa_stream_drop(data.stream);
        data.pulse.signal();
    }
}

extern "C" fn capture_state_changed(s: *mut pa_stream, userdata: *mut c_void) {
    // SAFETY: userdata is the boxed `CaptureData`.
    unsafe {
        let data = &*(userdata as *const CaptureData);
        let state = pa_stream_get_state(s);
        if state == PA_STREAM_READY {
            *data.state.lock() = CaptureState::Active {
                format: describe(data.spec.rate, data.spec.speakers.channels()),
            };
        } else if state == PA_STREAM_FAILED || state == PA_STREAM_TERMINATED {
            *data.state.lock() = CaptureState::Failed(msg::DEVICE_DISCONNECTED.into());
        }
        data.pulse.signal();
    }
}

/// `pulse_start_recording`.
pub fn start_capture(source: &str, hub: Arc<SourceHub>) -> Result<Box<dyn Capture>, Message> {
    let pulse = capture_loop();
    let (device, is_output) = resolve_source(source, &pulse)?;
    let device_c = device_name(&device)?;
    let is_default = source == DESKTOP;

    let info = pulse.source_info(&device).ok_or(msg::SOURCE_INFO)?;
    let spec = sample_spec(&info);
    // SAFETY: plain value check.
    if unsafe { pa_sample_spec_valid(&spec) } == 0 {
        return Err(msg::SAMPLE_SPEC.into());
    }
    let speakers = Speakers::from_channels(info.channels as usize);
    let map = channel_map(speakers);

    let mut data = Box::new(CaptureData {
        pulse: pulse.clone(),
        hub,
        stream: ptr::null_mut(),
        spec: AudioSpec {
            rate: info.rate,
            speakers,
            format: obs_format(info.format).unwrap_or(SampleFormat::Float),
        },
        // SAFETY: valid spec.
        bytes_per_frame: unsafe { pa_frame_size(&spec) },
        first_ts: AtomicU64::new(0),
        state: Mutex::new(CaptureState::Starting),
    });

    let stream = pulse.stream_new(
        if is_output {
            c"Desktop Audio"
        } else {
            c"Audio Input"
        },
        &spec,
        &map,
    );
    if stream.is_null() {
        return Err(msg::STREAM_CREATE.into());
    }
    data.stream = stream;
    let userdata = &mut *data as *mut CaptureData as *mut c_void;

    // SAFETY: stream set up under the lock, as in OBS.
    unsafe {
        let _g = pulse.lock();
        pa_stream_set_read_callback(stream, Some(stream_read), userdata);
        pa_stream_set_state_callback(stream, Some(capture_state_changed), userdata);

        let attr = pa_buffer_attr {
            fragsize: pa_usec_to_bytes(BUFFER_USEC, &spec) as u32,
            maxlength: u32::MAX,
            minreq: u32::MAX,
            prebuf: u32::MAX,
            tlength: u32::MAX,
        };
        let mut flags = PA_STREAM_ADJUST_LATENCY;
        if !is_default {
            flags |= PA_STREAM_DONT_MOVE;
        }
        if pa_stream_connect_record(stream, device_c.as_ptr(), &attr, flags) < 0 {
            pa_stream_set_read_callback(stream, None, ptr::null_mut());
            pa_stream_set_state_callback(stream, None, ptr::null_mut());
            pa_stream_unref(stream);
            return Err(msg::STREAM_CONNECT.into());
        }
    }

    log::info!("Started recording from '{device}'");
    Ok(Box::new(PulseCapture { data, is_default }))
}

impl Capture for PulseCapture {
    fn state(&self) -> CaptureState {
        self.data.state.lock().clone()
    }

    /// The desktop capture records the old sink's monitor: reopen it.
    fn default_output_changed(&self) -> bool {
        self.is_default
    }
}

impl Drop for PulseCapture {
    /// `pulse_stop_recording`.
    fn drop(&mut self) {
        let stream = self.data.stream;
        {
            let _g = self.data.pulse.lock();
            // SAFETY: stream owned by this capture, released under the lock.
            unsafe {
                pa_stream_set_read_callback(stream, None, ptr::null_mut());
                pa_stream_set_state_callback(stream, None, ptr::null_mut());
                pa_stream_disconnect(stream);
                pa_stream_unref(stream);
            }
        }
        self.data.stream = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Monitoring
// ---------------------------------------------------------------------------

struct MonitorStream {
    stream: *mut pa_stream,
    attr: pa_buffer_attr,
    format: pa_sample_format_t,
    bytes_per_frame: usize,
    new_data: VecDeque<u8>,
    /// Cap on `new_data`, from [`MAX_BACKLOG_USEC`].
    max_backlog: usize,
    rate: u32,
    drift: DriftControl,
    resampler: Resampler,
}

// SAFETY: the stream is only touched under the monitor mainloop lock.
unsafe impl Send for MonitorStream {}

/// The playback stream, kept aside from `playback` so its state can be read
/// without waiting for the capture thread to finish a write. Set once at
/// creation and never changed.
struct StreamHandle(*mut pa_stream);

// SAFETY: only dereferenced under the monitor mainloop lock.
unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

pub struct PulseMonitor {
    /// The loop the stream was made on.
    pulse: Arc<PulseLoop>,
    device: String,
    shared: Arc<OutputShared>,
    /// `playback_mutex`.
    playback: Mutex<MonitorStream>,
    handle: StreamHandle,
    format: String,
    /// Frames dropped: the write lock was held, or the backlog was cut back.
    /// Logged on powers of two, like the skipped packets of `wasapi.rs`.
    dropped: AtomicU64,
}

/// `audio_monitor_init`.
pub fn create_monitor(
    source: &str,
    device: &str,
    shared: Arc<OutputShared>,
) -> Result<MonitorInit, Message> {
    if let Ok((source_name, true)) = resolve_source(source, &capture_loop()) {
        if devices_match(&source_name, device) {
            return Ok(MonitorInit::Ignored);
        }
    }

    let device_c = device_name(device)?;
    let pulse = monitor_loop();
    pulse.server_info().ok_or(msg::SERVER_INFO)?;
    let info = pulse.sink_info(device).ok_or(msg::SOURCE_INFO)?;
    let spec = sample_spec(&info);
    // SAFETY: plain value check.
    if unsafe { pa_sample_spec_valid(&spec) } == 0 {
        return Err(msg::SAMPLE_SPEC.into());
    }

    let speakers = Speakers::from_channels(info.channels as usize);
    let from = AudioSpec {
        rate: OBS_SAMPLE_RATE,
        speakers: OBS_SPEAKERS,
        format: SampleFormat::FloatPlanar,
    };
    let to = AudioSpec {
        rate: info.rate,
        speakers,
        format: obs_format(info.format).unwrap_or(SampleFormat::Float),
    };
    let resampler = Resampler::adjustable(to, from).ok_or(msg::RESAMPLER)?;

    let map = channel_map(speakers);
    let stream = pulse.stream_new(c"Audio Mirror", &spec, &map);
    if stream.is_null() {
        return Err(msg::STREAM_CREATE.into());
    }

    // SAFETY: spec is valid.
    let attr = pa_buffer_attr {
        fragsize: u32::MAX,
        maxlength: u32::MAX,
        minreq: u32::MAX,
        prebuf: u32::MAX,
        tlength: unsafe { pa_usec_to_bytes(BUFFER_USEC, &spec) } as u32,
    };
    // `DONT_MOVE` is ours. Pulse moves a stream whose sink disappears to
    // the fallback sink and leaves it READY, which for a mirror means this
    // output's audio silently joining another one, twice on the same
    // device and with no way to tell. Pinned, the stream fails instead and
    // the supervisor rebuilds the monitor when the device comes back.
    let flags = PA_STREAM_INTERPOLATE_TIMING
        | PA_STREAM_AUTO_TIMING_UPDATE
        | PA_STREAM_START_CORKED
        | PA_STREAM_DONT_MOVE;

    if !pulse.context_ready() {
        return Err(msg::STREAM_CONNECT.into());
    }
    {
        let _g = pulse.lock();
        // SAFETY: stream connected under the lock.
        let ret = unsafe {
            pa_stream_connect_playback(
                stream,
                device_c.as_ptr(),
                &attr,
                flags,
                ptr::null(),
                ptr::null_mut(),
            )
        };
        if ret < 0 {
            // SAFETY: stream owned here.
            unsafe {
                pa_stream_disconnect(stream);
                pa_stream_unref(stream);
            }
            return Err(msg::STREAM_CONNECT.into());
        }
    }

    log::info!("Started Monitoring in '{device}'");
    Ok(MonitorInit::Active(Arc::new(PulseMonitor {
        pulse,
        device: device.to_string(),
        shared,
        handle: StreamHandle(stream),
        playback: Mutex::new(MonitorStream {
            stream,
            attr,
            format: info.format,
            // SAFETY: valid spec.
            bytes_per_frame: unsafe { pa_frame_size(&spec) },
            new_data: VecDeque::new(),
            // SAFETY: valid spec.
            max_backlog: unsafe { pa_usec_to_bytes(MAX_BACKLOG_USEC, &spec) },
            rate: info.rate,
            drift: DriftControl::new(info.rate),
            resampler,
        }),
        format: describe(info.rate, info.channels as usize),
        dropped: AtomicU64::new(0),
    })))
}

impl PulseMonitor {
    /// Counts dropped frames, logging on powers of two so a drift leaves a
    /// handful of lines rather than one per packet.
    fn note_drop(&self, frames: usize) {
        if frames == 0 {
            return;
        }
        let total = self.dropped.fetch_add(frames as u64, Ordering::Relaxed) + frames as u64;
        if total.is_power_of_two() {
            log::warn!("'{}': {total} frames dropped so far", self.device);
        }
    }

    /// Frames queued ahead of the sink before the last `written` frames:
    /// the backlog here plus the stream latency, which Pulse interpolates
    /// on the sink's clock. `None` until Pulse has timing for the stream.
    ///
    /// # Safety
    /// Called under the monitor mainloop lock.
    unsafe fn queued(data: &MonitorStream, written: u32) -> Option<f64> {
        let mut usec: pa_usec_t = 0;
        let mut negative: c_int = 0;
        // SAFETY: the caller holds the mainloop lock.
        if unsafe { pa_stream_get_latency(data.stream, &mut usec, &mut negative) } != 0 {
            return None;
        }
        let latency = usec as f64 * f64::from(data.rate) / 1e6;
        let latency = if negative != 0 { -latency } else { latency };
        let backlog = (data.new_data.len() / data.bytes_per_frame.max(1)) as f64;
        Some(backlog - f64::from(written) + latency)
    }

    /// `do_stream_write`. `written` is the number of frames the capture
    /// thread just added to the backlog.
    fn stream_write(&self, written: u32) {
        let _g = self.pulse.lock();
        let mut guard = self.playback.lock();
        let data = &mut *guard;

        // The device is not taking what the source produces: drop the
        // oldest audio rather than play further and further behind it.
        if data.new_data.len() > data.max_backlog {
            let excess = data.new_data.len() - data.max_backlog;
            data.new_data.drain(..excess);
            self.note_drop(excess / data.bytes_per_frame.max(1));
        }

        // SAFETY: stream used under the monitor mainloop lock.
        unsafe {
            // Until the sink answers, nothing can be written: keep only what
            // the first write takes rather than start the output up to
            // `max_backlog` late, a level the drift loop would then hold.
            // Pulse also refuses new buffer attributes on such a stream.
            if pa_stream_get_state(data.stream) != PA_STREAM_READY {
                let room = data.attr.tlength as usize;
                if data.new_data.len() > room {
                    let excess = data.new_data.len() - room;
                    let excess = excess - excess % data.bytes_per_frame.max(1);
                    data.new_data.drain(..excess);
                }
                return;
            }

            // Grow the Pulse buffer when a large backlog built up. It is
            // never shrunk back, exactly as in `do_stream_write`, so the
            // added latency stays until the monitor is built again. Worth a
            // line in the log, because nothing else says it happened.
            if data.new_data.len() > data.attr.tlength as usize * 2 {
                log::info!(
                    "'{}': buffer grown from {} to {} bytes, latency follows",
                    self.device,
                    data.attr.tlength,
                    data.new_data.len()
                );
                let attr = pa_buffer_attr {
                    fragsize: u32::MAX,
                    maxlength: u32::MAX,
                    prebuf: u32::MAX,
                    minreq: u32::MAX,
                    tlength: data.new_data.len().min(data.max_backlog) as u32,
                };
                let op = pa_stream_set_buffer_attr(data.stream, &attr, None, ptr::null_mut());
                // Kept only when Pulse took it, so the cork threshold below
                // never waits for a buffer the stream does not have.
                if !op.is_null() {
                    data.attr = attr;
                    pa_operation_unref(op);
                }
            }

            // Applied from the next packet on; see `drift.rs`.
            if written > 0 && pa_stream_is_corked(data.stream) == 0 {
                if let Some(queued) = Self::queued(data, written) {
                    let ppm = data.drift.update(queued, written);
                    data.resampler.set_drift(ppm);
                    if let Some((ppm, off)) = data.drift.report() {
                        log::info!(
                            "'{}': clock {ppm:+.1} ppm, level {off:+.1} ms from its mark",
                            self.device
                        );
                    }
                }
            }

            // Buffer up enough data before we start playing.
            if pa_stream_is_corked(data.stream) == 1 {
                if data.new_data.len() >= data.attr.tlength as usize {
                    let op = pa_stream_cork(data.stream, 0, None, ptr::null_mut());
                    if !op.is_null() {
                        pa_operation_unref(op);
                    }
                } else {
                    return;
                }
            }

            while !data.new_data.is_empty() {
                let mut buffer: *mut c_void = ptr::null_mut();
                let mut bytes_to_fill = data.new_data.len();
                if pa_stream_begin_write(data.stream, &mut buffer, &mut bytes_to_fill) != 0 {
                    return;
                }
                // PA may ask for more than we have: wait for more data.
                if bytes_to_fill > data.new_data.len() {
                    pa_stream_cancel_write(data.stream);
                    return;
                }
                let out = std::slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_fill);
                // A ring buffer is at most two runs, so this is two memcpy
                // rather than a branch per byte, inside the write callback.
                let (front, back) = data.new_data.as_slices();
                let head = front.len().min(bytes_to_fill);
                out[..head].copy_from_slice(&front[..head]);
                out[head..].copy_from_slice(&back[..bytes_to_fill - head]);
                data.new_data.drain(..bytes_to_fill);
                pa_stream_write(
                    data.stream,
                    buffer,
                    bytes_to_fill,
                    None,
                    0,
                    PA_SEEK_RELATIVE,
                );
            }
        }
    }
}

impl AudioCallback for PulseMonitor {
    /// `on_audio_playback`.
    fn on_audio(&self, audio: &ObsAudio) {
        let Some(mut data) = self.playback.try_lock() else {
            // `TryAcquireSRWLockExclusive` in OBS: the packet is skipped,
            // which is audible, so it is counted.
            self.note_drop(audio.frames as usize);
            self.stream_write(0);
            return;
        };
        let input = [
            audio.planes[0].as_ptr() as *const u8,
            audio.planes[1].as_ptr() as *const u8,
        ];
        let mut written = 0;
        if let Some(frames) = data.resampler.resample(&input, audio.frames) {
            written = frames;
            let bytes = data.bytes_per_frame * frames as usize;
            let vol = self.shared.volume();
            // Disjoint fields, so the samples are scaled in place and
            // handed over without the round trip through a scratch copy.
            let data = &mut *data;
            let format = data.format;
            let samples = &mut data.resampler.plane_mut(0)[..bytes];
            match format {
                PA_SAMPLE_FLOAT32LE => {
                    // SAFETY: the resampler hands out planes aligned for
                    // `f32`, so this covers all of them.
                    let (_, floats, _) = unsafe { samples.align_to_mut::<f32>() };
                    self.shared.apply_f32(floats, vol);
                }
                PA_SAMPLE_U8 => self.shared.record_peak(scale_u8(samples, vol)),
                PA_SAMPLE_S16LE => self.shared.record_peak(scale_s16(samples, vol)),
                PA_SAMPLE_S32LE => self.shared.record_peak(scale_s32(samples, vol)),
                _ => {}
            }
            data.new_data.extend(samples.iter().copied());
        }
        drop(data);
        self.stream_write(written);
    }
}

impl Monitor for PulseMonitor {
    /// PulseAudio moves a stream whose sink went away to FAILED or
    /// TERMINATED and never plays it again. `pulseaudio-output.c` never
    /// reads the stream state back, because OBS rebuilds a monitor only
    /// from `obs_reset_audio_monitoring`; without this the output would
    /// stay silent while the panel reported it as playing. See the retry
    /// deviation on `MONITOR_RETRY`.
    fn state(&self) -> MonitorState {
        let _g = self.pulse.lock();
        // SAFETY: the stream lives as long as this monitor and is read here
        // under the mainloop lock.
        let state = unsafe { pa_stream_get_state(self.handle.0) };
        if state == PA_STREAM_READY || state == PA_STREAM_CREATING {
            MonitorState::Playing {
                format: self.format.clone(),
            }
        } else {
            MonitorState::Reconnecting(msg::DEVICE_DISCONNECTED.into())
        }
    }
}

impl Drop for PulseMonitor {
    /// `pulseaudio_stop_playback`.
    fn drop(&mut self) {
        let stream = self.playback.get_mut().stream;
        let pulse = &self.pulse;
        {
            let _g = pulse.lock();
            // SAFETY: stream owned by this monitor.
            unsafe { pa_stream_disconnect(stream) };
        }
        {
            let _g = pulse.lock();
            // SAFETY: stream owned by this monitor, released once.
            unsafe {
                pa_stream_set_write_callback(stream, None, ptr::null_mut());
                pa_stream_unref(stream);
            }
        }
        log::info!("Stopped Monitoring in '{}'", self.device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_names_match_like_obs() {
        assert!(devices_match(
            "alsa_output.pci.analog-stereo.monitor",
            "alsa_output.pci.analog-stereo"
        ));
        assert!(!devices_match(
            "alsa_input.usb",
            "alsa_output.pci.analog-stereo"
        ));
    }

    #[test]
    fn channel_maps_follow_obs() {
        assert_eq!(channel_map(Speakers::Mono).channels, 1);
        assert_eq!(channel_map(Speakers::Mono).map[0], PA_CHANNEL_POSITION_MONO);
        assert_eq!(
            channel_map(Speakers::TwoPointOne).map[2],
            PA_CHANNEL_POSITION_LFE
        );
        assert_eq!(channel_map(Speakers::SevenPointOne).channels, 8);
    }

    #[test]
    fn unsupported_formats_fall_back() {
        let info = stream_info(&pa_sample_spec {
            format: PA_SAMPLE_S24LE,
            rate: 44_100,
            channels: 7,
        });
        assert_eq!(info.format, PA_SAMPLE_FLOAT32LE);
        assert_eq!(info.channels, 2);
    }
}
