//! Windows backend.
//!
//! - Capture: port of `WASAPISource` (`plugins/win-wasapi/win-wasapi.cpp`),
//!   thread flavour: event driven, 10 ms wake-ups for loopback, silent
//!   packets replaced by zeros, silent render buffer before loopback starts,
//!   3 s reconnect interval, immediate restart when the default output
//!   changes.
//! - Monitoring: port of `libobs/audio-monitoring/win32/wasapi-output.c`:
//!   each packet is resampled and written straight away from the capture
//!   thread; any failure drops the client, which is reopened on the next
//!   packet.

use std::ffi::c_void;
use std::sync::Arc;
use std::thread::JoinHandle;

use parking_lot::Mutex;
use windows::core::{implement, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, ERole, IAudioCaptureClient, IAudioClient,
    IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    DEVICE_STATE, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, ResetEvent,
    SetEvent, WaitForMultipleObjects, INFINITE,
};

use super::format::{
    AudioSpec, ObsAudio, SampleFormat, SourceAudio, Speakers, OBS_SAMPLE_RATE, OBS_SPEAKERS,
};
use super::hub::{AudioCallback, SourceHub};
use super::swr::Resampler;
use super::volume::OutputShared;
use super::{
    describe, Capture, CaptureState, DeviceList, Monitor, MonitorInit, MonitorState, OutputInfo,
    SourceInfo, SourceKind, SystemEvent, DESKTOP, DESKTOP_NAME,
};

const OUTPUT_PREFIX: &str = "output:";
const INPUT_PREFIX: &str = "input:";

/// `BUFFER_TIME_100NS` of `win-wasapi`: 5 seconds.
const CAPTURE_BUFFER_100NS: i64 = 5 * 10_000_000;
/// Buffer duration of `wasapi-output.c`: 1 second.
const MONITOR_BUFFER_100NS: i64 = 10_000_000;
/// `RECONNECT_INTERVAL` of `win-wasapi`.
const RECONNECT_INTERVAL_MS: u32 = 3000;
/// "Windows 7 does not seem to wake up for LOOPBACK".
const LOOPBACK_WAKE_MS: u32 = 10;

const SPEAKER_FRONT_LEFT: u32 = 0x1;
const SPEAKER_FRONT_RIGHT: u32 = 0x2;
const SPEAKER_FRONT_CENTER: u32 = 0x4;
const SPEAKER_LOW_FREQUENCY: u32 = 0x8;
const SPEAKER_BACK_LEFT: u32 = 0x10;
const SPEAKER_BACK_RIGHT: u32 = 0x20;
const SPEAKER_FRONT_LEFT_OF_CENTER: u32 = 0x40;
const SPEAKER_FRONT_RIGHT_OF_CENTER: u32 = 0x80;
const SPEAKER_BACK_CENTER: u32 = 0x100;
const SPEAKER_SIDE_LEFT: u32 = 0x200;
const SPEAKER_SIDE_RIGHT: u32 = 0x400;

const KSAUDIO_SPEAKER_STEREO: u32 = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT;
const KSAUDIO_SPEAKER_2POINT1: u32 = KSAUDIO_SPEAKER_STEREO | SPEAKER_LOW_FREQUENCY;
const KSAUDIO_SPEAKER_SURROUND: u32 =
    KSAUDIO_SPEAKER_STEREO | SPEAKER_FRONT_CENTER | SPEAKER_BACK_CENTER;
const OBS_KSAUDIO_SPEAKER_4POINT1: u32 = KSAUDIO_SPEAKER_SURROUND | SPEAKER_LOW_FREQUENCY;
const KSAUDIO_SPEAKER_5POINT1: u32 = KSAUDIO_SPEAKER_STEREO
    | SPEAKER_FRONT_CENTER
    | SPEAKER_LOW_FREQUENCY
    | SPEAKER_BACK_LEFT
    | SPEAKER_BACK_RIGHT;
