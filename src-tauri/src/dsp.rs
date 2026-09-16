//! Real-time audio processing, independent of any device.
//!
//! The design follows OBS Studio's audio monitoring
//! (`libobs/audio-monitoring/*`): each output gets a queue, a resampler to the
//! device's native format, a prefill before playback starts, and the volume
//! applied right before writing. OBS cuts or delays audio when clocks drift;
//! on top of that, we steer the resampling ratio to keep the queue level steady.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use rtrb::{Consumer, Producer};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, WindowFunction,
};

/// Fixed block size produced by the resampler.
const CHUNK_FRAMES: usize = 128;
/// Maximum ratio deviation allowed for drift correction (0.5%).
const MAX_DRIFT: f64 = 0.005;
/// Volume ramp duration in seconds, to avoid clicks.
const VOLUME_RAMP_SECS: f32 = 0.015;

/// Bounds of the OBS logarithmic fader (`obs-audio-controls.c`).
const LOG_OFFSET_DB: f32 = 6.0;
const LOG_RANGE_DB: f32 = 96.0;

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

/// Fader position (0..1) to linear gain.
pub fn fader_to_gain(def: f32) -> f32 {
    let db = fader_to_db(def);
    if db == f32::NEG_INFINITY || db <= -LOG_RANGE_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Lock-free float shared between the UI and the audio thread.
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

/// Playback state of an output, read by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayState {
    Buffering = 0,
    Playing = 1,
}

/// Values shared between an output's audio thread and the rest of the app.
#[derive(Debug)]
pub struct OutputShared {
    /// Target linear gain.
    pub gain: AtomicF32,
    pub muted: std::sync::atomic::AtomicBool,
    /// Peak since the last read, after volume.
    pub peak: AtomicF32,
    pub underruns: AtomicU64,
    pub state: AtomicU8,
}

impl OutputShared {
    pub fn new(gain: f32, muted: bool) -> Self {
        Self {
            gain: AtomicF32::new(gain),
            muted: std::sync::atomic::AtomicBool::new(muted),
            peak: AtomicF32::new(0.0),
            underruns: AtomicU64::new(0),
            state: AtomicU8::new(PlayState::Buffering as u8),
        }
    }

    pub fn play_state(&self) -> PlayState {
        if self.state.load(Ordering::Relaxed) == PlayState::Playing as u8 {
            PlayState::Playing
        } else {
            PlayState::Buffering
        }
    }
}

/// Copies `src` (interleaved, `in_ch` channels) to `dst` (interleaved,
/// `out_ch` channels). Layouts follow WAVE/SMPTE order: L, R, C, LFE, ...
pub fn remap_channels(src: &[f32], in_ch: usize, dst: &mut [f32], out_ch: usize) {
    let frames = (src.len() / in_ch).min(dst.len() / out_ch);
    let src = &src[..frames * in_ch];
    let dst = &mut dst[..frames * out_ch];

    if in_ch == out_ch {
        dst.copy_from_slice(src);
        return;
    }

    for (i, o) in src.chunks_exact(in_ch).zip(dst.chunks_exact_mut(out_ch)) {
        match (in_ch, out_ch) {
            (1, _) => o.fill(i[0]),
            (_, 1) => {
                let (l, r) = downmix_stereo(i);
                o[0] = (l + r) * 0.5;
            }
            (_, 2) => {
                let (l, r) = downmix_stereo(i);
                o[0] = l;
                o[1] = r;
            }
            (2, _) => {
                // Stereo to multichannel: front left and right only.
                o.fill(0.0);
                o[0] = i[0];
                o[1] = i[1];
            }
            _ => {
                let n = in_ch.min(out_ch);
                o[..n].copy_from_slice(&i[..n]);
                o[n..].fill(0.0);
            }
        }
    }
}

/// Stereo downmix (ITU-R BS.775 coefficients, LFE dropped).
fn downmix_stereo(i: &[f32]) -> (f32, f32) {
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match i.len() {
        1 => (i[0], i[0]),
        2 => (i[0], i[1]),
        3 => (i[0] + K * i[2], i[1] + K * i[2]),
        4 => (i[0] + K * i[2], i[1] + K * i[3]),
        5 => (i[0] + K * i[3], i[1] + K * i[4]),
        6 | 7 => {
            let (c, sl, sr) = (i[2], i[4], i[5]);
            let norm = 1.0 / (1.0 + 2.0 * K);
            (
                (i[0] + K * c + K * sl) * norm,
                (i[1] + K * c + K * sr) * norm,
            )
        }
        _ => {
            let (c, bl, br, sl, sr) = (i[2], i[4], i[5], i[6], i[7]);
            let norm = 1.0 / (1.0 + 3.0 * K);
            (
                (i[0] + K * (c + bl + sl)) * norm,
                (i[1] + K * (c + br + sr)) * norm,
            )
        }
    }
}

