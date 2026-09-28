//! Knob definitions and the lock-free parameter store shared by the GUI and audio thread.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Continuous,
    Stepped,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDef {
    /// Stable key used in preset files.
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

macro_rules! params {
    ($( $id:ident => ($key:literal, $label:literal, $group:literal, $min:expr, $max:expr, $def:expr, $unit:literal, $kind:ident, $help:literal) ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum P { $($id),* }

        pub const DEFS: &[ParamDef] = &[
            $(ParamDef { key: $key, label: $label, group: $group, min: $min, max: $max, default: $def, unit: $unit, kind: Kind::$kind, help: $help }),*
        ];
    };
}

params! {
    InGain        => ("in_gain", "Gain", "Input", -24.0, 24.0, 0.0, "dB", Continuous, "Mic input gain."),
    Gate          => ("gate", "Gate", "Input", -80.0, -20.0, -55.0, "dB", Continuous, "Noise gate threshold. Also stops glitches firing on room noise."),

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

    Layers        => ("layers", "Layers", "Voices", 0.0, 4.0, 2.0, "", Stepped, "Extra copies of the voice."),
    Detune        => ("detune", "Detune", "Voices", 0.0, 60.0, 18.0, "ct", Continuous, "How out of tune the copies are."),
    Spread        => ("spread", "Spread", "Voices", 0.0, 1.0, 0.45, "", Continuous, "Time offset and stereo width of the copies."),
    Chaos         => ("chaos", "Chaos", "Voices", 0.0, 1.0, 0.3, "", Continuous, "How often copies jump to their own pitch (SS2 style)."),
    LayerLevel    => ("layer_level", "Level", "Voices", 0.0, 1.0, 0.6, "", Continuous, "Loudness of the copies."),

    FlangeDepth   => ("flange_depth", "Depth", "Metal", 0.0, 1.0, 0.35, "", Continuous, "Sweep depth of the metallic flanger."),
    FlangeRate    => ("flange_rate", "Rate", "Metal", 0.02, 4.0, 0.25, "Hz", Continuous, "Sweep speed."),
    FlangeFb      => ("flange_fb", "Feedback", "Metal", -0.95, 0.95, -0.7, "", Continuous, "Resonance. Negative = hollow pipe, positive = ringing."),
    FlangeMix     => ("flange_mix", "Mix", "Metal", 0.0, 1.0, 0.5, "", Continuous, "Amount of flanger."),

    LofiRate      => ("lofi_rate", "Rate", "Lo-fi", 4000.0, 48000.0, 22050.0, "Hz", Continuous, "Sample rate reduction (1994 grit)."),
    LofiBits      => ("lofi_bits", "Bits", "Lo-fi", 4.0, 16.0, 12.0, "bit", Continuous, "Bit depth reduction."),
    LofiTone      => ("lofi_tone", "Tone", "Lo-fi", 0.0, 1.0, 0.3, "", Continuous, "0 = full range, 1 = narrow intercom band."),

    DryWet        => ("dry_wet", "Dry/Wet", "Output", 0.0, 1.0, 1.0, "", Continuous, "Blend of your clean voice and SHODAN."),
    OutGain       => ("out_gain", "Gain", "Output", -24.0, 12.0, 0.0, "dB", Continuous, "Output level (a limiter follows)."),
    Bypass        => ("bypass", "Bypass", "Output", 0.0, 1.0, 0.0, "", Toggle, "Pass your clean voice through."),
}

pub const COUNT: usize = DEFS.len();

/// Parameter values as f32 bits in atomics: the GUI writes, the audio thread reads.
pub struct Params {
    values: [AtomicU32; COUNT],
}

impl Params {
    pub fn new() -> Self {
        Self { values: std::array::from_fn(|i| AtomicU32::new(DEFS[i].default.to_bits())) }
    }

    pub fn get(&self, p: P) -> f32 {
        self.get_index(p as usize)
    }

    pub fn get_index(&self, i: usize) -> f32 {
        f32::from_bits(self.values[i].load(Ordering::Relaxed))
    }

    pub fn set(&self, p: P, v: f32) {
        self.set_index(p as usize, v);
    }

    pub fn set_index(&self, i: usize, v: f32) {
        let d = &DEFS[i];
        let v = v.clamp(d.min, d.max);
        let v = if d.kind == Kind::Continuous { v } else { v.round() };
        self.values[i].store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> [f32; COUNT] {
        std::array::from_fn(|i| self.get_index(i))
    }
}

impl Default for Params {
    fn default() -> Self {
        Self::new()
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
