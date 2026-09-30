//! Knobs generated from a parameter table.

use super::knob::Knob;
use crate::params::{Kind, ParamDef, Params};
use eframe::egui;

fn shown(d: &ParamDef, group: Option<&str>) -> bool {
    d.kind != Kind::Toggle && group.is_none_or(|g| d.group == g)
}

/// How many knobs [`param_knobs`] will draw.
pub fn knob_count(params: &Params, group: Option<&str>) -> usize {
    params.defs().iter().filter(|d| shown(d, group)).count()
}

/// One knob per parameter (of `group`, or all of them), laid out by the enclosing `ui`.
pub fn param_knobs(ui: &mut egui::Ui, params: &Params, group: Option<&str>) {
    for (i, d) in params.defs().iter().enumerate() {
        if !shown(d, group) {
            continue;
        }
        let mut v = params.get_index(i);
        let resp = Knob::new(&mut v, d.min, d.max, d.default, d.label).format(|v| format_value(d, v)).stepped(d.kind == Kind::Stepped).help(d.help).show(ui);
        if resp.changed() {
            params.set_index(i, v);
        }
    }
}

fn note_name(midi: f32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let n = midi.round() as i32;
    format!("{}{} {:.0}Hz", NAMES[n.rem_euclid(12) as usize], n.div_euclid(12) - 1, crate::dsp::lpc::midi_to_hz(n as f32))
}

fn format_value(d: &ParamDef, v: f32) -> String {
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
