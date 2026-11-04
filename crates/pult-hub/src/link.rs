//! The way between the hub and the thread that owns the mixer.
//!
//! Reading a view takes the peaks since the previous one, and the status line or the window
//! need those. So the hub never reads the mixer: the owner hands each view it took to
//! [`Pump::pump`], which publishes it and carries out what clients asked for meanwhile. A
//! client that reads often costs the owner nothing, and a slow client only misses states.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use pult_core::midi::check_target;
use pult_core::protocol::{EventState, Op, State};
use pult_core::view::{Desk, MixerView};
use tokio::sync::{mpsc, oneshot, watch};

/// How many events a state carries.
const KEPT_EVENTS: usize = 32;

/// How many asked-for operations wait for the owner before clients hear "busy".
const WAITING_OPS: usize = 16;

/// How often a new state is published; views in between only raise its peaks.
pub const PUBLISH_EVERY: Duration = Duration::from_millis(100);

/// An operation and where its answer goes.
pub(crate) struct Asked {
    pub(crate) op: Op,
    pub(crate) answer: oneshot::Sender<Result<Vec<String>, String>>,
}

/// What a client asked of the MIDI controller, for the one who runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiAsk {
    /// The next movement binds this.
    Learn(String),
    /// Take the binding of this away.
    Forget(String),
}

/// The hub's end of the way.
pub struct Link {
    pub(crate) state: watch::Receiver<Option<Arc<str>>>,
    pub(crate) ops: mpsc::Sender<Asked>,
}

/// The owner's end of the way.
pub struct Pump {
    state: watch::Sender<Option<Arc<str>>>,
    ops: mpsc::Receiver<Asked>,
    events: VecDeque<EventState>,
    seq: u64,
    /// The view to publish next, with the highest peaks since the last one.
    pending: Option<MixerView>,
    published_at: Option<Duration>,
    /// A client asked for the route to end.
    stop_asked: bool,
    /// What clients asked of the MIDI controller.
    midi_asks: Vec<MidiAsk>,
}

/// A way between a hub and the owner of a mixer.
#[must_use]
pub fn link() -> (Arc<Link>, Pump) {
    let (state_tx, state_rx) = watch::channel(None);
    let (ops_tx, ops_rx) = mpsc::channel(WAITING_OPS);
    (
        Arc::new(Link {
            state: state_rx,
            ops: ops_tx,
        }),
        Pump {
            state: state_tx,
            ops: ops_rx,
            events: VecDeque::with_capacity(KEPT_EVENTS),
            seq: 0,
            pending: None,
            published_at: None,
            stop_asked: false,
            midi_asks: Vec::new(),
        },
    )
}

/// The most of a line a client can get shown.
const LONGEST_LINE: usize = 300;

