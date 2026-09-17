//! macOS backend.
//!
//! - Desktop audio: port of `plugins/mac-capture/mac-sck-audio-capture.m`:
//!   ScreenCaptureKit on the main display, stereo, excluding this process's
//!   own audio (so monitoring can never feed back into the capture).
//! - Input devices: port of `plugins/mac-capture/mac-audio.c` (AUHAL, with
//!   its reconnect thread), downmix enabled as in OBS's defaults. Loopback
//!   drivers such as BlackHole are listed as output captures, like OBS does.
//! - Monitoring: port of `libobs/audio-monitoring/osx/coreaudio-output.c`:
//!   an AudioQueue in the OBS format, three 30 ms buffers and a 90 ms
//!   prefill, paused and prefilled again whenever it runs dry.

#![allow(non_upper_case_globals, non_snake_case)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamOutput, SCStreamOutputType,
};
use parking_lot::Mutex;

use super::format::{
    AudioSpec, ObsAudio, SampleFormat, SourceAudio, Speakers, MAX_AUDIO_CHANNELS, OBS_CHANNELS,
    OBS_SAMPLE_RATE, OBS_SPEAKERS,
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

// ---------------------------------------------------------------------------
// C API (CoreFoundation, CoreAudio, AudioToolbox, CoreMedia, CoreGraphics)
// ---------------------------------------------------------------------------

type OSStatus = i32;
type AudioObjectID = u32;
type CFStringRef = *const c_void;
type AudioUnit = *mut c_void;
type AudioQueueRef = *mut c_void;

const noErr: OSStatus = 0;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

const kAudioObjectSystemObject: AudioObjectID = 1;
const kAudioObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
const kAudioObjectPropertyScopeInput: u32 = fourcc(b"inpt");
const kAudioObjectPropertyScopeOutput: u32 = fourcc(b"outp");
const kAudioObjectPropertyElementMain: u32 = 0;
const kAudioObjectPropertyName: u32 = fourcc(b"lnam");
const kAudioHardwarePropertyDevices: u32 = fourcc(b"dev#");
const kAudioHardwarePropertyDefaultInputDevice: u32 = fourcc(b"dIn ");
const kAudioHardwarePropertyDefaultOutputDevice: u32 = fourcc(b"dOut");
const kAudioHardwarePropertyDeviceForUID: u32 = fourcc(b"duid");
const kAudioDevicePropertyStreams: u32 = fourcc(b"stm#");
const kAudioDevicePropertyDeviceUID: u32 = fourcc(b"uid ");
const kAudioDevicePropertyDeviceIsAlive: u32 = fourcc(b"livn");
const kAudioDevicePropertyNominalSampleRate: u32 = fourcc(b"nsrt");
const kAudioDevicePropertyBufferFrameSize: u32 = fourcc(b"fsiz");
const kAudioStreamPropertyAvailablePhysicalFormats: u32 = fourcc(b"pft?");

const kAudioUnitType_Output: u32 = fourcc(b"auou");
const kAudioUnitSubType_HALOutput: u32 = fourcc(b"ahal");
const kAudioUnitManufacturer_Apple: u32 = fourcc(b"appl");
const kAudioUnitProperty_StreamFormat: u32 = 8;
const kAudioOutputUnitProperty_CurrentDevice: u32 = 2000;
const kAudioOutputUnitProperty_EnableIO: u32 = 2003;
const kAudioOutputUnitProperty_SetInputCallback: u32 = 2005;
const kAudioUnitScope_Global: u32 = 0;
const kAudioUnitScope_Input: u32 = 1;
const kAudioUnitScope_Output: u32 = 2;
const BUS_OUTPUT: u32 = 0;
const BUS_INPUT: u32 = 1;

const kAudioFormatLinearPCM: u32 = fourcc(b"lpcm");
const kAudioFormatFlagIsFloat: u32 = 1 << 0;
const kAudioFormatFlagIsSignedInteger: u32 = 1 << 2;
const kAudioFormatFlagIsPacked: u32 = 1 << 3;
const kAudioFormatFlagIsNonInterleaved: u32 = 1 << 5;

const kAudioQueueProperty_CurrentDevice: u32 = fourcc(b"aqcd");
const kAudioQueueParam_Volume: u32 = 1;

const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct AudioTimeStamp {
    _opaque: [u8; 64],
}

#[repr(C)]
struct AudioValueTranslation {
    input_data: *mut c_void,
    input_data_size: u32,
    output_data: *mut c_void,
    output_data_size: u32,
}

#[repr(C)]
struct AudioQueueBuffer {
    audio_data_bytes_capacity: u32,
    audio_data: *mut c_void,
    audio_data_byte_size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut c_void,
    packet_description_count: u32,
}

type AudioQueueBufferRef = *mut AudioQueueBuffer;

/// Offset of `mBuffers` in `AudioBufferList`.
const BUFFER_LIST_HEADER: usize = std::mem::offset_of!(AudioBufferList, buffers);

type AURenderCallback = unsafe extern "C" fn(
    ref_con: *mut c_void,
    action_flags: *mut u32,
    time_stamp: *const AudioTimeStamp,
    bus_number: u32,
    number_frames: u32,
    data: *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
struct AURenderCallbackStruct {
    input_proc: AURenderCallback,
    input_proc_ref_con: *mut c_void,
}

type AudioObjectPropertyListenerProc = unsafe extern "C" fn(
    object: AudioObjectID,
    number_addresses: u32,
    addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus;

type AudioQueueOutputCallback =
    unsafe extern "C" fn(user_data: *mut c_void, aq: AudioQueueRef, buffer: AudioQueueBufferRef);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
    ) -> OSStatus;
    fn AudioObjectGetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    fn AudioObjectAddPropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;
    fn AudioObjectRemovePropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client_data: *mut c_void,
    ) -> OSStatus;
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        component: *mut c_void,
        desc: *const AudioComponentDescription,
    ) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, instance: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(instance: AudioUnit) -> OSStatus;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> OSStatus;
    fn AudioUnitGetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
    fn AudioUnitRender(
        unit: AudioUnit,
        action_flags: *mut u32,
        time_stamp: *const AudioTimeStamp,
        bus: u32,
        frames: u32,
        data: *mut AudioBufferList,
    ) -> OSStatus;

    fn AudioQueueNewOutput(
        format: *const AudioStreamBasicDescription,
        callback: AudioQueueOutputCallback,
        user_data: *mut c_void,
        run_loop: *const c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        out_aq: *mut AudioQueueRef,
    ) -> OSStatus;
    fn AudioQueueSetProperty(
        aq: AudioQueueRef,
        id: u32,
        data: *const c_void,
        size: u32,
    ) -> OSStatus;
    fn AudioQueueSetParameter(aq: AudioQueueRef, id: u32, value: f32) -> OSStatus;
    fn AudioQueueAllocateBuffer(
        aq: AudioQueueRef,
        size: u32,
        out_buffer: *mut AudioQueueBufferRef,
    ) -> OSStatus;
    fn AudioQueueFreeBuffer(aq: AudioQueueRef, buffer: AudioQueueBufferRef) -> OSStatus;
    fn AudioQueueEnqueueBuffer(
        aq: AudioQueueRef,
        buffer: AudioQueueBufferRef,
        num_packet_descs: u32,
        packet_descs: *const c_void,
    ) -> OSStatus;
    fn AudioQueueStart(aq: AudioQueueRef, start_time: *const c_void) -> OSStatus;
    fn AudioQueuePause(aq: AudioQueueRef) -> OSStatus;
    fn AudioQueueStop(aq: AudioQueueRef, immediate: u8) -> OSStatus;
    fn AudioQueueDispose(aq: AudioQueueRef, immediate: u8) -> OSStatus;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetFormatDescription(sbuf: *const c_void) -> *const c_void;
    fn CMAudioFormatDescriptionGetStreamBasicDescription(
        desc: *const c_void,
    ) -> *const AudioStreamBasicDescription;
    fn CMSampleBufferGetDataBuffer(sbuf: *const c_void) -> *const c_void;
    fn CMBlockBufferGetDataLength(buffer: *const c_void) -> usize;
    fn CMBlockBufferGetDataPointer(
        buffer: *const c_void,
        offset: usize,
        length_at_offset: *mut usize,
        total_length: *mut usize,
        data_pointer: *mut *mut c_char,
    ) -> OSStatus;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

