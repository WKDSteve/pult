//! Devices that do not speak 32-bit float. pult mixes in `f32`; the shared mode of WASAPI gives that
//! everywhere, but ALSA and `CoreAudio` often offer integers. Such a stream's callback converts
//! through a buffer made beforehand, a piece at a time: however long a callback is, nothing
//! allocates in it. An `f32` stream takes no detour.

use std::time::Duration;

use cpal::traits::DeviceTrait;
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig};
use cpal::{I24, U24};

use crate::slot::OPEN_TIMEOUT;
use crate::{Error, audio};

/// Frames converted at a time.
const CHUNK_FRAMES: usize = 1024;

/// Whether pult can open a stream in `format`: all but DSD, which is no PCM to mix.
#[must_use]
pub(crate) fn supported(format: SampleFormat) -> bool {
    use SampleFormat::{F32, F64, I8, I16, I24, I32, I64, U8, U16, U24, U32, U64};
    matches!(
        format,
        F32 | F64 | I8 | I16 | I24 | I32 | I64 | U8 | U16 | U24 | U32 | U64
    )
}

/// The buffer of one stream's conversions; always whole frames.
#[derive(Debug)]
struct Converter {
    scratch: Vec<f32>,
}

impl Converter {
    fn new(channels: usize) -> Self {
        Self {
            scratch: vec![0.0; CHUNK_FRAMES * channels.max(1)],
        }
    }

    /// Hands `data` on to `push` as `f32`, piece by piece.
    fn input<T>(&mut self, data: &[T], mut push: impl FnMut(&[f32]))
    where
        T: Sample,
        f32: FromSample<T>,
    {
        for chunk in data.chunks(self.scratch.len()) {
            let part = &mut self.scratch[..chunk.len()];
            for (to, from) in part.iter_mut().zip(chunk) {
                *to = f32::from_sample_(*from);
            }
            push(part);
        }
    }

    /// Fills `out` with what `pull` makes in `f32`, piece by piece.
    fn output<T>(&mut self, out: &mut [T], mut pull: impl FnMut(&mut [f32]))
    where
        T: Sample + FromSample<f32>,
    {
        for chunk in out.chunks_mut(self.scratch.len()) {
            let part = &mut self.scratch[..chunk.len()];
            pull(part);
            for (to, from) in chunk.iter_mut().zip(part.iter()) {
                *to = T::from_sample_(*from);
            }
        }
    }
}

fn input_as<T>(
    device: &Device,
    config: &SupportedStreamConfig,
    mut push: impl FnMut(&[f32]) + Send + 'static,
    errors: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut converter = Converter::new(usize::from(config.channels()));
    device.build_input_stream::<T, _, _>(
        config.config(),
        move |data, _| converter.input(data, &mut push),
        errors,
        Some(OPEN_TIMEOUT),
    )
}

fn output_as<T>(
    device: &Device,
    config: &SupportedStreamConfig,
    mut pull: impl FnMut(&mut [f32]) + Send + 'static,
    mut timed: impl FnMut(Duration) + Send + 'static,
    errors: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let mut converter = Converter::new(usize::from(config.channels()));
    device.build_output_stream::<T, _, _>(
        config.config(),
        move |out, info| {
            timed(latency(info));
            converter.output(out, &mut pull);
        },
        errors,
        Some(OPEN_TIMEOUT),
    )
}

/// `$build::<T>(...)` for the sample type of `$format`; DSD and what comes later are refused.
macro_rules! by_format {
    ($format:expr, $name:expr, $build:ident($($args:expr),*)) => {
        match $format {
            SampleFormat::I8 => $build::<i8>($($args),*),
            SampleFormat::I16 => $build::<i16>($($args),*),
            SampleFormat::I24 => $build::<I24>($($args),*),
            SampleFormat::I32 => $build::<i32>($($args),*),
            SampleFormat::I64 => $build::<i64>($($args),*),
            SampleFormat::U8 => $build::<u8>($($args),*),
            SampleFormat::U16 => $build::<u16>($($args),*),
            SampleFormat::U24 => $build::<U24>($($args),*),
            SampleFormat::U32 => $build::<u32>($($args),*),
            SampleFormat::U64 => $build::<u64>($($args),*),
            SampleFormat::F32 => $build::<f32>($($args),*),
            SampleFormat::F64 => $build::<f64>($($args),*),
            other => {
                return Err(Error::UnsupportedFormat {
                    device: $name.to_owned(),
                    format: other.to_string(),
                });
            }
        }
    };
}

