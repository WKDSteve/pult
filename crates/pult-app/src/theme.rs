//! The look of pult (and later takt): colors, type, spacing, as named tokens.
//!
//! Widgets never use a raw color or size; they take them from here, so the module can move to a
//! crate shared with takt unchanged.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals,
};

/// Colors by role. A dark, quiet base with few accents: one for focus and selection, one each
/// for mute and solo, and three meter zones.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Window background, behind the strips.
    pub base: Color32,
    /// Background of a strip or panel.
    pub surface: Color32,
    /// Background of the bus section, set off from the input strips.
    pub surface_alt: Color32,
    /// A control at rest (button off, knob body, fader cap).
    pub raised: Color32,
    /// A control under the pointer.
    pub raised_hover: Color32,
    /// Recessed areas: fader slot, meter well, readouts.
    pub well: Color32,
    /// Hairlines between areas.
    pub line: Color32,
    /// Outlines of controls, scale ticks.
    pub line_strong: Color32,
    /// Main text and values.
    pub text: Color32,
    /// Secondary text: device, rate, units.
    pub text_dim: Color32,
    /// Scale numbers, disabled text.
    pub text_faint: Color32,
    /// Focus, selection, active bus assignment, pan arc.
    pub accent: Color32,
    /// Mute on; also the state of a missing device.
    pub mute: Color32,
    /// Solo on.
    pub solo: Color32,
    /// Meter below -18 dBFS.
    pub meter_low: Color32,
    /// Meter from -18 to -6 dBFS.
    pub meter_mid: Color32,
    /// Meter above -6 dBFS and the clip lamp.
    pub meter_high: Color32,
    /// Good state in the header (running, no dropouts).
    pub ok: Color32,
}

/// Spacing and sizes in points (egui scales them for high-DPI screens).
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// Width of an input strip.
    pub strip_width: f32,
    /// Width of a bus (master) strip.
    pub bus_strip_width: f32,
    /// Gap between strips.
    pub strip_gap: f32,
    /// Inner padding of a strip.
    pub strip_pad: f32,
    /// Corner radius of strips and panels.
    pub radius: u8,
    /// Corner radius of buttons and readouts.
    pub radius_small: u8,
    /// Height of a toggle button (mute, solo, bus).
    pub button_height: f32,
    /// Diameter of the pan knob.
    pub knob_size: f32,
    /// Fader cap width and height.
    pub cap_size: [f32; 2],
    /// Width of one meter bar.
    pub meter_width: f32,
    /// Height of the header bar.
    pub header_height: f32,
}

/// The one theme: dark.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub palette: Palette,
    pub metrics: Metrics,
}

pub const DARK: Theme = Theme {
    palette: Palette {
        base: Color32::from_rgb(0x0E, 0x10, 0x13),
        surface: Color32::from_rgb(0x17, 0x1A, 0x1F),
        surface_alt: Color32::from_rgb(0x1B, 0x1F, 0x25),
        raised: Color32::from_rgb(0x25, 0x2A, 0x32),
        raised_hover: Color32::from_rgb(0x2E, 0x34, 0x3D),
        well: Color32::from_rgb(0x0A, 0x0C, 0x0F),
        line: Color32::from_rgb(0x24, 0x29, 0x30),
        line_strong: Color32::from_rgb(0x3A, 0x41, 0x4B),
        text: Color32::from_rgb(0xE3, 0xE6, 0xEB),
        text_dim: Color32::from_rgb(0x95, 0x9D, 0xA8),
        text_faint: Color32::from_rgb(0x5D, 0x64, 0x6E),
        accent: Color32::from_rgb(0x4F, 0xB4, 0xD8),
        mute: Color32::from_rgb(0xE0, 0x55, 0x4B),
        solo: Color32::from_rgb(0xE8, 0xB4, 0x3A),
        meter_low: Color32::from_rgb(0x48, 0xC2, 0x6E),
        meter_mid: Color32::from_rgb(0xD8, 0xC3, 0x48),
        meter_high: Color32::from_rgb(0xE0, 0x55, 0x4B),
        ok: Color32::from_rgb(0x48, 0xC2, 0x6E),
    },
    metrics: Metrics {
        strip_width: 88.0,
        bus_strip_width: 100.0,
        strip_gap: 6.0,
        strip_pad: 8.0,
        radius: 6,
        radius_small: 3,
        button_height: 20.0,
        knob_size: 34.0,
        cap_size: [34.0, 16.0],
        meter_width: 7.0,
        header_height: 42.0,
    },
};

impl Theme {
    /// The theme in effect; one place to switch once there is more than one.
    #[must_use]
    pub fn current() -> &'static Self {
        &DARK
    }

    /// Font for values that change (gain, peak, counters): monospace, so digits do not jump.
    #[must_use]
    pub fn value_font(size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }

    /// Font for labels.
    #[must_use]
    pub fn label_font(size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }

    /// Installs the theme into egui: visuals of the built-in widgets and the text styles.
    pub fn apply(&self, ctx: &egui::Context) {
        let p = &self.palette;
        let mut visuals = Visuals::dark();
        visuals.panel_fill = p.base;
        visuals.window_fill = p.surface;
        visuals.extreme_bg_color = p.well;
        visuals.faint_bg_color = p.surface;
        visuals.override_text_color = Some(p.text);
        visuals.hyperlink_color = p.accent;
        visuals.selection.bg_fill = p.accent.gamma_multiply(0.35);
        visuals.selection.stroke = Stroke::new(1.0, p.accent);
        visuals.window_stroke = Stroke::new(1.0, p.line);
        visuals.window_corner_radius = CornerRadius::same(self.metrics.radius);
        let small = CornerRadius::same(self.metrics.radius_small);
        for (state, fill) in [
            (&mut visuals.widgets.noninteractive, p.surface),
            (&mut visuals.widgets.inactive, p.raised),
            (&mut visuals.widgets.hovered, p.raised_hover),
            (&mut visuals.widgets.active, p.raised_hover),
            (&mut visuals.widgets.open, p.raised),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = small;
            state.bg_stroke = Stroke::new(1.0, p.line);
            state.fg_stroke.color = p.text;
        }
        visuals.widgets.noninteractive.fg_stroke.color = p.text_dim;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.line_strong);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        ctx.set_visuals_of(egui::Theme::Dark, visuals);
        ctx.set_theme(egui::Theme::Dark);

        ctx.all_styles_mut(|style| {
            style.text_styles = [
                (TextStyle::Small, Self::label_font(10.0)),
                (TextStyle::Body, Self::label_font(12.5)),
                (TextStyle::Button, Self::label_font(12.0)),
                (TextStyle::Heading, Self::label_font(16.0)),
                (TextStyle::Monospace, Self::value_font(11.5)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(6.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 3.0);
            style.spacing.window_margin = Margin::same(10);
        });
    }
}
