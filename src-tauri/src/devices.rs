//! Device enumeration and source resolution.
//!
//! Like OBS, "desktop audio" is a loopback capture of the default output:
//! WASAPI loopback on Windows, a CoreAudio tap on macOS (14.6 and later) and
//! the default sink's `.monitor` source on PulseAudio.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, DeviceId, Host, SupportedStreamConfig};
use serde::Serialize;

/// Special id: the system default output, captured in loopback.
pub const DESKTOP: &str = "desktop";
/// Prefix of a loopback capture of a specific output.
const LOOPBACK_PREFIX: &str = "loopback:";
/// Suffix of PulseAudio monitor sources.
const MONITOR_SUFFIX: &str = ".monitor";

/// Whether cpal can capture output devices directly.
const NATIVE_LOOPBACK: bool = cfg!(any(target_os = "windows", target_os = "macos"));

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
    /// Output currently captured by "Desktop audio".
    pub desktop_output: Option<String>,
}

fn device_id(d: &Device) -> Option<String> {
    d.id().ok().map(|id| id.to_string())
}

fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| d.to_string())
}

pub fn enumerate(host: &Host) -> Result<DeviceList, String> {
    let default_out = default_output_id(host);
    let default_in = host.default_input_device().and_then(|d| device_id(&d));

    let mut list = DeviceList {
        desktop_output: default_out.clone(),
        ..Default::default()
    };
    list.sources.push(SourceInfo {
        id: DESKTOP.into(),
        name: "Desktop audio".into(),
        kind: SourceKind::Desktop,
        is_default: false,
    });

    let devices = host.devices().map_err(|e| e.to_string())?;
    let mut loopbacks = Vec::new();
    let mut captures = Vec::new();

    for d in devices {
        let Some(id) = device_id(&d) else { continue };
        let name = device_name(&d);
        let (is_in, is_out) = match d.description() {
            Ok(desc) => (desc.supports_input(), desc.supports_output()),
            Err(_) => (d.supports_input(), d.supports_output()),
        };
        let is_default_out = default_out.as_deref() == Some(id.as_str());

        if is_out {
            if NATIVE_LOOPBACK {
                loopbacks.push(SourceInfo {
                    id: format!("{LOOPBACK_PREFIX}{id}"),
                    name: name.clone(),
                    kind: SourceKind::Loopback,
                    is_default: is_default_out,
                });
            }
            list.outputs.push(OutputInfo {
                id: id.clone(),
                name: name.clone(),
                is_default: is_default_out,
            });
        }
        if is_in {
            let monitor = monitored_output(&id).is_some();
            let info = SourceInfo {
                is_default: default_in.as_deref() == Some(id.as_str()),
                kind: if monitor {
                    SourceKind::Loopback
                } else {
                    SourceKind::Capture
                },
                id,
                name,
            };
            if monitor {
                loopbacks.push(info);
            } else {
                captures.push(info);
            }
        }
    }

    list.sources.extend(loopbacks);
    list.sources.extend(captures);
    Ok(list)
}

/// Sink watched by a PulseAudio `xxx.monitor` source.
fn monitored_output(source_id: &str) -> Option<String> {
    source_id
        .strip_suffix(MONITOR_SUFFIX)
        .filter(|_| source_id.starts_with("pulseaudio:"))
        .map(str::to_string)
}

fn find(host: &Host, id: &str) -> Result<Device, String> {
    let parsed: DeviceId = id.parse().map_err(|e: cpal::Error| e.to_string())?;
    host.device_by_id(&parsed)
        .ok_or_else(|| "Device not found".to_string())
}

/// Id of the default output, used to follow its changes.
pub fn default_output_id(host: &Host) -> Option<String> {
    host.default_output_device().and_then(|d| device_id(&d))
}

pub struct ResolvedSource {
    pub device: Device,
    pub config: SupportedStreamConfig,
    /// Output captured in loopback, never to be used as a destination
    /// (the equivalent of `OBS_SOURCE_DO_NOT_SELF_MONITOR`).
    pub loopback_of: Option<String>,
}

pub fn resolve_source(host: &Host, id: &str) -> Result<ResolvedSource, String> {
    let (device, loopback_of, from_output) = if id == DESKTOP {
        let out = default_output_id(host).ok_or_else(|| "No default output".to_string())?;
        if NATIVE_LOOPBACK {
            (find(host, &out)?, Some(out), true)
        } else {
            let monitor = format!("{out}{MONITOR_SUFFIX}");
            let dev = find(host, &monitor)
                .map_err(|_| "Desktop capture needs PulseAudio or PipeWire".to_string())?;
            (dev, Some(out), false)
        }
    } else if let Some(out) = id.strip_prefix(LOOPBACK_PREFIX) {
        (find(host, out)?, Some(out.to_string()), true)
    } else {
        (find(host, id)?, monitored_output(id), false)
    };

    let config = if from_output {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(|e| e.to_string())?;

    Ok(ResolvedSource {
        device,
        config,
        loopback_of,
    })
}

pub fn resolve_output(host: &Host, id: &str) -> Result<(Device, SupportedStreamConfig), String> {
    let device = find(host, id)?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    Ok((device, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_monitor_maps_to_its_sink() {
        assert_eq!(
            monitored_output("pulseaudio:alsa_output.pci.analog-stereo.monitor").as_deref(),
            Some("pulseaudio:alsa_output.pci.analog-stereo")
        );
        assert_eq!(monitored_output("pulseaudio:alsa_input.usb"), None);
        assert_eq!(monitored_output("alsa:foo.monitor"), None);
    }

    #[test]
    fn enumeration_does_not_panic_without_hardware() {
        // CI machines have no sound card; a clean error is fine.
        let host = cpal::default_host();
        if let Ok(list) = enumerate(&host) {
            assert_eq!(list.sources[0].id, DESKTOP);
        }
    }
}
