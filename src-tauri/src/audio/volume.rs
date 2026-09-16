//! Volume handling shared by every monitor.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Bounds of the OBS logarithmic fader (`obs-audio-controls.c`).
const LOG_OFFSET_DB: f32 = 6.0;
const LOG_RANGE_DB: f32 = 96.0;
/// `EPSILON` from `util/c99defs.h` / `close_float`.
const EPSILON: f32 = 1e-4;

/// Fader position (0..1) to decibels, `OBS_FADER_LOG` curve.
pub fn fader_to_db(def: f32) -> f32 {
    if def >= 1.0 {
        0.0
    } else if def <= 0.0 {
        f32::NEG_INFINITY
    } else {
        -(LOG_RANGE_DB + LOG_OFFSET_DB)
            * ((LOG_RANGE_DB + LOG_OFFSET_DB) / LOG_OFFSET_DB).powf(-def)
            + LOG_OFFSET_DB
    }
}

/// Fader position (0..1) to linear gain, as `obs_fader` does (`db_to_mul`).
pub fn fader_to_gain(def: f32) -> f32 {
    let db = fader_to_db(def);
    if db.is_finite() {
        10f32.powf(db / 20.0)
    } else {
        0.0
    }
}

/// Lock-free float.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    /// Keeps the maximum of the current value and `v`.
    pub fn raise(&self, v: f32) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                (v > f32::from_bits(cur)).then_some(v.to_bits())
            });
    }
    /// Reads the value and resets it to zero.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0f32.to_bits(), Ordering::Relaxed))
    }
}

/// Per-output controls. OBS reads `source->user_volume` on every packet;
/// this is the per-output equivalent, plus a mute.
#[derive(Debug)]
pub struct OutputShared {
    pub gain: AtomicF32,
    pub muted: AtomicBool,
    /// Peak written since the last read, after volume.
    pub peak: AtomicF32,
}

impl OutputShared {
    pub fn new(gain: f32, muted: bool) -> Self {
        Self {
            gain: AtomicF32::new(gain),
            muted: AtomicBool::new(muted),
            peak: AtomicF32::new(0.0),
        }
    }

    /// Volume for this packet.
    pub fn volume(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.gain.load()
        }
    }

    /// `apply volume` step of the OBS monitors: skipped at unity gain.
    pub fn apply_f32(&self, samples: &mut [f32], vol: f32) {
        if !close_float(vol, 1.0) {
            samples.iter_mut().for_each(|s| *s *= vol);
        }
        self.peak
            .raise(samples.iter().fold(0f32, |m, s| m.max(s.abs())));
    }

    /// Records the peak of already scaled integer samples.
    pub fn record_peak(&self, peak: f32) {
        self.peak.raise(peak);
    }
}

pub fn close_float(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPSILON
}

/// `process_volume` of the PulseAudio monitor, for integer sample formats.
/// Scaling is skipped at unity gain; the peak is always measured.
pub fn scale_u8(samples: &mut [u8], vol: f32) -> f32 {
    let unity = close_float(vol, 1.0);
    let mut peak = 0f32;
    for s in samples {
        if !unity {
            *s = ((*s as i32 - 128) as f32 * vol + 128.0) as u8;
        }
        peak = peak.max(((*s as f32) - 128.0).abs() / 128.0);
    }
    peak
}

pub fn scale_s16(samples: &mut [u8], vol: f32) -> f32 {
    let unity = close_float(vol, 1.0);
    let mut peak = 0f32;
    for c in samples.chunks_exact_mut(2) {
        let mut v = i16::from_ne_bytes([c[0], c[1]]);
        if !unity {
            v = (v as f32 * vol) as i16;
            c.copy_from_slice(&v.to_ne_bytes());
        }
        peak = peak.max((v as f32 / 32768.0).abs());
    }
    peak
}

pub fn scale_s32(samples: &mut [u8], vol: f32) -> f32 {
    let unity = close_float(vol, 1.0);
    let mut peak = 0f32;
    for c in samples.chunks_exact_mut(4) {
        let mut v = i32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
        if !unity {
            v = (v as f32 * vol) as i32;
            c.copy_from_slice(&v.to_ne_bytes());
        }
        peak = peak.max((v as f32 / 2_147_483_648.0).abs());
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_matches_obs_curve() {
        assert_eq!(fader_to_db(1.0), 0.0);
        assert_eq!(fader_to_db(0.0), f32::NEG_INFINITY);
        // Reference value computed with OBS's C formula.
        assert!((fader_to_db(0.5) - (-18.739)).abs() < 0.01);
        assert_eq!(fader_to_gain(0.0), 0.0);
        assert!((fader_to_gain(1.0) - 1.0).abs() < 1e-6);
        let mut prev = 0.0;
        for i in 1..=100 {
            let g = fader_to_gain(i as f32 / 100.0);
            assert!(g >= prev, "curve is not monotonic at {i}");
            prev = g;
        }
    }

    #[test]
    fn volume_is_skipped_at_unity_and_mute_wins() {
        let shared = OutputShared::new(1.0, false);
        let mut s = [0.5f32, -0.25];
        shared.apply_f32(&mut s, shared.volume());
        assert_eq!(s, [0.5, -0.25]);
        assert_eq!(shared.peak.take(), 0.5);

        shared.muted.store(true, Ordering::Relaxed);
        shared.apply_f32(&mut s, shared.volume());
        assert_eq!(s, [0.0, -0.0]);
    }

    #[test]
    fn integer_scaling() {
        let mut s16 = 1000i16.to_ne_bytes();
        scale_s16(&mut s16, 0.5);
        assert_eq!(i16::from_ne_bytes(s16), 500);
        let mut u8s = [228u8];
        scale_u8(&mut u8s, 0.5);
        assert_eq!(u8s[0], 178);

        let mut unity = 16384i16.to_ne_bytes();
        assert!((scale_s16(&mut unity, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(i16::from_ne_bytes(unity), 16384);
    }
}
