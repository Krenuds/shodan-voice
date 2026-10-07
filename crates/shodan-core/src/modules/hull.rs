//! Hull: the Dreadnought sarcophagus. Low cut, an octave-down sub, chest resonance, drive,
//! a metal comb and a small steel-room reverb. Works in mono (the hull is one speaker grille).

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::dsp::filters::{Biquad, DelayLine, Envelope};
use crate::dsp::rng::Rng;
use std::sync::Arc;

crate::params::define_params! {
    HullLowCut  => ("hull_lowcut", "Low cut", "Hull", 20.0, 200.0, 60.0, "Hz", Continuous, "High-pass before everything else."),
    SubLevel    => ("sub_level", "Sub", "Hull", 0.0, 1.0, 0.4, "", Continuous, "Octave-down sub layer."),
    SubTone     => ("sub_tone", "Sub tone", "Hull", 60.0, 400.0, 160.0, "Hz", Continuous, "Low-pass on the sub layer."),
    BodyFreq    => ("body_freq", "Body", "Hull", 60.0, 400.0, 130.0, "Hz", Continuous, "Centre of the chest resonance."),
    BodyGain    => ("body_gain", "Body gain", "Hull", -12.0, 18.0, 6.0, "dB", Continuous, "Boost of the chest resonance."),
    Drive       => ("drive", "Drive", "Hull", 0.0, 36.0, 10.0, "dB", Continuous, "Saturation into a tanh: grit and highs."),
    Presence    => ("presence", "Presence", "Hull", -12.0, 18.0, 4.0, "dB", Continuous, "Boost around 4 kHz after the drive."),
    Grit        => ("grit", "Grit", "Hull", 0.0, 1.0, 0.3, "", Continuous, "Hiss above 3 kHz that follows the voice: the highs an octave drop takes away."),
    CombMs      => ("comb_ms", "Comb", "Hull", 0.5, 10.0, 2.6, "ms", Continuous, "Delay of the metal comb resonance."),
    CombFb      => ("comb_fb", "Comb fb", "Hull", -0.9, 0.9, 0.5, "", Continuous, "Comb feedback. 0 = off."),
    RoomMs      => ("room_ms", "Room", "Hull", 30.0, 800.0, 120.0, "ms", Continuous, "Decay time (T60) of the steel room."),
    RoomMix     => ("room_mix", "Room mix", "Hull", 0.0, 1.0, 0.15, "", Continuous, "Amount of steel room."),
}

pub const KIND: ModuleKind = ModuleKind { id: "hull", name: "Hull", defs: DEFS, make: |sr, seed, shared, _app| Box::new(Hull::new(sr, seed, shared)) };

/// Comb delays of the steel room in ms: short and inharmonic, a cramped metal box.
const ROOM_COMBS: [f32; 4] = [7.3, 9.1, 11.3, 13.7];
const ROOM_ALLPASS: [(f32, f32); 2] = [(2.9, 0.5), (1.1, 0.5)];

struct Hull {
    shared: Arc<ModuleShared>,
    s: Smoothed,
    sr: f32,
    lowcut: Biquad,
    detect: [Biquad; 2],
    detect_env: Envelope,
    flip: f32,
    high: bool,
    sub_lp: [Biquad; 2],
    body: Biquad,
    presence: Biquad,
    rng: Rng,
    grit_env: Envelope,
    grit_hp: [Biquad; 2],
    comb: DelayLine,
    room: [DelayLine; 4],
    room_lp: [f32; 4],
    allpass: [DelayLine; 2],
}

