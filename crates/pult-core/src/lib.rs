//! The device-free heart of pult: levels, channel mapping, drift control and the mixer that
//! the audio callbacks drive.
//!
//! Nothing here touches a device, a file or the clock, so all of it runs in plain tests.
//! The realtime rule of the callbacks holds for every function they call: no allocation, no
//! lock, no logging. The control thread talks to them through atomics only.

pub mod drift;
pub mod level;
pub mod mixer;
pub mod remix;
pub mod select;
pub mod tone;

pub use drift::{DriftControl, Event, Plan};
pub use level::{AtomicF32, Counter, PeakMeter, db_to_linear, linear_to_db};
pub use mixer::{
    Capture, InputConfig, InputControls, InputHandle, InputStats, MasterStats, Mixer, MixerInput,
    connect,
};
pub use remix::remix_add;
pub use select::{SelectError, select};
pub use tone::Sine;