/// A line that holds a client's words, fit for a terminal: no control characters, which could
/// move the cursor or rewrite what the console shows, and no novel.
fn printable(line: &str) -> String {
    line.chars()
        .take(LONGEST_LINE)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

impl Pump {
    /// Hands on `view`, the owner's newest of `desk`, and carries out what clients asked for.
    /// Returns what those operations did, one line each, for the owner to show: someone else
    /// just changed this mixer.
    pub fn pump(&mut self, desk: &mut dyn Desk, view: &MixerView) -> Vec<String> {
        self.keep(view);
        let due = self
            .published_at
            .is_none_or(|at| view.elapsed.saturating_sub(at) >= PUBLISH_EVERY);
        if due {
            self.publish(view.elapsed);
        }
        let mut done = Vec::new();
        while let Ok(asked) = self.ops.try_recv() {
            // Not an action of the desk: ending the route is the owner's.
            if matches!(asked.op, Op::Stop {}) {
                self.stop_asked = true;
                let _ = asked.answer.send(Ok(vec!["stopping".to_owned()]));
                done.push("asked to stop".to_owned());
                continue;
            }
            match &asked.op {
                Op::Learn { target } => {
                    let answer = check_target(target, view).map(|()| {
                        self.midi_asks.push(MidiAsk::Learn(target.clone()));
                        vec![format!("move a control on the device for {target}")]
                    });
                    let _ = asked.answer.send(answer);
                    continue;
                }
                Op::Unlearn { target } => {
                    self.midi_asks.push(MidiAsk::Forget(target.clone()));
                    let _ = asked.answer.send(Ok(Vec::new()));
                    continue;
                }
                _ => {}
            }
            let answer = asked.op.action(view).and_then(|action| desk.act(action));
            match &answer {
                Ok(lines) => done.extend(lines.iter().map(|line| printable(line))),
                Err(why) => done.push(format!("refused: {}", printable(why))),
            }
            // A client that stopped waiting does not need the answer.
            let _ = asked.answer.send(answer);
        }
        done
    }

    /// What clients asked of the MIDI controller since the last call.
    pub fn take_midi_asks(&mut self) -> Vec<MidiAsk> {
        std::mem::take(&mut self.midi_asks)
    }

    /// Whether a client asked for the route to end; the owner ends it, as if the person had.
    #[must_use]
    pub fn stop_asked(&self) -> bool {
        self.stop_asked
    }

    /// Notes the events of `view` and merges it into the state that is due next.
    fn keep(&mut self, view: &MixerView) {
        for event in &view.events {
            self.seq += 1;
            if self.events.len() == KEPT_EVENTS {
                self.events.pop_front();
            }
            self.events.push_back(EventState::new(self.seq, event));
        }
        let mut next = view.clone();
        if let Some(earlier) = &self.pending {
            for (input, old) in next.inputs.iter_mut().zip(&earlier.inputs) {
                input.peak_db = higher(input.peak_db, old.peak_db);
            }
            for (bus, old) in next.buses.iter_mut().zip(&earlier.buses) {
                bus.peak_db = higher(bus.peak_db, old.peak_db);
            }
        }
        self.pending = Some(next);
    }

    fn publish(&mut self, now: Duration) {
        let Some(view) = self.pending.take() else {
            return;
        };
        let state = State::new(&view, self.events.iter().cloned().collect());
        // A state is plain data, so writing it cannot fail; if it did, clients keep the last.
        if let Ok(text) = serde_json::to_string(&state) {
            self.state.send_replace(Some(Arc::from(text)));
            self.published_at = Some(now);
        }
    }
}

fn higher(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::protocol::OpValue;
    use pult_core::view::{EventKind, EventView, FakeDesk};

    fn latest(link: &Link) -> State {
        let text = link.state.borrow().clone().expect("a state");
        serde_json::from_str(&text).expect("a state as JSON")
    }

    fn at(view: &mut MixerView, millis: u64) {
        view.elapsed = Duration::from_millis(millis);
    }

    #[test]
    fn views_between_two_states_raise_the_peaks() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        let mut view = desk.view();
        at(&mut view, 0);
        view.inputs[0].peak_db = Some(-20.0);
        pump.pump(&mut desk, &view);
        assert_eq!(latest(&link).inputs[0].peak_db, Some(-20.0));

        at(&mut view, 50);
        view.inputs[0].peak_db = Some(-3.0);
        view.buses[0].peak_db = Some(-9.0);
        pump.pump(&mut desk, &view);
        assert_eq!(latest(&link).inputs[0].peak_db, Some(-20.0), "not due yet");

        at(&mut view, 100);
        view.inputs[0].peak_db = None;
        view.buses[0].peak_db = None;
        pump.pump(&mut desk, &view);
        let state = latest(&link);
        assert_eq!(state.inputs[0].peak_db, Some(-3.0));
        assert_eq!(state.buses[0].peak_db, Some(-9.0));

        // Published peaks are spent.
        at(&mut view, 200);
        pump.pump(&mut desk, &view);
        assert_eq!(latest(&link).inputs[0].peak_db, None);
    }

    #[test]
    fn events_are_numbered_and_only_the_latest_kept() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        let mut view = desk.view();
        for number in 0..KEPT_EVENTS + 3 {
            at(&mut view, 1000 * number as u64);
            view.events = vec![EventView {
                elapsed: view.elapsed,
                kind: EventKind::Lost,
                text: format!("event {number}"),
            }];
            pump.pump(&mut desk, &view);
        }
        let events = latest(&link).events;
        assert_eq!(events.len(), KEPT_EVENTS);
        assert_eq!(events[0].seq, 4);
        assert_eq!(events[KEPT_EVENTS - 1].seq, KEPT_EVENTS as u64 + 3);
    }

    #[test]
    fn asked_operations_reach_the_desk_and_answer() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        let (good, mut good_answer) = oneshot::channel();
        let sent = link.ops.try_send(Asked {
            op: Op::Set {
                address: "input/1/gain".to_owned(),
                value: OpValue::Number(-12.0),
                ramp_ms: None,
            },
            answer: good,
        });
        assert!(sent.is_ok());
        let (bad, mut bad_answer) = oneshot::channel();
        let sent = link.ops.try_send(Asked {
            op: Op::Set {
                address: "input/9/gain".to_owned(),
                value: OpValue::Number(0.0),
                ramp_ms: None,
            },
            answer: bad,
        });
        assert!(sent.is_ok());
        let view = desk.view();
        let done = pump.pump(&mut desk, &view);
        assert_eq!(done.len(), 2, "{done:?}");
        assert!(done[1].starts_with("refused: "), "{done:?}");
        assert!(good_answer.try_recv().expect("an answer").is_ok());
        assert!(bad_answer.try_recv().expect("an answer").is_err());
        assert!((desk.view().inputs[0].gain_db + 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn midi_is_for_the_owner_and_a_target_is_checked_first() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        let ask = |op| {
            let (asked, answer) = oneshot::channel();
            assert!(link.ops.try_send(Asked { op, answer: asked }).is_ok());
            answer
        };
        let mut good = ask(Op::Learn {
            target: "input/1/gain".to_owned(),
        });
        let mut bad = ask(Op::Learn {
            target: "input/9/gain".to_owned(),
        });
        let mut free = ask(Op::Unlearn {
            target: "input/9/gain".to_owned(),
        });
        let view = desk.view();
        pump.pump(&mut desk, &view);
        assert!(good.try_recv().expect("an answer").is_ok());
        assert!(bad.try_recv().expect("an answer").is_err());
        assert!(free.try_recv().expect("an answer").is_ok());
        assert_eq!(
            pump.take_midi_asks(),
            [
                MidiAsk::Learn("input/1/gain".to_owned()),
                MidiAsk::Forget("input/9/gain".to_owned())
            ]
        );
        assert_eq!(pump.take_midi_asks(), []);
    }

    #[test]
    fn a_stop_is_for_the_owner_and_not_for_the_desk() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        assert!(!pump.stop_asked());
        let (asked, mut answer) = oneshot::channel();
        let sent = link.ops.try_send(Asked {
            op: Op::Stop {},
            answer: asked,
        });
        assert!(sent.is_ok());
        let view = desk.view();
        let done = pump.pump(&mut desk, &view);
        assert_eq!(done, ["asked to stop"]);
        assert!(pump.stop_asked());
        assert!(answer.try_recv().expect("an answer").is_ok());
    }

    #[test]
    fn what_a_client_said_reaches_the_owner_without_control_characters() {
        let (link, mut pump) = link();
        let mut desk = FakeDesk::new();
        let name = format!("\u{1b}[2J\nnothing{}", "x".repeat(10_000));
        let (asked, _answer) = oneshot::channel();
        let sent = link.ops.try_send(Asked {
            op: Op::Scene { name },
            answer: asked,
        });
        assert!(sent.is_ok());
        let view = desk.view();
        let done = pump.pump(&mut desk, &view);
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(done[0].contains("?[2J?nothing"), "{done:?}");
        assert!(!done[0].chars().any(char::is_control), "{done:?}");
        assert!(done[0].chars().count() < LONGEST_LINE + 20);
    }
}
