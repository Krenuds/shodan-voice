//! Source/filter split with linear prediction, VT-4 style.
//!
//! The [`Analyzer`] runs on the raw mic signal. Every hop it fits an all-pole model of the vocal
//! tract (the formants) to the recent past and whitens the signal through it, leaving the
//! residual: the buzz of the vocal folds without the throat shape. It also tracks the pitch.
//! Everything downstream (tape glitches, pitch shifting, robot flattening) works on that residual,
//! and the [`Synthesizer`] puts the throat shape back at the end, optionally warped (formant
//! shift). Both are causal lattice filters, so the split adds no delay.

use super::filters::Biquad;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

pub const ORDER: usize = 32;
pub const HOP: usize = 128;
const WINDOW: usize = 1024;
const FRAMES: usize = 4096;
const FFT_LEN: usize = 512;

/// Model of one hop: reflection coefficients and pitch (0 = unvoiced/unknown).
#[derive(Clone, Copy)]
pub struct Frame {
    pub k: [f32; ORDER],
    pub f0: f32,
}

impl Default for Frame {
    fn default() -> Self {
        Self { k: [0.0; ORDER], f0: 0.0 }
    }
}

/// Widen formant bandwidths slightly and add a noise floor so the model stays well behaved.
fn condition(r: &mut [f32; ORDER + 1], sr: f32) {
    for (i, v) in r.iter_mut().enumerate().skip(1) {
        let x = 2.0 * PI * 50.0 * i as f32 / sr;
        *v *= (-0.5 * x * x).exp();
    }
    r[0] = r[0] * 1.0001 + 1e-10;
}

/// Levinson-Durbin: autocorrelation → reflection coefficients. Returns the prediction error.
fn levinson(r: &[f32; ORDER + 1], k: &mut [f32; ORDER]) -> f32 {
    let mut a = [0f32; ORDER + 1];
    let mut prev = [0f32; ORDER + 1];
    a[0] = 1.0;
    let mut err = r[0];
    if err <= 1e-9 {
        k.fill(0.0);
        return err.max(0.0);
    }
    for i in 1..=ORDER {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let ki = (-acc / err).clamp(-0.999, 0.999);
        prev[..=i].copy_from_slice(&a[..=i]);
        for j in 1..i {
            a[j] = prev[j] + ki * prev[i - j];
        }
        a[i] = ki;
        k[i - 1] = ki;
        err *= 1.0 - ki * ki;
    }
    err
}

/// Reflection coefficients → direct-form predictor polynomial A(z) = 1 + Σ a_j z^-j.
fn step_up(k: &[f32; ORDER]) -> [f32; ORDER + 1] {
    let mut a = [0f32; ORDER + 1];
    let mut prev = [0f32; ORDER + 1];
    a[0] = 1.0;
    for i in 1..=ORDER {
        prev[..=i].copy_from_slice(&a[..=i]);
        for j in 1..i {
            a[j] = prev[j] + k[i - 1] * prev[i - j];
        }
        a[i] = k[i - 1];
    }
    a
}

/// Whitening (FIR) lattice: signal → residual.
#[derive(Clone)]
struct AnalysisLattice {
    /// b_m(n-1) for each stage.
    b: [f32; ORDER],
}

impl AnalysisLattice {
    #[inline]
    fn process(&mut self, x: f32, k: &[f32; ORDER]) -> f32 {
        let mut f = x;
        let mut b_in = x;
        for m in 0..ORDER {
            let b_prev = self.b[m];
            let f_next = f + k[m] * b_prev;
            let b_next = b_prev + k[m] * f;
            self.b[m] = b_in;
            b_in = b_next;
            f = f_next;
        }
        f
    }
}

/// All-pole (IIR) lattice: residual → signal. Exact inverse of [`AnalysisLattice`], and stable
/// for any |k| < 1, even while the coefficients move.
#[derive(Clone)]
struct SynthesisLattice {
    b: [f32; ORDER + 1],
}

impl SynthesisLattice {
    #[inline]
    fn process(&mut self, e: f32, k: &[f32; ORDER]) -> f32 {
        let mut f = e;
        for m in (0..ORDER).rev() {
            f -= k[m] * self.b[m];
            self.b[m + 1] = self.b[m] + k[m] * f;
        }
        self.b[0] = f;
        f
    }
}

