//! Built-in presets plus JSON save/load of user presets and app settings.

use crate::params::{DEFS, P, Params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Preset {
    pub name: &'static str,
    /// Values that differ from the parameter defaults.
    pub values: &'static [(P, f32)],
}

impl Preset {
    pub fn apply(&self, params: &Params) {
        for (i, d) in DEFS.iter().enumerate() {
            if d.key != "bypass" {
                params.set_index(i, d.default);
            }
        }
        for &(p, v) in self.values {
            params.set(p, v);
        }
    }
}

pub const BUILTIN: &[Preset] = &[
    Preset {
        name: "SS1 - Citadel",
        values: &[
            (P::StutterChance, 0.35),
            (P::Repeats, 2.0),
            (P::Slice, 80.0),
            (P::ReverseChance, 0.05),
            (P::WarpChance, 0.1),
            (P::JumpRange, 3.0),
            (P::JumpChance, 0.35),
            (P::Layers, 1.0),
            (P::Detune, 12.0),
            (P::Spread, 0.3),
            (P::Chaos, 0.15),
            (P::FlangeDepth, 0.4),
            (P::FlangeFb, -0.78),
            (P::FlangeMix, 0.55),
            (P::LofiRate, 11025.0),
            (P::LofiBits, 9.0),
            (P::LofiTone, 0.45),
            (P::OutGain, 5.0),
        ],
    },
    Preset {
        name: "SS2 - Von Braun",
        values: &[
            (P::StutterChance, 0.4),
            (P::Repeats, 3.0),
            (P::Slice, 95.0),
            (P::ReverseChance, 0.12),
            (P::WarpChance, 0.15),
            (P::CatchUp, 0.6),
            (P::JumpRange, 5.0),
            (P::JumpChance, 0.45),
            (P::Layers, 3.0),
            (P::Detune, 25.0),
            (P::Spread, 0.6),
            (P::Chaos, 0.45),
            (P::LayerLevel, 0.65),
            (P::FlangeDepth, 0.3),
            (P::FlangeFb, -0.6),
            (P::FlangeMix, 0.45),
            (P::LofiRate, 22050.0),
            (P::LofiBits, 14.0),
            (P::LofiTone, 0.2),
            (P::OutGain, 3.0),
        ],
    },
    Preset {
        name: "Subtle",
        values: &[
            (P::StutterChance, 0.12),
            (P::ReverseChance, 0.0),
            (P::WarpChance, 0.04),
            (P::JumpRange, 2.0),
            (P::JumpChance, 0.15),
            (P::Layers, 1.0),
            (P::Detune, 10.0),
            (P::LayerLevel, 0.4),
            (P::FlangeMix, 0.3),
            (P::LofiRate, 32000.0),
            (P::LofiBits, 16.0),
            (P::LofiTone, 0.1),
            (P::OutGain, 2.5),
        ],
    },
    Preset {
        name: "Glitch Storm",
        values: &[
            (P::Sensitivity, 0.8),
            (P::StutterChance, 0.7),
            (P::Repeats, 4.0),
            (P::Slice, 70.0),
            (P::ReverseChance, 0.3),
            (P::WarpChance, 0.3),
            (P::CatchUp, 0.85),
            (P::JumpRange, 7.0),
            (P::JumpChance, 0.8),
            (P::Layers, 4.0),
            (P::Detune, 35.0),
            (P::Spread, 0.8),
            (P::Chaos, 0.8),
            (P::FlangeFb, -0.85),
            (P::FlangeMix, 0.6),
            (P::LofiRate, 16000.0),
            (P::LofiBits, 8.0),
            (P::OutGain, 6.0),
        ],
    },
];

pub fn builtin(name: &str) -> Option<&'static Preset> {
    let wanted = name.to_lowercase().replace([' ', '-', '_'], "");
    BUILTIN.iter().find(|p| {
        let n = p.name.to_lowercase().replace([' ', '-', '_'], "");
        n == wanted || n.starts_with(&wanted)
    })
}

/// Knob values keyed by parameter key, so presets survive adding/reordering parameters.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Values(pub BTreeMap<String, f32>);

impl Values {
    pub fn capture(params: &Params) -> Self {
        Self(DEFS.iter().enumerate().filter(|(_, d)| d.key != "bypass").map(|(i, d)| (d.key.to_string(), params.get_index(i))).collect())
    }

    pub fn apply(&self, params: &Params) {
        for (i, d) in DEFS.iter().enumerate() {
            if let Some(&v) = self.0.get(d.key) {
                params.set_index(i, v);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SlotSettings {
    /// Path of the .clap bundle and the plugin id inside it.
    pub bundle: Option<PathBuf>,
    pub plugin_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub enabled: bool,
    pub mix: f32,
    /// Parameter ids shown as knobs.
    pub knobs: Vec<u32>,
    /// Last known values of the plugin's parameters.
    pub values: BTreeMap<u32, f64>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub monitor_device: Option<String>,
    pub monitor: bool,
    pub knobs: Values,
    pub user_presets: BTreeMap<String, Values>,
    pub slots: Vec<SlotSettings>,
    pub hotkey_bypass: bool,
}

pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "shodan-voice").map(|d| d.config_dir().to_path_buf())
}

impl Settings {
    fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| Self { hotkey_bypass: true, ..Self::default() })
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }
}
