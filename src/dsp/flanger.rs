//! Short modulated comb filter: the metallic, talking-through-a-pipe colour.

use super::filters::DelayLine;
use std::f32::consts::TAU;

pub struct Flanger {
    line: DelayLine,
    sr: f32,
    phase: f32,
    feedback_sample: f32,
}

pub struct FlangerSettings {
    pub depth: f32,
    pub rate_hz: f32,
    pub feedback: f32,
    pub mix: f32,
}

impl Flanger {
    pub fn new(sr: f32, phase: f32) -> Self {
        Self { line: DelayLine::new((0.012 * sr) as usize + 8), sr, phase, feedback_sample: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32, s: &FlangerSettings) -> f32 {
        self.phase = (self.phase + s.rate_hz / self.sr).fract();
        let lfo = 0.5 + 0.5 * (self.phase * TAU).sin();
        let delay_ms = 0.7 + s.depth * 6.5 * lfo;
        self.line.push(x + s.feedback * self.feedback_sample);
        let d = self.line.read_delay(delay_ms * 0.001 * self.sr);
        // Soft-limit the loop so high feedback rings instead of exploding.
        self.feedback_sample = d.clamp(-2.0, 2.0);
        let wet = (x + d) * 0.5 * (1.0 - s.feedback.abs() * 0.5);
        x + (wet - x) * s.mix
    }
}
