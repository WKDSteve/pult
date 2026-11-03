//! The window of pult: a mixing console on egui, built on `pult_core::view`.
//!
//! `theme` and the controls in `widgets` (all but `strip`) know nothing about pult's mixer and
//! can move to a crate shared with takt; `strip` and `console` lay them out against the view.

#![forbid(unsafe_code)]

pub mod console;
pub mod log;
pub mod matrix;
pub mod start;
pub mod theme;
pub mod widgets;