fn cf_string(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut buf = [0 as c_char; 1024];
    // SAFETY: fixed buffer; the string is released by the caller.
    unsafe {
        if CFStringGetCString(
            s,
            buf.as_mut_ptr(),
            buf.len() as isize,
            kCFStringEncodingUTF8,
        ) == 0
        {
            return String::new();
        }
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        selector,
        scope,
        element: kAudioObjectPropertyElementMain,
    }
}

/// Reads a `CFStringRef` property and releases it.
fn string_property(id: AudioObjectID, selector: u32, scope: u32) -> Option<String> {
    let addr = address(selector, scope);
    let mut value: CFStringRef = ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    // SAFETY: the property holds a retained CFStringRef.
    unsafe {
        let stat = AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut value as *mut _ as *mut c_void,
        );
        if stat != noErr || value.is_null() {
            return None;
        }
        let s = cf_string(value);
        CFRelease(value);
        Some(s)
    }
}

fn has_streams(id: AudioObjectID, scope: u32) -> bool {
    let addr = address(kAudioDevicePropertyStreams, scope);
    let mut size = 0u32;
    // SAFETY: size query only.
    unsafe { AudioObjectGetPropertyDataSize(id, &addr, 0, ptr::null(), &mut size) };
    size > 0
}

fn device_ids() -> Vec<AudioObjectID> {
    let addr = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut size = 0u32;
    // SAFETY: size then data, into a buffer of that size.
    unsafe {
        if AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject,
            &addr,
            0,
            ptr::null(),
            &mut size,
        ) != noErr
        {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / std::mem::size_of::<AudioObjectID>()];
        if AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            ptr::null(),
            &mut size,
            ids.as_mut_ptr() as *mut c_void,
        ) != noErr
        {
            return Vec::new();
        }
        ids
    }
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    let addr = address(selector, kAudioObjectPropertyScopeGlobal);
    let mut id: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    // SAFETY: fixed size output.
    let stat = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut id as *mut _ as *mut c_void,
        )
    };
    (stat == noErr && id != 0).then_some(id)
}

/// `coreaudio_get_device_id`.
fn device_id_for_uid(uid: &str) -> Option<AudioObjectID> {
    let c_uid = std::ffi::CString::new(uid).ok()?;
    // SAFETY: the CFString is released before returning.
    unsafe {
        let mut cf_uid =
            CFStringCreateWithCString(ptr::null(), c_uid.as_ptr(), kCFStringEncodingUTF8);
        if cf_uid.is_null() {
            return None;
        }
        let mut id: AudioObjectID = 0;
        let mut translation = AudioValueTranslation {
            input_data: &mut cf_uid as *mut _ as *mut c_void,
            input_data_size: std::mem::size_of::<CFStringRef>() as u32,
            output_data: &mut id as *mut _ as *mut c_void,
            output_data_size: std::mem::size_of::<AudioObjectID>() as u32,
        };
        let mut size = std::mem::size_of::<AudioValueTranslation>() as u32;
        let addr = address(
            kAudioHardwarePropertyDeviceForUID,
            kAudioObjectPropertyScopeGlobal,
        );
        let stat = AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut translation as *mut _ as *mut c_void,
        );
        CFRelease(cf_uid);
        (stat == noErr && id != 0).then_some(id)
    }
}

/// `kAudioDevicePropertyDeviceIsAlive`. A device that was unplugged answers
/// false, or stops answering at all.
fn device_is_alive(id: AudioObjectID) -> bool {
    let addr = address(
        kAudioDevicePropertyDeviceIsAlive,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut alive = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: fixed size output.
    let stat = unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut alive as *mut _ as *mut c_void,
        )
    };
    stat == noErr && alive != 0
}

