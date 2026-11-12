//! What `pult run` shows and logs: snapshots of a running route's meters and counters, the
//! live status line, and the CSV rows of `--log`.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pult_core::{Dropouts, MeterPoint, MixerView, dropouts, linear_to_db};
use pult_engine::Running;

/// How often `--log` writes a row.
pub const LOG_INTERVAL: Duration = Duration::from_secs(10);

/// One input at one moment: its peak since the last snapshot and its counters so far,
/// summed over the rings of all buses it feeds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputSnapshot {
    /// Linear peak since the previous snapshot.
    pub peak: f32,
    /// Times a ring ran dry.
    pub underruns: u64,
    /// Frames lost at capture.
    pub lost: u64,
    /// The resampler's correction against clock drift, in ppm; with several buses the one
    /// furthest from zero.
    pub drift_ppm: f32,
    /// Hard resyncs.
    pub resyncs: u64,
    /// Frames skipped once after settling.
    pub aligned: u64,
    /// Whether the device is playing.
    pub present: bool,
    /// Whether it is muted.
    pub muted: bool,
    /// Times the device was lost so far.
    pub outages: u64,
}

/// One bus at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BusSnapshot {
    /// Linear peak of the bus output since the previous snapshot.
    pub peak: f32,
    /// Clipped or invalid output samples so far.
    pub clipped: u64,
    /// How far its limiter turned down at most since the previous snapshot, in dB.
    pub limited_db: f32,
    /// How far its limiter turned down at most so far, in dB.
    pub most_limited_db: f32,
    /// Whether the device is playing.
    pub present: bool,
    /// Whether it is muted.
    pub muted: bool,
    /// Times the device was lost so far.
    pub outages: u64,
}

/// The whole route at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Time since the route started.
    pub elapsed: Duration,
    /// The inputs, in route order.
    pub inputs: Vec<InputSnapshot>,
    /// The buses, in route order.
    pub buses: Vec<BusSnapshot>,
    /// Glitches the devices reported so far.
    pub glitches: u64,
    /// Dropouts so far as events ([`pult_core::dropouts`]).
    pub dropouts: u64,
}

impl Snapshot {
    /// Reads the route's meters (which starts their next measurement) and counters, and
    /// feeds the raw dropouts to `events`.
    pub fn take(running: &Running, elapsed: Duration, events: &mut Dropouts) -> Self {
        let inputs = running
            .inputs
            .iter()
            .map(|input| {
                let start = InputSnapshot {
                    present: input.present,
                    muted: input.controls.muted.load(Ordering::Relaxed),
                    outages: input.outages,
                    ..InputSnapshot::default()
                };
                let pre = input.controls.pre_peak.take();
                let sum = input.routes.iter().fold(start, |sum, route| {
                    let stats = &route.stats;
                    InputSnapshot {
                        peak: sum.peak.max(stats.peak.take()),
                        underruns: sum.underruns + stats.underruns.get(),
                        lost: sum.lost + stats.dropped_at_capture.get(),
                        drift_ppm: farther(sum.drift_ppm, stats.correction_ppm.load()),
                        resyncs: sum.resyncs + stats.resyncs.get(),
                        aligned: sum.aligned + stats.aligned.get(),
                        ..sum
                    }
                });
                // Before the fader the input has one meter, its stage's.
                if input.controls.meter_pre_fader.load(Ordering::Relaxed) {
                    InputSnapshot { peak: pre, ..sum }
                } else {
                    sum
                }
            })
            .collect();
        let buses = running
            .buses
            .iter()
            .map(|bus| BusSnapshot {
                peak: bus.handle.stats.peak.take(),
                clipped: bus.handle.stats.clipped.get(),
                limited_db: bus.handle.stats.limited_db.take(),
                most_limited_db: bus.handle.stats.most_limited_db.peek(),
                present: bus.present,
                muted: bus.handle.controls.muted.load(Ordering::Relaxed),
                outages: bus.outages,
            })
            .collect();
        let mut snap = Self {
            elapsed,
            inputs,
            buses,
            glitches: running.xruns(),
            dropouts: 0,
        };
        snap.dropouts = events.update(snap.raw_dropouts(), elapsed);
        snap
    }

    /// Inputs that ran dry, hard resyncs and the glitches the devices reported, each counted
    /// on its own; one stall can bring several.
    #[must_use]
    pub fn raw_dropouts(&self) -> u64 {
        let inputs: u64 = self.inputs.iter().map(|i| i.underruns + i.resyncs).sum();
        inputs + self.glitches
    }

    /// [`Self::dropouts`] per hour of run time; zero before the first second.
    #[must_use]
    pub fn dropouts_per_hour(&self) -> f64 {
        dropouts::per_hour(self.dropouts, self.elapsed)
    }

