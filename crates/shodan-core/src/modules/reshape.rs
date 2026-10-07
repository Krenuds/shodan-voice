//! Reshape: re-pitch the whole utterance before the rack, keeping the vocal tract (WORLD or
//! PSOLA). Offline only: `render` runs it over the full input first. Live it passes through.

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx};
use crate::dsp::{psola, world};
use crate::params::Params;

crate::params::define_params! {
    SrcEngine  => ("src_engine", "Engine", "Reshape", 0.0, 1.0, 0.0, "", Stepped, "0 = WORLD vocoder, 1 = PSOLA grains."),
    SrcF0      => ("src_f0", "Pitch", "Reshape", 40.0, 300.0, 98.0, "Hz", Continuous, "Where the median pitch lands."),
    SrcFlatten => ("src_flatten", "Flatten", "Reshape", 0.0, 1.0, 0.85, "", Continuous, "Pull toward a monotone. 1 = dead flat."),
    SrcFormant => ("src_formant", "Formant", "Reshape", -12.0, 12.0, -2.0, "st", Continuous, "Vocal tract size. Below -6 vowels start to blur."),
    SrcTracker => ("src_tracker", "Tracker", "Reshape", 0.0, 1.0, 0.0, "", Stepped, "Pitch tracker: 0 = DIO (fast), 1 = Harvest (slow, steadier)."),
    SrcGrit    => ("src_grit", "Grit", "Reshape", 0.0, 1.0, 0.0, "", Continuous, "Aperiodic excitation above 1 kHz (WORLD only)."),
}

pub const KIND: ModuleKind = ModuleKind { id: "reshape", name: "Reshape", defs: DEFS, make: |_sr, _seed, _shared, _app| Box::new(Passthrough) };

struct Passthrough;

impl Module for Passthrough {
    fn process(&mut self, _ctx: &ModuleCtx, _left: &mut [f32], _right: &mut [f32]) {}
}

/// Reshape a whole mono utterance with the knobs in `params`.
pub fn offline(params: &Params, sr: f32, x: &[f32]) -> Vec<f32> {
    let r = world::Reshape {
        f0: params.get(P::SrcF0),
        flatten: params.get(P::SrcFlatten),
        formant: 2f32.powf(params.get(P::SrcFormant) / 12.0),
        grit: params.get(P::SrcGrit),
        harvest: params.get(P::SrcTracker) >= 0.5,
    };
    if params.get(P::SrcEngine) >= 0.5 { psola::psola(x, sr, &r) } else { world::reshape(x, sr, &r) }
}