/// `device_is_input` of `audio-device-enum.c`: loopback drivers are shown as
/// output captures.
fn device_is_input(name: &str) -> bool {
    let name = name.to_lowercase();
    ![
        "soundflower",
        "wavtap",
        "soundsiphon",
        "ishowu",
        "blackhole",
        "loopback",
        "groundcontrol",
        "vbcable",
    ]
    .iter()
    .any(|k| name.contains(k))
}

pub fn init_thread() {}

pub fn enumerate() -> Result<DeviceList, String> {
    let default_out = default_device(kAudioHardwarePropertyDefaultOutputDevice);
    let default_in = default_device(kAudioHardwarePropertyDefaultInputDevice);

    let mut list = DeviceList::default();
    list.sources.push(SourceInfo {
        id: DESKTOP.into(),
        name: DESKTOP_NAME.into(),
        kind: SourceKind::Desktop,
        is_default: false,
        // ScreenCaptureKit leaves this process out: no feedback possible.
        captures_output: None,
    });

    for id in device_ids() {
        let Some(uid) = string_property(
            id,
            kAudioDevicePropertyDeviceUID,
            kAudioObjectPropertyScopeGlobal,
        ) else {
            continue;
        };
        let name = string_property(
            id,
            kAudioObjectPropertyName,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or_default();

        if has_streams(id, kAudioObjectPropertyScopeOutput) {
            list.outputs.push(OutputInfo {
                id: uid.clone(),
                name: name.clone(),
                is_default: default_out == Some(id),
            });
        }
        if has_streams(id, kAudioObjectPropertyScopeInput) {
            if device_is_input(&name) {
                list.sources.push(SourceInfo {
                    id: format!("{INPUT_PREFIX}{uid}"),
                    name,
                    kind: SourceKind::Capture,
                    is_default: default_in == Some(id),
                    captures_output: None,
                });
            } else {
                list.sources.push(SourceInfo {
                    id: format!("{OUTPUT_PREFIX}{uid}"),
                    name,
                    kind: SourceKind::Loopback,
                    is_default: false,
                    captures_output: Some(uid),
                });
            }
        }
    }
    Ok(list)
}

// ---------------------------------------------------------------------------
// Device notifications
// ---------------------------------------------------------------------------

type EventCallback = Box<dyn Fn(SystemEvent) + Send + Sync>;

struct Watcher {
    callback: Box<EventCallback>,
}

// SAFETY: the callback is Send + Sync; the pointer is only a listener key.
unsafe impl Send for Watcher {}
unsafe impl Sync for Watcher {}

unsafe extern "C" fn devices_changed(
    _object: AudioObjectID,
    _count: u32,
    _addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus {
    // SAFETY: client_data is the boxed callback owned by `Watcher`.
    let callback = unsafe { &*(client_data as *const EventCallback) };
    callback(SystemEvent::DevicesChanged);
    noErr
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let addr = address(
            kAudioHardwarePropertyDevices,
            kAudioObjectPropertyScopeGlobal,
        );
        // SAFETY: removes the listener added in `watch_system`.
        unsafe {
            AudioObjectRemovePropertyListener(
                kAudioObjectSystemObject,
                &addr,
                devices_changed,
                &*self.callback as *const EventCallback as *mut c_void,
            )
        };
    }
}

pub fn watch_system(callback: EventCallback) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    let watcher = Watcher {
        callback: Box::new(callback),
    };
    let addr = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    // SAFETY: the client data lives as long as the returned watcher.
    let stat = unsafe {
        AudioObjectAddPropertyListener(
            kAudioObjectSystemObject,
            &addr,
            devices_changed,
            &*watcher.callback as *const EventCallback as *mut c_void,
        )
    };
    (stat == noErr).then(|| Box::new(watcher) as Box<dyn std::any::Any + Send + Sync>)
}

// ---------------------------------------------------------------------------
// Desktop audio: ScreenCaptureKit
// ---------------------------------------------------------------------------

struct SckShared {
    hub: Arc<SourceHub>,
    state: Mutex<CaptureState>,
}

struct OutputIvars {
    shared: Arc<SckShared>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AudioMirrorScreenCaptureDelegate"]
    #[ivars = OutputIvars]
    struct ScreenCaptureDelegate;

    unsafe impl NSObjectProtocol for ScreenCaptureDelegate {}

    unsafe impl SCStreamOutput for ScreenCaptureDelegate {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Audio {
                screen_stream_audio_update(&self.ivars().shared, sample_buffer);
            }
        }
    }

    unsafe impl SCStreamDelegate for ScreenCaptureDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let message = format!("Stream stopped with error {}", error.code());
            log::warn!("{message}");
            *self.ivars().shared.state.lock() = CaptureState::Failed(message);
        }
    }
);

