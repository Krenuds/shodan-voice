//! Sample-and-hold downsampling, bit reduction and a narrowing band-pass "era" filter.

use super::filters::Biquad;

pub struct Lofi {
    sr: f32,
    hold: f32,
    acc: f32,
    hp: Biquad,
    lp: Biquad,
    tone: f32,
}

pub struct LofiSettings {
    pub rate_hz: f32,
    pub bits: f32,
    pub tone: f32,
}

impl Lofi {
    pub fn new(sr: f32) -> Self {
        let mut l = Self { sr, hold: 0.0, acc: 1.0, hp: Biquad::default(), lp: Biquad::default(), tone: -1.0 };
        l.set_tone(0.0);
        l
    }

    fn set_tone(&mut self, tone: f32) {
        if (tone - self.tone).abs() < 1e-3 {
            return;
        }
        self.tone = tone;
        let hp = 60.0 * (550.0f32 / 60.0).powf(tone);
        let lp = 16000.0 * (2800.0f32 / 16000.0).powf(tone);
        self.hp.set_highpass(self.sr, hp, 0.707);
        self.lp.set_lowpass(self.sr, lp, 0.9);
    }

    /// Call once per sub-block before `process`.
    pub fn update(&mut self, s: &LofiSettings) {
        self.set_tone(s.tone);
    }

    #[inline]
    pub fn process(&mut self, x: f32, s: &LofiSettings) -> f32 {
        let y = self.lp.process(self.hp.process(x));
        self.acc += s.rate_hz / self.sr;
        if self.acc >= 1.0 {
            self.acc -= self.acc.floor();
            self.hold = y;
        }
        if s.bits >= 15.9 {
            return self.hold;
        }
        let levels = 2f32.powf(s.bits - 1.0);
        (self.hold * levels).round() / levels
    }
}
