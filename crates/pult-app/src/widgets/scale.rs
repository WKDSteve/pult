//! dB scales: where a level sits on a fader or a meter, and back.
//!
//! Both scales are piecewise linear in dB between fixed points, like the printed scale of a
//! console fader: the range around 0 dB gets most of the travel, the quiet end is compressed.

/// Lowest gain a fader can set; below it the fader is off (`-inf`).
pub const FADER_FLOOR_DB: f32 = -60.0;
/// Highest gain a fader can set.
pub const FADER_TOP_DB: f32 = 12.0;
/// Lowest level a meter shows; anything quieter is an empty meter.
pub const METER_FLOOR_DB: f32 = -60.0;

/// One point of a scale: a level in dB at a position from 0 (bottom) to 1 (top).
type Point = (f32, f32);

/// The fader law; 0 dB sits at three quarters of the travel.
const FADER: [Point; 10] = [
    (FADER_FLOOR_DB, 0.0),
    (-40.0, 0.12),
    (-30.0, 0.22),
    (-20.0, 0.35),
    (-12.0, 0.50),
    (-6.0, 0.63),
    (0.0, 0.76),
    (6.0, 0.88),
    (10.0, 0.96),
    (FADER_TOP_DB, 1.0),
];

/// The meter law, in dBFS; the top 18 dB get almost half of the height.
const METER: [Point; 8] = [
    (METER_FLOOR_DB, 0.0),
    (-48.0, 0.10),
    (-36.0, 0.24),
    (-24.0, 0.42),
    (-18.0, 0.52),
    (-12.0, 0.65),
    (-6.0, 0.81),
    (0.0, 1.0),
];

/// Marks printed next to a fader.
pub const FADER_MARKS: [f32; 9] = [12.0, 6.0, 0.0, -6.0, -12.0, -20.0, -30.0, -40.0, -60.0];
/// Marks printed next to a meter.
pub const METER_MARKS: [f32; 7] = [0.0, -6.0, -12.0, -18.0, -24.0, -36.0, -48.0];

fn to_pos(points: &[Point], db: f32) -> f32 {
    if db.is_nan() || db <= points[0].0 {
        return 0.0;
    }
    for pair in points.windows(2) {
        let ((db0, pos0), (db1, pos1)) = (pair[0], pair[1]);
        if db <= db1 {
            return pos0 + (db - db0) / (db1 - db0) * (pos1 - pos0);
        }
    }
    1.0
}

fn to_db(points: &[Point], pos: f32) -> f32 {
    let pos = pos.clamp(0.0, 1.0);
    for pair in points.windows(2) {
        let ((db0, pos0), (db1, pos1)) = (pair[0], pair[1]);
        if pos <= pos1 {
            return db0 + (pos - pos0) / (pos1 - pos0) * (db1 - db0);
        }
    }
    points[points.len() - 1].0
}

/// Position of a gain on the fader, 0 (off) to 1 (+12 dB).
#[must_use]
pub fn fader_pos(db: f32) -> f32 {
    to_pos(&FADER, db)
}

/// Gain at a fader position; the bottom is `-inf` (off).
#[must_use]
pub fn fader_db(pos: f32) -> f32 {
    if pos <= 0.0 {
        f32::NEG_INFINITY
    } else {
        to_db(&FADER, pos)
    }
}

/// Height of a level on the meter, 0 (silent) to 1 (full scale).
#[must_use]
pub fn meter_pos(dbfs: f32) -> f32 {
    to_pos(&METER, dbfs)
}

/// A gain as the strip prints it: `-6.0`, `+3.5`, `0.0`, `-70.0`, and `-inf` for off. A gain
/// below the fader's floor still prints as a number: it still sounds.
#[must_use]
pub fn format_db(db: f32) -> String {
    if db == f32::NEG_INFINITY {
        "-inf".to_owned()
    } else if db.abs() < 0.05 {
        "0.0".to_owned()
    } else {
        format!("{db:+.1}")
    }
}

/// A mark as the scale prints it: `+12`, `0`, `-60`.
#[must_use]
pub fn format_mark(db: f32) -> String {
    if db > 0.0 {
        format!("+{db:.0}")
    } else {
        format!("{db:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn fader_ends_and_unity() {
        assert!(close(fader_pos(FADER_TOP_DB), 1.0));
        assert!(close(fader_pos(0.0), 0.76));
        assert!(close(fader_pos(FADER_FLOOR_DB), 0.0));
        assert!(close(fader_pos(f32::NEG_INFINITY), 0.0));
        assert!(close(fader_pos(40.0), 1.0));
        assert_eq!(fader_db(0.0), f32::NEG_INFINITY);
        assert!(close(fader_db(1.0), FADER_TOP_DB));
    }

    #[test]
    fn fader_round_trips_and_is_monotonic() {
        let mut last = -1.0;
        for step in 0..=720 {
            let db = FADER_FLOOR_DB + step as f32 * 0.1;
            let pos = fader_pos(db);
            assert!(pos > last, "{db} dB at {pos} after {last}");
            last = pos;
            if step > 0 {
                assert!(
                    (fader_db(pos) - db).abs() < 1e-3,
                    "{db} dB comes back as {}",
                    fader_db(pos)
                );
            }
        }
    }

    #[test]
    fn meter_law() {
        assert!(close(meter_pos(0.0), 1.0));
        assert!(close(meter_pos(6.0), 1.0));
        assert!(close(meter_pos(-18.0), 0.52));
        assert!(close(meter_pos(-90.0), 0.0));
        assert!(close(meter_pos(f32::NAN), 0.0));
        assert!(meter_pos(-3.0) > meter_pos(-6.0));
    }

    #[test]
    fn formats() {
        assert_eq!(format_db(-6.0), "-6.0");
        assert_eq!(format_db(3.5), "+3.5");
        assert_eq!(format_db(0.01), "0.0");
        assert_eq!(format_db(-0.04), "0.0");
        assert_eq!(format_db(f32::NEG_INFINITY), "-inf");
        assert_eq!(format_db(-60.0), "-60.0");
        assert_eq!(format_db(-70.0), "-70.0");
        assert_eq!(format_mark(12.0), "+12");
        assert_eq!(format_mark(0.0), "0");
        assert_eq!(format_mark(-60.0), "-60");
    }
}
