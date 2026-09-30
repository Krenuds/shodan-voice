//! Rotary knob: drag up/down to turn (Shift = fine), scroll to nudge, double-click to reset.
//! Drawn like the K.O. II volume knob: a white cap with a dark notch, the value arc in orange.

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Response, Sense, Shape, Stroke, Ui, Vec2};
use std::f32::consts::PI;

const START: f32 = PI * 0.75;
const SWEEP: f32 = PI * 1.5;
pub const KNOB_WIDTH: f32 = 72.0;

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
        Self { value, min, max, default, label, format: Box::new(|v| format!("{v:.2}")), stepped: false, bipolar: min < 0.0 && max > 0.0, help: "", diameter: 44.0 }
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
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(KNOB_WIDTH, self.diameter + 38.0), Sense::click_and_drag());
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

        let painter = ui.painter_at(rect.expand(4.0));
        let r = self.diameter * 0.5;
        let center = Pos2::new(rect.center().x, rect.top() + 3.0 + r);
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

        // Value arc: a printed track, orange where the value is.
        let track_r = r - 1.5;
        painter.add(Shape::line(arc(START, START + SWEEP, track_r), Stroke::new(3.0, theme::KNOB_TRACK)));
        let zero = if self.bipolar { (0.0 - self.min) / range } else { 0.0 };
        let (a0, a1) = (START + SWEEP * zero, START + SWEEP * norm);
        let accent = if hot { theme::ACCENT_HOT } else { theme::ACCENT };
        if (a1 - a0).abs() > 0.01 {
            painter.add(Shape::line(arc(a0.min(a1), a0.max(a1), track_r), Stroke::new(3.0, accent)));
        }
        // End ticks printed on the chassis.
        for a in [START, START + SWEEP] {
            painter.line_segment([center + Vec2::angled(a) * (r + 1.5), center + Vec2::angled(a) * (r + 4.0)], Stroke::new(1.0, theme::INK_DIM));
        }

        // The cap: a white dome sitting in a shadow ring, with a dark pointer notch.
        let cap_r = r - 6.0;
        painter.circle_filled(center + Vec2::new(0.0, 1.5), cap_r + 1.5, Color32::from_black_alpha(45));
        painter.circle_filled(center, cap_r + 0.5, Color32::from_rgb(0xC4, 0xC4, 0xBF));
        painter.circle_filled(center, cap_r, theme::KNOB_BODY);
        painter.circle_filled(center - Vec2::new(0.0, cap_r * 0.18), cap_r * 0.72, Color32::from_white_alpha(if hot { 255 } else { 150 }));
        let dir = Vec2::angled(a1);
        painter.line_segment([center + dir * cap_r * 0.3, center + dir * (cap_r - 2.5)], Stroke::new(3.0, theme::INK));

        // Label printed underneath in small caps, shrunk to fit; the value in monospace ink.
        let label = self.label.to_uppercase();
        let mut size = 9.5;
        let mut galley = painter.layout_no_wrap(label.clone(), FontId::monospace(size), theme::INK_DIM);
        if galley.size().x > KNOB_WIDTH - 2.0 {
            size = (size * (KNOB_WIDTH - 2.0) / galley.size().x).max(7.0);
            galley = painter.layout_no_wrap(label, FontId::monospace(size), theme::INK_DIM);
        }
        let label_pos = Pos2::new(center.x - galley.size().x * 0.5, rect.bottom() - 28.0);
        painter.galley(label_pos, galley, theme::INK_DIM);
        let value_color = if hot { theme::ORANGE } else { theme::INK };
        let text = (self.format)(*self.value);
        painter.text(Pos2::new(center.x, rect.bottom() - 7.0), Align2::CENTER_CENTER, text, FontId::monospace(11.0), value_color);

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

/// Horizontal level meter: a printed label and a pixel meter, -60..0 dBFS.
#[allow(dead_code)] // kept for panels that want a labelled meter
pub fn meter(ui: &mut Ui, label: &str, level: f32, width: f32) {
    ui.horizontal(|ui| {
        ui.add_sized([28.0, 14.0], egui::Label::new(theme::silk(label)));
        super::widgets::pixel_meter(ui, level, Vec2::new(width, 12.0), false);
    });
}