/// A capture stream in the device's format that gives `push` its sound in `f32`.
pub(crate) fn build_input(
    device: &Device,
    config: &SupportedStreamConfig,
    mut push: impl FnMut(&[f32]) + Send + 'static,
    errors: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, Error> {
    let name = device.to_string();
    let built = if config.sample_format() == SampleFormat::F32 {
        device.build_input_stream::<f32, _, _>(
            config.config(),
            move |data, _| push(data),
            errors,
            Some(OPEN_TIMEOUT),
        )
    } else {
        by_format!(
            config.sample_format(),
            name,
            input_as(device, config, push, errors)
        )
    };
    built.map_err(audio(format!("opening {name}")))
}

/// How long the device says what this callback writes takes to its output.
fn latency(info: &cpal::OutputCallbackInfo) -> Duration {
    let at = info.timestamp();
    at.playback.duration_since(at.callback)
}

/// A playback stream in the device's format that `pull` fills in `f32`. Before each callback
/// is pulled, `timed` hears how long the device says its sound takes to the output.
pub(crate) fn build_output(
    device: &Device,
    config: &SupportedStreamConfig,
    mut pull: impl FnMut(&mut [f32]) + Send + 'static,
    mut timed: impl FnMut(Duration) + Send + 'static,
    errors: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, Error> {
    let name = device.to_string();
    let built = if config.sample_format() == SampleFormat::F32 {
        device.build_output_stream::<f32, _, _>(
            config.config(),
            move |out, info| {
                timed(latency(info));
                pull(out);
            },
            errors,
            Some(OPEN_TIMEOUT),
        )
    } else {
        by_format!(
            config.sample_format(),
            name,
            output_as(device, config, pull, timed, errors)
        )
    };
    built.map_err(audio(format!("opening {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full scale either way and something between survive the way out and back.
    fn round_trip<T>()
    where
        T: Sample + FromSample<f32>,
        f32: FromSample<T>,
    {
        let mut converter = Converter::new(1);
        for value in [1.0_f32, -1.0, 0.5, -0.5, 0.0] {
            let mut out = [T::EQUILIBRIUM; 1];
            converter.output(&mut out, |part| part.fill(value));
            let mut back = 9.0;
            converter.input(&out, |part| back = part[0]);
            assert!(
                (back - value).abs() < 0.01,
                "{value} came back as {back} through {}",
                std::any::type_name::<T>()
            );
        }
    }

    #[test]
    fn every_format_keeps_the_level_up_to_full_scale() {
        round_trip::<i8>();
        round_trip::<i16>();
        round_trip::<I24>();
        round_trip::<i32>();
        round_trip::<i64>();
        round_trip::<u8>();
        round_trip::<u16>();
        round_trip::<U24>();
        round_trip::<u32>();
        round_trip::<u64>();
        round_trip::<f64>();
    }

    #[test]
    fn a_long_callback_comes_in_whole_frames_and_complete() {
        let channels = 2;
        let mut converter = Converter::new(channels);
        let data: Vec<i16> = (0..2 * CHUNK_FRAMES * channels + 6)
            .map(|i| i16::try_from(i).expect("small enough"))
            .collect();
        let mut seen = Vec::new();
        let mut pieces = 0;
        converter.input(&data, |part| {
            assert_eq!(part.len() % channels, 0);
            pieces += 1;
            seen.extend_from_slice(part);
        });
        assert_eq!(pieces, 3);
        let expected: Vec<f32> = data.iter().map(|&s| f32::from_sample_(s)).collect();
        assert_eq!(seen, expected);

        let mut out = vec![0_i16; data.len()];
        let mut asked = 0;
        converter.output(&mut out, |part| {
            assert_eq!(part.len() % channels, 0);
            asked += part.len();
            part.fill(0.5);
        });
        assert_eq!(asked, data.len());
        assert!(out.iter().all(|&s| s == i16::from_sample_(0.5_f32)));
    }

    /// Only here does `assert_no_alloc` see the allocator; a binary brings its own. Debug only:
    /// release builds of `assert_no_alloc` have no `AllocDisabler` (see pult-core's lib.rs).
    #[cfg(debug_assertions)]
    #[global_allocator]
    static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    #[test]
    fn converting_does_not_allocate() {
        let mut converter = Converter::new(2);
        let mut out = vec![0_i16; 5000];
        let data = vec![0.25_f64; 5000];
        assert_no_alloc::assert_no_alloc(|| {
            converter.output(&mut out, |part| part.fill(0.5));
            converter.input(&data, |_| {});
        });
    }

    #[test]
    fn dsd_is_refused_and_every_pcm_format_taken() {
        for format in [
            SampleFormat::I16,
            SampleFormat::I24,
            SampleFormat::U8,
            SampleFormat::F32,
            SampleFormat::F64,
        ] {
            assert!(supported(format), "{format}");
        }
        for format in [
            SampleFormat::DsdU8,
            SampleFormat::DsdU16,
            SampleFormat::DsdU32,
        ] {
            assert!(!supported(format), "{format}");
        }
    }
}
