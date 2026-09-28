use std::f32::consts::PI;

/// RBJ biquad, transposed direct form II.
#[derive(Clone, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn highpass(sr: f32, freq: f32, q: f32) -> Self {
        let mut f = Self::default();
        f.set_highpass(sr, freq, q);
        f
    }

    pub fn set_highpass(&mut self, sr: f32, freq: f32, q: f32) {
        let (cos, alpha) = Self::prewarp(sr, freq, q);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 + cos) / 2.0 / a0;
        self.b1 = -(1.0 + cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    pub fn set_lowpass(&mut self, sr: f32, freq: f32, q: f32) {
        let (cos, alpha) = Self::prewarp(sr, freq, q);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - cos) / 2.0 / a0;
        self.b1 = (1.0 - cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn prewarp(sr: f32, freq: f32, q: f32) -> (f32, f32) {
        let w = 2.0 * PI * freq.clamp(10.0, sr * 0.49) / sr;
        (w.cos(), w.sin() / (2.0 * q))
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[derive(Clone, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}

impl DcBlock {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.995 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Coefficient for a one-pole smoother reaching ~63% in `ms` milliseconds.
pub fn one_pole_coef(sr: f32, ms: f32) -> f32 {
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * sr)).exp() }
}

/// Envelope follower with separate attack and release times.
#[derive(Clone)]
pub struct Envelope {
    att: f32,
    rel: f32,
    pub value: f32,
}

impl Envelope {
    pub fn new(sr: f32, attack_ms: f32, release_ms: f32) -> Self {
        Self { att: one_pole_coef(sr, attack_ms), rel: one_pole_coef(sr, release_ms), value: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x.abs();
        let c = if x > self.value { self.att } else { self.rel };
        self.value = x + c * (self.value - x);
        self.value
    }
}

/// 4-point cubic (Catmull-Rom) interpolation between `y1` and `y2`.
#[inline]
pub fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

/// Power-of-two circular buffer with fractional (cubic) reads by absolute sample index.
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    /// Absolute index of the next sample to be written.
    pub write: u64,
}

impl DelayLine {
    pub fn new(min_len: usize) -> Self {
        let len = min_len.next_power_of_two();
        Self { buf: vec![0.0; len], mask: len - 1, write: 0 }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buf[self.write as usize & self.mask] = x;
        self.write += 1;
    }

    #[inline]
    fn at(&self, i: i64) -> f32 {
        self.buf[i as usize & self.mask]
    }

    /// Read at an absolute (fractional) position. Needs pos+2 < write.
    #[inline]
    pub fn read_abs(&self, pos: f64) -> f32 {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let i = i as i64;
        cubic(self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2), t)
    }

    /// Read `delay` samples behind the most recently written sample.
    #[inline]
    pub fn read_delay(&self, delay: f32) -> f32 {
        self.read_abs(self.write as f64 - 1.0 - delay.max(2.0) as f64)
    }
}
