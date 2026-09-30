//! Knob definitions and the lock-free parameter store shared by the GUI and audio thread.
//!
//! Every rack module kind declares its own table with [`define_params!`]; each module instance
//! then owns one [`Params`] over that table. Keys are unique across all tables.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Continuous,
    Stepped,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDef {
    /// Stable key used in preset files.
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

/// Generates a `P` enum and the matching `DEFS` table in the invoking module.
macro_rules! define_params {
    ($( $id:ident => ($key:literal, $label:literal, $group:literal, $min:expr, $max:expr, $def:expr, $unit:literal, $kind:ident, $help:literal) ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum P { $($id),* }

        impl From<P> for usize {
            fn from(p: P) -> usize {
                p as usize
            }
        }

        pub const DEFS: &[$crate::params::ParamDef] = &[
            $($crate::params::ParamDef { key: $key, label: $label, group: $group, min: $min, max: $max, default: $def, unit: $unit, kind: $crate::params::Kind::$kind, help: $help }),*
        ];
    };
}
pub(crate) use define_params;

/// Parameters of the fixed input/output stage around the rack.
pub mod io {
    super::define_params! {
        InGain  => ("in_gain", "Gain", "Input", -24.0, 24.0, 0.0, "dB", Continuous, "Mic input gain."),
        OutGain => ("out_gain", "Gain", "Output", -24.0, 12.0, 0.0, "dB", Continuous, "Output level (a limiter follows)."),
        Bypass  => ("bypass", "Bypass", "Output", 0.0, 1.0, 0.0, "", Toggle, "Pass your clean voice through."),
    }
}

/// Parameter values as f32 bits in atomics: the GUI writes, the audio thread reads.
pub struct Params {
    defs: &'static [ParamDef],
    values: Box<[AtomicU32]>,
}

impl Params {
    pub fn new(defs: &'static [ParamDef]) -> Self {
        Self { defs, values: defs.iter().map(|d| AtomicU32::new(d.default.to_bits())).collect() }
    }

    pub fn defs(&self) -> &'static [ParamDef] {
        self.defs
    }

    pub fn get(&self, p: impl Into<usize>) -> f32 {
        self.get_index(p.into())
    }

    pub fn get_index(&self, i: usize) -> f32 {
        f32::from_bits(self.values[i].load(Ordering::Relaxed))
    }

    pub fn set(&self, p: impl Into<usize>, v: f32) {
        self.set_index(p.into(), v);
    }

    pub fn set_index(&self, i: usize, v: f32) {
        let d = &self.defs[i];
        let v = v.clamp(d.min, d.max);
        let v = if d.kind == Kind::Continuous { v } else { v.round() };
        self.values[i].store(v.to_bits(), Ordering::Relaxed);
    }

    /// Set by preset key. Returns false if this table has no such key.
    pub fn set_key(&self, key: &str, v: f32) -> bool {
        match self.defs.iter().position(|d| d.key == key) {
            Some(i) => {
                self.set_index(i, v);
                true
            }
            None => false,
        }
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
