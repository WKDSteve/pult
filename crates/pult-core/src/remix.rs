//! Maps interleaved frames from one channel layout onto another while mixing.

/// Adds `input` (interleaved, `in_channels` per frame) times `gain` into `output`
/// (interleaved, `out_channels` per frame).
///
/// Equal layouts map channel to channel; mono feeds every output channel; a mono output
/// gets the average of the inputs; otherwise the first channels map one to one and the rest
/// stay untouched. Only whole frames present in both buffers are mixed.
pub fn remix_add(
    input: &[f32],
    in_channels: usize,
    output: &mut [f32],
    out_channels: usize,
    gain: f32,
) {
    if in_channels == 0 || out_channels == 0 {
        return;
    }
    let frames = (input.len() / in_channels).min(output.len() / out_channels);
    let frames_in = input.chunks_exact(in_channels).take(frames);
    let frames_out = output.chunks_exact_mut(out_channels).take(frames);
    for (src, dst) in frames_in.zip(frames_out) {
        if in_channels == 1 {
            let value = src[0] * gain;
            for d in dst {
                *d += value;
            }
        } else if out_channels == 1 {
            dst[0] += src.iter().sum::<f32>() / in_channels as f32 * gain;
        } else {
            for (d, s) in dst.iter_mut().zip(src) {
                *d += s * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_stereo_adds_with_gain() {
        let mut out = [1.0, 1.0, 0.0, 0.0];
        remix_add(&[0.2, 0.4, 0.6, 0.8], 2, &mut out, 2, 0.5);
        assert_eq!(out, [1.1, 1.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_spreads_to_every_channel() {
        let mut out = [0.0; 4];
        remix_add(&[0.5, -0.5], 1, &mut out, 2, 1.0);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        let mut out = [0.0; 2];
        remix_add(&[0.2, 0.4, 1.0, 0.0], 2, &mut out, 1, 1.0);
        assert!((out[0] - 0.3).abs() < 1e-6 && (out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wider_input_keeps_the_first_channels() {
        let mut out = [0.0; 2];
        remix_add(&[0.1, 0.2, 0.9, 0.9], 4, &mut out, 2, 1.0);
        assert_eq!(out, [0.1, 0.2]);
    }

    #[test]
    fn wider_output_leaves_extra_channels_alone() {
        let mut out = [0.0; 4];
        remix_add(&[0.1, 0.2], 2, &mut out, 4, 1.0);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0]);
    }

    #[test]
    fn mixes_only_whole_frames_of_the_shorter_buffer() {
        let mut out = [0.0; 6];
        remix_add(&[1.0, 1.0, 1.0], 2, &mut out, 2, 1.0);
        assert_eq!(out, [1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        remix_add(&[1.0], 0, &mut out, 2, 1.0);
        remix_add(&[1.0], 1, &mut out, 0, 1.0);
    }
}
