//! Dropouts as events. One stall of a device can run several rings dry or over at once and
//! bring a glitch report with it; what the buses play has one gap, so it counts once.
//!
//! The counters behind it (underruns, resyncs, glitches) only grow; [`Dropouts::update`]
//! reads their sum a few times a second and opens an event on a rise. Rises within
//! [`COALESCE`] of the event's start belong to it; a stall that goes on counts once per
//! [`COALESCE`].

use std::time::Duration;

/// How long after its start rises still belong to one event.
pub const COALESCE: Duration = Duration::from_secs(1);

/// Counts events from a growing sum of raw counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dropouts {
    raw: u64,
    events: u64,
    started: Option<Duration>,
}

impl Dropouts {
    /// Takes the raw sum at `now` (time since the run started) and returns the events so
    /// far. A sum that falls (a route rebuilt) only becomes the new base.
    pub fn update(&mut self, raw: u64, now: Duration) -> u64 {
        if raw > self.raw
            && self
                .started
                .is_none_or(|start| now.saturating_sub(start) >= COALESCE)
        {
            self.events += 1;
            self.started = Some(now);
        }
        self.raw = raw;
        self.events
    }

    /// Events so far.
    #[must_use]
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Events per hour of `elapsed`; zero before the first second.
    #[must_use]
    pub fn per_hour(&self, elapsed: Duration) -> f64 {
        per_hour(self.events, elapsed)
    }
}

/// `count` per hour of `elapsed`; zero before the first second.
#[must_use]
pub fn per_hour(count: u64, elapsed: Duration) -> f64 {
    if elapsed < Duration::from_secs(1) {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let count = count as f64;
        count * 3600.0 / elapsed.as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn a_stall_on_two_inputs_with_a_glitch_is_one_event() {
        let mut dropouts = Dropouts::default();
        assert_eq!(dropouts.update(0, ms(200)), 0);
        // Both inputs resync and the device reports a glitch, seen over two reads.
        assert_eq!(dropouts.update(2, ms(400)), 1);
        assert_eq!(dropouts.update(3, ms(600)), 1);
        assert_eq!(dropouts.update(3, ms(5_000)), 1);
        // A later stall is a new event.
        assert_eq!(dropouts.update(5, ms(5_200)), 2);
    }

    #[test]
    fn a_stall_that_goes_on_counts_once_a_second() {
        let mut dropouts = Dropouts::default();
        let mut raw = 0;
        for read in 1..=25 {
            raw += 1;
            dropouts.update(raw, ms(read * 200));
        }
        // Rises from 0.2 s to 5.0 s: events at 0.2, 1.2, 2.2, 3.2 and 4.2 s.
        assert_eq!(dropouts.events(), 5);
    }

    #[test]
    fn a_falling_sum_is_the_new_base() {
        let mut dropouts = Dropouts::default();
        dropouts.update(4, ms(200));
        assert_eq!(dropouts.update(1, ms(3_000)), 1);
        assert_eq!(dropouts.update(1, ms(6_000)), 1);
        assert_eq!(dropouts.update(2, ms(9_000)), 2);
    }

    #[test]
    fn scales_to_an_hour() {
        let mut dropouts = Dropouts::default();
        dropouts.update(3, ms(1_000));
        assert!((dropouts.per_hour(Duration::from_secs(1_800)) - 2.0).abs() < 1e-9);
        assert!(dropouts.per_hour(ms(500)).abs() < f64::EPSILON);
    }
}
