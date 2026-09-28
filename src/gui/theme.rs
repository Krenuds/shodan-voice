//! Citadel-terminal colours.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(10, 14, 12);
pub const PANEL: Color32 = Color32::from_rgb(17, 24, 21);
pub const CARD: Color32 = Color32::from_rgb(22, 31, 27);
pub const KNOB_BODY: Color32 = Color32::from_rgb(30, 42, 37);
pub const KNOB_TRACK: Color32 = Color32::from_rgb(44, 58, 52);
pub const ACCENT: Color32 = Color32::from_rgb(64, 214, 128);
pub const ACCENT_HOT: Color32 = Color32::from_rgb(150, 255, 190);
pub const TEXT: Color32 = Color32::from_rgb(200, 222, 210);
pub const TEXT_DIM: Color32 = Color32::from_rgb(120, 150, 135);
pub const WARN: Color32 = Color32::from_rgb(240, 180, 70);
pub const DANGER: Color32 = Color32::from_rgb(235, 80, 90);

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT_HOT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);
    let radius = CornerRadius::same(4);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
        w.corner_radius = radius;
    }
    v.widgets.inactive.weak_bg_fill = KNOB_BODY;
    v.widgets.inactive.bg_fill = KNOB_BODY;
    v.widgets.hovered.weak_bg_fill = KNOB_TRACK;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.35);
    ctx.set_visuals(v);
}

pub fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title.to_uppercase()).monospace().size(12.0).color(ACCENT));
            ui.add_space(2.0);
            add(ui);
        });
    });
}