/// Pushes captured samples into a queue. Never blocks: when the queue is full
/// (stalled output), extra frames are dropped.
pub fn push_frames(producer: &mut Producer<f32>, samples: &[f32], channels: usize) -> usize {
    let room = producer.slots() / channels * channels;
    let n = room.min(samples.len() / channels * channels);
    if n == 0 {
        return 0;
    }
    if let Ok(chunk) = producer.write_chunk_uninit(n) {
        chunk.fill_from_iter(samples[..n].iter().copied());
    }
    n / channels
}

/// Parameters of a source to output queue.
#[derive(Debug, Clone, Copy)]
pub struct PipeFormat {
    pub in_rate: u32,
    pub in_channels: usize,
    pub out_rate: u32,
    pub out_channels: usize,
    /// Target latency kept in the queue, in source frames.
    pub target_frames: usize,
}

impl PipeFormat {
    /// Queue capacity: one second, and at least eight times the target.
    pub fn ring_capacity(&self) -> usize {
        let frames = (self.in_rate as usize).max(self.target_frames * 8);
        frames * self.in_channels
    }
}

/// Output side reader: pulls the queue, resamples, remaps channels and applies
/// the volume. Everything is preallocated; `render` never allocates.
pub struct Pipe {
    fmt: PipeFormat,
    consumer: Consumer<f32>,
    resampler: Async<f32>,
    shared: std::sync::Arc<OutputShared>,

    in_buf: Vec<f32>,
    res_buf: Vec<f32>,
    pending: Vec<f32>,
    pending_pos: usize,
    pending_len: usize,

    buffering: bool,
    fill_avg: f64,
    drift_tick: u32,
    gain: f32,
    ramp_step: f32,
}

impl Pipe {
    pub fn new(
        fmt: PipeFormat,
        consumer: Consumer<f32>,
        shared: std::sync::Arc<OutputShared>,
    ) -> Result<Self, String> {
        let ratio = fmt.out_rate as f64 / fmt.in_rate as f64;
        let params = SincInterpolationParameters::new(128, WindowFunction::BlackmanHarris2);
        let resampler = Async::<f32>::new_sinc(
            ratio,
            1.0 + MAX_DRIFT * 2.0,
            &params,
            CHUNK_FRAMES,
            fmt.in_channels,
            FixedAsync::Output,
        )
        .map_err(|e| e.to_string())?;

        let in_max = resampler.input_frames_max();
        let out_max = resampler.output_frames_max();
        let gain = if shared.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            shared.gain.load()
        };

