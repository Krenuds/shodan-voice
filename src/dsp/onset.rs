//! Syllable-start detection: a fast envelope jumping well above a slow one.

use super::filters::{Biquad, Envelope};

pub struct Onset {
    band: Biquad,
    fast: Envelope,
    slow: Envelope,
    refractory: u32,
    cooldown: u32,
}

impl Onset {
    pub fn new(sr: f32) -> Self {
        Self {
            band: Biquad::highpass(sr, 200.0, 0.707),
            fast: Envelope::new(sr, 1.0, 15.0),
            slow: Envelope::new(sr, 60.0, 250.0),
            refractory: (0.12 * sr) as u32,
            cooldown: 0,
        }
    }

    /// Returns true on the sample a syllable starts. `sensitivity` 0..1, `gate` is a linear level.
    #[inline]
    pub fn process(&mut self, x: f32, sensitivity: f32, gate: f32) -> bool {
        let y = self.band.process(x);
        let f = self.fast.process(y);
        let s = self.slow.process(y);
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        let ratio = 3.0 - 1.7 * sensitivity.clamp(0.0, 1.0);
        if f > gate && f > s * ratio + 1e-5 {
            self.cooldown = self.refractory;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::rng::Rng;

    #[test]
    fn fires_on_bursts_not_on_steady_noise() {
        let sr = 48000.0;
        let mut rng = Rng::new(3);
        let mut noise = Onset::new(sr);
        let mut hits = 0;
        for _ in 0..(sr as usize * 2) {
            if noise.process(rng.range(-0.05, 0.05), 0.6, 0.001) {
                hits += 1;
            }
        }
        // The very first moment of noise after silence is itself an onset; nothing after that.
        assert!(hits <= 1, "steady noise triggered {hits} onsets");

        let mut bursts = Onset::new(sr);
        let mut hits = 0;
        for n in 0..(sr as usize * 2) {
            let t = n as f32 / sr;
            // 4 syllables per second: 120 ms of 300 Hz tone, then silence.
            let on = (t * 4.0).fract() < 0.48;
            let x = if on { 0.3 * (t * 300.0 * std::f32::consts::TAU).sin() } else { 0.0 };
            if bursts.process(x, 0.6, 0.001) {
                hits += 1;
            }
        }
        assert_eq!(hits, 8);
    }
}
