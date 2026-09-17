//! Port of `libobs/media-io/audio-resampler-ffmpeg.c` on top of FFmpeg's
//! libswresample, linked statically (see `build.rs`).

use std::ffi::{c_int, c_void};
use std::ptr;

use super::format::{AudioSpec, SampleFormat, Speakers, MAX_AUDIO_CHANNELS};

#[repr(C)]
struct SwrContext {
    _private: [u8; 0],
}

/// `AVChannelLayout` (FFmpeg 5.1 and later).
#[repr(C)]
#[derive(Clone, Copy)]
struct AvChannelLayout {
    order: c_int,
    nb_channels: c_int,
    /// Union of `uint64_t mask` and `AVChannelCustom *map`.
    mask: u64,
    opaque: *mut c_void,
}

const AV_CHANNEL_ORDER_NATIVE: c_int = 1;
/// `AV_CH_LAYOUT_4POINT1`: FL | FR | FC | LFE | BC.
const AV_CH_LAYOUT_4POINT1: u64 = 0x1 | 0x2 | 0x4 | 0x8 | 0x100;
const AV_ROUND_UP: c_int = 3;
/// `SWR_FLAG_RESAMPLE`: run the resampling filter even at equal rates, so
/// the ratio can be moved.
const SWR_FLAG_RESAMPLE: i64 = 1;

unsafe extern "C" {
    fn av_channel_layout_default(ch_layout: *mut AvChannelLayout, nb_channels: c_int);
    fn av_rescale_rnd(a: i64, b: i64, c: i64, rnd: c_int) -> i64;
    fn av_opt_set_int(
        obj: *mut c_void,
        name: *const std::ffi::c_char,
        val: i64,
        search_flags: c_int,
    ) -> c_int;
    fn swr_alloc_set_opts2(
        ps: *mut *mut SwrContext,
        out_ch_layout: *const AvChannelLayout,
        out_sample_fmt: c_int,
        out_sample_rate: c_int,
        in_ch_layout: *const AvChannelLayout,
        in_sample_fmt: c_int,
        in_sample_rate: c_int,
        log_offset: c_int,
        log_ctx: *mut c_void,
    ) -> c_int;
    fn swr_set_matrix(s: *mut SwrContext, matrix: *const f64, stride: c_int) -> c_int;
    fn swr_init(s: *mut SwrContext) -> c_int;
    fn swr_free(s: *mut *mut SwrContext);
    fn swr_set_compensation(
        s: *mut SwrContext,
        sample_delta: c_int,
        compensation_distance: c_int,
    ) -> c_int;
    fn swr_get_delay(s: *mut SwrContext, base: i64) -> i64;
    fn swr_convert(
        s: *mut SwrContext,
        out: *const *mut u8,
        out_count: c_int,
        input: *const *const u8,
        in_count: c_int,
    ) -> c_int;
}

/// `convert_audio_format`: OBS format to `AVSampleFormat`.
fn av_sample_format(format: SampleFormat) -> c_int {
    match format {
        SampleFormat::U8 => 0,
        SampleFormat::S16 => 1,
        SampleFormat::S32 => 2,
        SampleFormat::Float => 3,
        SampleFormat::U8Planar => 5,
        SampleFormat::S16Planar => 6,
        SampleFormat::S32Planar => 7,
        SampleFormat::FloatPlanar => 8,
    }
}

fn channel_layout(speakers: Speakers) -> AvChannelLayout {
    let mut layout = AvChannelLayout {
        order: 0,
        nb_channels: 0,
        mask: 0,
        opaque: ptr::null_mut(),
    };
    if speakers == Speakers::FourPointOne {
        layout.order = AV_CHANNEL_ORDER_NATIVE;
        layout.nb_channels = 5;
        layout.mask = AV_CH_LAYOUT_4POINT1;
    } else {
        // SAFETY: plain FFmpeg call on a stack value.
        unsafe { av_channel_layout_default(&mut layout, speakers.channels() as c_int) };
    }
    layout
}

/// Mono upmix matrix used by OBS, verbatim: the mono channel feeds every
/// speaker, and the LFE too in 4.0 and 5.1 but not in 2.1, 4.1 or 7.1.
const MONO_UPMIX: [[f64; MAX_AUDIO_CHANNELS]; MAX_AUDIO_CHANNELS] = [
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0],
    [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0],
    [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0],
];

/// An output plane, kept aligned for `f32`.
///
/// OBS allocates these with `av_samples_alloc` and its monitors then cast
/// straight to `float *` to apply the volume, with no alignment check
/// (`coreaudio-output.c`, `wasapi-output.c`). Our monitors go through
/// `align_to` instead, which silently leaves samples unscaled on a plane
/// that is not aligned: a mute that does not mute. Backing the bytes with a
/// `Vec<f32>` restores the guarantee OBS relies on.
#[derive(Clone, Default)]
struct Plane {
    samples: Vec<f32>,
    bytes: usize,
}

