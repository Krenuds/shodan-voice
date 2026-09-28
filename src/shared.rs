//! State shared between the GUI thread and the audio thread (all lock-free atomics).

use crate::params::Params;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const SLOT_COUNT: usize = 2;

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    /// Keep the maximum; used for peak meters the GUI resets after reading.
    pub fn max(&self, v: f32) {
        let _ = self.0.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
            (v > f32::from_bits(old)).then_some(v.to_bits())
        });
    }
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

#[derive(Default)]
pub struct Meters {
    pub input_peak: AtomicF32,
    pub output_peak: AtomicF32,
    /// Semitone offset of the main voice.
    pub pitch: AtomicF32,
    /// How far the tape head is behind live, in ms.
    pub lag_ms: AtomicF32,
    /// Incremented for every glitch fired, so the GUI can flash.
    pub glitches: AtomicU32,
    /// Audio callbacks that had to pad with silence (input ran dry).
    pub underruns: AtomicU32,
}

pub struct SlotShared {
    pub enabled: AtomicBool,
    pub mix: AtomicF32,
}

impl Default for SlotShared {
    fn default() -> Self {
        Self { enabled: AtomicBool::new(true), mix: AtomicF32::new(1.0) }
    }
}

#[derive(Default)]
pub struct Shared {
    pub params: Params,
    pub meters: Meters,
    pub slots: [SlotShared; SLOT_COUNT],
}
