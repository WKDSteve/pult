//! Maps interleaved frames from one channel layout onto another while mixing.

/// Adds `input` (interleaved, `in_channels` per frame) times `gain` into `output`
/// (interleaved, `out_channels` per frame), placed by `pan`.
///
/// Equal layouts map channel to channel; mono feeds every output channel; a mono output
/// gets the average of the inputs; otherwise the first channels map one to one and the rest
/// stay untouched. Only whole frames present in both buffers are mixed.
///
/// `pan` runs from -1 (left) through 0 (centre, both sides at full gain) to 1 (right) and
/// lowers the other side linearly: a pan for mono inputs, a balance for stereo ones. It acts
/// on the first two output channels and only when there are at least two; NaN is centre.
pub fn remix_add(
    input: &[f32],
    in_channels: usize,
    output: &mut [f32],
    out_channels: usize,
    gain: f32,
    pan: f32,
) {
    if in_channels == 0 || out_channels == 0 {
        return;
    }
    let pan = if pan.is_nan() {
        0.0
    } else {
        pan.clamp(-1.0, 1.0)
    };
    let (left, right) = if out_channels >= 2 {
        (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
    } else {
        (gain, gain)
    };
    let channel_gain = |channel: usize| match channel {
        0 => left,
        1 => right,
        _ => gain,
    };
    let frames = (input.len() / in_channels).min(output.len() / out_channels);
    let frames_in = input.chunks_exact(in_channels).take(frames);
    let frames_out = output.chunks_exact_mut(out_channels).take(frames);
    for (src, dst) in frames_in.zip(frames_out) {
        if in_channels == 1 {
            for (channel, d) in dst.iter_mut().enumerate() {
                *d += src[0] * channel_gain(channel);
            }
        } else if out_channels == 1 {
            dst[0] += src.iter().sum::<f32>() / in_channels as f32 * gain;
        } else {
            for (channel, (d, s)) in dst.iter_mut().zip(src).enumerate() {
                *d += s * channel_gain(channel);
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
        remix_add(&[0.2, 0.4, 0.6, 0.8], 2, &mut out, 2, 0.5, 0.0);
        assert_eq!(out, [1.1, 1.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_spreads_to_every_channel() {
        let mut out = [0.0; 4];
        remix_add(&[0.5, -0.5], 1, &mut out, 2, 1.0, 0.0);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        let mut out = [0.0; 2];
        remix_add(&[0.2, 0.4, 1.0, 0.0], 2, &mut out, 1, 1.0, 0.0);
        assert!((out[0] - 0.3).abs() < 1e-6 && (out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wider_input_keeps_the_first_channels() {
        let mut out = [0.0; 2];
        remix_add(&[0.1, 0.2, 0.9, 0.9], 4, &mut out, 2, 1.0, 0.0);
        assert_eq!(out, [0.1, 0.2]);
    }

    #[test]
    fn wider_output_leaves_extra_channels_alone() {
        let mut out = [0.0; 4];
        remix_add(&[0.1, 0.2], 2, &mut out, 4, 1.0, 0.0);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0]);
    }

    #[test]
    fn mixes_only_whole_frames_of_the_shorter_buffer() {
        let mut out = [0.0; 6];
        remix_add(&[1.0, 1.0, 1.0], 2, &mut out, 2, 1.0, 0.0);
        assert_eq!(out, [1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        remix_add(&[1.0], 0, &mut out, 2, 1.0, 0.0);
        remix_add(&[1.0], 1, &mut out, 0, 1.0, 0.0);
    }

    #[test]
    fn pan_lowers_the_other_side() {
        let mut out = [0.0; 2];
        remix_add(&[0.8], 1, &mut out, 2, 1.0, 0.5);
        assert_eq!(out, [0.4, 0.8]);
        let mut out = [0.0; 2];
        remix_add(&[0.8, 0.8], 2, &mut out, 2, 0.5, -1.0);
        assert_eq!(out, [0.4, 0.0]);
    }

    #[test]
    fn pan_leaves_mono_outputs_extra_channels_and_nan_alone() {
        let mut out = [0.0];
        remix_add(&[0.8], 1, &mut out, 1, 1.0, 1.0);
        assert_eq!(out, [0.8]);
        let mut out = [0.0; 3];
        remix_add(&[0.6], 1, &mut out, 3, 1.0, 1.0);
        assert_eq!(out, [0.0, 0.6, 0.6]);
        let mut out = [0.0; 2];
        remix_add(&[0.6], 1, &mut out, 2, 1.0, f32::NAN);
        assert_eq!(out, [0.6, 0.6]);
    }
}
