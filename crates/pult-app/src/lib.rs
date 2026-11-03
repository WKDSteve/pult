//! The window of pult: a mixing console on egui, built on `pult_core::view`.
//!
//! `theme` and the controls in `widgets` (all but `strip`) know nothing about pult's mixer and
//! can move to a crate shared with takt. `layout` lists the elements of a console, each bound
//! to a parameter address (`binding`); `strip` and `console` draw them against the view.

#![forbid(unsafe_code)]

pub mod binding;
pub mod console;
pub mod layout;
pub mod log;
pub mod matrix;
pub mod start;
pub mod theme;
pub mod widgets;
