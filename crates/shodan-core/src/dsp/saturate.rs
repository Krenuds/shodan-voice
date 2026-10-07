//! Antialiased tanh: first-order ADAA (Parker, Zavalishin, Le Bivic, DAFx 2016) at 2x oversampling
//! through windowed-sinc halfband filters. A bias before the curve adds even harmonics.

/// Halfband FIR length (odd). Up plus down adds `LATENCY` samples at the base rate.
const TAPS: usize = 31;
pub const LATENCY: usize = TAPS / 2;

fn halfband() -> [f32; TAPS] {
    let c = (TAPS / 2) as f32;
    let mut h = [0.0; TAPS];
    for (n, v) in h.iter_mut().enumerate() {
        let t = n as f32 - c;
        let sinc = if t == 0.0 { 0.5 } else { (0.5 * std::f32::consts::PI * t).sin() / (std::f32::consts::PI * t) };
        let w = 2.0 * std::f32::consts::PI * n as f32 / (TAPS - 1) as f32;
        *v = sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos());
    }
    let sum: f32 = h.iter().sum();
    h.map(|v| v / sum)
}

#[derive(Clone)]
struct Fir {
    h: [f32; TAPS],
    z: [f32; TAPS],
    pos: usize,
}

impl Fir {
    fn new() -> Self {
        Self { h: halfband(), z: [0.0; TAPS], pos: 0 }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.pos = (self.pos + TAPS - 1) % TAPS;
        self.z[self.pos] = x;
        let (a, b) = self.z.split_at(self.pos);
        b.iter().chain(a).zip(&self.h).map(|(z, h)| z * h).sum()
    }
}

/// ln(cosh(x)) without overflow.
#[inline]
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

#[derive(Clone)]
pub struct Saturator {
    up: Fir,
    down: Fir,
    x1: f64,
}

impl Default for Saturator {
    fn default() -> Self {
        Self { up: Fir::new(), down: Fir::new(), x1: 0.0 }
    }
}

impl Saturator {
    /// tanh(x + bias) - tanh(bias), antialiased. Output is delayed by `LATENCY` samples.
    #[inline]
    pub fn process(&mut self, x: f32, bias: f32) -> f32 {
        let b = bias as f64;
        let tb = b.tanh();
        let anti = |u: f64| ln_cosh(u + b) - u * tb;
        let mut out = 0.0;
        for (k, u) in [2.0 * x, 0.0].into_iter().enumerate() {
            let u = self.up.process(u) as f64;
            let d = u - self.x1;
            let y = if d.abs() > 1e-6 { (anti(u) - anti(self.x1)) / d } else { (0.5 * (u + self.x1) + b).tanh() - tb };
            self.x1 = u;
            let y = self.down.process(y as f32);
            if k == 0 {
                out = y;
            }
        }
        out
    }
}
