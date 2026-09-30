//! The rack module interface: anything that can sit in the chain between the mic and the output.

use crate::dsp::filters::one_pole_coef;
use crate::params::{Kind, ParamDef, Params};
use crate::shared::AtomicF32;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Identifies a module instance for as long as it is in the rack. Never 0.
pub type ModuleId = u32;

/// Native modules update their smoothed parameters once per this many samples.
pub const SUB_BLOCK: usize = 32;
/// Modules are never asked to process more than this many frames at once.
pub const MAX_BLOCK: usize = 1024;

pub struct ModuleCtx<'a> {
    /// The mic signal after input gain, before any module. Same length as the block.
    #[allow(dead_code)] // for modules that key off or replace the voice
    pub dry: &'a [f32],
}

/// A stereo processor in the rack. `process` runs on the audio thread: no allocation, no locks.
pub trait Module: Send {
    /// Replace `left`/`right` (the previous module's output) with this module's output.
    fn process(&mut self, ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]);
    /// Delay this module adds, for the latency readout.
    fn latency_ms(&self) -> f32 {
        0.0
    }
    /// Called on the audio thread just before the module is handed back for disposal.
    fn stop(&mut self) {}
}

/// Per-instance state shared between the GUI and the module on the audio thread.
pub struct ModuleShared {
    pub params: Params,
    pub enabled: AtomicBool,
    /// Blend of the module's output with its input.
    pub mix: AtomicF32,
}

impl ModuleShared {
    pub fn new(defs: &'static [ParamDef]) -> Arc<Self> {
        Arc::new(Self { params: Params::new(defs), enabled: AtomicBool::new(true), mix: AtomicF32::new(1.0) })
    }
}

/// A module as it travels between the GUI thread and the engine.
pub struct RackModule {
    pub id: ModuleId,
    pub module: Box<dyn Module>,
    pub shared: Arc<ModuleShared>,
}

/// One-pole smoothing of continuous parameters, stepped once per sub-block.
pub struct Smoothed {
    values: Vec<f32>,
    coef: f32,
}

impl Smoothed {
    pub fn new(sr: f32, params: &Params) -> Self {
        Self { values: (0..params.defs().len()).map(|i| params.get_index(i)).collect(), coef: one_pole_coef(sr / SUB_BLOCK as f32, 25.0) }
    }

    pub fn update(&mut self, params: &Params) {
        for (i, d) in params.defs().iter().enumerate() {
            let target = params.get_index(i);
            self.values[i] = if d.kind == Kind::Continuous { target + self.coef * (self.values[i] - target) } else { target };
        }
    }

    pub fn get(&self, p: impl Into<usize>) -> f32 {
        self.values[p.into()]
    }
}
