//! The main pitch-shifted voice plus up to four detuned, delayed, independently jumping copies.

use super::pitch::{JumpSettings, Jumper, Shifter, semis_to_ratio};
use super::rng::Rng;
use std::f32::consts::FRAC_PI_4;

pub const MAX_LAYERS: usize = 4;
const LAYER_DELAY_MS: [f32; MAX_LAYERS] = [9.0, 16.0, 23.0, 31.0];
const LAYER_DETUNE: [f32; MAX_LAYERS] = [1.0, -1.0, 0.55, -0.6];
const LAYER_PAN: [f32; MAX_LAYERS] = [-0.8, 0.8, -0.45, 0.45];

struct Layer {
    shifter: Shifter,
    jumper: Jumper,
    own_pitch: bool,
    gain: f32,
}

pub struct VoiceSettings {
    pub base_pitch: f32,
    pub jump: JumpSettings,
    pub layers: usize,
    pub detune_cents: f32,
    pub spread: f32,
    pub chaos: f32,
    pub level: f32,
    pub grain_ms: f32,
    /// Extra pitch ratio from the robot (applies to every voice).
    pub robot_ratio: f32,
}

pub struct Voices {
    main: Shifter,
    main_jump: Jumper,
    layers: Vec<Layer>,
    rng: Rng,
    sr: f32,
    /// Last main-voice pitch offset, for the GUI.
    pub offset: f32,
}

impl Voices {
    pub fn new(sr: f32, rng: &mut Rng) -> Self {
        Self {
            main: Shifter::new(sr, 40.0, 0.0),
            main_jump: Jumper::new(sr, rng.fork()),
            layers: (0..MAX_LAYERS)
                .map(|_| Layer {
                    shifter: Shifter::new(sr, 40.0, 40.0),
                    jumper: Jumper::new(sr, rng.fork()),
                    own_pitch: false,
                    gain: 0.0,
                })
                .collect(),
            rng: rng.fork(),
            sr,
            offset: 0.0,
        }
    }

    /// Process one sub-block (mono in, stereo out). `onset` = a syllable started in this block.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32], onset: bool, voiced: bool, s: &VoiceSettings) {
        let n = input.len() as u32;
        let offset = self.main_jump.update(n, onset, voiced, &s.jump);
        self.offset = offset;
        let main_ratio = semis_to_ratio(s.base_pitch + offset) * s.robot_ratio;
        self.main.set_window_ms(s.grain_ms);
        for (i, &x) in input.iter().enumerate() {
            let y = self.main.process(x, main_ratio);
            out_l[i] = y;
            out_r[i] = y;
        }

        let follow = JumpSettings { chance: 0.0, range: s.jump.range, glide_ms: s.jump.glide_ms };
        let mut power = 1.0;
        for (k, layer) in self.layers.iter_mut().enumerate() {
            let target_gain = if k < s.layers { s.level } else { 0.0 };
            if onset {
                layer.own_pitch = self.rng.chance(s.chaos);
                if layer.own_pitch {
                    layer.jumper.jump(s.jump.range);
                }
            }
            if !layer.own_pitch {
                layer.jumper.set_target(self.main_jump.target());
            }
            let layer_offset = layer.jumper.update(n, false, voiced, &follow);

            layer.shifter.set_window_ms(s.grain_ms);
            let g0 = layer.gain;
            layer.gain += (target_gain - layer.gain) * 0.05;
            if g0 < 1e-4 && layer.gain < 1e-4 {
                // Keep the delay line fed so a layer fades in cleanly.
                for &x in input {
                    layer.shifter.process(x, 1.0);
                }
                continue;
            }
            power += layer.gain * layer.gain;

            let ratio = semis_to_ratio(s.base_pitch + layer_offset + LAYER_DETUNE[k] * s.detune_cents / 100.0) * s.robot_ratio;
            layer.shifter.extra_delay = (1.0 + LAYER_DELAY_MS[k] * s.spread) * 0.001 * self.sr;
            let angle = (LAYER_PAN[k] * s.spread + 1.0) * FRAC_PI_4;
            let (pl, pr) = (angle.cos() * std::f32::consts::SQRT_2, angle.sin() * std::f32::consts::SQRT_2);
            let step = (layer.gain - g0) / n as f32;
            for (i, &x) in input.iter().enumerate() {
                let g = g0 + step * i as f32;
                let y = layer.shifter.process(x, ratio) * g;
                out_l[i] += y * pl;
                out_r[i] += y * pr;
            }
        }
        let norm = 1.0 / power.sqrt();
        for i in 0..input.len() {
            out_l[i] *= norm;
            out_r[i] *= norm;
        }
    }
}
