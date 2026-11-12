//! The device-free heart of pult: levels, channel mapping, drift control, gate, compressor and
//! equalizer, and the mixer that the audio callbacks drive.
//!
//! Nothing here touches a device, a file or the clock, so all of it runs in plain tests.
//! The realtime rule of the callbacks holds for every function they call: no allocation, no
//! lock, no logging. The control thread talks to them through atomics only.

#![forbid(unsafe_code)]

// The tests check that the audio path allocates nothing (`assert_no_alloc`).
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

pub mod automix;
mod binding;
pub mod clicks;
pub mod control;
pub mod drift;
pub mod dropouts;
pub mod dynamics;
pub mod eq;
pub mod level;
pub mod limiter;
pub mod loudness;
pub mod midi;
pub mod mixer;
pub mod protocol;
pub mod ramp;
pub mod rate;
pub mod remix;
pub mod scale;
pub mod select;
pub mod stage;
pub mod tone;
pub mod view;

pub use automix::{AutomixGroup, AutomixMember, PowerFollower};
pub use clicks::{Click, Clicks};
pub use drift::{DriftControl, Event, Plan};
pub use dropouts::Dropouts;
pub use dynamics::{
    Compressor, CompressorControls, CompressorParams, Gate, GateControls, GateParams,
};
pub use eq::{Band, BandKind, EQ_BANDS, EqControls, Equalizer, response_db};
pub use level::{AtomicF32, Counter, PeakMeter, db_to_linear, linear_to_db};
pub use limiter::{LimitChange, Limiter, LimiterControls};
pub use loudness::{LoudnessMeter, LoudnessStats, LoudnessTap, loudness};
pub use mixer::{
    BusControls, BusHandle, BusStats, Capture, InputConfig, InputControls, InputHandle, InputStats,
    Mixer, MixerInput, connect, connect_with,
};
pub use ramp::{Follower, Glide, MAX_RAMP_MS, MIN_RAMP_MS, Ramp};
pub use rate::RateControl;
pub use remix::{remix_add, remix_add_ramped};
pub use select::{SelectError, select};
pub use stage::{InputStage, STAGE_FRAMES};
pub use tone::Sine;
pub use view::{
    Action, BusView, Desk, EventKind, EventView, FakeDesk, InputView, LoudnessView, MeterPoint,
    MixerView, Reply, SendView, SourceKind,
};
