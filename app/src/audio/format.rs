//! Audio formats, mirroring `libobs/media-io/audio-io.h`.

/// OBS default output format: 48 kHz, stereo, 32-bit float planar.
/// Every source is converted to it before monitoring, as in libobs.
pub const OBS_SAMPLE_RATE: u32 = 48_000;
pub const OBS_SPEAKERS: Speakers = Speakers::Stereo;
pub const OBS_FORMAT: SampleFormat = SampleFormat::FloatPlanar;
pub const OBS_CHANNELS: usize = 2;

pub const MAX_AUDIO_CHANNELS: usize = 8;

/// `enum speaker_layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speakers {
    Unknown,
    Mono,
    Stereo,
    TwoPointOne,
    FourPointZero,
    FourPointOne,
    FivePointOne,
    SevenPointOne,
}

impl Speakers {
    pub fn channels(self) -> usize {
        match self {
            Speakers::Unknown => 0,
            Speakers::Mono => 1,
            Speakers::Stereo => 2,
            Speakers::TwoPointOne => 3,
            Speakers::FourPointZero => 4,
            Speakers::FourPointOne => 5,
            Speakers::FivePointOne => 6,
            Speakers::SevenPointOne => 8,
        }
    }

    /// Layout for a bare channel count, like `(enum speaker_layout)channels`
    /// in the OBS backends.
    pub fn from_channels(channels: usize) -> Speakers {
        match channels {
            1 => Speakers::Mono,
            2 => Speakers::Stereo,
            3 => Speakers::TwoPointOne,
            4 => Speakers::FourPointZero,
            5 => Speakers::FourPointOne,
            6 => Speakers::FivePointOne,
            8 => Speakers::SevenPointOne,
            _ => Speakers::Unknown,
        }
    }
}

/// `enum audio_format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8,
    S16,
    S32,
    Float,
    U8Planar,
    S16Planar,
    S32Planar,
    FloatPlanar,
}

impl SampleFormat {
    pub fn is_planar(self) -> bool {
        matches!(
            self,
            SampleFormat::U8Planar
                | SampleFormat::S16Planar
                | SampleFormat::S32Planar
                | SampleFormat::FloatPlanar
        )
    }

    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::U8 | SampleFormat::U8Planar => 1,
            SampleFormat::S16 | SampleFormat::S16Planar => 2,
            SampleFormat::S32
            | SampleFormat::S32Planar
            | SampleFormat::Float
            | SampleFormat::FloatPlanar => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpec {
    pub rate: u32,
    pub speakers: Speakers,
    pub format: SampleFormat,
}

impl AudioSpec {
    pub fn planes(&self) -> usize {
        if self.format.is_planar() {
            self.speakers.channels()
        } else {
            1
        }
    }

    /// Bytes of one frame in one plane.
    pub fn plane_frame_bytes(&self) -> usize {
        let per_plane = if self.format.is_planar() {
            1
        } else {
            self.speakers.channels()
        };
        per_plane * self.format.bytes_per_sample()
    }
}

/// Raw audio handed to a source, like `struct obs_source_audio`.
pub struct SourceAudio<'a> {
    /// One slice per plane (a single slice for interleaved formats).
    pub planes: &'a [&'a [u8]],
    pub frames: u32,
    pub spec: AudioSpec,
}

/// Audio in the OBS output format, handed to monitors, like
/// `struct audio_data`: one `f32` slice per channel.
pub struct ObsAudio<'a> {
    pub planes: [&'a [f32]; OBS_CHANNELS],
    pub frames: u32,
}

/// Reinterprets a byte plane as `f32` samples. The plane must hold whole
/// samples; misaligned buffers are copied.
pub fn as_f32(bytes: &[u8]) -> std::borrow::Cow<'_, [f32]> {
    let len = bytes.len() / 4;
    // SAFETY: f32 has no invalid bit patterns; alignment is checked.
    let (head, body, _) = unsafe { bytes[..len * 4].align_to::<f32>() };
    if head.is_empty() {
        std::borrow::Cow::Borrowed(body)
    } else {
        std::borrow::Cow::Owned(
            bytes[..len * 4]
                .chunks_exact(4)
                .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_counts_round_trip() {
        for n in [1, 2, 3, 4, 5, 6, 8] {
            assert_eq!(Speakers::from_channels(n).channels(), n);
        }
        assert_eq!(Speakers::from_channels(7), Speakers::Unknown);
    }

    #[test]
    fn plane_sizes() {
        let spec = AudioSpec {
            rate: 48_000,
            speakers: Speakers::FivePointOne,
            format: SampleFormat::S16,
        };
        assert_eq!(spec.planes(), 1);
        assert_eq!(spec.plane_frame_bytes(), 12);
        let planar = AudioSpec {
            format: SampleFormat::FloatPlanar,
            ..spec
        };
        assert_eq!(planar.planes(), 6);
        assert_eq!(planar.plane_frame_bytes(), 4);
    }
}
