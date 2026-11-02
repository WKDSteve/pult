//! What `pult run` shows and logs: snapshots of a running route's meters and counters, the
//! live status line, and the CSV rows of `--log`.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use pult_core::linear_to_db;
use pult_engine::Running;

/// How often `--log` writes a row.
pub const LOG_INTERVAL: Duration = Duration::from_secs(10);

/// One input at one moment: its peak since the last snapshot and its counters so far.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputSnapshot {
    /// Linear peak since the previous snapshot.
    pub peak: f32,
    /// Times the ring ran dry.
    pub underruns: u64,
    /// Frames lost at capture.
    pub lost: u64,
    /// Single frames dropped against drift.
    pub drops: u64,
    /// Single frames repeated against drift.
    pub repeats: u64,
    /// Hard resyncs.
    pub resyncs: u64,
}

/// The whole route at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Time since the route started.
    pub elapsed: Duration,
    /// The inputs, in route order.
    pub inputs: Vec<InputSnapshot>,
    /// Linear peak of the mix since the previous snapshot.
    pub mix_peak: f32,
    /// Clipped or invalid output samples so far.
    pub clipped: u64,
    /// Glitches the devices reported so far.
    pub glitches: u64,
}

impl Snapshot {
    /// Reads the route's meters (which starts their next measurement) and counters.
    pub fn take(running: &Running, elapsed: Duration) -> Self {
        let inputs = running
            .inputs
            .iter()
            .map(|input| {
                let stats = &input.handle.stats;
                InputSnapshot {
                    peak: stats.peak.take(),
                    underruns: stats.underruns.get(),
                    lost: stats.dropped_at_capture.get(),
                    drops: stats.drops.get(),
                    repeats: stats.repeats.get(),
                    resyncs: stats.resyncs.get(),
                }
            })
            .collect();
        Self {
            elapsed,
            inputs,
            mix_peak: running.master.peak.take(),
            clipped: running.master.clipped.get(),
            glitches: running.xruns(),
        }
    }

    /// Keeps the higher peaks of `self` and `later`, and everything else of `later`: the
    /// snapshot of a whole log interval.
    #[must_use]
    pub fn merge(&self, later: &Self) -> Self {
        let mut merged = later.clone();
        for (m, earlier) in merged.inputs.iter_mut().zip(&self.inputs) {
            m.peak = m.peak.max(earlier.peak);
        }
        merged.mix_peak = merged.mix_peak.max(self.mix_peak);
        merged
    }
}

