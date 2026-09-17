//! Status messages the engine reports to the user interface.
//!
//! The port keeps OBS's English wording, which is also what reaches the log,
//! but every message carries a stable code so the interface can translate it
//! without matching on the text. `ui/i18n.js` keys its table on these codes,
//! and [`tests::codes_are_translated`] checks that the two stay in step.

use std::fmt;

use serde::Serialize;

/// One kind of message: a stable code and its English wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub code: &'static str,
    pub text: &'static str,
}

impl Kind {
    /// The same message with a detail the interface cannot translate, such
    /// as an `HRESULT` or an error string that the system already localized.
    pub fn detail(self, detail: impl fmt::Display) -> Message {
        Message {
            code: self.code,
            text: self.text,
            detail: Some(detail.to_string()),
        }
    }
}

/// A message on its way to the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    /// Translation key, stable across wording changes.
    pub code: &'static str,
    /// English wording, used when the interface has no translation.
    pub text: &'static str,
    pub detail: Option<String>,
}

impl From<Kind> for Message {
    fn from(kind: Kind) -> Self {
        Message {
            code: kind.code,
            text: kind.text,
            detail: None,
        }
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.detail, self.text.is_empty()) {
            (Some(detail), true) => write!(f, "{detail}"),
            (Some(detail), false) => write!(f, "{}: {detail}", self.text),
            (None, _) => write!(f, "{}", self.text),
        }
    }
}

const fn kind(code: &'static str, text: &'static str) -> Kind {
    Kind { code, text }
}

/// An error the system reported in its own words; only the detail is shown.
pub const SYSTEM: Kind = kind("system", "");

// Shared by every backend.
pub const CAPTURED_BY_SOURCE: Kind = kind("capturedBySource", "Captured by the source");
pub const DEVICE_DISCONNECTED: Kind = kind("deviceDisconnected", "Device disconnected");
pub const DEVICE_UNAVAILABLE: Kind = kind("deviceUnavailable", "Device unavailable");
pub const UNKNOWN_SOURCE: Kind = kind("unknownSource", "Unknown source");
pub const RESAMPLER: Kind = kind("resampler", "Failed to create resampler");

// Linux (PulseAudio).
pub const PULSE_UNAVAILABLE: Kind = kind("pulseUnavailable", "PulseAudio is not available");
pub const SERVER_INFO: Kind = kind("serverInfo", "Unable to get server info");
pub const SOURCE_INFO: Kind = kind(
    "sourceInfo",
    "An error occurred while getting the source info",
);
pub const SAMPLE_SPEC: Kind = kind("sampleSpec", "Sample spec is not valid");
pub const STREAM_CREATE: Kind = kind("streamCreate", "Unable to create stream");
pub const STREAM_CONNECT: Kind = kind("streamConnect", "Unable to connect to stream");
pub const INVALID_DEVICE_NAME: Kind = kind("invalidDeviceName", "Invalid device name");

// macOS (ScreenCaptureKit, AUHAL, AudioQueue).
pub const SCREEN_PERMISSION: Kind = kind(
    "screenPermission",
    "Screen Recording permission is required for desktop audio",
);
pub const SCREEN_TIMEOUT: Kind = kind("screenTimeout", "Screen capture content timed out");
pub const MAIN_DISPLAY: Kind = kind("mainDisplay", "Main display not found");
pub const VIDEO_STREAM_OUTPUT: Kind =
    kind("videoStreamOutput", "Failed to add video stream output");
pub const AUDIO_STREAM_OUTPUT: Kind =
    kind("audioStreamOutput", "Failed to add audio stream output");
pub const CAPTURE_START: Kind = kind("captureStart", "Failed to start capture");
pub const STREAM_STOPPED: Kind = kind("streamStopped", "Stream stopped with error");
pub const WAITING_FOR_DEVICE: Kind = kind("waitingForDevice", "Waiting for the device");
pub const QUEUE_CREATE: Kind = kind("queueCreate", "Failed to create the audio queue");
pub const OUTPUT_UNAVAILABLE: Kind = kind("outputUnavailable", "Output device unavailable");
pub const QUEUE_VOLUME: Kind = kind("queueVolume", "Failed to set the queue volume");
pub const QUEUE_BUFFERS: Kind = kind("queueBuffers", "Failed to allocate audio buffers");
pub const QUEUE_START: Kind = kind("queueStart", "Failed to start the audio queue");

// Windows (WASAPI). Each of these carries the `HRESULT` as its detail.
pub const WASAPI_ENUMERATOR: Kind = kind("wasapiEnumerator", "Failed to create enumerator");
pub const WASAPI_DEFAULT_ENDPOINT: Kind =
    kind("wasapiDefaultEndpoint", "Failed GetDefaultAudioEndpoint");
pub const WASAPI_ENUMERATE_DEVICE: Kind =
    kind("wasapiEnumerateDevice", "Failed to enumerate device");