const KSAUDIO_SPEAKER_5POINT1_SURROUND: u32 = KSAUDIO_SPEAKER_STEREO
    | SPEAKER_FRONT_CENTER
    | SPEAKER_LOW_FREQUENCY
    | SPEAKER_SIDE_LEFT
    | SPEAKER_SIDE_RIGHT;
const KSAUDIO_SPEAKER_7POINT1: u32 =
    KSAUDIO_SPEAKER_5POINT1 | SPEAKER_FRONT_LEFT_OF_CENTER | SPEAKER_FRONT_RIGHT_OF_CENTER;
const KSAUDIO_SPEAKER_7POINT1_SURROUND: u32 =
    KSAUDIO_SPEAKER_5POINT1_SURROUND | SPEAKER_BACK_LEFT | SPEAKER_BACK_RIGHT;

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// COM pointers are free-threaded here, as OBS uses them.
struct Com<T>(T);
// SAFETY: MMDevice and audio client interfaces are agile.
unsafe impl<T> Send for Com<T> {}
unsafe impl<T> Sync for Com<T> {}

struct Event(HANDLE);
// SAFETY: kernel event handles can be used from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    fn new(manual_reset: bool) -> Event {
        // SAFETY: plain Win32 call; the handle is closed on drop.
        Event(
            unsafe { CreateEventW(None, manual_reset, false, PCWSTR::null()) }
                .expect("CreateEvent"),
        )
    }
    fn set(&self) {
        // SAFETY: valid handle.
        let _ = unsafe { SetEvent(self.0) };
    }
    fn reset(&self) {
        // SAFETY: valid handle.
        let _ = unsafe { ResetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: valid handle, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn init_thread() {
    // SAFETY: plain COM initialization; failure means COM is already set up.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    init_thread();
    // SAFETY: standard COM activation.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn take_pwstr(p: PWSTR) -> String {
    // SAFETY: `p` was allocated by COM and is freed here.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn device_id(device: &IMMDevice) -> Option<String> {
    // SAFETY: COM call on a valid device.
    unsafe { device.GetId() }.ok().map(take_pwstr)
}

fn device_name(device: &IMMDevice) -> String {
    // SAFETY: COM calls on a valid device.
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName as *const PROPERTYKEY))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// `get_default_id`: the default render endpoint for the console role.
fn default_output_id() -> Option<String> {
    let en = enumerator().ok()?;
    // SAFETY: COM call.
    let device = unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole) }.ok()?;
    device_id(&device)
}

fn list_devices(en: &IMMDeviceEnumerator, flow: EDataFlow) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // SAFETY: COM calls on valid objects.
    unsafe {
        let Ok(collection) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let count = collection.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(device) = collection.Item(i) else {
                continue;
            };
            if let Some(id) = device_id(&device) {
                out.push((id, device_name(&device)));
            }
        }
    }
    out
}

pub fn enumerate() -> Result<DeviceList, String> {
    let en = enumerator().map_err(|e| e.message())?;
    let default_out = default_output_id();
    // SAFETY: COM call.
    let default_in = unsafe { en.GetDefaultAudioEndpoint(eCapture, eConsole) }
        .ok()
        .and_then(|d| device_id(&d));

    let mut list = DeviceList::default();
    list.sources.push(SourceInfo {
        id: DESKTOP.into(),
        name: DESKTOP_NAME.into(),
        kind: SourceKind::Desktop,
        is_default: false,
        captures_output: default_out.clone(),
    });

    for (id, name) in list_devices(&en, eRender) {
        let is_default = default_out.as_deref() == Some(id.as_str());
        list.sources.push(SourceInfo {
            id: format!("{OUTPUT_PREFIX}{id}"),
            name: name.clone(),
            kind: SourceKind::Loopback,
            is_default,
            captures_output: Some(id.clone()),
        });
        list.outputs.push(OutputInfo {
            id,
            name,
            is_default,
        });
    }
    for (id, name) in list_devices(&en, eCapture) {
        list.sources.push(SourceInfo {
            is_default: default_in.as_deref() == Some(id.as_str()),
            id: format!("{INPUT_PREFIX}{id}"),
            name,
            kind: SourceKind::Capture,
            captures_output: None,
        });
    }
    Ok(list)
}

