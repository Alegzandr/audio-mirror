//! Port of the audio half of `obs_source_t` (`libobs/obs-source.c`):
//! `obs_source_output_audio` converts incoming audio to the OBS output
//! format (`process_audio` / `reset_resampler`) and hands it to every
//! registered capture callback on the capture thread
//! (`source_signal_audio_data`). Monitors are those callbacks.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::format::{
    as_f32, AudioSpec, ObsAudio, SourceAudio, MAX_AUDIO_CHANNELS, OBS_CHANNELS, OBS_FORMAT,
    OBS_SAMPLE_RATE, OBS_SPEAKERS,
};
use super::swr::Resampler;

/// `obs_source_audio_capture_t`.
pub trait AudioCallback: Send + Sync {
    fn on_audio(&self, audio: &ObsAudio);
}

pub type CallbackId = u64;

struct ProcessState {
    sample_info: Option<AudioSpec>,
    resampler: Option<Resampler>,
    audio_failed: bool,
    /// `source->audio_data`, one `f32` buffer per OBS channel.
    storage: [Vec<f32>; OBS_CHANNELS],
}

/// `audio_cb_list`. Behind an `Arc` so the capture thread can take the list
/// and call the monitors without holding the lock: one device blocking in
/// `on_audio` then holds up neither the other outputs nor a monitor being
/// added or removed.
type Callbacks = Arc<Vec<(CallbackId, Arc<dyn AudioCallback>)>>;

pub struct SourceHub {
    process: Mutex<ProcessState>,
    /// Guarded like `audio_cb_mutex`.
    callbacks: Mutex<Callbacks>,
    next_id: Mutex<CallbackId>,
    /// Peak of the converted audio since the last read, for the UI.
    peak: AtomicU32,
}

impl Default for SourceHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceHub {
    pub fn new() -> Self {
        Self {
            process: Mutex::new(ProcessState {
                sample_info: None,
                resampler: None,
                audio_failed: false,
                storage: [Vec::new(), Vec::new()],
            }),
            callbacks: Mutex::new(Callbacks::default()),
            next_id: Mutex::new(1),
            peak: AtomicU32::new(0f32.to_bits()),
        }
    }

    /// `obs_source_add_audio_capture_callback`.
    pub fn add_callback(&self, cb: Arc<dyn AudioCallback>) -> CallbackId {
        let id = {
            let mut next = self.next_id.lock();
            let id = *next;
            *next += 1;
            id
        };
        Arc::make_mut(&mut self.callbacks.lock()).push((id, cb));
        id
    }

    /// `obs_source_remove_audio_capture_callback`.
    pub fn remove_callback(&self, id: CallbackId) {
        Arc::make_mut(&mut self.callbacks.lock()).retain(|(i, _)| *i != id);
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0f32.to_bits(), Ordering::Relaxed))
    }

    /// `obs_source_output_audio`. Called on the capture thread.
    pub fn output_audio(&self, audio: &SourceAudio) {
        let mut st = self.process.lock();
        let Some(frames) = process_audio(&mut st, audio) else {
            return;
        };

        let planes = [&st.storage[0][..frames], &st.storage[1][..frames]];
        let peak = planes
            .iter()
            .flat_map(|p| p.iter())
            .fold(0f32, |m, s| m.max(s.abs()));
        let _ = self
            .peak
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                (peak > f32::from_bits(cur)).then_some(peak.to_bits())
            });

        let data = ObsAudio {
            planes,
            frames: frames as u32,
        };
        // `source_signal_audio_data`: newest callback first. A monitor
        // removed while a packet is on its way still receives it, and its
        // last `Arc` is released here rather than in the supervisor.
        let callbacks = self.callbacks.lock().clone();
        for (_, cb) in callbacks.iter().rev() {
            cb.on_audio(&data);
        }
    }
}

fn obs_spec() -> AudioSpec {
    AudioSpec {
        rate: OBS_SAMPLE_RATE,
        speakers: OBS_SPEAKERS,
        format: OBS_FORMAT,
    }
}

