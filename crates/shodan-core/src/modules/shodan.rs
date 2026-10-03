//! SHODAN core: gate, glitch tape, pitch-shifted voices and formants. Mono in, stereo out.

use super::ModuleKind;
use crate::audio::module::{MAX_BLOCK, Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::dsp::filters::{Biquad, DcBlock, Envelope, one_pole_coef};
use crate::dsp::lpc::{Analyzer, Synthesizer, midi_to_hz};
use crate::dsp::onset::Onset;
use crate::dsp::pitch::JumpSettings;
use crate::dsp::rng::Rng;
use crate::dsp::tape::{Glitch, Tape};
use crate::dsp::voices::{VoiceSettings, Voices};
use crate::params::db_to_gain;
use crate::shared::Shared;
use std::sync::Arc;
use std::sync::atomic::Ordering;

crate::params::define_params! {
    Gate          => ("gate", "Gate", "Input", -80.0, -20.0, -55.0, "dB", Continuous, "Noise gate threshold. Also stops glitches firing on room noise."),

    Formant       => ("formant", "Formant", "Voice", -12.0, 12.0, 0.0, "st", Continuous, "Throat size, independent of pitch. Negative = bigger/darker, positive = smaller/brighter."),
    Robot         => ("robot", "Robot", "Voice", 0.0, 1.0, 0.0, "", Continuous, "Flattens your intonation towards Note. 0 = natural, 100% = dead monotone machine."),
    Note          => ("note", "Note", "Voice", 36.0, 72.0, 55.0, "note", Stepped, "The pitch Robot pulls towards."),

    Sensitivity   => ("sensitivity", "Sensitivity", "Glitch", 0.0, 1.0, 0.6, "", Continuous, "How easily a syllable start is detected."),
    StutterChance => ("stutter_chance", "Stutter", "Glitch", 0.0, 1.0, 0.35, "", Continuous, "Chance a syllable gets stuttered: L-l-look at you."),
    Repeats       => ("repeats", "Repeats", "Glitch", 1.0, 5.0, 2.0, "x", Stepped, "Extra repeats of a stuttered slice."),
    Slice         => ("slice", "Slice", "Glitch", 40.0, 250.0, 90.0, "ms", Continuous, "Length of the stuttered/reversed fragment."),
    ReverseChance => ("reverse_chance", "Reverse", "Glitch", 0.0, 1.0, 0.08, "", Continuous, "Chance a syllable fragment is played backwards."),
    WarpChance    => ("warp_chance", "Warp", "Glitch", 0.0, 1.0, 0.12, "", Continuous, "Chance a syllable is dragged at slow tape speed."),
    CatchUp       => ("catch_up", "Catch-up", "Glitch", 0.0, 1.0, 0.55, "", Continuous, "0 = skip back to live. Higher = sped-up tape rush back to live."),

    BasePitch     => ("base_pitch", "Pitch", "Pitch", -12.0, 12.0, 0.0, "st", Continuous, "Constant pitch shift."),
    JumpRange     => ("jump_range", "Jump range", "Pitch", 0.0, 12.0, 4.0, "st", Continuous, "Max size of random pitch jumps."),
    JumpChance    => ("jump_chance", "Jumps", "Pitch", 0.0, 1.0, 0.4, "", Continuous, "How often the pitch jumps."),
    Glide         => ("glide", "Glide", "Pitch", 0.0, 200.0, 0.0, "ms", Continuous, "0 = hard snaps. Higher = slides between pitches."),
    Grain         => ("grain", "Grain", "Pitch", 12.0, 40.0, 20.0, "ms", Continuous, "Pitch-shifter window. The voice is delayed by about half of this: shorter = less delay, rougher/buzzier sound."),

    Layers        => ("layers", "Layers", "Voices", 0.0, 4.0, 2.0, "", Stepped, "Extra copies of the voice."),
    Detune        => ("detune", "Detune", "Voices", 0.0, 60.0, 18.0, "ct", Continuous, "How out of tune the copies are."),
    Spread        => ("spread", "Spread", "Voices", 0.0, 1.0, 0.45, "", Continuous, "Time offset and stereo width of the copies."),
    Chaos         => ("chaos", "Chaos", "Voices", 0.0, 1.0, 0.3, "", Continuous, "How often copies jump to their own pitch (SS2 style)."),
    LayerLevel    => ("layer_level", "Level", "Voices", 0.0, 1.0, 0.6, "", Continuous, "Loudness of the copies."),
}

pub const KIND: ModuleKind =
    ModuleKind { id: "shodan_core", name: "SHODAN Core", defs: DEFS, make: |sr, seed, shared, app| Box::new(Core::new(sr, seed, shared.clone(), app.clone())) };

pub struct Core {
    shared: Arc<ModuleShared>,
    app: Arc<Shared>,
    sr: f32,
    rng: Rng,
    s: Smoothed,

    dc: DcBlock,
    hp: Biquad,
    gate_env: Envelope,
    gate_gain: f32,
    gate_att: f32,
    gate_rel: f32,
    onset: Onset,
    analyzer: Analyzer,
    synth: Synthesizer,
    robot_ratio: f32,
    tape: Tape,
    voices: Voices,

    mono: Vec<f32>,
}

impl Core {
    pub fn new(sr: f32, seed: u64, shared: Arc<ModuleShared>, app: Arc<Shared>) -> Self {
        let mut rng = Rng::new(seed);
        Self {
            s: Smoothed::new(sr, &shared.params),
            sr,
            dc: DcBlock::default(),
            hp: Biquad::highpass(sr, 90.0, 0.707),
            gate_env: Envelope::new(sr, 1.0, 80.0),
            gate_gain: 0.0,
            gate_att: one_pole_coef(sr, 2.0),
            gate_rel: one_pole_coef(sr, 150.0),
            onset: Onset::new(sr),
            analyzer: Analyzer::new(sr),
            synth: Synthesizer::new(sr),
            robot_ratio: 1.0,
            tape: Tape::new(sr),
            voices: Voices::new(sr, &mut rng),
            rng,
            shared,
            app,
            mono: vec![0.0; MAX_BLOCK],
        }
    }

    fn p(&self, p: P) -> f32 {
        self.s.get(p)
    }

    /// Robot: steer the pitch of what the shifters are about to play towards the Note.
    /// 0 = natural intonation, 1 = dead monotone. Unvoiced sounds keep the last ratio.
    fn update_robot(&mut self, grain_delay: f64) {
        let amount = self.p(P::Robot);
        let target = if amount < 0.001 {
            1.0
        } else {
            // The pitch tracker looks ~15 ms into the past, so read the model a little ahead.
            let lookahead = 0.015 * self.sr as f64;
            let f0 = self.analyzer.frame_at(self.tape.position() - grain_delay + lookahead).f0;
            if f0 <= 0.0 {
                return;
            }
            (midi_to_hz(self.p(P::Note)) / f0).powf(amount).clamp(0.25, 4.0)
        };
        let (cur, tgt) = (self.robot_ratio.ln(), target.ln());
        self.robot_ratio = (cur + (tgt - cur) * 0.35).exp();
    }

    /// Roll the dice for a glitch at a syllable start. Returns true if one fired.
    fn fire_glitch(&mut self) -> bool {
        if self.tape.busy() || self.tape.lag() > 0.3 * self.sr as f64 {
            return false;
        }
        let slice = (self.p(P::Slice) * 0.001 * self.sr) as f64;
        let glitch = if self.rng.chance(self.p(P::StutterChance)) {
            Glitch::Stutter { slice, repeats: self.p(P::Repeats) as u32 }
        } else if self.rng.chance(self.p(P::ReverseChance)) {
            Glitch::Reverse { slice }
        } else if self.rng.chance(self.p(P::WarpChance)) {
            let rate = self.rng.range(0.62, 0.82) as f64;
            Glitch::Warp { rate, len: (slice * 1.5 + 0.1 * self.sr as f64) as u32 }
        } else {
            return false;
        };
        self.tape.trigger(glitch);
        true
    }
}

impl Module for Core {
    fn process(&mut self, _ctx: &ModuleCtx, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        let mut glitches = 0;

        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            let gate = db_to_gain(self.p(P::Gate));
            let sensitivity = self.p(P::Sensitivity);
            let catch_up = self.p(P::CatchUp);

            let mut onset_in_block = false;
            for i in s0..s1 {
                let x = (out_l[i] + out_r[i]) * 0.5;
                let x = self.hp.process(self.dc.process(x));

                let env = self.gate_env.process(x);
                let target = if env > gate { 1.0 } else { 0.0 };
                let c = if target > self.gate_gain { self.gate_att } else { self.gate_rel };
                self.gate_gain = target + c * (self.gate_gain - target);
                let x = x * self.gate_gain;

                // Split off the vocal-tract model; everything until the synthesizer works on
                // the residual, so glitches and pitch moves keep the formants where they are.
                let e = self.analyzer.process(x);
                let y = self.tape.process(e, catch_up);
                if self.onset.process(x, sensitivity, gate) {
                    onset_in_block = true;
                    if self.fire_glitch() {
                        glitches += 1;
                    }
                }
                self.mono[i] = y;
            }

            let voiced = self.gate_env.value > gate;
            let grain_delay = (self.p(P::Grain) * 0.0005 * self.sr) as f64;
            self.update_robot(grain_delay);
            let vs = VoiceSettings {
                base_pitch: self.p(P::BasePitch),
                jump: JumpSettings { chance: self.p(P::JumpChance), range: self.p(P::JumpRange), glide_ms: self.p(P::Glide) },
                layers: self.p(P::Layers) as usize,
                detune_cents: self.p(P::Detune),
                spread: self.p(P::Spread),
                chaos: self.p(P::Chaos),
                level: self.p(P::LayerLevel),
                grain_ms: self.p(P::Grain),
                robot_ratio: self.robot_ratio,
            };
            self.voices.process(&self.mono[s0..s1], &mut out_l[s0..s1], &mut out_r[s0..s1], onset_in_block, voiced, &vs);

            // Put the (formant-shifted) throat back, using the model of the audio the shifter
            // is currently playing (about half a grain behind the tape head).
            let formant = 2f32.powf(self.p(P::Formant) / 12.0);
            for i in s0..s1 {
                if self.synth.needs_frame() {
                    let frame = self.analyzer.frame_at(self.tape.position() - grain_delay);
                    self.synth.set_frame(&frame, formant);
                }
                (out_l[i], out_r[i]) = self.synth.process(out_l[i], out_r[i]);
            }
            s0 = s1;
        }

        let m = &self.app.meters;
        m.pitch.set(self.p(P::BasePitch) + self.voices.offset);
        m.lag_ms.set(self.tape.lag() as f32 / self.sr * 1000.0);
        if glitches > 0 {
            m.glitches.fetch_add(glitches, Ordering::Relaxed);
        }
    }

    fn latency_ms(&self) -> f32 {
        self.p(P::Grain) * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::engine::Engine;
    use crate::dsp::lpc::Analyzer;
    use crate::modules::{self, Instance, lofi, metal};

    fn pitch_after_engine(input_hz: f32, robot: f32) -> f32 {
        let sr = 48000.0;
        let app = Arc::new(Shared::default());
        let rack = modules::default_instances();
        let [core, metal, lofi]: &[Instance; 3] = rack.as_slice().try_into().unwrap();
        for (param, v) in [
            (P::StutterChance, 0.0),
            (P::ReverseChance, 0.0),
            (P::WarpChance, 0.0),
            (P::JumpChance, 0.0),
            (P::Layers, 0.0),
            (P::Robot, robot),
            (P::Note, 57.0), // A3, 220 Hz
        ] {
            core.shared.params.set(param, v);
        }
        metal.shared.params.set(metal::P::FlangeMix, 0.0);
        lofi.shared.params.set(lofi::P::LofiRate, 48000.0);
        lofi.shared.params.set(lofi::P::LofiBits, 16.0);
        lofi.shared.params.set(lofi::P::LofiTone, 0.0);
        let mut engine = Engine::new(sr, app.clone(), modules::build(&rack, sr, 1, &app), None);
        let mut tracker = Analyzer::new(sr);
        let n = 480;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        let period = sr / input_hz;
        let mut phase = 0.0f32;
        let mut filt = [Biquad::default(), Biquad::default()];
        filt[0].set_lowpass(sr, 800.0, 5.0);
        filt[1].set_lowpass(sr, 1500.0, 4.0);
        for _ in 0..150 {
            let input: Vec<f32> = (0..n)
                .map(|_| {
                    phase += 1.0;
                    let pulse = if phase >= period {
                        phase -= period;
                        1.0
                    } else {
                        0.0
                    };
                    let y = filt[0].process(pulse);
                    0.4 * filt[1].process(y)
                })
                .collect();
            engine.process(&input, &mut l, &mut r);
            for &y in &l {
                tracker.process(y);
            }
        }
        tracker.f0()
    }

    #[test]
    fn robot_flattens_pitch_to_the_note() {
        for input_hz in [150.0, 300.0] {
            let natural = pitch_after_engine(input_hz, 0.0);
            let robot = pitch_after_engine(input_hz, 1.0);
            println!("{input_hz} Hz in: natural {natural:.1} Hz, robot {robot:.1} Hz");
            assert!((natural - input_hz).abs() / input_hz < 0.04, "natural pitch changed: {natural}");
            assert!((robot - 220.0).abs() / 220.0 < 0.04, "robot did not reach 220 Hz: {robot}");
        }
    }
}
