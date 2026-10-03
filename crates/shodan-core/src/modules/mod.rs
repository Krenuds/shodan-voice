//! Native rack modules. To add one: write a file with a `KIND`, then list it in [`NATIVE`].

pub mod shodan;
pub mod lofi;
pub mod metal;
pub mod titobot;

use crate::audio::module::{Module, ModuleShared, RackModule};
use crate::params::ParamDef;
use crate::shared::Shared;
use std::sync::Arc;

pub struct ModuleKind {
    /// Stable id stored in the settings file.
    pub id: &'static str,
    pub name: &'static str,
    pub defs: &'static [ParamDef],
    /// Build the audio-thread half for one instance. Called on the GUI thread, so it may allocate.
    pub make: fn(sr: f32, seed: u64, shared: &Arc<ModuleShared>, app: &Arc<Shared>) -> Box<dyn Module>,
}

pub static NATIVE: [ModuleKind; 4] = [shodan::KIND, metal::KIND, lofi::KIND, titobot::KIND];

pub fn kind(id: &str) -> Option<&'static ModuleKind> {
    NATIVE.iter().find(|k| k.id == id)
}

/// A native module instance before it has an audio-thread half.
pub struct Instance {
    pub kind: &'static ModuleKind,
    pub shared: Arc<ModuleShared>,
}

impl Instance {
    pub fn new(kind: &'static ModuleKind) -> Self {
        Self { kind, shared: ModuleShared::new(kind.defs) }
    }
}

/// The chain that makes up the classic SHODAN voice: the first three kinds.
pub fn default_instances() -> Vec<Instance> {
    NATIVE[..3].iter().map(Instance::new).collect()
}

/// Build the audio-thread halves of `instances`, in order, for offline use.
pub fn build(instances: &[Instance], sr: f32, seed: u64, app: &Arc<Shared>) -> Vec<RackModule> {
    instances
        .iter()
        .enumerate()
        .map(|(i, inst)| RackModule { id: i as u32 + 1, module: (inst.kind.make)(sr, seed, &inst.shared, app), shared: inst.shared.clone() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_across_all_tables() {
        let mut keys: Vec<&str> = NATIVE.iter().flat_map(|k| k.defs).chain(crate::params::io::DEFS).map(|d| d.key).collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), total, "two parameters share a key");
    }
}
