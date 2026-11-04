//! Maps interleaved frames from one channel layout onto another while mixing.

use crate::ramp::Ramp;

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
    let frames = (input.len() / in_channels).min(output.len() / out_channels);
    let frames_in = input.chunks_exact(in_channels).take(frames);
    let frames_out = output.chunks_exact_mut(out_channels).take(frames);
    for (src, dst) in frames_in.zip(frames_out) {
        add_frame(src, dst, gain, pan);
    }
}

/// [`remix_add`] with a gain and a pan that move frame by frame along their ramps.
pub fn remix_add_ramped(
    input: &[f32],
    in_channels: usize,
    output: &mut [f32],
    out_channels: usize,
    gain: &mut Ramp,
    pan: &mut Ramp,
) {
    if in_channels == 0 || out_channels == 0 {
        return;
    }
    let frames = (input.len() / in_channels).min(output.len() / out_channels);
    let frames_in = input.chunks_exact(in_channels).take(frames);
    let frames_out = output.chunks_exact_mut(out_channels).take(frames);
    for (src, dst) in frames_in.zip(frames_out) {
        add_frame(src, dst, gain.next_value(), pan.next_value());
    }
}

/// Adds one frame, both of at least one channel, under the rules of [`remix_add`].
#[inline]
fn add_frame(src: &[f32], dst: &mut [f32], gain: f32, pan: f32) {
    let pan = if pan.is_nan() {
        0.0
    } else {
        pan.clamp(-1.0, 1.0)
    };
    let (left, right) = if dst.len() >= 2 {
        (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
    } else {
        (gain, gain)
    };
    let channel_gain = |channel: usize| match channel {
        0 => left,
        1 => right,
        _ => gain,
    };
    if let [mono] = src {
        for (channel, d) in dst.iter_mut().enumerate() {
            *d += mono * channel_gain(channel);
        }
    } else if let [d] = dst {
        *d += src.iter().sum::<f32>() / src.len() as f32 * gain;
    } else {
        for (channel, (d, s)) in dst.iter_mut().zip(src).enumerate() {
            *d += s * channel_gain(channel);
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
    fn ramps_move_frame_by_frame_and_match_the_steady_mix_at_their_end() {
        let mut gain = Ramp::new(1000, 0.0);
        let mut pan = Ramp::new(1000, 0.0);
        gain.aim(1.0, 5);
        pan.aim(1.0, 5);
        let input = [1.0; 20];
        let mut ramped = [0.0; 20];
        remix_add_ramped(&input, 2, &mut ramped, 2, &mut gain, &mut pan);
        // Gain rises in fifths, the left side falls with the pan.
        let right: Vec<f32> = ramped
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[1])
            .collect();
        for (frame, value) in right[..5].iter().enumerate() {
            assert!((value - (frame + 1) as f32 / 5.0).abs() < 1e-6, "{right:?}");
        }
        assert!((ramped[0] - 0.2 * 0.8).abs() < 1e-6, "{ramped:?}");
        assert!(gain.is_steady() && pan.is_steady());

        let mut steady = [0.0; 2];
        remix_add(&input[..2], 2, &mut steady, 2, gain.value(), pan.value());
        assert_eq!(&ramped[18..], &steady);
    }

    #[test]
    fn ramps_leave_the_buffers_alone_without_channels() {
        let mut gain = Ramp::new(1000, 1.0);
        let mut pan = Ramp::new(1000, 0.0);
        let mut out = [0.5; 2];
        remix_add_ramped(&[1.0], 0, &mut out, 2, &mut gain, &mut pan);
        remix_add_ramped(&[1.0], 1, &mut out, 0, &mut gain, &mut pan);
        assert_eq!(out, [0.5; 2]);
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
