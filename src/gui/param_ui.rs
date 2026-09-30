//! Knobs generated from a parameter table.

use super::knob::Knob;
use crate::params::{Kind, ParamDef, Params};
use eframe::egui;

fn shown(d: &ParamDef, group: Option<&str>) -> bool {
    d.kind != Kind::Toggle && group.is_none_or(|g| d.group == g)
}

/// How many knobs [`param_knobs`] will draw.
#[allow(dead_code)] // kept for panels that size themselves by knob count
pub fn knob_count(params: &Params, group: Option<&str>) -> usize {
    params.defs().iter().filter(|d| shown(d, group)).count()
}

/// Which knob of a [`param_knobs`] call the pointer is on, and which one was turned, as
/// indices into the table. For help lines that follow the pointer.
#[derive(Clone, Copy, Default, Debug)]
pub struct KnobHits {
    pub hovered: Option<usize>,
    pub touched: Option<usize>,
}

/// One knob per parameter (of `group`, or all of them), laid out by the enclosing `ui`.
pub fn param_knobs(ui: &mut egui::Ui, params: &Params, group: Option<&str>) -> KnobHits {
    let mut hits = KnobHits::default();
    for (i, d) in params.defs().iter().enumerate() {
        if !shown(d, group) {
            continue;
        }
        let mut v = params.get_index(i);
        let resp = Knob::new(&mut v, d.min, d.max, d.default, d.label).format(|v| format_value(d, v)).stepped(d.kind == Kind::Stepped).help(d.help).show(ui);
        if resp.changed() {
            params.set_index(i, v);
        }
        if resp.hovered() || resp.dragged() {
            hits.hovered = Some(i);
        }
        if resp.changed() || resp.dragged() {
            hits.touched = Some(i);
        }
    }
    hits
}

/// Put every parameter of `group` back to its default.
pub fn reset_group(params: &Params, group: &str) {
    for (i, d) in params.defs().iter().enumerate() {
        if shown(d, Some(group)) {
            params.set_index(i, d.default);
        }
    }
}

/// Whether any parameter of `group` is away from its default.
pub fn group_changed(params: &Params, group: &str) -> bool {
    params.defs().iter().enumerate().any(|(i, d)| shown(d, Some(group)) && (params.get_index(i) - d.default).abs() > 1e-6)
}

/// The groups of a table that have knobs, in table order.
pub fn groups(params: &Params) -> Vec<&'static str> {
    let mut groups: Vec<&'static str> = Vec::new();
    for d in params.defs().iter().filter(|d| shown(d, None)) {
        if !groups.contains(&d.group) {
            groups.push(d.group);
        }
    }
    groups
}

fn note_name(midi: f32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let n = midi.round() as i32;
    format!("{}{} {:.0}Hz", NAMES[n.rem_euclid(12) as usize], n.div_euclid(12) - 1, crate::dsp::lpc::midi_to_hz(n as f32))
}

pub fn format_value(d: &ParamDef, v: f32) -> String {
    match d.unit {
        "dB" => format!("{v:+.1} dB"),
        "st" => format!("{v:+.1} st"),
        "ms" => format!("{v:.0} ms"),
        "ct" => format!("{v:.0} ct"),
        "bit" => format!("{v:.1} bit"),
        "x" => format!("{v:.0}x"),
        "note" => note_name(v),
        "Hz" if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
        "Hz" => format!("{v:.2} Hz"),
        _ if d.kind == Kind::Stepped => format!("{v:.0}"),
        _ if d.min < 0.0 => format!("{:+.0}%", v * 100.0),
        _ => format!("{:.0}%", v * 100.0),
    }
}