impl Hull {
    fn new(sr: f32, seed: u64, shared: &Arc<ModuleShared>) -> Self {
        let ms = |ms: f32| (ms * 0.001 * sr) as usize + 4;
        Self {
            shared: shared.clone(),
            s: Smoothed::new(sr, &shared.params),
            sr,
            lowcut: Biquad::default(),
            detect: Default::default(),
            detect_env: Envelope::new(sr, 2.0, 30.0),
            flip: 1.0,
            high: false,
            sub_lp: Default::default(),
            body: Biquad::default(),
            presence: Biquad::default(),
            rng: Rng::new(seed),
            grit_env: Envelope::new(sr, 1.0, 40.0),
            grit_hp: [Biquad::highpass(sr, 3000.0, 0.707), Biquad::highpass(sr, 3000.0, 0.707)],
            comb: DelayLine::new(ms(10.0)),
            room: ROOM_COMBS.map(|d| DelayLine::new(ms(d))),
            room_lp: [0.0; 4],
            allpass: ROOM_ALLPASS.map(|(d, _)| DelayLine::new(ms(d))),
        }
    }

    fn update(&mut self) {
        let sr = self.sr;
        self.lowcut.set_highpass(sr, self.s.get(P::HullLowCut), 0.707);
        for f in &mut self.detect {
            f.set_lowpass(sr, 250.0, 0.707);
        }
        for f in &mut self.sub_lp {
            f.set_lowpass(sr, self.s.get(P::SubTone), 0.707);
        }
        self.body.set_peak(sr, self.s.get(P::BodyFreq), 1.0, self.s.get(P::BodyGain));
        self.presence.set_peak(sr, 4000.0, 0.8, self.s.get(P::Presence));
    }

    /// Octave-down square from a Schmitt-triggered flip-flop on the low-passed voice, shaped by its envelope.
    fn sub(&mut self, x: f32) -> f32 {
        let lp = self.detect[0].process(x);
        let lp = self.detect[1].process(lp);
        let env = self.detect_env.process(lp);
        let th = 0.2 * env;
        if !self.high && lp > th {
            self.high = true;
            self.flip = -self.flip;
        } else if self.high && lp < -th {
            self.high = false;
        }
        let sq = self.flip * env;
        let sq = self.sub_lp[0].process(sq);
        self.sub_lp[1].process(sq)
    }

    fn room(&mut self, x: f32, t60_ms: f32) -> f32 {
        let sr = self.sr;
        let mut wet = 0.0;
        for (i, line) in self.room.iter_mut().enumerate() {
            let d = ROOM_COMBS[i];
            let g = 10f32.powf(-3.0 * d / t60_ms.max(1.0));
            let y = line.read_delay(d * 0.001 * sr);
            self.room_lp[i] = y + 0.2 * (self.room_lp[i] - y);
            line.push(x + g * self.room_lp[i]);
            wet += y;
        }
        wet *= 0.25;
        for (i, line) in self.allpass.iter_mut().enumerate() {
            let (d, g) = ROOM_ALLPASS[i];
            let z = line.read_delay(d * 0.001 * sr);
            let v = wet + g * z;
            line.push(v);
            wet = z - g * v;
        }
        wet
    }
}

impl Module for Hull {
    fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            self.update();
            let sub_level = 2.0 * self.s.get(P::SubLevel);
            let gain = 10f32.powf(self.s.get(P::Drive) / 20.0);
            let makeup = 1.0 / gain.sqrt();
            let grit = 0.5 * self.s.get(P::Grit);
            let comb_delay = self.s.get(P::CombMs) * 0.001 * self.sr;
            let comb_fb = self.s.get(P::CombFb);
            let room_ms = self.s.get(P::RoomMs);
            let room_mix = self.s.get(P::RoomMix);
            for i in s0..s1 {
                let x = self.lowcut.process(0.5 * (left[i] + right[i]));
                let x = x + sub_level * self.sub(x);
                let x = self.body.process(x);
                let x = (gain * x).tanh() * makeup;
                let env = self.grit_env.process(x);
                let hiss = self.rng.range(-1.0, 1.0) * env;
                let hiss = self.grit_hp[0].process(hiss);
                let x = self.presence.process(x) + grit * self.grit_hp[1].process(hiss);
                let y = x + comb_fb * self.comb.read_delay(comb_delay);
                self.comb.push(y);
                let x = y * (1.0 - comb_fb.abs());
                let x = x + room_mix * self.room(x, room_ms);
                left[i] = x;
                right[i] = x;
            }
            s0 = s1;
        }
    }
}
