//! Names of devices and programs in CJK or with an emoji: the fonts of egui have neither, so the window falls back to
//! the fonts of the system (the eframe feature `system_fonts`). Without a font for a character `egui_kittest` panics, so a
//! square in the window would be a red test here.

#![cfg(windows)]

use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{FallbackRequest, FontFamily, FontProvider};
use egui_kittest::Harness;
use egui_system_font_provider::SystemFontProvider;

const NAMES: &str = "音频设备 スピーカー 🎧";

#[test]
fn a_name_in_cjk_and_with_an_emoji_finds_a_font_of_the_system() {
    let mut added = false;
    let mut harness = Harness::builder().build_ui(move |ui| {
        if !added {
            ui.ctx()
                .add_font_provider(Arc::new(SystemFontProvider::new()));
            added = true;
        }
        ui.label(NAMES);
    });
    harness.run();
}

#[test]
fn the_window_keeps_the_fallback_switched_on() {
    assert!(eframe::NativeOptions::default().system_font_fallback);
    let main = include_str!("../src/main.rs");
    assert!(!main.contains("system_font_fallback"), "main.rs sets it");
}

/// The cost, with `cargo test -p pult-app --test system_fonts -- --nocapture --ignored`: nothing at the start of a window
/// whose names the fonts of egui can draw, and this for the first name they cannot.
#[test]
#[ignore = "a measurement, not a check"]
fn what_the_first_name_costs() {
    let start = Instant::now();
    let provider = SystemFontProvider::new();
    println!("provider: {:?}", start.elapsed());
    for (what, cluster) in [("CJK", "音"), ("emoji", "🎧")] {
        let start = Instant::now();
        let request = FallbackRequest {
            cluster,
            family: &FontFamily::Proportional,
        };
        let insert = provider.font_for(&request).expect("a font");
        println!(
            "{what}: {} found in {:?}, {} MB held",
            insert.name,
            start.elapsed(),
            (*insert.data.font).as_ref().len() / 1_000_000
        );
    }
}