impl ScreenCaptureDelegate {
    fn new(shared: Arc<SckShared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars { shared });
        // SAFETY: plain NSObject initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// `screen_stream_audio_update`.
fn screen_stream_audio_update(shared: &SckShared, sample_buffer: &CMSampleBuffer) {
    let sbuf = sample_buffer as *const CMSampleBuffer as *const c_void;
    // SAFETY: CoreMedia getters on a live sample buffer.
    unsafe {
        let desc = CMSampleBufferGetFormatDescription(sbuf);
        if desc.is_null() {
            return;
        }
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc);
        if asbd.is_null() || (*asbd).channels_per_frame < 1 {
            log::error!("Received sample buffer has less than 1 channel per frame");
            return;
        }
        let asbd = *asbd;

        let block = CMSampleBufferGetDataBuffer(sbuf);
        if block.is_null() {
            return;
        }
        let mut length = CMBlockBufferGetDataLength(block);
        let mut bytes: *mut c_char = ptr::null_mut();
        if CMBlockBufferGetDataPointer(block, 0, &mut length, ptr::null_mut(), &mut bytes) != noErr
            || bytes.is_null()
        {
            return;
        }

        let channels = (asbd.channels_per_frame as usize).min(MAX_AUDIO_CHANNELS);
        let plane_len = length / asbd.channels_per_frame as usize;
        let mut planes: [&[u8]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
        for (i, plane) in planes.iter_mut().enumerate().take(channels) {
            *plane = std::slice::from_raw_parts((bytes as *const u8).add(plane_len * i), plane_len);
        }
        let frames = (length
            / asbd.bytes_per_frame.max(1) as usize
            / asbd.channels_per_frame as usize) as u32;

        shared.hub.output_audio(&SourceAudio {
            planes: &planes[..channels],
            frames,
            spec: AudioSpec {
                rate: asbd.sample_rate as u32,
                speakers: Speakers::from_channels(asbd.channels_per_frame as usize),
                format: SampleFormat::FloatPlanar,
            },
        });
    }
}

struct SckCapture {
    stream: Retained<SCStream>,
    _delegate: Retained<ScreenCaptureDelegate>,
    shared: Arc<SckShared>,
}

// SAFETY: SCStream is thread safe; it is only started and stopped here.
unsafe impl Send for SckCapture {}

impl Capture for SckCapture {
    fn state(&self) -> CaptureState {
        self.shared.state.lock().clone()
    }
}

impl Drop for SckCapture {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |_err: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: stops the stream started in `start_desktop`.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}

/// `init_audio_screen_stream`.
fn start_desktop(hub: Arc<SourceHub>) -> Result<Box<dyn Capture>, String> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = if error.is_null() && !content.is_null() {
                // SAFETY: retained before the block returns.
                unsafe { Retained::retain(content) }.ok_or(())
            } else {
                Err(())
            };
            let _ = tx.send(result.map(SendRetained));
        },
    );
    // SAFETY: asynchronous query answered through the channel.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    let content = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "Screen capture content timed out".to_string())?
        .map_err(|_| "Screen Recording permission is required for desktop audio".to_string())?
        .0;

    // SAFETY: ScreenCaptureKit setup mirroring OBS.
    unsafe {
        let main = CGMainDisplayID();
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|d| d.displayID() == main)
            .ok_or_else(|| "Main display not found".to_string())?;

        let empty = NSArray::new();
        let filter = SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty,
        );

        let config = SCStreamConfiguration::new();
        config.setQueueDepth(8);
        config.setCapturesAudio(true);
        config.setExcludesCurrentProcessAudio(true);
        config.setShowsCursor(false);
        config.setChannelCount(if OBS_CHANNELS > 1 {
            2
        } else {
            OBS_CHANNELS as isize
        });

        let shared = Arc::new(SckShared {
            hub,
            state: Mutex::new(CaptureState::Starting),
        });
        let delegate = ScreenCaptureDelegate::new(shared.clone());
        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*delegate)),
        );

        // A dummy video output silences SCK errors; frames are dropped.
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*delegate),
                SCStreamOutputType::Screen,
                None,
            )
            .map_err(|e| format!("Failed to add video stream output: {}", e.code()))?;
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*delegate),
                SCStreamOutputType::Audio,
                None,
            )
            .map_err(|e| format!("Failed to add audio stream output: {}", e.code()))?;

        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |err: *mut NSError| {
            let _ = tx.send(err.is_null());
        });
        stream.startCaptureWithCompletionHandler(Some(&block));
        let started = rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false);
        if !started {
            return Err("Failed to start capture".into());
        }

        *shared.state.lock() = CaptureState::Active {
            format: describe(OBS_SAMPLE_RATE, 2),
        };
        Ok(Box::new(SckCapture {
            stream,
            _delegate: delegate,
            shared,
        }))
    }
}

struct SendRetained(Retained<SCShareableContent>);
// SAFETY: SCShareableContent is an immutable snapshot.
unsafe impl Send for SendRetained {}

// ---------------------------------------------------------------------------
// Input devices: AUHAL (`mac-audio.c`)
// ---------------------------------------------------------------------------

/// What the render callback owns while the unit runs.
///
/// `coreaudio_init` allocates these buffers and `coreaudio_uninit` frees
/// them, with `AudioOutputUnitStop` in between, so the callback really is
/// the only thread that touches them for as long as they exist. `mac-audio.c`
/// keeps them in the one structure the reconnect thread also mutates and
/// relies on that ordering alone; holding them apart is the same behaviour
/// with the exclusive access made real instead of assumed.
struct RenderState {
    hub: Arc<SourceHub>,
    unit: AudioUnit,
    spec: AudioSpec,
    buffers: Vec<Vec<u8>>,
    /// Backing store for an `AudioBufferList`, 8-byte aligned.
    buf_list: Vec<u64>,
}

impl RenderState {
    fn buf_list(&mut self) -> *mut AudioBufferList {
        self.buf_list.as_mut_ptr() as *mut AudioBufferList
    }
}

/// What the device listener reads. Built once and never changed afterwards,
/// so the listener thread and the reconnect thread only ever share it.
struct DeviceWatch {
    uid: String,
    state: Arc<Mutex<CaptureState>>,
}

struct CoreAudioData {
    hub: Arc<SourceHub>,
    /// Boxed so its address stays put for as long as the listener is
    /// registered against it.
    watch: Box<DeviceWatch>,
    unit: AudioUnit,
    device_id: AudioObjectID,
    au_initialized: bool,
    active: bool,
    spec: AudioSpec,
    /// Handed to the render callback for the lifetime of the unit, and read
    /// back here only once `AudioOutputUnitStop` has returned.
    render: *mut RenderState,
}

// SAFETY: the audio unit and the render state are driven from the reconnect
// thread, which only touches them while the unit is stopped.
unsafe impl Send for CoreAudioData {}

impl CoreAudioData {
    fn uid(&self) -> &str {
        &self.watch.uid
    }

    fn set_state(&self, state: CaptureState) {
        *self.watch.state.lock() = state;
    }
}

fn ca_success(stat: OSStatus, what: &str) -> bool {
    if stat != noErr {
        log::warn!("coreaudio: {what} failed: {stat}");
        false
    } else {
        true
    }
}

/// `convert_ca_format`.
fn convert_ca_format(flags: u32, bits: u32) -> Option<SampleFormat> {
    let planar = flags & kAudioFormatFlagIsNonInterleaved != 0;
    if flags & kAudioFormatFlagIsFloat != 0 {
        return Some(if planar {
            SampleFormat::FloatPlanar
        } else {
            SampleFormat::Float
        });
    }
    if flags & kAudioFormatFlagIsSignedInteger == 0 && bits == 8 {
        return Some(if planar {
            SampleFormat::U8Planar
        } else {
            SampleFormat::U8
        });
    }
    if flags & kAudioFormatFlagIsSignedInteger == 0 {
        return None;
    }
    match bits {
        16 => Some(if planar {
            SampleFormat::S16Planar
        } else {
            SampleFormat::S16
        }),
        32 => Some(if planar {
            SampleFormat::S32Planar
        } else {
            SampleFormat::S32
        }),
        _ => None,
    }
}

