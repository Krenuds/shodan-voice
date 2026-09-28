//! Delay-line granular pitch shifter plus the random "pitch jump" controller.

use super::filters::{DelayLine, one_pole_coef};
use super::rng::Rng;
use std::f32::consts::PI;

/// Two taps sweep through a short delay window at a speed set by the pitch ratio and are
/// crossfaded with sin² windows. Cheap, ~window/2 latency, and a little robotic.
pub struct Shifter {
    line: DelayLine,
    sr: f32,
    max_window: f32,
    window: f32,
    phase: f32,
    /// Constant extra delay (used to offset layered voices in time).
    pub extra_delay: f32,
}

impl Shifter {
    /// `max_window_ms` sizes the buffer; the window itself starts at 20 ms (see `set_window_ms`).
    pub fn new(sr: f32, max_window_ms: f32, max_extra_ms: f32) -> Self {
        let max_window = max_window_ms * 0.001 * sr;
        let len = (max_window + max_extra_ms * 0.001 * sr) as usize + 16;
        Self { line: DelayLine::new(len), sr, max_window, window: (0.02 * sr).min(max_window), phase: 0.5, extra_delay: 0.0 }
    }

    /// Grain size: shorter = less delay (about half the window), rougher sound.
    pub fn set_window_ms(&mut self, ms: f32) {
        self.window = (ms * 0.001 * self.sr).clamp(64.0, self.max_window);
    }

    #[inline]
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        self.line.push(x);
        let w = self.window;
        let mut inc = (1.0 - ratio) / w;
        if (ratio - 1.0).abs() < 1e-3 {
            // Unshifted: glide the phase to 0.5 so a single tap plays and the taps don't comb.
            let step = 0.0035 / w;
            let d = 0.5 - self.phase;
            inc = d.clamp(-step, step);
        }
        self.phase = (self.phase + inc).rem_euclid(1.0);
        let p2 = (self.phase + 0.5) % 1.0;
        let g1 = (PI * self.phase).sin().powi(2);
        let g2 = 1.0 - g1;
        let max_d = (self.line.len() - 8) as f32;
        let d1 = (self.extra_delay + self.phase * w).min(max_d);
        let d2 = (self.extra_delay + p2 * w).min(max_d);
        g1 * self.line.read_delay(d1) + g2 * self.line.read_delay(d2)
    }
}

/// Decides when and where the pitch jumps. Output is a semitone offset.
pub struct Jumper {
    rng: Rng,
    sr: f32,
    current: f32,
    target: f32,
    timer: u32,
    silence: u32,
}

pub struct JumpSettings {
    pub chance: f32,
    pub range: f32,
    pub glide_ms: f32,
}

impl Jumper {
    pub fn new(sr: f32, rng: Rng) -> Self {
        Self { rng, sr, current: 0.0, target: 0.0, timer: 0, silence: 0 }
    }

    pub fn jump(&mut self, range: f32) {
        self.target = if range < 0.01 || self.rng.chance(0.3) {
            0.0
        } else {
            let t = self.rng.range(-range, range);
            if range >= 1.0 { t.round() } else { t }
        };
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, t: f32) {
        self.target = t;
    }

    /// Advance by `n` samples. `onset` = a syllable just started, `voiced` = someone is talking.
    pub fn update(&mut self, n: u32, onset: bool, voiced: bool, s: &JumpSettings) -> f32 {
        if onset && self.rng.chance(s.chance) {
            self.jump(s.range);
        }
        if voiced {
            self.silence = 0;
            if self.timer <= n {
                if self.rng.chance(s.chance * 0.5) {
                    self.jump(s.range);
                }
                self.timer = (self.rng.range(0.15, 0.6) * self.sr) as u32;
            } else {
                self.timer -= n;
            }
        } else {
            self.silence += n;
            if self.silence as f32 > 0.5 * self.sr {
                self.target = 0.0;
            }
        }
        if s.range < 0.01 {
            self.target = 0.0;
        }
        let c = one_pole_coef(self.sr / n as f32, s.glide_ms);
        self.current = self.target + c * (self.current - self.target);
        self.current
    }
}

pub fn semis_to_ratio(semis: f32) -> f32 {
    2f32.powf(semis / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pitch of the shifted output via autocorrelation (robust to grain-crossfade dips).
    fn measure_hz(ratio: f32, window_ms: f32) -> f32 {
        let sr = 48000.0;
        let mut s = Shifter::new(sr, 40.0, 0.0);
        s.set_window_ms(window_ms);
        let out: Vec<f32> = (0..sr as usize).map(|n| s.process((n as f32 / sr * 220.0 * std::f32::consts::TAU).sin(), ratio)).collect();
        let seg = &out[24_000..24_000 + 8192];
        let corr = |lag: usize| -> f32 { seg[..seg.len() - lag].iter().zip(&seg[lag..]).map(|(a, b)| a * b).sum() };
        // First strong peak in the 80–400 Hz lag range (later peaks are period multiples).
        let c: Vec<f32> = (120..600).map(corr).collect();
        let max = c.iter().copied().fold(f32::MIN, f32::max);
        let first = (1..c.len() - 1).find(|&i| c[i] > 0.85 * max && c[i] >= c[i - 1] && c[i] >= c[i + 1]).unwrap();
        sr / (first + 120) as f32
    }

    #[test]
    fn shifts_up_and_down_a_fifth() {
        for window in [12.0, 20.0, 40.0] {
            for semis in [7.0f32, -7.0, 0.0] {
                let expected = 220.0 * semis_to_ratio(semis);
                let got = measure_hz(semis_to_ratio(semis), window);
                // Short grains smear pitch towards the input (the price of low latency).
                let tolerance = if window < 15.0 { 0.12 } else { 0.045 };
                assert!((got - expected).abs() / expected < tolerance, "{window} ms, {semis} st: got {got} Hz, want {expected}");
            }
        }
    }
}