#[derive(Clone, PartialEq)]
enum SourceType {
    /// `SourceType::DeviceOutput` with the default device.
    DefaultOutput,
    DeviceOutput(String),
    Input(String),
}

impl SourceType {
    fn parse(source: &str) -> Result<SourceType, String> {
        if source == DESKTOP {
            Ok(SourceType::DefaultOutput)
        } else if let Some(id) = source.strip_prefix(OUTPUT_PREFIX) {
            Ok(SourceType::DeviceOutput(id.to_string()))
        } else if let Some(id) = source.strip_prefix(INPUT_PREFIX) {
            Ok(SourceType::Input(id.to_string()))
        } else {
            Err(format!("Unknown source {source}"))
        }
    }

    fn is_input(&self) -> bool {
        matches!(self, SourceType::Input(_))
    }
}

/// Output device a source records, for `OBS_SOURCE_DO_NOT_SELF_MONITOR`
/// (only the output capture source carries that flag).
fn captured_output(source: &str) -> Option<String> {
    match SourceType::parse(source).ok()? {
        SourceType::DefaultOutput => default_output_id(),
        SourceType::DeviceOutput(id) => Some(id),
        SourceType::Input(_) => None,
    }
}

/// `ConvertSpeakerLayout` of `win-wasapi`.
fn capture_speakers(mask: u32, channels: usize) -> Speakers {
    match mask {
        KSAUDIO_SPEAKER_2POINT1 => Speakers::TwoPointOne,
        KSAUDIO_SPEAKER_SURROUND => Speakers::FourPointZero,
        OBS_KSAUDIO_SPEAKER_4POINT1 => Speakers::FourPointOne,
        KSAUDIO_SPEAKER_5POINT1_SURROUND => Speakers::FivePointOne,
        KSAUDIO_SPEAKER_7POINT1_SURROUND => Speakers::SevenPointOne,
        _ => Speakers::from_channels(channels),
    }
}

/// `convert_speaker_layout` of `wasapi-output.c`.
fn monitor_speakers(mask: u32, channels: usize) -> Speakers {
    match mask {
        KSAUDIO_SPEAKER_2POINT1 => Speakers::TwoPointOne,
        KSAUDIO_SPEAKER_SURROUND => Speakers::FourPointZero,
        OBS_KSAUDIO_SPEAKER_4POINT1 => Speakers::FourPointOne,
        KSAUDIO_SPEAKER_5POINT1 => Speakers::FivePointOne,
        KSAUDIO_SPEAKER_7POINT1 => Speakers::SevenPointOne,
        _ => Speakers::from_channels(channels),
    }
}

/// Mix format owned by COM.
struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    fn get(client: &IAudioClient) -> windows::core::Result<MixFormat> {
        // SAFETY: COM call; the pointer is freed on drop.
        unsafe { client.GetMixFormat() }.map(MixFormat)
    }
    fn wfex(&self) -> &WAVEFORMATEX {
        // SAFETY: non-null pointer returned by GetMixFormat.
        unsafe { &*self.0 }
    }
    fn channel_mask(&self) -> u32 {
        if self.wfex().wFormatTag == WAVE_FORMAT_EXTENSIBLE {
            // SAFETY: the tag guarantees the extensible layout.
            unsafe { (*(self.0 as *const WAVEFORMATEXTENSIBLE)).dwChannelMask }
        } else {
            0
        }
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        // SAFETY: allocated by COM.
        unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
    }
}

fn hr(context: &str, e: windows::core::Error) -> String {
    format!("{context}: {:08X}", e.code().0)
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

struct ActiveCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    spec: AudioSpec,
    silence: Vec<u8>,
}

impl Drop for ActiveCapture {
    fn drop(&mut self) {
        // SAFETY: COM call on a valid client.
        let _ = unsafe { self.client.Stop() };
    }
}