impl Plane {
    fn resize(&mut self, bytes: usize) {
        self.samples.resize(bytes.div_ceil(4), 0.0);
        self.bytes = bytes;
    }

    fn as_slice(&self) -> &[u8] {
        // SAFETY: `f32` has no padding; the length stays inside the buffer.
        unsafe { std::slice::from_raw_parts(self.samples.as_ptr() as *const u8, self.bytes) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as above, and the borrow is exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.samples.as_mut_ptr() as *mut u8, self.bytes) }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.samples.as_mut_ptr() as *mut u8
    }
}

/// `audio_resampler_t`.
pub struct Resampler {
    ctx: *mut SwrContext,
    input_freq: u32,
    output_freq: u32,
    output_ch: usize,
    output: AudioSpec,
    /// One buffer per output plane.
    buffers: Vec<Plane>,
    output_size: usize,
    /// Correction last handed to `swr_set_compensation`, in steps of
    /// [`DRIFT_DISTANCE_S`] output frames.
    drift_delta: c_int,
}

/// Span over which a drift correction is spread, in seconds of output. The
/// correction is handed over again long before it runs out, so this only
/// sets its resolution: one frame in an hour, well under a ppm.
const DRIFT_DISTANCE_S: u32 = 3600;

// SAFETY: the context is only used through `&mut self`.
unsafe impl Send for Resampler {}

impl Resampler {
    /// `audio_resampler_create(dst, src)`.
    pub fn new(dst: AudioSpec, src: AudioSpec) -> Option<Resampler> {
        Self::create(dst, src, false)
    }

    /// A resampler whose ratio can be moved with [`Resampler::set_drift`],
    /// for a monitor. OBS resamples only when the rates differ; this one
    /// always does, so a correction never has to rebuild it mid-stream.
    pub fn adjustable(dst: AudioSpec, src: AudioSpec) -> Option<Resampler> {
        Self::create(dst, src, true)
    }

    fn create(dst: AudioSpec, src: AudioSpec, adjustable: bool) -> Option<Resampler> {
        let output_ch = dst.speakers.channels();
        let in_layout = channel_layout(src.speakers);
        let out_layout = channel_layout(dst.speakers);
        let mut ctx: *mut SwrContext = ptr::null_mut();

        // SAFETY: FFI with valid pointers; `ctx` is freed on every failure path.
        unsafe {
            swr_alloc_set_opts2(
                &mut ctx,
                &out_layout,
                av_sample_format(dst.format),
                dst.rate as c_int,
                &in_layout,
                av_sample_format(src.format),
                src.rate as c_int,
                0,
                ptr::null_mut(),
            );
            if ctx.is_null() {
                log::error!("swr_alloc_set_opts failed");
                return None;
            }

            if src.speakers == Speakers::Mono && output_ch > 1 {
                let matrix = &MONO_UPMIX[output_ch - 1];
                if swr_set_matrix(ctx, matrix.as_ptr(), 1) < 0 {
                    log::debug!("swr_set_matrix failed for mono upmix");
                }
            }

            if adjustable
                && av_opt_set_int(ctx as *mut c_void, c"flags".as_ptr(), SWR_FLAG_RESAMPLE, 0) < 0
            {
                log::warn!("swr flags could not be set, drift is not compensated");
            }

            let err = swr_init(ctx);
            if err != 0 {
                log::error!("avresample_open failed: error code {err}");
                swr_free(&mut ctx);
                return None;
            }
        }

        let planes = if dst.format.is_planar() { output_ch } else { 1 };
        Some(Resampler {
            ctx,
            input_freq: src.rate,
            output_freq: dst.rate,
            output_ch,
            output: dst,
            buffers: vec![Plane::default(); planes],
            output_size: 0,
            drift_delta: 0,
        })
    }

    /// Plays `ppm` millionths more output frames than the rates give (fewer
    /// when negative), until the next call. See `drift.rs`.
    pub fn set_drift(&mut self, ppm: f64) {
        let distance = self.output_freq.saturating_mul(DRIFT_DISTANCE_S) as f64;
        let delta = (ppm * distance / 1e6).round() as c_int;
        if delta == self.drift_delta {
            return;
        }
        // SAFETY: `ctx` is valid; the distance fits an int for any rate
        // below 596 kHz.
        let ret = unsafe { swr_set_compensation(self.ctx, delta, distance as c_int) };
        if ret < 0 {
            log::warn!("swr_set_compensation failed: {ret}");
            return;
        }
        self.drift_delta = delta;
    }

