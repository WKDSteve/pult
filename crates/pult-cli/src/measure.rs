//! Measuring a real route: `pult record` writes what a device captures or plays to a WAV file,
//! `pult clicks` looks for clicks in a recording of a test tone, with the detector the click
//! tests of the engine use (`pult_core::clicks`).

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pult_core::Clicks;
use pult_core::wav::Wav;

use crate::args::{ClicksSpec, RecordSpec};

/// How many clicks `pult clicks` lists one by one.
const LISTED: usize = 20;

/// Records as `spec` says, until its time is over or Enter is pressed.
///
/// # Errors
///
/// A text for the terminal: the device, the stream or the file failed.
pub fn record(spec: &RecordSpec) -> Result<Vec<String>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let on_enter = Arc::clone(&stop);
    // Left running when the time ends first: it ends with the process.
    std::thread::spawn(move || {
        let mut line = String::new();
        // No terminal (a script that closed stdin) is no Enter.
        if matches!(std::io::stdin().read_line(&mut line), Ok(read) if read > 0) {
            on_enter.store(true, Ordering::Release);
        }
    });
    let recording =
        pult_engine::record(&spec.source, spec.seconds, || !stop.load(Ordering::Acquire))
            .map_err(|e| e.to_string())?;
    let wav = Wav {
        sample_rate: recording.sample_rate,
        channels: recording.channels,
        samples: recording.samples,
    };
    let bytes = wav.encode()?;
    std::fs::write(&spec.file, bytes)
        .map_err(|e| format!("cannot write {}: {e}", spec.file.display()))?;
    let mut lines = vec![format!(
        "recorded {:.2} s of {}{} ({} ch, {} Hz) to {}",
        wav.seconds(),
        if recording.loopback {
            "what plays on "
        } else {
            ""
        },
        recording.source,
        wav.channels,
        wav.sample_rate,
        spec.file.display()
    )];
    if recording.lost_frames > 0 {
        lines.push(format!(
            "note: {} frames were lost while recording; each gap reads as a click",
            recording.lost_frames
        ));
    }
    Ok(lines)
}

/// What `pult clicks` found in one recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// The lines for the terminal.
    pub lines: Vec<String>,
    /// How many clicks there are.
    pub clicks: usize,
}

/// Looks for clicks in `wav` as `spec` says.
///
/// # Errors
///
/// When the recording is shorter than what `spec` skips, or the tone does not fit its rate.
pub fn judge(wav: &Wav, spec: &ClicksSpec) -> Result<Verdict, String> {
    let channels = usize::from(wav.channels);
    let rate = f64::from(wav.sample_rate);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames_of = |seconds: f64| (seconds * rate) as usize;
    let quiet = frames_of(spec.quiet).min(wav.frames());
    let skip = frames_of(spec.skip).max(quiet);
    let end = spec
        .until
        .map_or(wav.frames(), |until| frames_of(until).min(wav.frames()));
    if skip >= end {
        return Err(format!(
            "the recording is {:.2} s long, nothing is left from {:.2} s to {:.2} s",
            wav.seconds(),
            skip as f64 / rate,
            end as f64 / rate
        ));
    }
    if !(spec.frequency > 0.0 && f64::from(spec.frequency) < rate / 2.0) {
        return Err(format!(
            "a tone of {} Hz does not fit a recording at {} Hz",
            spec.frequency, wav.sample_rate
        ));
    }
    let floor = Clicks::floor_of(&wav.samples[..quiet * channels], channels);
    let detector = Clicks::for_tone(spec.frequency, wav.sample_rate).with_floor(floor);
    let judged = &wav.samples[skip * channels..end * channels];
    let clicks = detector.find(judged, channels);
    let level = judged.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
    let mut lines = vec![format!(
        "{:.2} s at {} Hz, {} ch, judged from {:.2} s to {:.2} s: peak {:.1} dBFS, floor {:.1} dBFS{}, steepest step {:.2} of what a {} Hz tone can do (2 is a click)",
        wav.seconds(),
        wav.sample_rate,
        wav.channels,
        skip as f64 / rate,
        end as f64 / rate,
        pult_core::linear_to_db(level),
        pult_core::linear_to_db(floor),
        if quiet > 0 {
            " (from the quiet start)"
        } else {
            ""
        },
        detector.steepest(judged, channels),
        spec.frequency,
    )];
    if level <= floor {
        lines.push(
            "note: no tone in the recording, so nothing a click could stand out from".to_owned(),
        );
    }
    for click in clicks.iter().take(LISTED) {
        let mut line = String::new();
        let _ = write!(
            line,
            "click at {:.4} s, channel {}: a step of {:.4}, {:.4} allowed",
            (skip + click.frame) as f64 / rate,
            click.channel + 1,
            click.step,
            click.allowed
        );
        lines.push(line);
    }
    if clicks.len() > LISTED {
        lines.push(format!("... and {} more", clicks.len() - LISTED));
    }
    lines.push(match clicks.len() {
        0 => "no clicks".to_owned(),
        1 => "1 click".to_owned(),
        n => format!("{n} clicks"),
    });
    Ok(Verdict {
        lines,
        clicks: clicks.len(),
    })
}

