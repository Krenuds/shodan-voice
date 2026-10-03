//! The rack host. Runs on the audio thread (or offline for `--render`): no allocation, no locks.
//!
//! mic → input gain → rack modules in order → output gain → bypass crossfade → limiter.

use super::module::{MAX_BLOCK, ModuleCtx, ModuleId, RackModule, SUB_BLOCK, Smoothed};
use crate::dsp::limiter::{Limiter, soft_clip};
use crate::params::{db_to_gain, io};
use crate::shared::Shared;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Most modules the rack can hold; the engine's storage is allocated for this up front.
pub const MAX_MODULES: usize = 16;

pub enum Command {
    /// Append a module. It fades in; follow with `Order` to place it.
    Add(RackModule),
    Remove(ModuleId),
    /// The ids of all modules in playing order (unused tail is 0).
    Order([ModuleId; MAX_MODULES]),
}

/// Lock-free links to the GUI thread for changing the rack.
pub struct RackLink {
    pub commands: rtrb::Consumer<Command>,
    /// Removed modules go back to the GUI thread so they are never freed on the audio thread.
    pub garbage: rtrb::Producer<RackModule>,
}

struct Entry {
    m: RackModule,
    /// Current enable/mix crossfade position.
    mix: f32,
}

pub struct Engine {
    shared: Arc<Shared>,
    sr: f32,
    io: Smoothed,
    entries: Vec<Entry>,
    link: Option<RackLink>,
    limiter: Limiter,
    bypass_mix: f32,

    dry: Vec<f32>,
    wet_l: Vec<f32>,
    wet_r: Vec<f32>,
}

fn target_mix(m: &RackModule) -> f32 {
    if m.shared.enabled.load(Ordering::Relaxed) { m.shared.mix.get() } else { 0.0 }
}

impl Engine {
    pub fn new(sr: f32, shared: Arc<Shared>, modules: Vec<RackModule>, link: Option<RackLink>) -> Self {
        let mut entries = Vec::with_capacity(MAX_MODULES);
        entries.extend(modules.into_iter().take(MAX_MODULES).map(|m| Entry { mix: target_mix(&m), m }));
        Self {
            sr,
            io: Smoothed::new(sr, &shared.io),
            entries,
            link,
            limiter: Limiter::new(sr),
            bypass_mix: 0.0,
            shared,
            dry: vec![0.0; MAX_BLOCK],
            wet_l: vec![0.0; MAX_BLOCK],
            wet_r: vec![0.0; MAX_BLOCK],
        }
    }

    fn handle_commands(&mut self) {
        let Some(link) = self.link.as_mut() else { return };
        // If the garbage queue is somehow full, leaking beats freeing here.
        let mut discard = |mut m: RackModule| {
            m.module.stop();
            if let Err(rtrb::PushError::Full(m)) = link.garbage.push(m) {
                std::mem::forget(m);
            }
        };
        while let Ok(cmd) = link.commands.pop() {
            match cmd {
                Command::Add(m) if self.entries.len() < MAX_MODULES => self.entries.push(Entry { m, mix: 0.0 }),
                Command::Add(m) => discard(m),
                Command::Remove(id) => {
                    if let Some(i) = self.entries.iter().position(|e| e.m.id == id) {
                        discard(self.entries.remove(i).m);
                    }
                }
                Command::Order(ids) => {
                    let mut pos = 0;
                    for id in ids {
                        if let Some(j) = self.entries[pos..].iter().position(|e| e.m.id == id) {
                            // Rotate rather than swap, so modules the list doesn't name keep their order.
                            self.entries[pos..=pos + j].rotate_right(1);
                            pos += 1;
                        }
                    }
                }
            }
        }
    }

    /// Ids of the modules in playing order. Allocates: for tests, not the audio thread.
    pub fn order(&self) -> Vec<ModuleId> {
        self.entries.iter().map(|e| e.m.id).collect()
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

        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.io.update(&self.shared.io);
            let in_gain = db_to_gain(self.io.get(io::P::InGain));
            for i in s0..s1 {
                let x = input[i] * in_gain;
                self.dry[i] = x;
                in_peak = in_peak.max(x.abs());
            }
            s0 = s1;
        }
        out_l.copy_from_slice(&self.dry[..n]);
        out_r.copy_from_slice(&self.dry[..n]);

