//! Lo-fi: sample-rate and bit reduction plus the narrowing "era" filter.

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::dsp::lofi::{Lofi, LofiSettings};
use std::sync::Arc;

crate::params::define_params! {
    LofiRate => ("lofi_rate", "Rate", "Lo-fi", 4000.0, 48000.0, 22050.0, "Hz", Continuous, "Sample rate reduction (1994 grit)."),
    LofiBits => ("lofi_bits", "Bits", "Lo-fi", 4.0, 16.0, 12.0, "bit", Continuous, "Bit depth reduction."),
    LofiTone => ("lofi_tone", "Tone", "Lo-fi", 0.0, 1.0, 0.3, "", Continuous, "0 = full range, 1 = narrow intercom band."),
}

pub const KIND: ModuleKind = ModuleKind {
    id: "lofi",
    name: "Lo-fi",
    defs: DEFS,
    make: |sr, _seed, shared, _app| Box::new(LofiModule { s: Smoothed::new(sr, &shared.params), shared: shared.clone(), lofis: [Lofi::new(sr), Lofi::new(sr)] }),
};

struct LofiModule {
    shared: Arc<ModuleShared>,
    s: Smoothed,
    lofis: [Lofi; 2],
}

impl Module for LofiModule {
    fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            let ls = LofiSettings { rate_hz: self.s.get(P::LofiRate), bits: self.s.get(P::LofiBits), tone: self.s.get(P::LofiTone) };
            for (ch, out) in [&mut *left, &mut *right].into_iter().enumerate() {
                self.lofis[ch].update(&ls);
                for y in &mut out[s0..s1] {
                    *y = self.lofis[ch].process(*y, &ls);
                }
            }
            s0 = s1;
        }
    }
}