    /// Keeps the higher peaks of `self` and `later`, and everything else of `later`: the
    /// snapshot of a whole log interval.
    #[must_use]
    pub fn merge(&self, later: &Self) -> Self {
        let mut merged = later.clone();
        for (m, earlier) in merged.inputs.iter_mut().zip(&self.inputs) {
            m.peak = m.peak.max(earlier.peak);
        }
        for (m, earlier) in merged.buses.iter_mut().zip(&self.buses) {
            m.peak = m.peak.max(earlier.peak);
        }
        merged
    }
}

/// Whichever of two corrections lies further from zero.
fn farther(a: f32, b: f32) -> f32 {
    if b.abs() > a.abs() { b } else { a }
}

/// `h:mm:ss`.
pub fn clock(elapsed: Duration) -> String {
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

/// The level column of the status line: `-18.3 dB`, `missing` while the device is gone,
/// `muted` while it is muted.
fn level(present: bool, muted: bool, peak: f32) -> String {
    if !present {
        format!("{:>9}", "missing")
    } else if muted {
        format!("{:>9}", "muted")
    } else {
        format!("{} dB", db(peak))
    }
}

/// The live status line: run time; per input level, dropouts, frames lost at capture, the
/// resampler's drift correction in ppm and the hard resyncs; then each bus by `bus_names`;
/// last the device glitches and all dropouts per hour.
pub fn status_line(snap: &Snapshot, bus_names: &[String]) -> String {
    // Writing into a String cannot fail.
    let mut line = clock(snap.elapsed);
    for (i, input) in snap.inputs.iter().enumerate() {
        let _ = write!(
            line,
            " [{}] {} out {} lost {} drift {:+.0} ppm resync {}",
            i + 1,
            level(input.present, input.muted, input.peak),
            input.underruns,
            input.lost,
            input.drift_ppm,
            input.resyncs
        );
    }
    for (bus, name) in snap.buses.iter().zip(bus_names) {
        let _ = write!(
            line,
            " | {name} {} clipped {}",
            level(bus.present, bus.muted, bus.peak),
            bus.clipped
        );
        // While it works: `clipped` stays 0 then, and nothing else would say the bus is too hot.
        if bus.limited_db >= 0.05 {
            let _ = write!(line, " limit -{:.1} dB", bus.limited_db);
        }
    }
    let _ = write!(
        line,
        " | glitches {} | {:.1} dropouts/h",
        snap.glitches,
        snap.dropouts_per_hour()
    );
    line
}

/// The line `pult run` ends with: run time, dropouts in all and per hour, frames lost at
/// capture and clipped samples, over all inputs and buses.
pub fn summary(snap: &Snapshot) -> String {
    let lost: u64 = snap.inputs.iter().map(|i| i.lost).sum();
    let clipped: u64 = snap.buses.iter().map(|b| b.clipped).sum();
    let limited = snap
        .buses
        .iter()
        .map(|b| b.most_limited_db)
        .fold(0.0, f32::max);
    let mut line = format!(
        "ran {}: {} dropouts ({:.1} per hour), {lost} frames lost, {clipped} samples clipped",
        clock(snap.elapsed),
        snap.dropouts,
        snap.dropouts_per_hour()
    );
    if limited >= 0.05 {
        let _ = write!(line, ", limiter down by up to {limited:.1} dB");
    }
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

/// The CSV header for `inputs` inputs and the buses `bus_names`.
pub fn csv_header(inputs: usize, bus_names: &[String]) -> String {
    let mut line = "elapsed_s".to_owned();
    for i in 1..=inputs {
        for field in [
            "peak_db",
            "dropouts",
            "lost",
            "drift_ppm",
            "resyncs",
            "aligned",
            "present",
            "outages",
        ] {
            let _ = write!(line, ",in{i}_{field}");
        }
    }
    for name in bus_names {
        let _ = write!(
            line,
            ",{name}_peak_db,{name}_clipped,{name}_limited_db,{name}_present,{name}_outages"
        );
    }
    line.push_str(",glitches,dropouts,dropouts_per_hour");
    line
}

/// One CSV row; peaks in dB, counters cumulative.
pub fn csv_row(snap: &Snapshot) -> String {
    let mut line = format!("{:.1}", snap.elapsed.as_secs_f64());
    for input in &snap.inputs {
        let _ = write!(
            line,
            ",{},{},{},{:.1},{},{},{},{}",
            csv_db(input.peak),
            input.underruns,
            input.lost,
            input.drift_ppm,
            input.resyncs,
            input.aligned,
            u8::from(input.present),
            input.outages
        );
    }
    for bus in &snap.buses {
        let _ = write!(
            line,
            ",{},{},{:.1},{},{}",
            csv_db(bus.peak),
            bus.clipped,
            bus.limited_db,
            u8::from(bus.present),
            bus.outages
        );
    }
    let _ = write!(
        line,
        ",{},{},{:.2}",
        snap.glitches,
        snap.dropouts,
        snap.dropouts_per_hour()
    );
    line
}

/// Gives `view` the peaks of `snap`, which took them from the same meters just before: reading
/// empties a meter, and the view of a run comes second. Before the fader nobody else reads.
pub fn with_peaks_of(view: &mut MixerView, snap: &Snapshot) {
    let db = |peak: f32| (peak > 0.0).then(|| linear_to_db(peak));
    if view.meter_point == MeterPoint::PostFader {
        for (input, taken) in view.inputs.iter_mut().zip(&snap.inputs) {
            input.peak_db = db(taken.peak);
        }
    }
    for (bus, taken) in view.buses.iter_mut().zip(&snap.buses) {
        bus.peak_db = db(taken.peak);
    }
}

/// The `--log` file: a fresh CSV, flushed after every row so a closed window loses nothing.
pub struct Log {
    out: BufWriter<File>,
}

impl Log {
    /// Creates (or empties) the file at `path` and writes the header. The file stays ours
    /// for as long as the log lives: others can read it, but a second run on the same file
    /// is refused and leaves the rows alone.
    ///
    /// # Errors
    ///
    /// When another run or program has the file open, or it cannot be created or written.
    pub fn create(path: &Path, inputs: usize, bus_names: &[String]) -> io::Result<Self> {
        let mut log = Self {
            out: BufWriter::new(create_exclusive(path)?),
        };
        log.line(&csv_header(inputs, bus_names))?;
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

/// Windows refuses the open before it empties anything. A file lock would also keep readers
/// out there, and the rows are worth reading during a long run.
#[cfg(windows)]
fn create_exclusive(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|e| {
            if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                in_use()
            } else {
                e
            }
        })
}

/// Elsewhere file locks do not keep readers out, so the file is emptied once it is locked.
#[cfg(not(windows))]
fn create_exclusive(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => in_use(),
        std::fs::TryLockError::Error(e) => e,
    })?;
    file.set_len(0)?;
    Ok(file)
}

