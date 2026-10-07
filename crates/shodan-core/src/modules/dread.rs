//! Dread: the Dreadnought grind, built so the words survive it. An LR4 split keeps the
//! 1-4 kHz consonants out of the heavy drive; the low band gets the chest and the grind.
//! Then compression, short metal reflections and a hull resonance. Optional parallel sends:
//! an octave sub, a low ring mod and a dry consonant path. Mono (one speaker grille).

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::dsp::filters::{Biquad, DcBlock, DelayLine, Envelope};
use crate::dsp::pitch::Shifter;
use crate::dsp::rng::Rng;
use crate::dsp::saturate::{self, Saturator};
use std::f32::consts::PI;
use std::sync::Arc;

crate::params::define_params! {
    DreadLowCut => ("dread_lowcut", "Low cut", "Dread", 20.0, 200.0, 45.0, "Hz", Continuous, "High-pass before everything else."),
    Xover       => ("xover", "Crossover", "Dread", 300.0, 2500.0, 900.0, "Hz", Continuous, "Split between the driven low band and the cleaner high band."),
    ChestFreq   => ("chest_freq", "Chest", "Dread", 60.0, 400.0, 130.0, "Hz", Continuous, "Centre of the chest resonance, before the low drive."),
    ChestGain   => ("chest_gain", "Chest gain", "Dread", -12.0, 18.0, 6.0, "dB", Continuous, "Boost of the chest resonance."),
    LowDrive    => ("low_drive", "Low drive", "Dread", 0.0, 36.0, 15.0, "dB", Continuous, "Drive into the low band's saturator: size and grind."),
    LowBias     => ("low_bias", "Low bias", "Dread", 0.0, 1.0, 0.2, "", Continuous, "Asymmetry of the low saturator: even harmonics."),
    Weight      => ("weight", "Weight", "Dread", -6.0, 18.0, 0.0, "dB", Continuous, "Low peak after the drive, so the chest survives the saturator."),
    WeightFreq  => ("weight_freq", "Weight freq", "Dread", 50.0, 250.0, 100.0, "Hz", Continuous, "Centre of the weight peak."),
    HighDrive   => ("high_drive", "High drive", "Dread", 0.0, 24.0, 4.0, "dB", Continuous, "Drive into the high band's saturator. Keep it light."),
    HighGain    => ("high_gain", "High level", "Dread", -18.0, 12.0, 0.0, "dB", Continuous, "Level of the high band against the low."),
    Presence    => ("dread_presence", "Presence", "Dread", -12.0, 12.0, 4.0, "dB", Continuous, "Peak at 3 kHz on the high band."),
    CompThresh  => ("comp_thresh", "Comp threshold", "Dread", -48.0, 0.0, -24.0, "dB", Continuous, "Compressor threshold."),
    CompRatio   => ("comp_ratio", "Comp ratio", "Dread", 1.0, 20.0, 4.0, ":1", Continuous, "Compressor ratio. 1 = off."),
    Hash        => ("hash", "Hash", "Dread", 0.0, 1.0, 0.0, "", Continuous, "Noise above 2.5 kHz riding the voice's level: vox grit through vowels too."),
    ErMix       => ("er_mix", "Reflections", "Dread", 0.0, 1.0, 0.12, "", Continuous, "Short metal early reflections."),
    ErSize      => ("er_size", "Box size", "Dread", 0.3, 2.0, 1.0, "", Continuous, "Scales the reflection times (1 = 3 to 38 ms)."),
    TankMix     => ("tank_mix", "Tank", "Dread", 0.0, 1.0, 0.0, "", Continuous, "Dense steel tank (8-line FDN) that fills the gaps between syllables."),
    TankT60     => ("tank_t60", "Tank decay", "Dread", 50.0, 1500.0, 350.0, "ms", Continuous, "Decay time (T60) of the tank."),
    TankDamp    => ("tank_damp", "Tank damping", "Dread", 0.0, 0.95, 0.3, "", Continuous, "High-frequency loss per pass. Steel: low."),
    HullMix     => ("hull_mix", "Hull ring", "Dread", 0.0, 1.0, 0.0, "", Continuous, "Inharmonic plate modes: the voice inside a steel shell."),
    HullFreq    => ("hull_freq", "Hull pitch", "Dread", 80.0, 1200.0, 310.0, "Hz", Continuous, "Lowest plate mode."),
    SubMix      => ("sub_mix", "Sub", "Dread", 0.0, 1.0, 0.0, "", Continuous, "Octave-down layer, low-passed and driven."),
    RingMix     => ("ring_mix", "Ring", "Dread", 0.0, 1.0, 0.0, "", Continuous, "Low ring-mod send with diode clipping (Dalek grind)."),
    RingHz      => ("ring_hz", "Ring rate", "Dread", 10.0, 120.0, 40.0, "Hz", Continuous, "Ring-mod carrier."),
    Air         => ("air", "Air", "Dread", -60.0, 0.0, -60.0, "dB", Continuous, "Dry consonant path above 3.5 kHz. -60 = off."),
    DreadOut    => ("dread_out", "Output", "Dread", -24.0, 24.0, 0.0, "dB", Continuous, "Output level."),
}

