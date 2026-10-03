//! Peak limiter followed by a gentle soft clipper so the output never exceeds 0 dBFS.

use super::filters::one_pole_coef;

pub struct Limiter {
    env: f32,
    release: f32,
    threshold: f32,
}

impl Limiter {
    pub fn new(sr: f32) -> Self {
        Self { env: 0.0, release: one_pole_coef(sr, 120.0), threshold: 0.8 }
    }

    /// Returns the gain to apply to a linked stereo pair with peak `peak`.
    #[inline]
    pub fn gain(&mut self, peak: f32) -> f32 {
        self.env = if peak > self.env { peak } else { peak + self.release * (self.env - peak) };
        if self.env > self.threshold { self.threshold / self.env } else { 1.0 }
    }
}

#[inline]
pub fn soft_clip(x: f32) -> f32 {
    // Linear below 0.8, then a tanh knee that approaches (but never reaches) 1.0.
    let a = x.abs();
    if a <= 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()) }
}