/// Reads the recording `spec` names and looks for clicks in it.
///
/// # Errors
///
/// A text for the terminal: the file cannot be read, is no WAV file, or does not fit `spec`.
pub fn clicks(spec: &ClicksSpec) -> Result<Verdict, String> {
    let wav = read(&spec.file)?;
    judge(&wav, spec)
}

fn read(file: &Path) -> Result<Wav, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    Wav::decode(&bytes).map_err(|e| format!("{}: {e}", file.display()))
}

/// Prints `verdict` and says by the status whether there were clicks: 0 for none, 1 for some.
pub fn report(verdict: &Verdict) -> ExitCode {
    for line in &verdict.lines {
        println!("{line}");
    }
    if verdict.clicks == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pult_core::Sine;

    use super::*;

    const RATE: u32 = 48_000;

    fn spec(quiet: f64, skip: f64) -> ClicksSpec {
        ClicksSpec {
            file: PathBuf::from("unused.wav"),
            frequency: 103.0,
            quiet,
            skip,
            until: None,
        }
    }

    /// Half a second of silence with noise of `noise`, then two seconds of the tone with it.
    fn recording(noise: f32) -> Wav {
        let mut samples = vec![0.0; RATE as usize];
        let mut tone = vec![0.0; 4 * RATE as usize];
        Sine::new(103.0, RATE, 0.25).fill(&mut tone, 2);
        // The tone starts at its zero crossing, so the start itself is no jump.
        samples.extend_from_slice(&tone);
        for (n, sample) in samples.iter_mut().enumerate() {
            *sample += if (n / 2).is_multiple_of(2) {
                noise
            } else {
                -noise
            };
        }
        Wav {
            sample_rate: RATE,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn a_clean_recording_has_no_clicks_and_a_jump_is_found_where_it_is() {
        let mut wav = recording(0.0);
        let verdict = judge(&wav, &spec(0.0, 0.0)).expect("judged");
        assert_eq!(verdict.clicks, 0, "{:?}", verdict.lines);
        assert_eq!(verdict.lines.last().map(String::as_str), Some("no clicks"));
        // One second in, on the right channel only.
        let at = RATE as usize;
        for frame in at..at + 480 {
            wav.samples[frame * 2 + 1] = 0.0;
        }
        let verdict = judge(&wav, &spec(0.0, 0.0)).expect("judged");
        assert!(verdict.clicks >= 1, "{:?}", verdict.lines);
        assert!(
            verdict
                .lines
                .iter()
                .any(|line| line.starts_with("click at 1.0") && line.contains("channel 2")),
            "{:?}",
            verdict.lines
        );
    }

    #[test]
    fn the_noise_of_the_quiet_start_sets_the_floor() {
        let wav = recording(2e-3);
        // Judged with the usual floor, the noise of the cable is a click at every sample.
        let strict = judge(&wav, &spec(0.0, 0.0)).expect("judged");
        assert!(strict.clicks > 1000, "{}", strict.clicks);
        assert!(strict.lines.iter().any(|line| line.starts_with("... and ")));
        let lenient = judge(&wav, &spec(0.4, 0.0)).expect("judged");
        assert_eq!(lenient.clicks, 0, "{:?}", lenient.lines);
        assert!(lenient.lines[0].contains("from the quiet start"));
    }

    #[test]
    fn what_cannot_be_judged_is_refused() {
        let wav = recording(0.0);
        assert!(
            judge(&wav, &spec(0.0, 9.0))
                .unwrap_err()
                .contains("nothing is left")
        );
        // A jump behind the end of what is judged does not count.
        let mut cut = recording(0.0);
        let at = 2 * RATE as usize;
        for sample in &mut cut.samples[at * 2..(at + 480) * 2] {
            *sample = 0.0;
        }
        assert!(judge(&cut, &spec(0.0, 0.0)).expect("judged").clicks > 0);
        let early = ClicksSpec {
            until: Some(1.9),
            ..spec(0.0, 0.0)
        };
        assert_eq!(judge(&cut, &early).expect("judged").clicks, 0);
        let nothing = ClicksSpec {
            until: Some(0.5),
            ..spec(0.0, 1.0)
        };
        assert!(
            judge(&cut, &nothing)
                .unwrap_err()
                .contains("nothing is left")
        );
        let mut high = spec(0.0, 0.0);
        high.frequency = 30_000.0;
        assert!(judge(&wav, &high).unwrap_err().contains("does not fit"));
        let silent = Wav {
            sample_rate: RATE,
            channels: 1,
            samples: vec![0.0; 1000],
        };
        let verdict = judge(&silent, &spec(0.0, 0.0)).expect("judged");
        assert!(verdict.lines.iter().any(|line| line.contains("no tone")));
        assert!(clicks(&spec(0.0, 0.0)).unwrap_err().contains("cannot read"));
    }
}