pub const KIND: ModuleKind = ModuleKind { id: "dread", name: "Dread", defs: DEFS, make: |sr, seed, shared, _app| Box::new(Dread::new(sr, seed, shared)) };

/// Early reflections of a cramped steel box: (ms, gain). Alternating signs for a hard, hollow wall.
const TAPS: [(f32, f32); 9] = [(3.1, 0.7), (5.3, -0.6), (7.9, 0.5), (11.2, -0.45), (14.7, 0.4), (19.3, -0.33), (23.9, 0.28), (31.1, -0.22), (37.7, 0.18)];
/// Tank delay lengths in ms: mutually prime-ish, a cramped hull.
const TANK: [f32; 8] = [3.1, 4.3, 5.9, 7.1, 8.9, 10.7, 12.7, 14.9];
/// Mode ratios of a clamped circular plate.
const MODES: [f32; 8] = [1.0, 1.59, 2.14, 2.3, 2.65, 2.92, 3.16, 3.5];

struct Dread {
    shared: Arc<ModuleShared>,
    s: Smoothed,
    sr: f32,
    lowcut: Biquad,
    dc_in: DcBlock,
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    chest: Biquad,
    weight: Biquad,
    presence: Biquad,
    low_sat: Saturator,
    high_sat: Saturator,
    dc_low: DcBlock,
    dc_high: DcBlock,
    /// Aligns the parallel sends with the oversampled bands.
    align: DelayLine,
    comp: Envelope,
    reflect: DelayLine,
    modes: [Biquad; 8],
    tank: [DelayLine; 8],
    tank_lp: [f32; 8],
    shifter: Shifter,
    sub_lp: [Biquad; 2],
    ring_phase: f32,
    ring_bp: [Biquad; 2],
    dc_ring: DcBlock,
    air_hp: [Biquad; 2],
    rng: Rng,
    hash_env: Envelope,
    hash_hp: [Biquad; 2],
}

impl Dread {
    fn new(sr: f32, seed: u64, shared: &Arc<ModuleShared>) -> Self {
        let mut shifter = Shifter::new(sr, 60.0, 0.0);
        shifter.set_window_ms(40.0);
        Self {
            shared: shared.clone(),
            s: Smoothed::new(sr, &shared.params),
            sr,
            lowcut: Biquad::default(),
            dc_in: DcBlock::default(),
            lp: Default::default(),
            hp: Default::default(),
            chest: Biquad::default(),
            weight: Biquad::default(),
            presence: Biquad::default(),
            low_sat: Saturator::default(),
            high_sat: Saturator::default(),
            dc_low: DcBlock::default(),
            dc_high: DcBlock::default(),
            align: DelayLine::new(saturate::LATENCY + 8),
            comp: Envelope::new(sr, 3.0, 80.0),
            reflect: DelayLine::new((0.04 * 2.0 * sr) as usize + 8),
            modes: Default::default(),
            tank: TANK.map(|ms| DelayLine::new((ms * 0.001 * sr) as usize + 8)),
            tank_lp: [0.0; 8],
            shifter,
            sub_lp: Default::default(),
            ring_phase: 0.0,
            ring_bp: [Biquad::highpass(sr, 150.0, 0.707), Biquad::default()],
            dc_ring: DcBlock::default(),
            air_hp: Default::default(),
            rng: Rng::new(seed),
            hash_env: Envelope::new(sr, 5.0, 60.0),
            hash_hp: Default::default(),
        }
    }

    /// One sample of the FDN: Householder feedback, a one-pole low-pass and decay gain per line.
    fn tank(&mut self, x: f32, t60_ms: f32, damp: f32) -> f32 {
        let mut taps = [0f32; 8];
        for (t, (line, ms)) in taps.iter_mut().zip(self.tank.iter().zip(TANK)) {
            *t = line.read_delay(ms * 0.001 * self.sr);
        }
        let mut out = 0.0;
        for (i, ms) in TANK.into_iter().enumerate() {
            out += if i % 2 == 0 { taps[i] } else { -taps[i] };
            self.tank_lp[i] = taps[i] + damp * (self.tank_lp[i] - taps[i]);
            taps[i] = 10f32.powf(-3.0 * ms / t60_ms.max(1.0)) * self.tank_lp[i];
        }
        let mean = 2.0 / 8.0 * taps.iter().sum::<f32>();
        for (i, line) in self.tank.iter_mut().enumerate() {
            line.push(if i % 2 == 0 { x } else { -x } + taps[i] - mean);
        }
        out * 0.35
    }