pub const WASAPI_GET_DEVICE: Kind = kind("wasapiGetDevice", "Failed to get device");
pub const WASAPI_ACTIVATE: Kind = kind("wasapiActivate", "Failed to activate device");
pub const WASAPI_ACTIVATE_CLIENT: Kind =
    kind("wasapiActivateClient", "Failed to activate client context");
pub const WASAPI_MIX_FORMAT: Kind = kind("wasapiMixFormat", "Failed to get mix format");
pub const WASAPI_INITIALIZE: Kind = kind("wasapiInitialize", "Failed to initialize audio client");
pub const WASAPI_CAPTURE_CLIENT: Kind =
    kind("wasapiCaptureClient", "Failed to create capture context");
pub const WASAPI_EVENT_HANDLE: Kind = kind("wasapiEventHandle", "Failed to set event handle");
pub const WASAPI_START_CAPTURE: Kind = kind("wasapiStartCapture", "Failed to start capture client");
pub const WASAPI_BUFFER_SIZE: Kind = kind("wasapiBufferSize", "Failed to get buffer size");
pub const WASAPI_RENDER_CLIENT: Kind = kind("wasapiRenderClient", "Failed to get render client");
pub const WASAPI_GET_BUFFER: Kind = kind("wasapiGetBuffer", "Failed to get buffer");
pub const WASAPI_START_RENDER: Kind = kind("wasapiStartRender", "Failed to start audio");
// The monitoring side of OBS words three of them differently.
pub const WASAPI_MONITOR_ENUMERATOR: Kind = kind(
    "wasapiMonitorEnumerator",
    "Failed to create IMMDeviceEnumerator",
);
pub const WASAPI_MONITOR_INITIALIZE: Kind = kind("wasapiMonitorInitialize", "Failed to initialize");
pub const WASAPI_MONITOR_RENDER_CLIENT: Kind = kind(
    "wasapiMonitorRenderClient",
    "Failed to get IAudioRenderClient",
);

/// Every kind, for the tests and for anyone auditing the translations.
pub const ALL: &[Kind] = &[
    SYSTEM,
    CAPTURED_BY_SOURCE,
    DEVICE_DISCONNECTED,
    DEVICE_UNAVAILABLE,
    UNKNOWN_SOURCE,
    RESAMPLER,
    PULSE_UNAVAILABLE,
    SERVER_INFO,
    SOURCE_INFO,
    SAMPLE_SPEC,
    STREAM_CREATE,
    STREAM_CONNECT,
    INVALID_DEVICE_NAME,
    SCREEN_PERMISSION,
    SCREEN_TIMEOUT,
    MAIN_DISPLAY,
    VIDEO_STREAM_OUTPUT,
    AUDIO_STREAM_OUTPUT,
    CAPTURE_START,
    STREAM_STOPPED,
    WAITING_FOR_DEVICE,
    QUEUE_CREATE,
    OUTPUT_UNAVAILABLE,
    QUEUE_VOLUME,
    QUEUE_BUFFERS,
    QUEUE_START,
    WASAPI_ENUMERATOR,
    WASAPI_DEFAULT_ENDPOINT,
    WASAPI_ENUMERATE_DEVICE,
    WASAPI_GET_DEVICE,
    WASAPI_ACTIVATE,
    WASAPI_ACTIVATE_CLIENT,
    WASAPI_MIX_FORMAT,
    WASAPI_INITIALIZE,
    WASAPI_CAPTURE_CLIENT,
    WASAPI_EVENT_HANDLE,
    WASAPI_START_CAPTURE,
    WASAPI_BUFFER_SIZE,
    WASAPI_RENDER_CLIENT,
    WASAPI_GET_BUFFER,
    WASAPI_START_RENDER,
    WASAPI_MONITOR_ENUMERATOR,
    WASAPI_MONITOR_INITIALIZE,
    WASAPI_MONITOR_RENDER_CLIENT,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        let mut codes: Vec<&str> = ALL.iter().map(|k| k.code).collect();
        codes.sort_unstable();
        let count = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), count, "two kinds share a code");
    }

    /// The interface translates by code, so every code must appear in the
    /// table of `ui/i18n.js`. A new message without a translation fails here
    /// instead of silently reaching the user in English.
    #[test]
    fn codes_are_translated() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/i18n.js");
        let js = std::fs::read_to_string(path).expect("ui/i18n.js");
        let missing: Vec<&str> = ALL
            .iter()
            .map(|k| k.code)
            .filter(|code| *code != SYSTEM.code && !js.contains(&format!("\"{code}\"")))
            .collect();
        assert!(
            missing.is_empty(),
            "not translated in ui/i18n.js: {missing:?}"
        );
    }

    #[test]
    fn detail_is_appended() {
        assert_eq!(
            Message::from(SAMPLE_SPEC).to_string(),
            "Sample spec is not valid"
        );
        assert_eq!(
            WASAPI_GET_DEVICE.detail("80070005").to_string(),
            "Failed to get device: 80070005"
        );
        assert_eq!(SYSTEM.detail("Accès refusé").to_string(), "Accès refusé");
    }
}