/// YIN pitch tracker on a ~11 kHz decimated copy of the input.
struct PitchTracker {
    decimate: usize,
    phase: usize,
    lowpass: [Biquad; 2],
    ring: Vec<f32>,
    written: usize,
    since: usize,
    sr: f32,
    window: usize,
    tau_min: usize,
    tau_max: usize,
    diff: Vec<f32>,
    f0: f32,
}

impl PitchTracker {
    fn new(sr: f32) -> Self {
        let decimate = (sr / 11025.0).round().max(1.0) as usize;
        let dsr = sr / decimate as f32;
        let tau_max = (dsr / 70.0) as usize;
        Self {
            decimate,
            phase: 0,
            lowpass: [Biquad::default(), Biquad::default()].map(|mut b| {
                b.set_lowpass(sr, 2400.0, 0.707);
                b
            }),
            ring: vec![0.0; 1024],
            written: 0,
            since: 0,
            sr: dsr,
            window: (0.022 * dsr) as usize,
            tau_min: (dsr / 500.0) as usize,
            tau_max,
            diff: vec![0.0; tau_max + 2],
            f0: 0.0,
        }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        let y = self.lowpass[0].process(x);
        let y = self.lowpass[1].process(y);
        self.phase += 1;
        if self.phase < self.decimate {
            return;
        }
        self.phase = 0;
        self.ring[self.written & 1023] = y;
        self.written += 1;
        self.since += 1;
        if self.since >= 32 && self.written > self.window + self.tau_max + 2 {
            self.since = 0;
            self.detect();
        }
    }

    fn detect(&mut self) {
        let (w, tmax) = (self.window, self.tau_max);
        let base = self.written - (w + tmax + 1);
        let x = |j: usize| self.ring[(base + j) & 1023];
        let mut energy = 0.0;
        for j in 0..w {
            energy += x(j) * x(j);
        }
        if energy / (w as f32) < 1e-7 {
            self.f0 = 0.0;
            return;
        }
        // Cumulative-mean-normalised difference function.
        let mut running = 0.0;
        self.diff[0] = 1.0;
        for tau in 1..=tmax {
            let mut d = 0.0;
            for j in 0..w {
                let v = x(j) - x(j + tau);
                d += v * v;
            }
            running += d;
            self.diff[tau] = if running > 0.0 { d * tau as f32 / running } else { 1.0 };
        }
        let mut tau = self.tau_min.max(2);
        while tau < tmax && self.diff[tau] >= 0.15 {
            tau += 1;
        }
        if tau >= tmax {
            self.f0 = 0.0;
            return;
        }
        while tau + 1 < tmax && self.diff[tau + 1] < self.diff[tau] {
            tau += 1;
        }
        let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
        let denom = a - 2.0 * b + c;
        let shift = if denom.abs() > 1e-9 { (0.5 * (a - c) / denom).clamp(-1.0, 1.0) } else { 0.0 };
        self.f0 = self.sr / (tau as f32 + shift);
    }
}

pub struct Analyzer {
    sr: f32,
    history: Vec<f32>,
    written: u64,
    window: Vec<f32>,
    segment: Vec<f32>,
    lattice: AnalysisLattice,
    k_prev: [f32; ORDER],
    k_next: [f32; ORDER],
    frames: Vec<Frame>,
    pitch: PitchTracker,
}

