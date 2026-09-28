//! Rotary knob: drag up/down to turn (Shift = fine), scroll to nudge, double-click to reset.

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Response, Sense, Shape, Stroke, Ui, Vec2};
use std::f32::consts::PI;

const START: f32 = PI * 0.75;
const SWEEP: f32 = PI * 1.5;
pub const KNOB_WIDTH: f32 = 68.0;

pub struct Knob<'a> {
    value: &'a mut f32,
    min: f32,
    max: f32,
    default: f32,
    label: &'a str,
    format: Box<dyn FnMut(f32) -> String + 'a>,
    stepped: bool,
    bipolar: bool,
    help: &'a str,
    diameter: f32,
}

impl<'a> Knob<'a> {
    pub fn new(value: &'a mut f32, min: f32, max: f32, default: f32, label: &'a str) -> Self {
        Self { value, min, max, default, label, format: Box::new(|v| format!("{v:.2}")), stepped: false, bipolar: min < 0.0 && max > 0.0, help: "", diameter: 46.0 }
    }

    /// How the value is displayed under the knob.
    pub fn format(mut self, f: impl FnMut(f32) -> String + 'a) -> Self {
        self.format = Box::new(f);
        self
    }

    pub fn stepped(mut self, stepped: bool) -> Self {
        self.stepped = stepped;
        self
    }

    pub fn help(mut self, help: &'a str) -> Self {
        self.help = help;
        self
    }

    pub fn show(mut self, ui: &mut Ui) -> Response {
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(KNOB_WIDTH, self.diameter + 34.0), Sense::click_and_drag());
        let range = self.max - self.min;
        let old = *self.value;

        if response.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            let dy = -response.drag_delta().y;
            let speed = if fine { 1200.0 } else { 180.0 };
            *self.value += dy / speed * range;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let step = if self.stepped { scroll.signum() } else { scroll / 2000.0 * range };
                *self.value += step;
            }
        }
        if response.double_clicked() {
            *self.value = self.default;
        }
        *self.value = self.value.clamp(self.min, self.max);
        if self.stepped && !response.dragged() {
            *self.value = self.value.round();
        }
        if *self.value != old {
            response.mark_changed();
        }

        let painter = ui.painter_at(rect.expand(2.0));
        let r = self.diameter * 0.5;
        let center = Pos2::new(rect.center().x, rect.top() + 2.0 + r);
        let norm = if range > 0.0 { (*self.value - self.min) / range } else { 0.0 };
        let hot = response.hovered() || response.dragged();

        let arc = |from: f32, to: f32, radius: f32| -> Vec<Pos2> {
            let steps = ((to - from).abs() / 0.08).ceil().max(2.0) as usize;
            (0..=steps)
                .map(|k| {
                    let a = from + (to - from) * k as f32 / steps as f32;
                    center + Vec2::angled(a) * radius
                })
                .collect()
        };

        painter.circle_filled(center, r - 5.0, theme::KNOB_BODY);
        painter.add(Shape::line(arc(START, START + SWEEP, r - 1.5), Stroke::new(3.0, theme::KNOB_TRACK)));
        let zero = if self.bipolar { (0.0 - self.min) / range } else { 0.0 };
        let (a0, a1) = (START + SWEEP * zero, START + SWEEP * norm);
        let accent = if hot { theme::ACCENT_HOT } else { theme::ACCENT };
        if (a1 - a0).abs() > 0.01 {
            painter.add(Shape::line(arc(a0.min(a1), a0.max(a1), r - 1.5), Stroke::new(3.0, accent)));
        }
        let tip = center + Vec2::angled(a1) * (r - 8.0);
        painter.line_segment([center + Vec2::angled(a1) * 4.0, tip], Stroke::new(2.5, accent));

        painter.text(Pos2::new(center.x, rect.bottom() - 20.0), Align2::CENTER_CENTER, self.label, FontId::proportional(12.0), theme::TEXT);
        let value_color = if hot { theme::ACCENT_HOT } else { theme::TEXT_DIM };
        let text = (self.format)(*self.value);
        painter.text(Pos2::new(center.x, rect.bottom() - 6.0), Align2::CENTER_CENTER, text, FontId::monospace(11.0), value_color);

        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if self.help.is_empty() {
            response
        } else {
            response.on_hover_text(format!("{}\n\nDrag to turn · Shift = fine · double-click = reset", self.help))
        }
    }
}

/// Horizontal level meter with a peak colour change near clipping.
pub fn meter(ui: &mut Ui, label: &str, level: f32, width: f32) {
    ui.horizontal(|ui| {
        ui.add_sized([28.0, 14.0], egui::Label::new(egui::RichText::new(label).small().color(theme::TEXT_DIM)));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 10.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, theme::KNOB_TRACK);
        // -60..0 dBFS
        let db = 20.0 * level.max(1e-6).log10();
        let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let mut fill = rect;
        fill.set_width(rect.width() * frac);
        let color = if db > -1.0 { Color32::from_rgb(255, 80, 60) } else if db > -9.0 { Color32::from_rgb(230, 200, 60) } else { theme::ACCENT };
        p.rect_filled(fill, 2.0, color);
    });
}
