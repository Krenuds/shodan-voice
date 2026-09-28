//! The SHODAN effect chain. Runs on the audio thread (or offline for `--render`):
//! no allocation, no locks.

use crate::dsp::filters::{Biquad, DcBlock, Envelope, one_pole_coef};
use crate::dsp::flanger::{Flanger, FlangerSettings};
use crate::dsp::limiter::{Limiter, soft_clip};
use crate::dsp::lofi::{Lofi, LofiSettings};
use crate::dsp::onset::Onset;
use crate::dsp::pitch::JumpSettings;
use crate::dsp::rng::Rng;
use crate::dsp::tape::{Glitch, Tape};
use crate::dsp::voices::{VoiceSettings, Voices};
use crate::params::{COUNT, DEFS, Kind, P, db_to_gain};
use crate::shared::{SLOT_COUNT, Shared};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const SUB_BLOCK: usize = 32;
const MAX_BLOCK: usize = 1024;

/// A stereo effect hosted in a plugin slot (implemented by the CLAP host).
pub trait SlotProcessor: Send {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Called on the audio thread just before the processor is handed back for disposal.
    fn stop(&mut self) {}
}

pub enum Command {
    SetSlot(usize, Option<Box<dyn SlotProcessor>>),
}

/// Lock-free links to the GUI thread for swapping plugin processors.
pub struct SlotLink {
    pub commands: rtrb::Consumer<Command>,
    /// Old processors go back to the GUI thread so they are never freed on the audio thread.
    pub garbage: rtrb::Producer<Box<dyn SlotProcessor>>,
}

pub struct Engine {
    shared: Arc<Shared>,
    sr: f32,
    rng: Rng,
    smoothed: [f32; COUNT],
    smooth_coef: f32,

    dc: DcBlock,
    hp: Biquad,
    gate_env: Envelope,
    gate_gain: f32,
    gate_att: f32,
    gate_rel: f32,
    onset: Onset,
    tape: Tape,
    voices: Voices,
    flangers: [Flanger; 2],
    lofis: [Lofi; 2],
    limiter: Limiter,
    bypass_mix: f32,
    slot_mix: [f32; SLOT_COUNT],

    slots: [Option<Box<dyn SlotProcessor>>; SLOT_COUNT],
    link: Option<SlotLink>,

    dry: Vec<f32>,
    mono: Vec<f32>,
    slot_l: Vec<f32>,
    slot_r: Vec<f32>,
}

impl Engine {
    pub fn new(sr: f32, shared: Arc<Shared>, seed: u64, link: Option<SlotLink>) -> Self {
        let mut rng = Rng::new(seed);
        let smoothed = shared.params.snapshot();
        Self {
            sr,
            smoothed,
            smooth_coef: one_pole_coef(sr / SUB_BLOCK as f32, 25.0),
            dc: DcBlock::default(),
            hp: Biquad::highpass(sr, 90.0, 0.707),
            gate_env: Envelope::new(sr, 1.0, 80.0),
            gate_gain: 0.0,
            gate_att: one_pole_coef(sr, 2.0),
            gate_rel: one_pole_coef(sr, 150.0),
            onset: Onset::new(sr),
            tape: Tape::new(sr),
            voices: Voices::new(sr, &mut rng),
            flangers: [Flanger::new(sr, 0.0), Flanger::new(sr, 0.25)],
            lofis: [Lofi::new(sr), Lofi::new(sr)],
            limiter: Limiter::new(sr),
            bypass_mix: 0.0,
            slot_mix: [0.0; SLOT_COUNT],
            slots: [None, None],
            link,
            rng,
            shared,
            dry: vec![0.0; MAX_BLOCK],
            mono: vec![0.0; MAX_BLOCK],
            slot_l: vec![0.0; MAX_BLOCK],
            slot_r: vec![0.0; MAX_BLOCK],
        }
    }

    fn p(&self, p: P) -> f32 {
        self.smoothed[p as usize]
    }