/// `reset_resampler`.
fn reset_resampler(st: &mut ProcessState, spec: AudioSpec) {
    st.sample_info = Some(spec);
    st.resampler = None;
    if spec == obs_spec() {
        st.audio_failed = false;
        return;
    }
    st.resampler = Resampler::new(obs_spec(), spec);
    st.audio_failed = st.resampler.is_none();
    if st.audio_failed {
        log::error!("creation of resampler failed");
    }
}

/// `process_audio` + `copy_audio_data`. Returns the frame count stored.
fn process_audio(st: &mut ProcessState, audio: &SourceAudio) -> Option<usize> {
    if st.sample_info != Some(audio.spec) {
        reset_resampler(st, audio.spec);
    }
    if st.audio_failed {
        return None;
    }

    let frames = match st.resampler.as_mut() {
        Some(rs) => {
            let mut ptrs = [std::ptr::null::<u8>(); MAX_AUDIO_CHANNELS];
            for (slot, plane) in ptrs.iter_mut().zip(audio.planes) {
                *slot = plane.as_ptr();
            }
            let n = rs.resample(&ptrs[..audio.planes.len()], audio.frames)? as usize;
            for ch in 0..OBS_CHANNELS {
                let samples = as_f32(rs.plane(ch));
                copy_plane(&mut st.storage[ch], &samples[..n]);
            }
            n
        }
        None => {
            let n = audio.frames as usize;
            // Already in the OBS format, so the backend owes us one plane per
            // channel holding `frames` samples. Dropping a packet that does
            // not is better than taking the capture thread down with it.
            if audio.planes.len() < OBS_CHANNELS {
                return None;
            }
            for ch in 0..OBS_CHANNELS {
                let samples = as_f32(audio.planes[ch]);
                if samples.len() < n {
                    return None;
                }
                copy_plane(&mut st.storage[ch], &samples[..n]);
            }
            n
        }
    };
    Some(frames)
}

fn copy_plane(dst: &mut Vec<f32>, src: &[f32]) {
    if dst.len() < src.len() {
        dst.resize(src.len(), 0.0);
    }
    dst[..src.len()].copy_from_slice(src);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::format::{SampleFormat, Speakers};

    struct Probe(Mutex<Vec<(u32, f32)>>);

    impl AudioCallback for Probe {
        fn on_audio(&self, audio: &ObsAudio) {
            self.0
                .lock()
                .push((audio.frames, audio.planes[1][audio.frames as usize - 1]));
        }
    }

    fn interleaved(frames: usize, channels: usize, value: f32) -> Vec<u8> {
        std::iter::repeat_n(value, frames * channels)
            .flat_map(f32::to_ne_bytes)
            .collect()
    }

    #[test]
    fn converts_then_signals_callbacks() {
        let hub = SourceHub::new();
        let probe = Arc::new(Probe(Mutex::new(Vec::new())));
        let id = hub.add_callback(probe.clone());

        let spec = AudioSpec {
            rate: 48_000,
            speakers: Speakers::Mono,
            format: SampleFormat::Float,
        };
        let buf = interleaved(480, 1, 0.5);
        hub.output_audio(&SourceAudio {
            planes: &[&buf],
            frames: 480,
            spec,
        });
        let calls = probe.0.lock().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 480);
        assert!(
            (calls[0].1 - 0.5).abs() < 1e-3,
            "mono reaches the right channel"
        );
        assert!(hub.take_peak() > 0.49);

        hub.remove_callback(id);
        hub.output_audio(&SourceAudio {
            planes: &[&buf],
            frames: 480,
            spec,
        });
        assert_eq!(probe.0.lock().len(), 1);
    }

    #[test]
    fn obs_format_passes_through() {
        let hub = SourceHub::new();
        let probe = Arc::new(Probe(Mutex::new(Vec::new())));
        hub.add_callback(probe.clone());
        let l = interleaved(100, 1, 0.1);
        let r = interleaved(100, 1, 0.2);
        hub.output_audio(&SourceAudio {
            planes: &[&l, &r],
            frames: 100,
            spec: obs_spec(),
        });
        assert_eq!(probe.0.lock()[0], (100, 0.2));
    }
}