fn in_use() -> io::Error {
    io::Error::other("another run or program has it open")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(elapsed_ms: u64, peak: f32, drift_ppm: f32) -> Snapshot {
        Snapshot {
            elapsed: Duration::from_millis(elapsed_ms),
            inputs: vec![InputSnapshot {
                peak,
                drift_ppm,
                present: true,
                ..InputSnapshot::default()
            }],
            buses: vec![BusSnapshot {
                peak,
                clipped: 0,
                limited_db: 0.0,
                most_limited_db: 0.0,
                present: true,
                muted: false,
                outages: 0,
            }],
            glitches: 1,
            dropouts: 1,
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn status_line_starts_with_the_run_time_and_ends_with_the_buses() {
        let mut two = snap(3_723_000, 0.5, -7.4);
        two.buses.push(BusSnapshot {
            peak: 0.0,
            clipped: 4,
            limited_db: 3.25,
            most_limited_db: 9.0,
            present: true,
            muted: false,
            outages: 0,
        });
        let line = status_line(&two, &names(&["A1", "B1"]));
        assert!(line.starts_with("1:02:03 [1]   -6.0 dB"), "{line}");
        assert!(
            line.contains(
                "drift -7 ppm resync 0 | A1   -6.0 dB clipped 0 | B1   -inf dB clipped 4 limit -3.2 dB"
            ),
            "{line}"
        );
        // One glitch in 1:02:03.
        assert!(line.ends_with(" | glitches 1 | 1.0 dropouts/h"), "{line}");
        assert!(summary(&two).ends_with(", limiter down by up to 9.0 dB"));
    }

    #[test]
    fn status_line_marks_missing_devices() {
        let mut gone = snap(0, 0.5, 0.0);
        gone.inputs[0].present = false;
        gone.buses[0].muted = true;
        let line = status_line(&gone, &names(&["A1"]));
        assert!(line.starts_with("0:00:00 [1]   missing out 0"), "{line}");
        assert!(line.contains(" | A1     muted clipped 0 |"), "{line}");
    }

    #[test]
    fn csv_header_and_row_have_the_same_columns() {
        let mut two = snap(10_000, 0.0, 12.34);
        two.inputs.push(InputSnapshot::default());
        let header = csv_header(2, &names(&["A1"]));
        let row = csv_row(&two);
        assert_eq!(header.split(',').count(), row.split(',').count());
        assert!(header.starts_with("elapsed_s,in1_peak_db,in1_dropouts"));
        assert!(header.ends_with(
            ",in2_aligned,in2_present,in2_outages,A1_peak_db,A1_clipped,A1_limited_db,A1_present,A1_outages,\
             glitches,dropouts,dropouts_per_hour"
        ));
        assert_eq!(
            row,
            "10.0,-inf,0,0,12.3,0,0,1,0,-inf,0,0,0.0,0,0,0,0,-inf,0,0.0,1,0,1,1,360.00"
        );
    }

    #[test]
    fn dropouts_count_events_and_scale_to_an_hour() {
        let mut half_hour = snap(1_800_000, 0.0, 0.0);
        half_hour.inputs[0].underruns = 2;
        half_hour.inputs[0].resyncs = 1;
        half_hour.inputs[0].lost = 7;
        half_hour.buses[0].clipped = 3;
        half_hour.dropouts = 4;
        assert_eq!(half_hour.raw_dropouts(), 4);
        assert!((half_hour.dropouts_per_hour() - 8.0).abs() < 1e-9);
        assert_eq!(
            summary(&half_hour),
            "ran 0:30:00: 4 dropouts (8.0 per hour), 7 frames lost, 3 samples clipped"
        );
        assert!(snap(500, 0.0, 0.0).dropouts_per_hour().abs() < f64::EPSILON);
    }

    #[test]
    fn merge_keeps_the_interval_peak_and_the_latest_counters() {
        let merged = snap(1_000, 0.8, 1.0).merge(&snap(2_000, 0.2, 5.0));
        assert_eq!(merged.inputs[0].peak, 0.8);
        assert_eq!(merged.inputs[0].drift_ppm, 5.0);
        assert_eq!(merged.buses[0].peak, 0.8);
        assert_eq!(merged.elapsed, Duration::from_secs(2));
    }

    #[test]
    fn a_view_gets_the_peaks_the_snapshot_took() {
        let mut view = pult_core::view::Desk::view(&mut pult_core::FakeDesk::new());
        let mut taken = snap(0, 0.5, 0.0);
        taken
            .inputs
            .resize(view.inputs.len(), InputSnapshot::default());
        taken.buses.resize(view.buses.len(), BusSnapshot::default());
        taken.buses[0].peak = 0.25;

        with_peaks_of(&mut view, &taken);
        let close = |a: Option<f32>, b: f32| (a.expect("a peak") - b).abs() < 0.05;
        assert!(close(view.inputs[0].peak_db, -6.0), "{view:?}");
        assert_eq!(view.inputs[1].peak_db, None, "silence");
        assert!(close(view.buses[0].peak_db, -12.0), "{view:?}");

        // Before the fader the view has measured for itself.
        view.meter_point = MeterPoint::PreFader;
        view.inputs[1].peak_db = Some(-3.0);
        with_peaks_of(&mut view, &taken);
        assert_eq!(view.inputs[1].peak_db, Some(-3.0));
    }

    #[test]
    fn log_writes_header_and_rows() {
        let path = std::env::temp_dir().join(format!("pult-log-test-{}.csv", std::process::id()));
        {
            let mut log = Log::create(&path, 1, &names(&["A1"])).expect("create log");
            log.row(&snap(10_000, 0.5, 2.0)).expect("write row");
        }
        let text = std::fs::read_to_string(&path).expect("read log");
        std::fs::remove_file(&path).expect("remove log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1],
            "10.0,-6.0,0,0,2.0,0,0,1,0,-6.0,0,0.0,1,0,1,1,360.00"
        );
    }

    #[test]
    fn a_second_run_on_the_same_log_is_refused_and_leaves_it_alone() {
        let path =
            std::env::temp_dir().join(format!("pult-log-busy-test-{}.csv", std::process::id()));
        let bus = names(&["A1"]);
        let mut first = Log::create(&path, 1, &bus).expect("create log");
        first.row(&snap(10_000, 0.5, 2.0)).expect("write row");

        let refused = Log::create(&path, 1, &bus)
            .err()
            .expect("second run refused");
        assert_eq!(refused.to_string(), "another run or program has it open");

        // Nothing was emptied, the file can be read meanwhile and the first run goes on.
        first.row(&snap(20_000, 0.5, 2.0)).expect("write row");
        let text = std::fs::read_to_string(&path).expect("read log");
        assert_eq!(text.lines().count(), 3);

        // With the first run gone the file is free again, and starts over.
        drop(first);
        drop(Log::create(&path, 1, &bus).expect("create log again"));
        let text = std::fs::read_to_string(&path).expect("read log");
        std::fs::remove_file(&path).expect("remove log");
        assert_eq!(text.lines().count(), 1);
    }
}