pub struct WasapiCapture {
    source_type: SourceType,
    stop: Arc<Event>,
    restart: Arc<Event>,
    state: Arc<Mutex<CaptureState>>,
    thread: Option<JoinHandle<()>>,
}

pub fn start_capture(source: &str, hub: Arc<SourceHub>) -> Result<Box<dyn Capture>, String> {
    let source_type = SourceType::parse(source)?;
    let stop = Arc::new(Event::new(true));
    let restart = Arc::new(Event::new(true));
    let state = Arc::new(Mutex::new(CaptureState::Starting));

    let thread = {
        let (source_type, stop, restart, state) = (
            source_type.clone(),
            stop.clone(),
            restart.clone(),
            state.clone(),
        );
        std::thread::Builder::new()
            .name("win-wasapi: capture thread".into())
            .spawn(move || capture_thread(source_type, hub, &stop, &restart, &state))
            .map_err(|e| e.to_string())?
    };

    Ok(Box::new(WasapiCapture {
        source_type,
        stop,
        restart,
        state,
        thread: Some(thread),
    }))
}

impl Capture for WasapiCapture {
    fn state(&self) -> CaptureState {
        self.state.lock().clone()
    }

    /// `WASAPISource::SetDefaultDevice`: only the default device source
    /// restarts.
    fn default_output_changed(&self) -> bool {
        if self.source_type == SourceType::DefaultOutput {
            self.restart.set();
        }
        false
    }
}

impl Drop for WasapiCapture {
    fn drop(&mut self) {
        self.stop.set();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// `WASAPISource::CaptureThread` with the reconnect thread folded in.
fn capture_thread(
    source_type: SourceType,
    hub: Arc<SourceHub>,
    stop: &Event,
    restart: &Event,
    state: &Mutex<CaptureState>,
) {
    init_thread();
    let task: Vec<u16> = "Audio\0".encode_utf16().collect();
    let mut index = 0u32;
    // SAFETY: MMCSS registration, reverted before leaving.
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(task.as_ptr()), &mut index) };

    let receive = Event::new(false);
    let mut previously_failed = false;

    loop {
        let mut reconnect_ms = RECONNECT_INTERVAL_MS;

        match initialize_capture(&source_type, &receive) {
            Ok(mut active) => {
                previously_failed = false;
                *state.lock() = CaptureState::Active {
                    format: describe(active.spec.rate, active.spec.speakers.channels()),
                };
                let wake = if source_type.is_input() {
                    INFINITE
                } else {
                    LOOPBACK_WAKE_MS
                };
                let sigs = [stop.0, receive.0, restart.0];
                loop {
                    // SAFETY: valid handles.
                    let ret = unsafe { WaitForMultipleObjects(&sigs, false, wake) };
                    if ret == WAIT_OBJECT_0 {
                        return finish(mmcss);
                    } else if ret.0 == WAIT_OBJECT_0.0 + 1 || ret == WAIT_TIMEOUT {
                        if !process_capture_data(&mut active, &hub) {
                            log::info!("Device invalidated. Retrying");
                            *state.lock() = CaptureState::Retrying("Device disconnected".into());
                            break;
                        }
                    } else {
                        restart.reset();
                        reconnect_ms = 0;
                        *state.lock() = CaptureState::Starting;
                        break;
                    }
                }
            }
            Err(e) => {
                if !previously_failed {
                    log::warn!("[WASAPISource::TryInitialize] {e}");
                }
                previously_failed = true;
                *state.lock() = CaptureState::Retrying(e);
            }
        }

        if reconnect_ms > 0 {
            let sigs = [stop.0, restart.0];
            // SAFETY: valid handles.
            let ret = unsafe { WaitForMultipleObjects(&sigs, false, reconnect_ms) };
            if ret == WAIT_OBJECT_0 {
                return finish(mmcss);
            }
            if ret.0 == WAIT_OBJECT_0.0 + 1 {
                restart.reset();
            }
        }
    }

    fn finish(mmcss: windows::core::Result<HANDLE>) {
        if let Ok(handle) = mmcss {
            // SAFETY: handle from AvSetMmThreadCharacteristicsW.
            let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
        }
    }
}

