//! The device-free heart of pult: levels, channel mapping, drift control and the mixer that
//! the audio callbacks drive.
//!
//! Nothing here touches a device, a file or the clock, so all of it runs in plain tests.
//! The realtime rule of the callbacks holds for every function they call: no allocation, no
//! lock, no logging. The control thread talks to them through atomics only.

#![forbid(unsafe_code)]

// The tests check that the audio path allocates nothing (`assert_no_alloc`).
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

mod binding;
pub mod control;
pub mod drift;
pub mod dropouts;
pub mod level;
pub mod midi;
pub mod mixer;
pub mod protocol;
pub mod ramp;
pub mod rate;
pub mod remix;
pub mod scale;
pub mod select;
pub mod tone;
pub mod view;

pub use drift::{DriftControl, Event, Plan};
pub use dropouts::Dropouts;
pub use level::{AtomicF32, Counter, PeakMeter, db_to_linear, linear_to_db};
pub use mixer::{
    BusControls, BusHandle, BusStats, Capture, InputConfig, InputControls, InputHandle, InputStats,
    Mixer, MixerInput, connect, connect_with,
};
pub use ramp::{Follower, Glide, MAX_RAMP_MS, MIN_RAMP_MS, Ramp};
pub use rate::RateControl;
pub use remix::{remix_add, remix_add_ramped};
pub use select::{SelectError, select};
pub use tone::Sine;
pub use view::{
    Action, BusView, Desk, EventKind, EventView, FakeDesk, InputView, MeterPoint, MixerView, Reply,
    SourceKind,
};