    /// `audio_resampler_resample`. Returns the number of output frames;
    /// the samples are then read with [`Resampler::plane`].
    pub fn resample(&mut self, input: &[*const u8], in_frames: u32) -> Option<u32> {
        // SAFETY: `ctx` is valid for the lifetime of `self`.
        let delay = unsafe { swr_get_delay(self.ctx, self.input_freq as i64) };
        let estimated = unsafe {
            av_rescale_rnd(
                delay + in_frames as i64,
                self.output_freq as i64,
                self.input_freq as i64,
                AV_ROUND_UP,
            )
        } as usize;

        if estimated > self.output_size {
            let plane_bytes = if self.output.format.is_planar() {
                self.output.format.bytes_per_sample()
            } else {
                self.output_ch * self.output.format.bytes_per_sample()
            };
            for buf in &mut self.buffers {
                buf.resize(estimated * plane_bytes);
            }
            self.output_size = estimated;
        }

        let mut inputs = [ptr::null::<u8>(); MAX_AUDIO_CHANNELS];
        for (slot, p) in inputs.iter_mut().zip(input) {
            *slot = *p;
        }
        let mut outputs = [ptr::null_mut::<u8>(); MAX_AUDIO_CHANNELS];
        for (slot, buf) in outputs.iter_mut().zip(self.buffers.iter_mut()) {
            *slot = buf.as_mut_ptr();
        }

        // SAFETY: buffers hold `output_size` frames; inputs hold `in_frames`.
        let ret = unsafe {
            swr_convert(
                self.ctx,
                outputs.as_ptr(),
                self.output_size as c_int,
                inputs.as_ptr(),
                in_frames as c_int,
            )
        };
        if ret < 0 {
            log::error!("swr_convert failed: {ret}");
            return None;
        }
        Some(ret as u32)
    }

    /// Output plane `i` after [`Resampler::resample`].
    pub fn plane(&self, i: usize) -> &[u8] {
        self.buffers[i].as_slice()
    }

    pub fn plane_mut(&mut self, i: usize) -> &mut [u8] {
        self.buffers[i].as_mut_slice()
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: `ctx` came from `swr_alloc_set_opts2`.
        unsafe { swr_free(&mut self.ctx) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::format::{as_f32, OBS_SAMPLE_RATE};

    fn spec(rate: u32, speakers: Speakers, format: SampleFormat) -> AudioSpec {
        AudioSpec {
            rate,
            speakers,
            format,
        }
    }

    #[test]
    fn converts_rate_and_layout() {
        let src = spec(44_100, Speakers::Stereo, SampleFormat::Float);
        let dst = spec(OBS_SAMPLE_RATE, Speakers::Stereo, SampleFormat::FloatPlanar);
        let mut rs = Resampler::new(dst, src).expect("resampler");

        let frames = 4410u32;
        let input: Vec<u8> = (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * 0.05).sin() * 0.5;
                [v, v]
            })
            .flat_map(f32::to_ne_bytes)
            .collect();

        let mut total = 0;
        for _ in 0..10 {
            total += rs.resample(&[input.as_ptr()], frames).unwrap();
        }
        // 1 s of input gives about 1 s of output, minus the filter delay.
        assert!((47_000..=48_000).contains(&total), "{total}");
        let left = as_f32(rs.plane(0));
        assert!(left.iter().any(|s| s.abs() > 0.3));
    }

    fn mono_to(speakers: Speakers) -> Vec<f32> {
        let src = spec(OBS_SAMPLE_RATE, Speakers::Mono, SampleFormat::Float);
        let dst = spec(OBS_SAMPLE_RATE, speakers, SampleFormat::FloatPlanar);
        let mut rs = Resampler::new(dst, src).unwrap();
        let input: Vec<u8> = std::iter::repeat_n(0.5f32, 480)
            .flat_map(f32::to_ne_bytes)
            .collect();
        let n = rs.resample(&[input.as_ptr()], 480).unwrap() as usize;
        assert_eq!(n, 480);
        (0..speakers.channels())
            .map(|i| as_f32(rs.plane(i))[n - 1])
            .collect()
    }

    #[test]
    fn mono_upmix_follows_obs_matrix() {
        let surround = mono_to(Speakers::FivePointOne);
        assert!(
            surround.iter().all(|v| (v - 0.5).abs() < 1e-3),
            "{surround:?}"
        );

        let wide = mono_to(Speakers::SevenPointOne);
        assert!(wide[3].abs() < 1e-6, "7.1 LFE stays silent: {wide:?}");
        assert!((wide[0] - 0.5).abs() < 1e-3 && (wide[7] - 0.5).abs() < 1e-3);
    }