/// `WASAPISource::Initialize` for device sources.
fn initialize_capture(source_type: &SourceType, receive: &Event) -> Result<ActiveCapture, String> {
    let en = enumerator().map_err(|e| hr("Failed to create enumerator", e))?;
    // SAFETY: COM calls on valid objects throughout.
    unsafe {
        let device = match source_type {
            SourceType::DefaultOutput => en
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| hr("Failed GetDefaultAudioEndpoint", e))?,
            SourceType::DeviceOutput(id) | SourceType::Input(id) => en
                .GetDevice(&HSTRING::from(id.as_str()))
                .map_err(|e| hr("Failed to enumerate device", e))?,
        };

        receive.reset();

        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| hr("Failed to activate client context", e))?;
        let mix = MixFormat::get(&client).map_err(|e| hr("Failed to get mix format", e))?;

        // `InitFormat`: WASAPI is always float.
        let spec = AudioSpec {
            rate: mix.wfex().nSamplesPerSec,
            speakers: capture_speakers(mix.channel_mask(), mix.wfex().nChannels as usize),
            format: SampleFormat::Float,
        };

        let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        if !source_type.is_input() {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                CAPTURE_BUFFER_100NS,
                0,
                mix.0,
                None,
            )
            .map_err(|e| hr("Failed to initialize audio client", e))?;

        if !source_type.is_input() {
            clear_buffer(&device)?;
        }

        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| hr("Failed to create capture context", e))?;
        client
            .SetEventHandle(receive.0)
            .map_err(|e| hr("Failed to set event handle", e))?;
        client
            .Start()
            .map_err(|e| hr("Failed to start capture client", e))?;

        log::info!(
            "WASAPI: Device '{}' [{} Hz] initialized",
            device_name(&device),
            spec.rate
        );

        Ok(ActiveCapture {
            client,
            capture,
            spec,
            silence: Vec::new(),
        })
    }
}

/// `WASAPISource::ClearBuffer`, the "silent loopback fix".
fn clear_buffer(device: &IMMDevice) -> Result<(), String> {
    // SAFETY: COM calls on valid objects.
    unsafe {
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| hr("Failed to activate client context", e))?;
        let mix = MixFormat::get(&client).map_err(|e| hr("Failed to get mix format", e))?;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                CAPTURE_BUFFER_100NS,
                0,
                mix.0,
                None,
            )
            .map_err(|e| hr("Failed to initialize audio client", e))?;
        let frames = client
            .GetBufferSize()
            .map_err(|e| hr("Failed to get buffer size", e))?;
        let render: IAudioRenderClient = client
            .GetService()
            .map_err(|e| hr("Failed to get render client", e))?;
        let buffer = render
            .GetBuffer(frames)
            .map_err(|e| hr("Failed to get buffer", e))?;
        std::ptr::write_bytes(buffer, 0, frames as usize * mix.wfex().nBlockAlign as usize);
        let _ = render.ReleaseBuffer(frames, 0);
    }
    Ok(())
}

/// `WASAPISource::ProcessCaptureData`.
fn process_capture_data(active: &mut ActiveCapture, hub: &SourceHub) -> bool {
    let block = active.spec.plane_frame_bytes();
    loop {
        // SAFETY: COM calls on a started capture client; the buffer is
        // released before the next GetBuffer.
        unsafe {
            let size = match active.capture.GetNextPacketSize() {
                Ok(size) => size,
                Err(e) => {
                    log::warn!("capture->GetNextPacketSize failed: {:08X}", e.code().0);
                    return false;
                }
            };
            if size == 0 {
                return true;
            }

            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if let Err(e) = active
                .capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            {
                log::warn!("capture->GetBuffer failed: {:08X}", e.code().0);
                return false;
            }

            let bytes = frames as usize * block;
            let buffer: &[u8] = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                if active.silence.len() < bytes {
                    active.silence.resize(bytes, 0);
                }
                &active.silence[..bytes]
            } else {
                std::slice::from_raw_parts(data, bytes)
            };

            hub.output_audio(&SourceAudio {
                planes: &[buffer],
                frames,
                spec: active.spec,
            });

            let _ = active.capture.ReleaseBuffer(frames);
        }
    }
}

