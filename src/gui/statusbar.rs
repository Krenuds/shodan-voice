//! The status bar along the bottom, printed on the chassis: in/out meters, glitch LED, pitch and
//! tape lag, the audio stream's state and its latest error.

use super::state::Levels;
use super::theme;
use super::widgets;
use crate::audio::io::Running;
use crate::shared::Shared;
use eframe::egui::{self, FontId, RichText, Sense, Stroke, vec2};
use std::sync::atomic::Ordering;

pub struct StatusCtx<'a> {
    pub shared: &'a Shared,
    pub running: Option<&'a Running>,
    pub audio_error: Option<&'a str>,
    /// Latest last.
    pub stream_errors: &'a [String],
    pub levels: &'a Levels,
    /// The rack has a SHODAN Core (glitches, pitch and tape lag only mean something then).
    pub has_core: bool,
}

/// A small monospace readout printed on the chassis.
fn readout(text: String) -> RichText {
    RichText::new(text).font(FontId::monospace(11.0)).color(theme::INK)
}

/// Small caps text in a colour, for the device line and warnings.
fn printed(text: &str, color: egui::Color32) -> RichText {
    RichText::new(text.to_uppercase()).font(FontId::monospace(10.0)).color(color)
}

pub fn status_bar(ui: &mut egui::Ui, s: StatusCtx) {
    let m = &s.shared.meters;
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::GRID;
        ui.set_min_height(20.0);

        theme::silkscreen(ui, "in");
        widgets::pixel_meter(ui, s.levels.input, vec2(112.0, 12.0), false).on_hover_text("Microphone level after input gain");
        ui.add_space(theme::GRID);
        theme::silkscreen(ui, "out");
        widgets::pixel_meter(ui, s.levels.output, vec2(112.0, 12.0), false).on_hover_text("Output level");

        // Glitches, pitch and tape lag are SHODAN Core's; without one they mean nothing.
        if s.has_core {
            ui.add_space(theme::GRID);
            let (r, _) = ui.allocate_exact_size(vec2(theme::GRID, 12.0), Sense::hover());
            widgets::paint_led(ui.painter(), r.center(), 3.0, s.levels.glitch_flash(), theme::ORANGE);
            theme::silkscreen(ui, "glitch");
            ui.label(readout(format!("{:>4}", s.levels.glitches)));
            ui.add_space(theme::GRID);
            theme::silkscreen(ui, "pitch");
            ui.label(readout(format!("{:+5.1} st", m.pitch.get())));
            ui.add_space(theme::GRID);
            theme::silkscreen(ui, "tape lag");
            ui.label(readout(format!("{:>3.0} ms", m.lag_ms.get())));
        }

        ui.add_space(theme::GRID);
        let (r, _) = ui.allocate_exact_size(vec2(1.0, 16.0), Sense::hover());
        ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, theme::RULE));
        ui.add_space(theme::GRID);

        // Trouble first, so it is never pushed off the edge by the device name.
        if let Some(e) = s.audio_error {
            ui.add(egui::Label::new(printed(&format!("audio error: {e}"), theme::DANGER)).truncate()).on_hover_text(e);
            return;
        }
        let u = m.underruns.load(Ordering::Relaxed);
        if s.running.is_some() && u > 0 {
            ui.label(printed(&format!("{u} dropouts"), theme::WARN));
        }
        // The device line takes what it needs, up to 40 % of the bar; errors get the rest.
        let device_w = match s.running {
            Some(a) => {
                let g = ui.painter().layout_no_wrap(a.description.to_uppercase(), FontId::monospace(10.0), theme::INK_DIM);
                g.size().x.min(0.4 * width) + theme::GRID
            }
            None => 0.0,
        };
        if let Some(e) = s.stream_errors.last() {
            let w = (ui.available_width() - device_w).max(80.0);
            ui.scope(|ui| {
                ui.set_max_width(w);
                ui.add(egui::Label::new(printed(e, theme::DANGER)).truncate()).on_hover_text(e);
            });
        }
        match s.running {
            Some(a) => {
                let w = (0.4 * width).min(ui.available_width());
                if w > 40.0 {
                    ui.scope(|ui| {
                        ui.set_max_width(w);
                        ui.add(egui::Label::new(printed(&a.description, theme::INK_DIM)).truncate()).on_hover_text(&a.description);
                    });
                }
            }
            None => {
                ui.label(printed("audio stopped", theme::INK_DIM));
            }
        }
    });
}