impl CoreAudioData {
    /// `coreaudio_init_format` with downmix enabled.
    unsafe fn init_format(&mut self) -> bool {
        let mut input: AudioStreamBasicDescription = Default::default();
        let mut desc: AudioStreamBasicDescription = Default::default();
        let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        unsafe {
            if !ca_success(
                AudioUnitGetProperty(
                    self.unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Input,
                    BUS_INPUT,
                    &mut input as *mut _ as *mut c_void,
                    &mut size,
                ),
                "get input device format",
            ) {
                return false;
            }
            if !ca_success(
                AudioUnitGetProperty(
                    self.unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Output,
                    BUS_INPUT,
                    &mut desc as *mut _ as *mut c_void,
                    &mut size,
                ),
                "get input format",
            ) {
                return false;
            }

            desc.channels_per_frame = input.channels_per_frame.min(64);
            desc.sample_rate = input.sample_rate;
            if !ca_success(
                AudioUnitSetProperty(
                    self.unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Output,
                    BUS_INPUT,
                    &desc as *const _ as *const c_void,
                    size,
                ),
                "set output format",
            ) {
                return false;
            }
        }
        if desc.format_id != kAudioFormatLinearPCM {
            log::warn!("coreaudio: format is not PCM");
            return false;
        }
        let Some(format) = convert_ca_format(desc.format_flags, desc.bits_per_channel) else {
            log::warn!("coreaudio: unknown format flags");
            return false;
        };
        self.spec = AudioSpec {
            rate: desc.sample_rate as u32,
            speakers: Speakers::from_channels(
                desc.channels_per_frame.min(MAX_AUDIO_CHANNELS as u32) as usize,
            ),
            format,
        };
        true
    }

    /// `coreaudio_init_buffer`.
    unsafe fn init_buffer(&mut self) -> bool {
        let mut frames = 0u32;
        let mut size = 4u32;
        let mut desc: AudioStreamBasicDescription = Default::default();
        let mut rate = 0f64;
        unsafe {
            if !ca_success(
                AudioUnitGetProperty(
                    self.unit,
                    kAudioDevicePropertyBufferFrameSize,
                    kAudioUnitScope_Global,
                    0,
                    &mut frames as *mut _ as *mut c_void,
                    &mut size,
                ),
                "get buffer frame size",
            ) {
                return false;
            }
            let mut dsize = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
            if !ca_success(
                AudioUnitGetProperty(
                    self.unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Output,
                    BUS_INPUT,
                    &mut desc as *mut _ as *mut c_void,
                    &mut dsize,
                ),
                "get stream format",
            ) {
                return false;
            }
            let addr = address(
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
            );
            let mut rsize = 8u32;
            if !ca_success(
                AudioObjectGetPropertyData(
                    self.device_id,
                    &addr,
                    0,
                    ptr::null(),
                    &mut rsize,
                    &mut rate as *mut _ as *mut c_void,
                ),
                "get input sample rate",
            ) {
                return false;
            }
        }

        let bytes = frames as usize * std::mem::size_of::<f32>();
        let count = desc.channels_per_frame as usize;
        let bytes_needed = BUFFER_LIST_HEADER + std::mem::size_of::<AudioBuffer>() * count.max(1);
        let mut render = Box::new(RenderState {
            hub: self.hub.clone(),
            unit: self.unit,
            spec: self.spec,
            buffers: (0..count).map(|_| vec![0u8; bytes]).collect(),
            buf_list: vec![0u64; bytes_needed.div_ceil(8)],
        });

        let list = render.buf_list();
        // SAFETY: the byte buffer is sized for `count` AudioBuffer entries.
        unsafe {
            (*list).number_buffers = count as u32;
            let entries = (list as *mut u8).add(BUFFER_LIST_HEADER) as *mut AudioBuffer;
            for (i, buf) in render.buffers.iter_mut().enumerate() {
                entries.add(i).write(AudioBuffer {
                    number_channels: 1,
                    data_byte_size: bytes as u32,
                    data: buf.as_mut_ptr() as *mut c_void,
                });
            }
        }
        // From here the callback owns it, until `uninit` takes it back.
        self.render = Box::into_raw(render);
        true
    }
}