    fn update_params(&mut self) {
        for (i, d) in DEFS.iter().enumerate() {
            let target = self.shared.params.get_index(i);
            self.smoothed[i] = if d.kind == Kind::Continuous {
                target + self.smooth_coef * (self.smoothed[i] - target)
            } else {
                target
            };
        }
    }

    fn handle_commands(&mut self) {
        let Some(link) = self.link.as_mut() else { return };
        while let Ok(cmd) = link.commands.pop() {
            match cmd {
                Command::SetSlot(i, proc) => {
                    if let Some(mut old) = std::mem::replace(&mut self.slots[i], proc) {
                        old.stop();
                        // If the garbage queue is somehow full, leaking beats freeing here.
                        if let Err(rtrb::PushError::Full(old)) = link.garbage.push(old) {
                            std::mem::forget(old);
                        }
                    }
                    self.slot_mix[i] = 0.0;
                }
            }
        }
    }

    /// Process mono `input` into stereo output. All slices must have the same length.
    pub fn process(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        disable_denormals();
        self.handle_commands();
        let mut start = 0;
        while start < input.len() {
            let end = (start + MAX_BLOCK).min(input.len());
            self.process_block(&input[start..end], &mut out_l[start..end], &mut out_r[start..end]);
            start = end;
        }
    }

    fn process_block(&mut self, input: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = input.len();
        let mut in_peak = 0f32;
        let mut glitches = 0;

        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.update_params();
            let in_gain = db_to_gain(self.p(P::InGain));
            let gate = db_to_gain(self.p(P::Gate));
            let sensitivity = self.p(P::Sensitivity);
            let catch_up = self.p(P::CatchUp);

            let mut onset_in_block = false;
            for i in s0..s1 {
                let x = input[i] * in_gain;
                self.dry[i] = x;
                in_peak = in_peak.max(x.abs());
                let x = self.hp.process(self.dc.process(x));

                let env = self.gate_env.process(x);
                let target = if env > gate { 1.0 } else { 0.0 };
                let c = if target > self.gate_gain { self.gate_att } else { self.gate_rel };
                self.gate_gain = target + c * (self.gate_gain - target);
                let x = x * self.gate_gain;

                let y = self.tape.process(x, catch_up);
                if self.onset.process(x, sensitivity, gate) {
                    onset_in_block = true;
                    if self.fire_glitch() {
                        glitches += 1;
                    }
                }
                self.mono[i] = y;
            }

            let voiced = self.gate_env.value > gate;
            let vs = VoiceSettings {
                base_pitch: self.p(P::BasePitch),
                jump: JumpSettings { chance: self.p(P::JumpChance), range: self.p(P::JumpRange), glide_ms: self.p(P::Glide) },
                layers: self.p(P::Layers) as usize,
                detune_cents: self.p(P::Detune),
                spread: self.p(P::Spread),
                chaos: self.p(P::Chaos),
                level: self.p(P::LayerLevel),
            };
            self.voices.process(&self.mono[s0..s1], &mut out_l[s0..s1], &mut out_r[s0..s1], onset_in_block, voiced, &vs);

            let fs = FlangerSettings {
                depth: self.p(P::FlangeDepth),
                rate_hz: self.p(P::FlangeRate),
                feedback: self.p(P::FlangeFb),
                mix: self.p(P::FlangeMix),
            };
            let ls = LofiSettings { rate_hz: self.p(P::LofiRate), bits: self.p(P::LofiBits), tone: self.p(P::LofiTone) };
            for (ch, out) in [&mut *out_l, &mut *out_r].into_iter().enumerate() {
                self.lofis[ch].update(&ls);
                for y in &mut out[s0..s1] {
                    *y = self.lofis[ch].process(self.flangers[ch].process(*y, &fs), &ls);
                }
            }
            s0 = s1;
        }

        self.run_slots(out_l, out_r);