/// `h:mm:ss`.
fn clock(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn db(linear: f32) -> String {
    let value = linear_to_db(linear);
    if value < -90.0 {
        "  -inf".to_owned()
    } else {
        format!("{value:6.1}")
    }
}

/// The live status line: run time; per input level, dropouts, frames lost at capture and
/// the drift fixes (dropped / repeated frames, hard resyncs); then the mix.
pub fn status_line(snap: &Snapshot) -> String {
    // Writing into a String cannot fail.
    let mut line = clock(snap.elapsed);
    for (i, input) in snap.inputs.iter().enumerate() {
        let _ = write!(
            line,
            " [{}] {} dB out {} lost {} fix -{}/+{}/{}",
            i + 1,
            db(input.peak),
            input.underruns,
            input.lost,
            input.drops,
            input.repeats,
            input.resyncs
        );
    }
    let _ = write!(
        line,
        " | mix {} dB clipped {} | glitches {}",
        db(snap.mix_peak),
        snap.clipped,
        snap.glitches
    );
    line
}

fn csv_db(linear: f32) -> String {
    let value = linear_to_db(linear);
    if value < -90.0 {
        "-inf".to_owned()
    } else {
        format!("{value:.1}")
    }
}

/// The CSV header for `inputs` inputs.
pub fn csv_header(inputs: usize) -> String {
    let mut line = "elapsed_s".to_owned();
    for i in 1..=inputs {
        for field in ["peak_db", "dropouts", "lost", "drops", "repeats", "resyncs"] {
            let _ = write!(line, ",in{i}_{field}");
        }
    }
    line.push_str(",mix_peak_db,clipped,glitches");
    line
}

/// One CSV row; peaks in dB, counters cumulative.
pub fn csv_row(snap: &Snapshot) -> String {
    let mut line = format!("{:.1}", snap.elapsed.as_secs_f64());
    for input in &snap.inputs {
        let _ = write!(
            line,
            ",{},{},{},{},{},{}",
            csv_db(input.peak),
            input.underruns,
            input.lost,
            input.drops,
            input.repeats,
            input.resyncs
        );
    }
    let _ = write!(
        line,
        ",{},{},{}",
        csv_db(snap.mix_peak),
        snap.clipped,
        snap.glitches
    );
    line
}

/// The `--log` file: a fresh CSV, flushed after every row so a closed window loses nothing.
pub struct Log {
    out: BufWriter<File>,
}

impl Log {
    /// Creates (or empties) the file at `path` and writes the header.
    ///
    /// # Errors
    ///
    /// When the file cannot be created or written.
    pub fn create(path: &Path, inputs: usize) -> io::Result<Self> {
        let mut log = Self {
            out: BufWriter::new(File::create(path)?),
        };
        log.line(&csv_header(inputs))?;
        Ok(log)
    }

    /// Appends one row.
    ///
    /// # Errors
    ///
    /// When writing fails.
    pub fn row(&mut self, snap: &Snapshot) -> io::Result<()> {
        self.line(&csv_row(snap))
    }

    fn line(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.out, "{text}")?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(elapsed_ms: u64, peak: f32, drops: u64) -> Snapshot {
        Snapshot {
            elapsed: Duration::from_millis(elapsed_ms),
            inputs: vec![InputSnapshot {
                peak,
                drops,
                ..InputSnapshot::default()
            }],
            mix_peak: peak,
            clipped: 0,
            glitches: 1,
        }
    }

    #[test]
    fn status_line_starts_with_the_run_time() {
        let line = status_line(&snap(3_723_000, 0.5, 7));
        assert!(line.starts_with("1:02:03 [1]   -6.0 dB"), "{line}");
        assert!(line.contains("fix -7/+0/0 | mix   -6.0 dB clipped 0 | glitches 1"));
    }

    #[test]
    fn csv_header_and_row_have_the_same_columns() {
        let mut two = snap(10_000, 0.0, 3);
        two.inputs.push(InputSnapshot::default());
        let header = csv_header(2);
        let row = csv_row(&two);
        assert_eq!(header.split(',').count(), row.split(',').count());
        assert!(header.starts_with("elapsed_s,in1_peak_db,in1_dropouts"));
        assert_eq!(row, "10.0,-inf,0,0,3,0,0,-inf,0,0,0,0,0,-inf,0,1");
    }

    #[test]
    fn merge_keeps_the_interval_peak_and_the_latest_counters() {
        let merged = snap(1_000, 0.8, 1).merge(&snap(2_000, 0.2, 5));
        assert_eq!(merged.inputs[0].peak, 0.8);
        assert_eq!(merged.inputs[0].drops, 5);
        assert_eq!(merged.mix_peak, 0.8);
        assert_eq!(merged.elapsed, Duration::from_secs(2));
    }

    #[test]
    fn log_writes_header_and_rows() {
        let path = std::env::temp_dir().join(format!("pult-log-test-{}.csv", std::process::id()));
        {
            let mut log = Log::create(&path, 1).expect("create log");
            log.row(&snap(10_000, 0.5, 2)).expect("write row");
        }
        let text = std::fs::read_to_string(&path).expect("read log");
        std::fs::remove_file(&path).expect("remove log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "10.0,-6.0,0,0,2,0,0,-6.0,0,1");
    }
}