impl Analyzer {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            history: vec![0.0; WINDOW * 2],
            written: 0,
            window: (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / WINDOW as f32).cos()).collect(),
            segment: vec![0.0; WINDOW],
            lattice: AnalysisLattice { b: [0.0; ORDER] },
            k_prev: [0.0; ORDER],
            k_next: [0.0; ORDER],
            frames: vec![Frame::default(); FRAMES],
            pitch: PitchTracker::new(sr),
        }
    }

    fn fit(&mut self) -> [f32; ORDER] {
        let mask = self.history.len() - 1;
        let start = self.written as usize + self.history.len() - WINDOW;
        let mut prev = self.history[(start + mask) & mask];
        for j in 0..WINDOW {
            let x = self.history[(start + j) & mask];
            // Pre-emphasis so the model captures formants rather than the glottal tilt.
            self.segment[j] = (x - 0.9 * prev) * self.window[j];
            prev = x;
        }
        let mut r = [0f32; ORDER + 1];
        for (lag, v) in r.iter_mut().enumerate() {
            *v = self.segment[..WINDOW - lag].iter().zip(&self.segment[lag..]).map(|(a, b)| a * b).sum();
        }
        condition(&mut r, self.sr);
        let mut k = [0f32; ORDER];
        levinson(&r, &mut k);
        k
    }

    /// Feed one raw sample, get the residual back.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let pos = (self.written % HOP as u64) as usize;
        if pos == 0 {
            self.k_prev = self.k_next;
            self.k_next = self.fit();
            let hop = (self.written / HOP as u64) as usize;
            self.frames[hop % FRAMES] = Frame { k: self.k_next, f0: self.pitch.f0 };
        }
        self.pitch.push(x);
        let t = pos as f32 / HOP as f32;
        let mut k = [0f32; ORDER];
        for i in 0..ORDER {
            k[i] = self.k_prev[i] + (self.k_next[i] - self.k_prev[i]) * t;
        }
        let e = self.lattice.process(x, &k);
        let mask = self.history.len() - 1;
        self.history[self.written as usize & mask] = x;
        self.written += 1;
        e
    }

    /// The model for the audio at absolute sample `pos` (clamped to what exists).
    pub fn frame_at(&self, pos: f64) -> Frame {
        let newest = self.written.saturating_sub(1) / HOP as u64;
        let hop = (pos.max(0.0) as u64 / HOP as u64).min(newest);
        let oldest = newest.saturating_sub(FRAMES as u64 - 2);
        self.frames[(hop.max(oldest) as usize) % FRAMES]
    }

    /// Latest pitch estimate in Hz (0 = unvoiced).
    #[cfg(test)]
    pub fn f0(&self) -> f32 {
        self.pitch.f0
    }
}

/// Puts a (possibly formant-shifted) vocal-tract model back onto a stereo residual.
pub struct Synthesizer {
    sr: f32,
    lattices: [SynthesisLattice; 2],
    k_prev: [f32; ORDER],
    k_next: [f32; ORDER],
    g_prev: f32,
    g_next: f32,
    count: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    power: Vec<f32>,
}

impl Synthesizer {
    pub fn new(sr: f32) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_LEN);
        let ifft = planner.plan_fft_inverse(FFT_LEN);
        let scratch_len = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        Self {
            sr,
            lattices: [SynthesisLattice { b: [0.0; ORDER + 1] }, SynthesisLattice { b: [0.0; ORDER + 1] }],
            k_prev: [0.0; ORDER],
            k_next: [0.0; ORDER],
            g_prev: 1.0,
            g_next: 1.0,
            count: HOP,
            fft,
            ifft,
            spec: vec![Complex32::default(); FFT_LEN],
            scratch: vec![Complex32::default(); scratch_len],
            power: vec![0.0; FFT_LEN / 2 + 1],
        }
    }

    /// True when the next hop's model is due.
    pub fn needs_frame(&self) -> bool {
        self.count >= HOP
    }

    /// Start moving towards `frame`, formant-scaled by `factor` (>1 = smaller throat).
    pub fn set_frame(&mut self, frame: &Frame, factor: f32) {
        self.k_prev = self.k_next;
        self.g_prev = self.g_next;
        if (factor - 1.0).abs() < 1e-3 {
            self.k_next = frame.k;
            self.g_next = 1.0;
        } else {
            self.g_next = self.warp(&frame.k, factor);
        }
        self.count = 0;
    }

    /// Scale the model's spectral envelope along frequency and refit it. Returns the gain that
    /// keeps the level per frequency the same.
    fn warp(&mut self, k: &[f32; ORDER], factor: f32) -> f32 {
        let a = step_up(k);
        for (i, s) in self.spec.iter_mut().enumerate() {
            *s = Complex32::new(if i <= ORDER { a[i] } else { 0.0 }, 0.0);
        }
        self.fft.process_with_scratch(&mut self.spec, &mut self.scratch);
        let half = FFT_LEN / 2;
        for i in 0..=half {
            self.power[i] = 1.0 / self.spec[i].norm_sqr().max(1e-9);
        }
        for i in 0..=half {
            let src = i as f32 / factor;
            let p = if src >= half as f32 {
                self.power[half]
            } else {
                let j = src as usize;
                let t = src - j as f32;
                self.power[j] + (self.power[j + 1] - self.power[j]) * t
            };
            self.spec[i] = Complex32::new(p, 0.0);
            if i > 0 && i < half {
                self.spec[FFT_LEN - i] = Complex32::new(p, 0.0);
            }
        }
        self.ifft.process_with_scratch(&mut self.spec, &mut self.scratch);
        let mut r = [0f32; ORDER + 1];
        for (i, v) in r.iter_mut().enumerate() {
            *v = self.spec[i].re / FFT_LEN as f32;
        }
        condition(&mut r, self.sr);
        levinson(&r, &mut self.k_next).max(0.0).sqrt()
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let t = (self.count.min(HOP)) as f32 / HOP as f32;
        let mut k = [0f32; ORDER];
        for i in 0..ORDER {
            k[i] = self.k_prev[i] + (self.k_next[i] - self.k_prev[i]) * t;
        }
        let g = self.g_prev + (self.g_next - self.g_prev) * t;
        self.count += 1;
        (self.lattices[0].process(l * g, &k), self.lattices[1].process(r * g, &k))
    }
}

pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::filters::Biquad;
    use crate::dsp::rng::Rng;

    const SR: f32 = 48000.0;

    /// A crude vowel: a 120 Hz pulse train through resonances at 700 and 1200 Hz, plus breath.
    fn vowel(n: usize, f0: f32) -> Vec<f32> {
        let mut f1 = Biquad::default();
        let mut f2 = Biquad::default();
        f1.set_lowpass(SR, 700.0, 8.0);
        f2.set_lowpass(SR, 1200.0, 6.0);
        let mut rng = Rng::new(9);
        let period = SR / f0;
        (0..n)
            .map(|i| {
                let pulse = if (i as f32 % period) < 1.0 { 1.0 } else { 0.0 };
                0.3 * f2.process(f1.process(pulse + rng.range(-0.01, 0.01)))
            })
            .collect()
    }

    fn centroid(x: &[f32]) -> f32 {
        // Spectral centroid below 4 kHz via a direct DFT on a few hundred bins.
        let n = 4096.min(x.len());
        let seg = &x[x.len() - n..];
        let (mut num, mut den) = (0.0, 0.0);
        for bin in 1..(4000.0 * n as f32 / SR) as usize {
            let w = 2.0 * PI * bin as f32 / n as f32;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in seg.iter().enumerate() {
                re += v * (w * i as f32).cos();
                im -= v * (w * i as f32).sin();
            }
            let mag = re * re + im * im;
            num += mag * bin as f32 * SR / n as f32;
            den += mag;
        }
        num / den
    }

    #[test]
    fn analysis_then_synthesis_reconstructs() {
        let x = vowel(24000, 120.0);
        let mut an = Analyzer::new(SR);
        let mut sy = Synthesizer::new(SR);
        let mut err = 0.0f32;
        let mut peak = 0.0f32;
        for (n, &v) in x.iter().enumerate() {
            let e = an.process(v);
            if sy.needs_frame() {
                sy.set_frame(&an.frame_at(n as f64), 1.0);
            }
            let (y, _) = sy.process(e, e);
            if n > 4000 {
                err = err.max((y - v).abs());
                peak = peak.max(v.abs());
            }
        }
        assert!(err < peak * 1e-3, "reconstruction error {err} vs peak {peak}");
    }

    #[test]
    fn formant_shift_moves_the_spectrum() {
        let x = vowel(24000, 120.0);
        let render = |factor: f32| {
            let mut an = Analyzer::new(SR);
            let mut sy = Synthesizer::new(SR);
            x.iter()
                .enumerate()
                .map(|(n, &v)| {
                    let e = an.process(v);
                    if sy.needs_frame() {
                        sy.set_frame(&an.frame_at(n as f64), factor);
                    }
                    let y = sy.process(e, e).0;
                    assert!(y.is_finite());
                    y
                })
                .collect::<Vec<_>>()
        };
        let (c0, up, down) = (centroid(&render(1.0)), centroid(&render(1.3)), centroid(&render(0.77)));
        println!("centroid {c0:.0} Hz, up {up:.0}, down {down:.0}");
        assert!(up > c0 * 1.12, "formants did not move up: {c0} -> {up}");
        assert!(down < c0 * 0.9, "formants did not move down: {c0} -> {down}");
    }

    #[test]
    fn tracks_pitch() {
        for f0 in [90.0, 150.0, 220.0, 330.0] {
            let x = vowel(24000, f0);
            let mut an = Analyzer::new(SR);
            for &v in &x {
                an.process(v);
            }
            let got = an.f0();
            assert!((got - f0).abs() / f0 < 0.03, "wanted {f0} Hz, tracked {got}");
        }
    }
}