        // Output stage: dry/wet, gain, bypass crossfade, limiter.
        let wet = self.p(P::DryWet);
        let out_gain = db_to_gain(self.p(P::OutGain));
        let bypass = if self.p(P::Bypass) > 0.5 { 1.0 } else { 0.0 };
        let bypass_step = 1.0 / (0.02 * self.sr);
        let mut out_peak = 0f32;
        for i in 0..n {
            self.bypass_mix += (bypass - self.bypass_mix).clamp(-bypass_step, bypass_step);
            let d = self.dry[i];
            let fx_l = (d + (out_l[i] - d) * wet) * out_gain;
            let fx_r = (d + (out_r[i] - d) * wet) * out_gain;
            let l = fx_l + (d - fx_l) * self.bypass_mix;
            let r = fx_r + (d - fx_r) * self.bypass_mix;
            let g = self.limiter.gain(l.abs().max(r.abs()));
            out_l[i] = soft_clip(l * g);
            out_r[i] = soft_clip(r * g);
            out_peak = out_peak.max(out_l[i].abs()).max(out_r[i].abs());
        }

        let m = &self.shared.meters;
        m.input_peak.max(in_peak);
        m.output_peak.max(out_peak);
        m.pitch.set(self.p(P::BasePitch) + self.voices.offset);
        m.lag_ms.set(self.tape.lag() as f32 / self.sr * 1000.0);
        if glitches > 0 {
            m.glitches.fetch_add(glitches, Ordering::Relaxed);
        }
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

    fn run_slots(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        for i in 0..SLOT_COUNT {
            let Some(proc) = self.slots[i].as_mut() else { continue };
            let sh = &self.shared.slots[i];
            let target = if sh.enabled.load(Ordering::Relaxed) { sh.mix.get() } else { 0.0 };
            let m0 = self.slot_mix[i];
            if m0 < 1e-4 && target < 1e-4 {
                self.slot_mix[i] = 0.0;
                continue;
            }
            let (sl, sr) = (&mut self.slot_l[..n], &mut self.slot_r[..n]);
            sl.copy_from_slice(out_l);
            sr.copy_from_slice(out_r);
            proc.process(sl, sr);
            let step = (target - m0) / n as f32;
            for k in 0..n {
                let m = m0 + step * k as f32;
                let (pl, pr) = (sanitize(sl[k]), sanitize(sr[k]));
                out_l[k] += (pl - out_l[k]) * m;
                out_r[k] += (pr - out_r[k]) * m;
            }
            self.slot_mix[i] = target;
        }
    }
}

/// Set flush-to-zero / denormals-are-zero for this thread so decaying filters stay fast.
fn disable_denormals() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr);
        csr |= 0x8040;
        std::arch::asm!("ldmxcsr [{}]", in(reg) &csr);
    }
}

/// Plugins are outside our control: never let NaN/inf through to the limiter.
fn sanitize(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-4.0, 4.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;

    #[test]
    fn every_preset_stays_finite_and_below_full_scale() {
        for preset in presets::BUILTIN {
            let shared = Arc::new(Shared::default());
            preset.apply(&shared.params);
            let sr = 48000.0;
            let mut engine = Engine::new(sr, shared, 7, None);
            let mut rng = Rng::new(1);
            let n = 480;
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            for block in 0..400 {
                // Loud, bursty "speech": syllable-shaped tone bursts plus noise.
                let input: Vec<f32> = (0..n)
                    .map(|i| {
                        let t = (block * n + i) as f32 / sr;
                        let env = if (t * 5.0).fract() < 0.6 { 1.5 } else { 0.0 };
                        env * ((t * 180.0 * std::f32::consts::TAU).sin() + rng.range(-0.3, 0.3))
                    })
                    .collect();
                engine.process(&input, &mut l, &mut r);
                for &y in l.iter().chain(&r) {
                    assert!(y.is_finite(), "{}: non-finite output", preset.name);
                    assert!(y.abs() <= 1.0, "{}: output {y} above 0 dBFS", preset.name);
                    assert!(y == 0.0 || y.abs() > 1e-30, "{}: denormal", preset.name);
                }
            }
        }
    }
}
