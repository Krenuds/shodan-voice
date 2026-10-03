//! Metal: the stereo flanger that gives the talking-through-a-pipe colour.

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::dsp::flanger::{Flanger, FlangerSettings};
use std::sync::Arc;

crate::params::define_params! {
    FlangeDepth => ("flange_depth", "Depth", "Metal", 0.0, 1.0, 0.35, "", Continuous, "Sweep depth of the metallic flanger."),
    FlangeRate  => ("flange_rate", "Rate", "Metal", 0.02, 4.0, 0.25, "Hz", Continuous, "Sweep speed."),
    FlangeFb    => ("flange_fb", "Feedback", "Metal", -0.95, 0.95, -0.7, "", Continuous, "Resonance. Negative = hollow pipe, positive = ringing."),
    FlangeMix   => ("flange_mix", "Mix", "Metal", 0.0, 1.0, 0.5, "", Continuous, "Amount of flanger."),
}

pub const KIND: ModuleKind = ModuleKind {
    id: "metal",
    name: "Metal",
    defs: DEFS,
    make: |sr, _seed, shared, _app| Box::new(Metal { s: Smoothed::new(sr, &shared.params), shared: shared.clone(), flangers: [Flanger::new(sr, 0.0), Flanger::new(sr, 0.25)] }),
};

struct Metal {
    shared: Arc<ModuleShared>,
    s: Smoothed,
    flangers: [Flanger; 2],
}

impl Module for Metal {
    fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            let fs = FlangerSettings {
                depth: self.s.get(P::FlangeDepth),
                rate_hz: self.s.get(P::FlangeRate),
                feedback: self.s.get(P::FlangeFb),
                mix: self.s.get(P::FlangeMix),
            };
            for (ch, out) in [&mut *left, &mut *right].into_iter().enumerate() {
                for y in &mut out[s0..s1] {
                    *y = self.flangers[ch].process(*y, &fs);
                }
            }
            s0 = s1;
        }
    }
}