// ---------------------------------------------------------------------------
// Monitoring
// ---------------------------------------------------------------------------

struct MonitorClient {
    client: Com<IAudioClient>,
    render: Com<IAudioRenderClient>,
    resampler: Resampler,
    channels: usize,
    format: String,
}

impl Drop for MonitorClient {
    fn drop(&mut self) {
        // SAFETY: COM call on a valid client.
        let _ = unsafe { self.client.0.Stop() };
    }
}

pub struct WasapiMonitor {
    device_id: String,
    shared: Arc<OutputShared>,
    /// `playback_mutex`, taken with `TryAcquireSRWLockExclusive`.
    playback: Mutex<Option<MonitorClient>>,
    state: Mutex<MonitorState>,
}

/// `audio_monitor_init` + `audio_monitor_init_wasapi`.
pub fn create_monitor(
    source: &str,
    device: &str,
    shared: Arc<OutputShared>,
) -> Result<MonitorInit, String> {
    if captured_output(source).as_deref() == Some(device) {
        return Ok(MonitorInit::Ignored);
    }
    let client = init_monitor_client(device)?;
    let format = client.format.clone();
    Ok(MonitorInit::Active(Arc::new(WasapiMonitor {
        device_id: device.to_string(),
        shared,
        playback: Mutex::new(Some(client)),
        state: Mutex::new(MonitorState::Playing { format }),
    })))
}

/// `audio_monitor_init_wasapi`.
fn init_monitor_client(device_id: &str) -> Result<MonitorClient, String> {
    let en = enumerator().map_err(|e| hr("Failed to create IMMDeviceEnumerator", e))?;
    // SAFETY: COM calls on valid objects.
    unsafe {
        let device = en
            .GetDevice(&HSTRING::from(device_id))
            .map_err(|e| hr("Failed to get device", e))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| hr("Failed to activate device", e))?;
        let mix = MixFormat::get(&client).map_err(|e| hr("Failed to get mix format", e))?;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0,
                MONITOR_BUFFER_100NS,
                0,
                mix.0,
                None,
            )
            .map_err(|e| hr("Failed to initialize", e))?;

        let channels = mix.wfex().nChannels as usize;
        let from = AudioSpec {
            rate: OBS_SAMPLE_RATE,
            speakers: OBS_SPEAKERS,
            format: SampleFormat::FloatPlanar,
        };
        let to = AudioSpec {
            rate: mix.wfex().nSamplesPerSec,
            speakers: monitor_speakers(mix.channel_mask(), channels),
            format: SampleFormat::Float,
        };
        let resampler =
            Resampler::new(to, from).ok_or_else(|| "Failed to create resampler".to_string())?;

        client
            .GetBufferSize()
            .map_err(|e| hr("Failed to get buffer size", e))?;
        let render: IAudioRenderClient = client
            .GetService()
            .map_err(|e| hr("Failed to get IAudioRenderClient", e))?;
        client.Start().map_err(|e| hr("Failed to start audio", e))?;

        Ok(MonitorClient {
            client: Com(client),
            render: Com(render),
            resampler,
            channels,
            format: describe(to.rate, channels),
        })
    }
}

impl AudioCallback for WasapiMonitor {
    /// `on_audio_playback`.
    fn on_audio(&self, audio: &ObsAudio) {
        let Some(mut playback) = self.playback.try_lock() else {
            return;
        };

        if playback.is_none() {
            match init_monitor_client(&self.device_id) {
                Ok(client) => {
                    *self.state.lock() = MonitorState::Playing {
                        format: client.format.clone(),
                    };
                    *playback = Some(client);
                }
                Err(e) => {
                    *self.state.lock() = MonitorState::Reconnecting(e);
                    return;
                }
            }
        }

        let vol = self.shared.volume();
        let ok = playback
            .as_mut()
            .is_some_and(|client| write_packet(client, audio, &self.shared, vol).is_ok());
        if !ok {
            // `audio_monitor_free_for_reconnect`.
            *playback = None;
            *self.state.lock() = MonitorState::Reconnecting("Device unavailable".into());
        }
    }
}