    /// The monitors apply the volume through `align_to::<f32>()` on a plane.
    /// A plane whose head is not empty would play samples at full volume, so
    /// a mute would not mute: the alignment is part of the contract.
    #[test]
    fn planes_are_aligned_for_f32() {
        for speakers in [Speakers::Stereo, Speakers::FivePointOne] {
            for format in [SampleFormat::Float, SampleFormat::FloatPlanar] {
                let src = spec(44_100, Speakers::Stereo, SampleFormat::Float);
                let mut rs = Resampler::new(spec(OBS_SAMPLE_RATE, speakers, format), src).unwrap();
                let input: Vec<u8> = std::iter::repeat_n(0.25f32, 2 * 441)
                    .flat_map(f32::to_ne_bytes)
                    .collect();
                rs.resample(&[input.as_ptr()], 441).unwrap();
                let planes = if format.is_planar() {
                    speakers.channels()
                } else {
                    1
                };
                for i in 0..planes {
                    let bytes = rs.plane(i);
                    assert!(!bytes.is_empty(), "{speakers:?} {format:?} plane {i}");
                    // SAFETY: `f32` has no invalid bit patterns.
                    let (head, _, _) = unsafe { bytes.align_to::<f32>() };
                    assert!(
                        head.is_empty(),
                        "{speakers:?} {format:?} plane {i} misaligned"
                    );
                }
            }
        }
    }

    /// Frames out of an adjustable resampler fed `seconds` of 48 kHz audio.
    fn frames_out(rate: u32, ppm: f64, seconds: u32) -> u64 {
        let src = spec(OBS_SAMPLE_RATE, Speakers::Stereo, SampleFormat::FloatPlanar);
        let dst = spec(rate, Speakers::Stereo, SampleFormat::Float);
        let mut rs = Resampler::adjustable(dst, src).unwrap();
        let plane: Vec<u8> = (0..480)
            .map(|i| (i as f32 * 0.05).sin() * 0.5)
            .flat_map(f32::to_ne_bytes)
            .collect();
        let mut total = 0u64;
        for _ in 0..seconds * 100 {
            // Handed over on every packet, as the monitors do.
            rs.set_drift(ppm);
            total += rs.resample(&[plane.as_ptr(), plane.as_ptr()], 480).unwrap() as u64;
        }
        total
    }

    #[test]
    fn drift_moves_the_ratio_by_the_amount_asked() {
        for rate in [48_000, 44_100] {
            let base = frames_out(rate, 0.0, 60) as f64;
            let expected = rate as f64 * 60.0;
            assert!((base - expected).abs() < 100.0, "{rate}: {base}");
            for ppm in [500.0, -500.0, 50.0] {
                let got = frames_out(rate, ppm, 60) as f64;
                let measured = (got - base) / expected * 1e6;
                assert!(
                    (measured - ppm).abs() < 3.0,
                    "{rate} Hz, {ppm} ppm: measured {measured}"
                );
            }
        }
    }

    #[test]
    fn adjustable_resampler_is_transparent_at_equal_rates() {
        let src = spec(OBS_SAMPLE_RATE, Speakers::Stereo, SampleFormat::FloatPlanar);
        let dst = spec(OBS_SAMPLE_RATE, Speakers::Stereo, SampleFormat::Float);
        let mut rs = Resampler::adjustable(dst, src).unwrap();
        let plane: Vec<u8> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0).sin() * 0.5)
            .flat_map(f32::to_ne_bytes)
            .collect();
        let mut peak = 0f32;
        for i in 0..50 {
            let n = rs.resample(&[plane.as_ptr(), plane.as_ptr()], 480).unwrap() as usize;
            if i > 10 {
                peak = as_f32(&rs.plane(0)[..n * 8])
                    .iter()
                    .fold(peak, |m, s| m.max(s.abs()));
            }
        }
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
    }

    #[test]
    fn surround_downmix_is_not_normalized() {
        // FFmpeg keeps unity gain on front channels for float output.
        let src = spec(OBS_SAMPLE_RATE, Speakers::FivePointOne, SampleFormat::Float);
        let dst = spec(OBS_SAMPLE_RATE, Speakers::Stereo, SampleFormat::FloatPlanar);
        let mut rs = Resampler::new(dst, src).unwrap();
        let frame = [0.25f32, 0.0, 0.0, 0.0, 0.0, 0.0];
        let input: Vec<u8> = frame
            .iter()
            .cycle()
            .take(6 * 480)
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        let n = rs.resample(&[input.as_ptr()], 480).unwrap() as usize;
        let left = as_f32(rs.plane(0))[n - 1];
        assert!((left - 0.25).abs() < 1e-3, "{left}");
    }
}
