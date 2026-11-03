//! The window of pult (stage 3b): a mixing console on screen, built on `pult_core::view`.
//!
//! Until the window exists, this prints the demo mixer once, so the crate builds and runs.

#![forbid(unsafe_code)]

use pult_core::{Desk, FakeDesk};

fn main() {
    let view = FakeDesk::new().view();
    for input in &view.inputs {
        println!("{}  {:.1} dB", input.name, input.gain_db);
    }
    for bus in &view.buses {
        println!("{} -> {}", bus.name, bus.device);
    }
}