    fn update(&mut self) {
        let (sr, s) = (self.sr, &self.s);
        self.lowcut.set_highpass(sr, s.get(P::DreadLowCut), 0.707);
        let x = s.get(P::Xover);
        for f in &mut self.lp {
            f.set_lowpass(sr, x, 0.707);
        }
        for f in &mut self.hp {
            f.set_highpass(sr, x, 0.707);
        }
        self.chest.set_peak(sr, s.get(P::ChestFreq), 1.0, s.get(P::ChestGain));
        self.weight.set_peak(sr, s.get(P::WeightFreq), 0.8, s.get(P::Weight));
        self.presence.set_peak(sr, 3000.0, 0.7, s.get(P::Presence));
        let base = s.get(P::HullFreq);
        for (f, r) in self.modes.iter_mut().zip(MODES) {
            f.set_bandpass(sr, base * r, 25.0);
        }
        for f in &mut self.sub_lp {
            f.set_lowpass(sr, 250.0, 0.707);
        }
        self.ring_bp[1].set_lowpass(sr, 3000.0, 0.707);
        for f in &mut self.hash_hp {
            f.set_highpass(sr, 2500.0, 0.707);
        }
        for f in &mut self.air_hp {
            f.set_highpass(sr, 3500.0, 0.707);
        }
    }
}

fn cascade(filters: &mut [Biquad], x: f32) -> f32 {
    filters.iter_mut().fold(x, |x, f| f.process(x))
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

impl Module for Dread {
    fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            self.update();
            let s = &self.s;
            let low_drive = db(s.get(P::LowDrive));
            let low_makeup = 1.0 / low_drive.sqrt();
            let low_bias = s.get(P::LowBias);
            let high_drive = db(s.get(P::HighDrive));
            let high_makeup = db(s.get(P::HighGain)) / high_drive.sqrt();
            let thresh = s.get(P::CompThresh);
            let slope = 1.0 - 1.0 / s.get(P::CompRatio);
            let hash = 0.5 * s.get(P::Hash);
            let er_mix = s.get(P::ErMix);
            let er_size = s.get(P::ErSize);
            let hull_mix = s.get(P::HullMix);
            let tank_mix = s.get(P::TankMix);
            let tank_t60 = s.get(P::TankT60);
            let tank_damp = s.get(P::TankDamp);
            let sub_mix = s.get(P::SubMix);
            let ring_mix = s.get(P::RingMix);
            let ring_inc = 2.0 * PI * s.get(P::RingHz) / self.sr;
            let air = if s.get(P::Air) <= -59.9 { 0.0 } else { db(s.get(P::Air)) };
            let out = db(s.get(P::DreadOut));
            for i in s0..s1 {
                let x = self.dc_in.process(self.lowcut.process(0.5 * (left[i] + right[i])));

                let low = cascade(&mut self.lp, x);
                let high = cascade(&mut self.hp, x);
                let low = self.chest.process(low);
                let low = self.dc_low.process(self.low_sat.process(low_drive * low, low_bias)) * low_makeup;
                let high = self.dc_high.process(self.high_sat.process(high_drive * high, 0.0)) * high_makeup;
                let low = self.weight.process(low);
                let high = self.presence.process(high);
                let mut y = low + high;

                self.align.push(x);
                let xa = self.align.read_delay(saturate::LATENCY as f32 + 0.25);
                if sub_mix > 0.0 {
                    let sub = self.shifter.process(xa, 0.5);
                    let sub = cascade(&mut self.sub_lp, sub);
                    y += sub_mix * (4.0 * sub).tanh() * 0.5;
                }
                if ring_mix > 0.0 {
                    self.ring_phase = (self.ring_phase + ring_inc) % (2.0 * PI);
                    let r = xa * self.ring_phase.sin() * 3.0;
                    let r = if r > 0.0 { r.tanh() } else { 0.4 * (2.5 * r).tanh() };
                    let r = cascade(&mut self.ring_bp, self.dc_ring.process(r));
                    y += ring_mix * r;
                }

                let level = 20.0 * self.comp.process(y).max(1e-6).log10();
                y *= db(-slope * (level - thresh).max(0.0));

                let env = self.hash_env.process(y);
                if hash > 0.0 {
                    y += hash * cascade(&mut self.hash_hp, self.rng.range(-1.0, 1.0) * env);
                }

                self.reflect.push(y);
                let mut er = 0.0;
                for (ms, g) in TAPS {
                    er += g * self.reflect.read_delay(ms * er_size * 0.001 * self.sr);
                }
                let mut ring = 0.0;
                if hull_mix > 0.0 {
                    for f in &mut self.modes {
                        ring += f.process(y);
                    }
                }
                let tank = if tank_mix > 0.0 { self.tank(y, tank_t60, tank_damp) } else { 0.0 };
                y += er_mix * er + hull_mix * ring + tank_mix * tank;
                y += air * cascade(&mut self.air_hp, xa);

                left[i] = y * out;
                right[i] = y * out;
            }
            s0 = s1;
        }
    }

    fn latency_ms(&self) -> f32 {
        saturate::LATENCY as f32 * 1000.0 / self.sr
    }
}