unsafe extern "C" fn input_callback(
    ref_con: *mut c_void,
    action_flags: *mut u32,
    time_stamp: *const AudioTimeStamp,
    bus_number: u32,
    frames: u32,
    _ignored: *mut AudioBufferList,
) -> OSStatus {
    // SAFETY: `ref_con` is the `RenderState` built for this unit. It exists
    // from before `AudioOutputUnitStart` until after `AudioOutputUnitStop`
    // has returned, and nothing else reads it in between, so this reference
    // is the only one alive for the length of the call.
    let render = unsafe { &mut *(ref_con as *mut RenderState) };
    // SAFETY: the list is sized for the buffers, which outlive the render.
    unsafe {
        let list = render.buf_list();
        let entries = (list as *mut u8).add(BUFFER_LIST_HEADER) as *mut AudioBuffer;
        for (i, buf) in render.buffers.iter().enumerate() {
            (*entries.add(i)).data_byte_size = buf.len() as u32;
        }
        let stat = AudioUnitRender(
            render.unit,
            action_flags,
            time_stamp,
            bus_number,
            frames,
            list,
        );
        if !ca_success(stat, "audio retrieval") {
            return noErr;
        }
    }

    let bytes = frames as usize * render.spec.format.bytes_per_sample();
    let mut planes: [&[u8]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
    let count = render.buffers.len().min(MAX_AUDIO_CHANNELS);
    for (plane, buf) in planes.iter_mut().zip(&render.buffers).take(count) {
        *plane = &buf[..bytes.min(buf.len())];
    }
    render.hub.output_audio(&SourceAudio {
        planes: &planes[..count],
        frames,
        spec: render.spec,
    });
    noErr
}

unsafe extern "C" fn device_notification(
    _object: AudioObjectID,
    _count: u32,
    _addresses: *const AudioObjectPropertyAddress,
    client_data: *mut c_void,
) -> OSStatus {
    // SAFETY: `client_data` is the capture's `DeviceWatch`, which is built
    // once and never changed, so every holder only ever shares it.
    let watch = unsafe { &*(client_data as *const DeviceWatch) };
    log::info!("coreaudio: device '{}' disconnected or changed", watch.uid);
    *watch.state.lock() = CaptureState::Retrying("Device disconnected".into());
    noErr
}

impl CoreAudioData {
    /// `coreaudio_init`.
    unsafe fn init(&mut self) -> bool {
        if self.au_initialized {
            return true;
        }
        let Some(id) = device_id_for_uid(self.uid()) else {
            return false;
        };
        self.device_id = id;

        let desc = AudioComponentDescription {
            component_type: kAudioUnitType_Output,
            component_sub_type: kAudioUnitSubType_HALOutput,
            component_manufacturer: kAudioUnitManufacturer_Apple,
            component_flags: 0,
            component_flags_mask: 0,
        };
        unsafe {
            let component = AudioComponentFindNext(ptr::null_mut(), &desc);
            if component.is_null() {
                log::warn!("coreaudio: find component failed");
                return false;
            }
            if !ca_success(
                AudioComponentInstanceNew(component, &mut self.unit),
                "instance unit",
            ) {
                return false;
            }
            self.au_initialized = true;

            let enable: u32 = 1;
            let disable: u32 = 0;
            let ok = ca_success(
                AudioUnitSetProperty(
                    self.unit,
                    kAudioOutputUnitProperty_EnableIO,
                    kAudioUnitScope_Input,
                    BUS_INPUT,
                    &enable as *const _ as *const c_void,
                    4,
                ),
                "enable input io",
            ) && ca_success(
                AudioUnitSetProperty(
                    self.unit,
                    kAudioOutputUnitProperty_EnableIO,
                    kAudioUnitScope_Output,
                    BUS_OUTPUT,
                    &disable as *const _ as *const c_void,
                    4,
                ),
                "disable output io",
            ) && ca_success(
                AudioUnitSetProperty(
                    self.unit,
                    kAudioOutputUnitProperty_CurrentDevice,
                    kAudioUnitScope_Global,
                    0,
                    &self.device_id as *const _ as *const c_void,
                    4,
                ),
                "set current device",
            ) && self.init_format()
                && self.init_buffer()
                && self.init_hooks()
                && ca_success(AudioUnitInitialize(self.unit), "initialize")
                && ca_success(AudioOutputUnitStart(self.unit), "start audio");

            if !ok {
                self.uninit();
                return false;
            }
        }
        self.active = true;
        self.set_state(CaptureState::Active {
            format: describe(self.spec.rate, self.spec.speakers.channels()),
        });
        log::info!(
            "coreaudio: Device '{}' [{} Hz] initialized",
            self.uid(),
            self.spec.rate
        );
        true
    }

    unsafe fn init_hooks(&mut self) -> bool {
        let watch = &*self.watch as *const DeviceWatch as *mut c_void;
        unsafe {
            for selector in [
                kAudioDevicePropertyDeviceIsAlive,
                kAudioStreamPropertyAvailablePhysicalFormats,
            ] {
                let addr = address(selector, kAudioObjectPropertyScopeGlobal);
                if !ca_success(
                    AudioObjectAddPropertyListener(
                        self.device_id,
                        &addr,
                        device_notification,
                        watch,
                    ),
                    "set device callback",
                ) {
                    return false;
                }
            }
            let callback = AURenderCallbackStruct {
                input_proc: input_callback,
                input_proc_ref_con: self.render as *mut c_void,
            };
            ca_success(
                AudioUnitSetProperty(
                    self.unit,
                    kAudioOutputUnitProperty_SetInputCallback,
                    kAudioUnitScope_Global,
                    0,
                    &callback as *const _ as *const c_void,
                    std::mem::size_of::<AURenderCallbackStruct>() as u32,
                ),
                "set input callback",
            )
        }
    }

    unsafe fn remove_hooks(&mut self) {
        let watch = &*self.watch as *const DeviceWatch as *mut c_void;
        unsafe {
            for selector in [
                kAudioDevicePropertyDeviceIsAlive,
                kAudioStreamPropertyAvailablePhysicalFormats,
            ] {
                let addr = address(selector, kAudioObjectPropertyScopeGlobal);
                AudioObjectRemovePropertyListener(
                    self.device_id,
                    &addr,
                    device_notification,
                    watch,
                );
            }
        }
    }

    /// `coreaudio_stop` + `coreaudio_uninit`.
    unsafe fn uninit(&mut self) {
        if !self.au_initialized {
            return;
        }
        unsafe {
            if self.active {
                self.active = false;
                AudioOutputUnitStop(self.unit);
            }
            if !self.unit.is_null() {
                AudioUnitUninitialize(self.unit);
                self.remove_hooks();
                AudioComponentInstanceDispose(self.unit);
                self.unit = ptr::null_mut();
            }
        }
        self.au_initialized = false;
        // The unit is stopped and disposed, so the callback can no longer
        // run: the buffers it owned are ours to free.
        if !self.render.is_null() {
            // SAFETY: the pointer came from `Box::into_raw` in `init_buffer`
            // and is taken back exactly once.
            drop(unsafe { Box::from_raw(self.render) });
            self.render = ptr::null_mut();
        }
    }
}

struct InputCapture {
    /// Read straight through, so asking what the capture is doing never
    /// waits for the reconnect thread to finish opening a device.
    state: Arc<Mutex<CaptureState>>,
    data: Arc<Mutex<Box<CoreAudioData>>>,
    exit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Initial try, then the reconnect thread of `mac-audio.c` (2 s retries).
fn start_input(uid: &str, hub: Arc<SourceHub>) -> Result<Box<dyn Capture>, String> {
    let state = Arc::new(Mutex::new(CaptureState::Starting));
    let data = Arc::new(Mutex::new(Box::new(CoreAudioData {
        hub,
        watch: Box::new(DeviceWatch {
            uid: uid.to_string(),
            state: state.clone(),
        }),
        unit: ptr::null_mut(),
        device_id: 0,
        au_initialized: false,
        active: false,
        spec: AudioSpec {
            rate: OBS_SAMPLE_RATE,
            speakers: OBS_SPEAKERS,
            format: SampleFormat::FloatPlanar,
        },
        render: ptr::null_mut(),
    })));
    let exit = Arc::new(AtomicBool::new(false));

    let thread = {
        let shared = data.clone();
        let exit = exit.clone();
        let state = state.clone();
        std::thread::Builder::new()
            .name("coreaudio: reconnect".into())
            .spawn(move || loop {
                {
                    let mut ca = shared.lock();
                    let lost = matches!(*state.lock(), CaptureState::Retrying(_));
                    // SAFETY: the unit is driven from this thread only.
                    unsafe {
                        if lost {
                            ca.uninit();
                        }
                        if !ca.au_initialized && !ca.init() {
                            ca.set_state(CaptureState::Retrying("Waiting for the device".into()));
                        }
                    }
                }
                for _ in 0..20 {
                    if exit.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|e| e.to_string())?
    };

    Ok(Box::new(InputCapture {
        state,
        data,
        exit,
        thread: Some(thread),
    }))
}

impl Capture for InputCapture {
    fn state(&self) -> CaptureState {
        self.state.lock().clone()
    }
}

impl Drop for InputCapture {
    fn drop(&mut self) {
        self.exit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // SAFETY: the reconnect thread is gone.
        unsafe { self.data.lock().uninit() };
    }
}

pub fn start_capture(source: &str, hub: Arc<SourceHub>) -> Result<Box<dyn Capture>, String> {
    if source == DESKTOP {
        start_desktop(hub)
    } else if let Some(uid) = source
        .strip_prefix(INPUT_PREFIX)
        .or_else(|| source.strip_prefix(OUTPUT_PREFIX))
    {
        start_input(uid, hub)
    } else {
        Err(format!("Unknown source {source}"))
    }
}

// ---------------------------------------------------------------------------
// Monitoring: AudioQueue (`coreaudio-output.c`)
// ---------------------------------------------------------------------------

struct QueueState {
    empty_buffers: VecDeque<AudioQueueBufferRef>,
    new_data: VecDeque<u8>,
    buffer_size: usize,
    wait_size: usize,
    paused: bool,
}

struct QueueMonitor {
    /// UID of the output device, resolved again on every status read so an
    /// unplugged device is noticed.
    device: String,
    queue: AudioQueueRef,
    buffers: [AudioQueueBufferRef; 3],
    /// `monitor->mutex`.
    state: Mutex<QueueState>,
    resampler: Mutex<Resampler>,
    shared: Arc<OutputShared>,
    active: AtomicBool,
    channels: usize,
    format: String,
}

// SAFETY: the queue is thread safe; buffers are guarded by `state`.
unsafe impl Send for QueueMonitor {}
unsafe impl Sync for QueueMonitor {}

impl QueueMonitor {
    /// `fill_buffer`.
    unsafe fn fill_buffer(&self, st: &mut QueueState) -> bool {
        if st.new_data.len() < st.buffer_size {
            return false;
        }
        let Some(buf) = st.empty_buffers.pop_front() else {
            return false;
        };
        unsafe {
            let dst = std::slice::from_raw_parts_mut((*buf).audio_data as *mut u8, st.buffer_size);
            // A ring buffer is at most two runs, so this is two memcpy
            // rather than a branch per byte, inside the queue callback.
            let (front, back) = st.new_data.as_slices();
            let head = front.len().min(st.buffer_size);
            dst[..head].copy_from_slice(&front[..head]);
            dst[head..].copy_from_slice(&back[..st.buffer_size - head]);
            st.new_data.drain(..st.buffer_size);
            (*buf).audio_data_byte_size = st.buffer_size as u32;
            let stat = AudioQueueEnqueueBuffer(self.queue, buf, 0, ptr::null());
            if !ca_success(stat, "AudioQueueEnqueueBuffer") {
                AudioQueueStop(self.queue, 0);
            }
        }
        true
    }
}

/// `buffer_audio`.
unsafe extern "C" fn buffer_audio(
    user_data: *mut c_void,
    _aq: AudioQueueRef,
    buf: AudioQueueBufferRef,
) {
    // SAFETY: user_data is the monitor, alive while the queue exists.
    unsafe {
        let monitor = &*(user_data as *const QueueMonitor);
        let mut st = monitor.state.lock();
        st.empty_buffers.push_back(buf);
        while !st.empty_buffers.is_empty() {
            if !monitor.fill_buffer(&mut st) {
                break;
            }
        }
        if st.empty_buffers.len() == 3 {
            st.paused = true;
            st.wait_size = st.buffer_size * 3;
            AudioQueuePause(monitor.queue);
        }
    }
}

impl AudioCallback for QueueMonitor {
    /// `on_audio_playback`.
    fn on_audio(&self, audio: &ObsAudio) {
        if !self.active.load(Ordering::Relaxed) {
            return;
        }
        let input = [
            audio.planes[0].as_ptr() as *const u8,
            audio.planes[1].as_ptr() as *const u8,
        ];
        let mut rs = self.resampler.lock();
        let Some(frames) = rs.resample(&input, audio.frames) else {
            return;
        };
        let bytes = std::mem::size_of::<f32>() * self.channels * frames as usize;

        let vol = self.shared.volume();
        let data = &mut rs.plane_mut(0)[..bytes];
        // SAFETY: f32 has no invalid bit patterns.
        let (_, floats, _) = unsafe { data.align_to_mut::<f32>() };
        self.shared.apply_f32(floats, vol);

        let mut st = self.state.lock();
        st.new_data.extend(data.iter().copied());
        if st.new_data.len() >= st.wait_size {
            st.wait_size = 0;
            // SAFETY: queue calls under the monitor mutex, as in OBS.
            unsafe {
                while !st.empty_buffers.is_empty() {
                    if !self.fill_buffer(&mut st) {
                        break;
                    }
                }
                if st.paused {
                    AudioQueueStart(self.queue, ptr::null());
                    st.paused = false;
                }
            }
        }
    }
}

impl Monitor for QueueMonitor {
    /// An AudioQueue whose device went away keeps taking buffers and plays
    /// none of them, so the device is what has to be looked at.
    /// `coreaudio-output.c` never does, because OBS rebuilds a monitor only
    /// from `obs_reset_audio_monitoring`; without this the output would stay
    /// silent while the panel reported it as playing. The property is the
    /// one `coreaudio_init_hooks` already watches for an input capture. See
    /// the retry deviation on `MONITOR_RETRY`.
    fn state(&self) -> MonitorState {
        match device_id_for_uid(&self.device) {
            Some(id) if device_is_alive(id) => MonitorState::Playing {
                format: self.format.clone(),
            },
            _ => MonitorState::Reconnecting("Device disconnected".into()),
        }
    }
}

impl Drop for QueueMonitor {
    /// `audio_monitor_free`.
    fn drop(&mut self) {
        // SAFETY: tears down the queue created in `create_monitor`.
        unsafe {
            if self.active.load(Ordering::Relaxed) {
                AudioQueueStop(self.queue, 1);
            }
            for buf in self.buffers {
                if !buf.is_null() {
                    AudioQueueFreeBuffer(self.queue, buf);
                }
            }
            if !self.queue.is_null() {
                AudioQueueDispose(self.queue, 1);
            }
        }
    }
}

/// `audio_monitor_init`. Neither ScreenCaptureKit nor input capture carry
/// `OBS_SOURCE_DO_NOT_SELF_MONITOR`, except loopback drivers.
pub fn create_monitor(
    source: &str,
    device: &str,
    shared: Arc<OutputShared>,
) -> Result<MonitorInit, String> {
    if source.strip_prefix(OUTPUT_PREFIX) == Some(device) {
        return Ok(MonitorInit::Ignored);
    }

    let channels = OBS_SPEAKERS.channels();
    let desc = AudioStreamBasicDescription {
        sample_rate: OBS_SAMPLE_RATE as f64,
        format_id: kAudioFormatLinearPCM,
        format_flags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        bytes_per_packet: (4 * channels) as u32,
        frames_per_packet: 1,
        bytes_per_frame: (4 * channels) as u32,
        channels_per_frame: channels as u32,
        bits_per_channel: 32,
        reserved: 0,
    };
    let buffer_size = channels * 4 * OBS_SAMPLE_RATE as usize / 100 * 3;

    let from = AudioSpec {
        rate: OBS_SAMPLE_RATE,
        speakers: OBS_SPEAKERS,
        format: SampleFormat::FloatPlanar,
    };
    let to = AudioSpec {
        format: SampleFormat::Float,
        ..from
    };
    let resampler =
        Resampler::new(to, from).ok_or_else(|| "Failed to create resampler".to_string())?;

    let mut monitor = Arc::new(QueueMonitor {
        device: device.to_string(),
        queue: ptr::null_mut(),
        buffers: [ptr::null_mut(); 3],
        state: Mutex::new(QueueState {
            empty_buffers: VecDeque::new(),
            new_data: VecDeque::new(),
            buffer_size,
            wait_size: buffer_size * 3,
            paused: false,
        }),
        resampler: Mutex::new(resampler),
        shared,
        active: AtomicBool::new(false),
        channels,
        format: describe(OBS_SAMPLE_RATE, channels),
    });

    let user_data = Arc::as_ptr(&monitor) as *mut c_void;
    let m = Arc::get_mut(&mut monitor).expect("unique monitor");
    // SAFETY: straight port of the AudioQueue setup; the monitor outlives
    // the queue because the queue is disposed in its Drop.
    unsafe {
        let stat = AudioQueueNewOutput(
            &desc,
            buffer_audio,
            user_data,
            ptr::null(),
            ptr::null(),
            0,
            &mut m.queue,
        );
        if !ca_success(stat, "AudioQueueNewOutput") {
            return Err("Failed to create the audio queue".into());
        }

        let uid = std::ffi::CString::new(device).map_err(|e| e.to_string())?;
        let cf_uid = CFStringCreateWithCString(ptr::null(), uid.as_ptr(), kCFStringEncodingUTF8);
        let stat = AudioQueueSetProperty(
            m.queue,
            kAudioQueueProperty_CurrentDevice,
            &cf_uid as *const _ as *const c_void,
            std::mem::size_of::<CFStringRef>() as u32,
        );
        CFRelease(cf_uid);
        if !ca_success(stat, "set current device") {
            return Err("Output device unavailable".into());
        }

        if !ca_success(
            AudioQueueSetParameter(m.queue, kAudioQueueParam_Volume, 1.0),
            "set volume",
        ) {
            return Err("Failed to set the queue volume".into());
        }

        for i in 0..3 {
            let stat = AudioQueueAllocateBuffer(m.queue, buffer_size as u32, &mut m.buffers[i]);
            if !ca_success(stat, "allocation of buffer") {
                return Err("Failed to allocate audio buffers".into());
            }
            m.state.get_mut().empty_buffers.push_back(m.buffers[i]);
        }

        if !ca_success(AudioQueueStart(m.queue, ptr::null()), "start") {
            return Err("Failed to start the audio queue".into());
        }
    }
    m.active.store(true, Ordering::Relaxed);

    Ok(MonitorInit::Active(monitor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_drivers_are_output_captures() {
        assert!(!device_is_input("BlackHole 2ch"));
        assert!(device_is_input("MacBook Pro Microphone"));
    }

    #[test]
    fn ca_formats() {
        assert_eq!(
            convert_ca_format(
                kAudioFormatFlagIsFloat | kAudioFormatFlagIsNonInterleaved,
                32
            ),
            Some(SampleFormat::FloatPlanar)
        );
        assert_eq!(
            convert_ca_format(kAudioFormatFlagIsSignedInteger, 16),
            Some(SampleFormat::S16)
        );
        assert_eq!(convert_ca_format(0, 16), None);
    }
}