        Ok(Self {
            in_buf: vec![0.0; in_max * fmt.in_channels],
            res_buf: vec![0.0; out_max * fmt.in_channels],
            pending: vec![0.0; out_max * fmt.out_channels],
            pending_pos: 0,
            pending_len: 0,
            buffering: true,
            fill_avg: fmt.target_frames as f64,
            drift_tick: 0,
            gain,
            ramp_step: 1.0 / (VOLUME_RAMP_SECS * fmt.out_rate as f32).max(1.0),
            fmt,
            consumer,
            resampler,
            shared,
        })
    }

    fn queued_frames(&self) -> usize {
        self.consumer.slots() / self.fmt.in_channels
    }

    /// Fills `out` (interleaved, `out_channels`). Writes silence until the
    /// queue reaches the target latency, like OBS's `wait_size`.
    pub fn render(&mut self, out: &mut [f32]) {
        let och = self.fmt.out_channels;
        let target = self.fmt.target_frames;

        if self.buffering {
            if self.queued_frames() < target {
                out.fill(0.0);
                self.decay_gain_silent(out.len() / och);
                return;
            }
            self.buffering = false;
            self.fill_avg = target as f64;
            self.shared
                .state
                .store(PlayState::Playing as u8, Ordering::Relaxed);
        }

        // Too much audio queued up: drop the excess (OBS's "dragging" case).
        let queued = self.queued_frames();
        if queued > target * 4 + CHUNK_FRAMES * 2 {
            let drop = (queued - target) * self.fmt.in_channels;
            if let Ok(chunk) = self.consumer.read_chunk(drop) {
                chunk.commit_all();
            }
            self.fill_avg = target as f64;
        }

        let mut written = 0;
        while written < out.len() {
            if self.pending_pos >= self.pending_len && !self.refill() {
                // Nothing left to read: silence, then prefill again.
                out[written..].fill(0.0);
                self.buffering = true;
                self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                self.shared
                    .state
                    .store(PlayState::Buffering as u8, Ordering::Relaxed);
                break;
            }
            let n = (self.pending_len - self.pending_pos).min(out.len() - written);
            out[written..written + n]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
            self.pending_pos += n;
            written += n;
        }

        self.apply_gain(&mut out[..written]);
        self.track_drift();
    }

    /// Produces one resampled block into `pending`. False when the queue is empty.
    fn refill(&mut self) -> bool {
        let ich = self.fmt.in_channels;
        let need = self.resampler.input_frames_next();
        if self.queued_frames() < need {
            return false;
        }
        let Ok(chunk) = self.consumer.read_chunk(need * ich) else {
            return false;
        };
        let (a, b) = chunk.as_slices();
        self.in_buf[..a.len()].copy_from_slice(a);
        self.in_buf[a.len()..a.len() + b.len()].copy_from_slice(b);
        chunk.commit_all();

        let out_frames = self.resampler.output_frames_next();
        let (Ok(input), Ok(mut output)) = (
            InterleavedSlice::new(&self.in_buf[..need * ich], ich, need),
            InterleavedSlice::new_mut(&mut self.res_buf[..out_frames * ich], ich, out_frames),
        ) else {
            return false;
        };
        let produced = match self
            .resampler
            .process_into_buffer(&input, &mut output, None)
        {
            Ok((_, produced)) => produced,
            Err(_) => return false,
        };

        let och = self.fmt.out_channels;
        remap_channels(
            &self.res_buf[..produced * ich],
            ich,
            &mut self.pending[..produced * och],
            och,
        );
        self.pending_pos = 0;
        self.pending_len = produced * och;
        true
    }

    fn target_gain(&self) -> f32 {
        if self.shared.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.shared.gain.load()
        }
    }

    fn apply_gain(&mut self, out: &mut [f32]) {
        let och = self.fmt.out_channels;
        let target = self.target_gain();
        let mut peak = 0f32;

        if (self.gain - target).abs() < f32::EPSILON {
            self.gain = target;
            if (target - 1.0).abs() > f32::EPSILON {
                out.iter_mut().for_each(|s| *s *= target);
            }
            peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        } else {
            for frame in out.chunks_exact_mut(och) {
                if self.gain < target {
                    self.gain = (self.gain + self.ramp_step).min(target);
                } else {
                    self.gain = (self.gain - self.ramp_step).max(target);
                }
                for s in frame {
                    *s *= self.gain;
                    peak = peak.max(s.abs());
                }
            }
        }
        self.shared.peak.raise(peak);
    }

    /// Advances the ramp during silence so it does not replay later.
    fn decay_gain_silent(&mut self, frames: usize) {
        let target = self.target_gain();
        let step = self.ramp_step * frames as f32;
        self.gain = if self.gain < target {
            (self.gain + step).min(target)
        } else {
            (self.gain - step).max(target)
        };
    }

    /// Steers the ratio to keep the queue around the target.
    fn track_drift(&mut self) {
        let target = self.fmt.target_frames as f64;
        self.fill_avg += (self.queued_frames() as f64 - self.fill_avg) * 0.02;
        self.drift_tick += 1;
        if self.drift_tick < 8 {
            return;
        }
        self.drift_tick = 0;

        // Queue too full: consume faster, so use a smaller ratio.
        let err = (self.fill_avg - target) / target.max(1.0);
        let rel = 1.0 - (err * 0.002).clamp(-MAX_DRIFT, MAX_DRIFT);
        let _ = self.resampler.set_resample_ratio_relative(rel, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
    fn atomic_peak_keeps_maximum() {
        let p = AtomicF32::new(0.0);
        p.raise(0.3);
        p.raise(0.1);
        assert_eq!(p.take(), 0.3);
        assert_eq!(p.load(), 0.0);
    }

    #[test]
    fn remap_mono_to_stereo_and_back() {
        let mut st = [0.0; 4];
        remap_channels(&[0.5, -0.25], 1, &mut st, 2);
        assert_eq!(st, [0.5, 0.5, -0.25, -0.25]);

        let mut mono = [0.0; 2];
        remap_channels(&[1.0, 0.0, 0.2, 0.4], 2, &mut mono, 1);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn remap_surround_to_stereo_stays_bounded() {
        let src = [1.0f32; 6];
        let mut dst = [0.0; 2];
        remap_channels(&src, 6, &mut dst, 2);
        assert!(dst.iter().all(|s| *s > 0.5 && *s <= 1.0 + 1e-6));
    }

    #[test]
    fn remap_stereo_to_surround_fills_front_only() {
        let mut dst = [9.0; 6];
        remap_channels(&[0.1, 0.2], 2, &mut dst, 6);
        assert_eq!(dst, [0.1, 0.2, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn push_never_blocks_when_full() {
        let (mut p, _c) = rtrb::RingBuffer::<f32>::new(8);
        assert_eq!(push_frames(&mut p, &[0.0; 6], 2), 3);
        assert_eq!(push_frames(&mut p, &[0.0; 6], 2), 1);
        assert_eq!(push_frames(&mut p, &[0.0; 6], 2), 0);
    }

    fn pipe(in_rate: u32, out_rate: u32, gain: f32) -> (Producer<f32>, Pipe, Arc<OutputShared>) {
        let fmt = PipeFormat {
            in_rate,
            in_channels: 2,
            out_rate,
            out_channels: 2,
            target_frames: in_rate as usize / 20,
        };
        let (p, c) = rtrb::RingBuffer::new(fmt.ring_capacity());
        let shared = Arc::new(OutputShared::new(gain, false));
        let pipe = Pipe::new(fmt, c, shared.clone()).unwrap();
        (p, pipe, shared)
    }

    fn sine(frames: usize, rate: u32, offset: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v =
                    ((i + offset) as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn pipe_prebuffers_before_playing() {
        let (mut p, mut pipe, shared) = pipe(48_000, 48_000, 1.0);
        let mut out = vec![1.0; 960];
        push_frames(&mut p, &sine(1000, 48_000, 0), 2);
        pipe.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(shared.play_state(), PlayState::Buffering);

        push_frames(&mut p, &sine(2000, 48_000, 1000), 2);
        pipe.render(&mut out);
        assert_eq!(shared.play_state(), PlayState::Playing);
    }

    #[test]
    fn pipe_resamples_and_keeps_signal() {
        let (mut p, mut pipe, shared) = pipe(44_100, 48_000, 1.0);
        let mut offset = 0;
        let mut out = vec![0.0; 480 * 2];
        let mut energy = 0.0f32;
        for _ in 0..200 {
            push_frames(&mut p, &sine(441, 44_100, offset), 2);
            offset += 441;
            pipe.render(&mut out);
            energy += out.iter().map(|s| s * s).sum::<f32>();
        }
        assert!(energy > 100.0, "signal lost: {energy}");
        assert!(shared.peak.take() <= 0.51);
        // Matching rates on both sides: almost no underruns.
        assert!(shared.underruns.load(Ordering::Relaxed) <= 1);
    }

    #[test]
    fn pipe_applies_volume_and_mute() {
        let (mut p, mut pipe, shared) = pipe(48_000, 48_000, 0.25);
        let mut out = vec![0.0; 480 * 2];
        let mut offset = 0;
        for _ in 0..50 {
            push_frames(&mut p, &sine(480, 48_000, offset), 2);
            offset += 480;
            pipe.render(&mut out);
        }
        shared.peak.take();
        push_frames(&mut p, &sine(480, 48_000, offset), 2);
        offset += 480;
        pipe.render(&mut out);
        let peak = shared.peak.take();
        assert!(peak > 0.1 && peak <= 0.126, "unexpected peak: {peak}");

        shared.muted.store(true, Ordering::Relaxed);
        for _ in 0..10 {
            push_frames(&mut p, &sine(480, 48_000, offset), 2);
            offset += 480;
            pipe.render(&mut out);
        }
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn pipe_recovers_after_underrun() {
        let (mut p, mut pipe, shared) = pipe(48_000, 48_000, 1.0);
        let mut out = vec![0.0; 480 * 2];
        push_frames(&mut p, &sine(3000, 48_000, 0), 2);
        for _ in 0..10 {
            pipe.render(&mut out);
        }
        assert!(shared.underruns.load(Ordering::Relaxed) >= 1);
        assert_eq!(shared.play_state(), PlayState::Buffering);

        push_frames(&mut p, &sine(3000, 48_000, 3000), 2);
        pipe.render(&mut out);
        assert_eq!(shared.play_state(), PlayState::Playing);
    }

    #[test]
    fn pipe_trims_excess_latency() {
        let (mut p, mut pipe, _) = pipe(48_000, 48_000, 1.0);
        let mut out = vec![0.0; 480 * 2];
        push_frames(&mut p, &sine(40_000, 48_000, 0), 2);
        pipe.render(&mut out);
        assert!(pipe.queued_frames() < 2400 + 480);
    }
}