        let ctx = ModuleCtx { dry: &self.dry[..n] };
        let mut latency = 0.0;
        for e in &mut self.entries {
            let target = target_mix(&e.m);
            let m0 = e.mix;
            e.mix = target;
            if m0 < 1e-4 && target < 1e-4 {
                // Off: the chain passes through unchanged, and its cable still carries it.
                e.m.shared.out_peak.max(peak(out_l, out_r));
                continue;
            }
            latency += e.m.module.latency_ms();
            if m0 >= 1.0 && target >= 1.0 {
                e.m.module.process(&ctx, out_l, out_r);
                e.m.shared.out_peak.max(peak(out_l, out_r));
                continue;
            }
            let (wl, wr) = (&mut self.wet_l[..n], &mut self.wet_r[..n]);
            wl.copy_from_slice(out_l);
            wr.copy_from_slice(out_r);
            e.m.module.process(&ctx, wl, wr);
            let step = (target - m0) / n as f32;
            for k in 0..n {
                let m = m0 + step * k as f32;
                out_l[k] += (wl[k] - out_l[k]) * m;
                out_r[k] += (wr[k] - out_r[k]) * m;
            }
            e.m.shared.out_peak.max(peak(out_l, out_r));
        }

        // Output stage: gain, bypass crossfade, limiter. Never a dry/wet blend.
        let out_gain = db_to_gain(self.io.get(io::P::OutGain));
        let bypass = if self.io.get(io::P::Bypass) > 0.5 { 1.0 } else { 0.0 };
        let bypass_step = 1.0 / (0.02 * self.sr);
        let mut out_peak = 0f32;
        for i in 0..n {
            self.bypass_mix += (bypass - self.bypass_mix).clamp(-bypass_step, bypass_step);
            let d = self.dry[i];
            let fx_l = out_l[i] * out_gain;
            let fx_r = out_r[i] * out_gain;
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
        m.dsp_ms.set(latency);
    }
}

fn peak(l: &[f32], r: &[f32]) -> f32 {
    l.iter().chain(r).fold(0.0, |p, x| p.max(x.abs()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::module::{Module, ModuleShared};
    use crate::dsp::rng::Rng;
    use crate::modules;
    use crate::presets;

    #[test]
    fn every_preset_stays_finite_and_below_full_scale() {
        for preset in presets::BUILTIN {
            let shared = Arc::new(Shared::default());
            let rack = modules::default_instances();
            preset.apply(&presets::targets(&shared.io, rack.iter().map(|i| &i.shared.params)));
            let sr = 48000.0;
            let mut engine = Engine::new(sr, shared.clone(), modules::build(&rack, sr, 7, &shared), None);
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

    /// Multiplies by ten and adds its tag, so the output spells out the processing order.
    struct Tag(f32);
    impl Module for Tag {
        fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
            for x in left.iter_mut().chain(right) {
                *x = *x * 10.0 + self.0;
            }
        }
    }

    fn tag(id: ModuleId) -> RackModule {
        RackModule { id, module: Box::new(Tag(id as f32)), shared: ModuleShared::new(&[]) }
    }

    #[test]
    fn commands_add_reorder_and_remove_modules() {
        let shared = Arc::new(Shared::default());
        let (mut tx, commands) = rtrb::RingBuffer::new(8);
        let (garbage, mut garbage_rx) = rtrb::RingBuffer::new(8);
        let mut engine = Engine::new(48000.0, shared, vec![tag(1), tag(2)], Some(RackLink { commands, garbage }));
        let order = |engine: &mut Engine| {
            // Two blocks: a module added by a command fades in over the first one.
            let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
            engine.process(&[0.0; 64], &mut l, &mut r);
            engine.process(&[0.0; 64], &mut l, &mut r);
            engine.order()
        };
        assert_eq!(order(&mut engine), [1, 2]);

        let mut ids = [0; MAX_MODULES];
        ids[..3].copy_from_slice(&[3, 1, 2]);
        assert!(tx.push(Command::Add(tag(3))).is_ok());
        assert!(tx.push(Command::Order(ids)).is_ok());
        assert_eq!(order(&mut engine), [3, 1, 2]);
        // Silence in: 0 → 3 → 31 → 312, far above full scale, so the limiter has clamped it.
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        engine.process(&[0.0; 64], &mut l, &mut r);
        assert!(l[63] > 0.5 && l[63] <= 1.0);

        assert!(tx.push(Command::Remove(1)).is_ok());
        assert_eq!(order(&mut engine), [3, 2]);
        assert_eq!(garbage_rx.pop().map(|m| m.id).ok(), Some(1));
    }
}
