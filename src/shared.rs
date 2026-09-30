//! State shared between the GUI thread and the audio thread (all lock-free atomics).

use crate::params::{Params, io};
use std::sync::atomic::{AtomicU32, Ordering};

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
    /// Age of the oldest mic sample when its callback ran (driver capture delay), ms.
    pub input_ms: AtomicF32,
    /// Delay added by the rack modules, ms.
    pub dsp_ms: AtomicF32,
    /// Buffered + device playback delay for [main output, monitor], ms.
    pub output_ms: [AtomicF32; 2],
}

pub struct Shared {
    /// Input gain, output gain and bypass: the fixed stage around the rack.
    pub io: Params,
    pub meters: Meters,
}

impl Default for Shared {
    fn default() -> Self {
        Self { io: Params::new(io::DEFS), meters: Meters::default() }
    }
}