impl Monitor for WasapiMonitor {
    fn state(&self) -> MonitorState {
        self.state.lock().clone()
    }
}

fn write_packet(
    client: &mut MonitorClient,
    audio: &ObsAudio,
    shared: &OutputShared,
    vol: f32,
) -> windows::core::Result<()> {
    let input = [
        audio.planes[0].as_ptr() as *const u8,
        audio.planes[1].as_ptr() as *const u8,
    ];
    let Some(frames) = client.resampler.resample(&input, audio.frames) else {
        return Ok(());
    };

    // SAFETY: COM calls on a started render client; the buffer returned by
    // GetBuffer holds `frames * channels` float samples.
    unsafe {
        client.client.0.GetCurrentPadding()?;
        let output = client.render.0.GetBuffer(frames)?;

        let samples = frames as usize * client.channels;
        let data = client.resampler.plane_mut(0);
        let (_, floats, _) = data[..samples * 4].align_to_mut::<f32>();
        if floats.len() == samples {
            shared.apply_f32(floats, vol);
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), output, samples * 4);

        client.render.0.ReleaseBuffer(frames, 0)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Device notifications (`WASAPINotify`)
// ---------------------------------------------------------------------------

#[implement(IMMNotificationClient)]
struct NotificationClient {
    callback: Box<dyn Fn(SystemEvent) + Send + Sync>,
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        _id: &PCWSTR,
        _state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        (self.callback)(SystemEvent::DevicesChanged);
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        (self.callback)(SystemEvent::DevicesChanged);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        (self.callback)(SystemEvent::DevicesChanged);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            (self.callback)(SystemEvent::DefaultOutputChanged);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

struct Watcher {
    enumerator: Com<IMMDeviceEnumerator>,
    client: Com<IMMNotificationClient>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: unregisters the client registered in `watch_system`.
        let _ = unsafe {
            self.enumerator
                .0
                .UnregisterEndpointNotificationCallback(&self.client.0)
        };
    }
}

pub fn watch_system(
    callback: Box<dyn Fn(SystemEvent) + Send + Sync>,
) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    let en = enumerator().ok()?;
    let client: IMMNotificationClient = NotificationClient { callback }.into();
    // SAFETY: registration of a valid COM object.
    unsafe { en.RegisterEndpointNotificationCallback(&client) }.ok()?;
    Some(Box::new(Watcher {
        enumerator: Com(en),
        client: Com(client),
    }))
}

#[allow(dead_code)]
fn _assert_interface<T: Interface>() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_masks_follow_obs_tables() {
        assert_eq!(
            capture_speakers(KSAUDIO_SPEAKER_STEREO, 2),
            Speakers::Stereo
        );
        assert_eq!(
            capture_speakers(KSAUDIO_SPEAKER_5POINT1_SURROUND, 6),
            Speakers::FivePointOne
        );
        // win-wasapi does not list the "back" 5.1 mask: channel count wins.
        assert_eq!(
            capture_speakers(KSAUDIO_SPEAKER_5POINT1, 6),
            Speakers::FivePointOne
        );
        assert_eq!(
            monitor_speakers(KSAUDIO_SPEAKER_7POINT1, 8),
            Speakers::SevenPointOne
        );
        assert_eq!(monitor_speakers(0, 7), Speakers::Unknown);
    }

    #[test]
    fn source_ids() {
        assert!(SourceType::parse(DESKTOP).unwrap() == SourceType::DefaultOutput);
        assert!(SourceType::parse("input:{x}").unwrap().is_input());
        assert!(SourceType::parse("nope").is_err());
        assert_eq!(captured_output("output:abc").as_deref(), Some("abc"));
        assert_eq!(captured_output("input:abc"), None);
    }
}
