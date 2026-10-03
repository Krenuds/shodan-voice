//! Built-in presets plus JSON save/load of user presets and app settings.

use crate::modules;
use crate::params::Params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What a preset acts on: the I/O stage plus the first module of each kind in the rack.
/// Presets set knobs; they never add, remove or reorder modules.
pub fn targets<'a>(io: &'a Params, rack: impl Iterator<Item = &'a Params>) -> Vec<&'a Params> {
    let mut out = vec![io];
    for p in rack {
        if !out.iter().any(|seen| std::ptr::eq(seen.defs(), p.defs())) {
            out.push(p);
        }
    }
    out
}

fn set_key(targets: &[&Params], key: &str, v: f32) -> bool {
    targets.iter().any(|p| p.set_key(key, v))
}

pub struct Preset {
    pub name: &'static str,
    /// Values, by parameter key, that differ from the parameter defaults.
    pub values: &'static [(&'static str, f32)],
}

impl Preset {
    pub fn apply(&self, targets: &[&Params]) {
        for p in targets {
            for (i, d) in p.defs().iter().enumerate() {
                if d.key != "bypass" {
                    p.set_index(i, d.default);
                }
            }
        }
        for &(key, v) in self.values {
            set_key(targets, key, v);
        }
    }
}

pub const BUILTIN: &[Preset] = &[
    Preset {
        name: "SS1 - Citadel",
        values: &[
            ("stutter_chance", 0.35),
            ("repeats", 2.0),
            ("slice", 80.0),
            ("reverse_chance", 0.05),
            ("warp_chance", 0.1),
            ("jump_range", 3.0),
            ("jump_chance", 0.35),
            ("layers", 1.0),
            ("detune", 12.0),
            ("spread", 0.3),
            ("chaos", 0.15),
            ("flange_depth", 0.4),
            ("flange_fb", -0.78),
            ("flange_mix", 0.55),
            ("lofi_rate", 11025.0),
            ("lofi_bits", 9.0),
            ("lofi_tone", 0.45),
            ("out_gain", 5.0),
        ],
    },
    Preset {
        name: "SS2 - Von Braun",
        values: &[
            ("stutter_chance", 0.4),
            ("repeats", 3.0),
            ("slice", 95.0),
            ("reverse_chance", 0.12),
            ("warp_chance", 0.15),
            ("catch_up", 0.6),
            ("jump_range", 5.0),
            ("jump_chance", 0.45),
            ("layers", 3.0),
            ("detune", 25.0),
            ("spread", 0.6),
            ("chaos", 0.45),
            ("layer_level", 0.65),
            ("flange_depth", 0.3),
            ("flange_fb", -0.6),
            ("flange_mix", 0.45),
            ("lofi_rate", 22050.0),
            ("lofi_bits", 14.0),
            ("lofi_tone", 0.2),
            ("out_gain", 3.0),
        ],
    },
    Preset {
        name: "Subtle",
        values: &[
            ("stutter_chance", 0.12),
            ("reverse_chance", 0.0),
            ("warp_chance", 0.04),
            ("jump_range", 2.0),
            ("jump_chance", 0.15),
            ("layers", 1.0),
            ("detune", 10.0),
            ("layer_level", 0.4),
            ("flange_mix", 0.3),
            ("lofi_rate", 32000.0),
            ("lofi_bits", 16.0),
            ("lofi_tone", 0.1),
            ("out_gain", 2.5),
        ],
    },
    Preset {
        name: "Glitch Storm",
        values: &[
            ("sensitivity", 0.8),
            ("stutter_chance", 0.7),
            ("repeats", 4.0),
            ("slice", 70.0),
            ("reverse_chance", 0.3),
            ("warp_chance", 0.3),
            ("catch_up", 0.85),
            ("jump_range", 7.0),
            ("jump_chance", 0.8),
            ("layers", 4.0),
            ("detune", 35.0),
            ("spread", 0.8),
            ("chaos", 0.8),
            ("flange_fb", -0.85),
            ("flange_mix", 0.6),
            ("lofi_rate", 16000.0),
            ("lofi_bits", 8.0),
            ("out_gain", 6.0),
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
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Values(pub BTreeMap<String, f32>);

impl Values {
    pub fn capture(targets: &[&Params]) -> Self {
        Self(
            targets
                .iter()
                .flat_map(|p| p.defs().iter().enumerate().map(move |(i, d)| (d.key, p.get_index(i))))
                .filter(|(key, _)| *key != "bypass")
                .map(|(key, v)| (key.to_string(), v))
                .collect(),
        )
    }

    pub fn apply(&self, targets: &[&Params]) {
        for (key, &v) in &self.0 {
            set_key(targets, key, v);
        }
        // Saved before formants were separate: pitch used to drag them along, so match it.
        if !self.0.contains_key("formant") {
            set_key(targets, "formant", self.0.get("base_pitch").copied().unwrap_or(0.0));
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
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

/// `kind` of a rack item that hosts a CLAP plugin (everything else is a native module id).
pub const CLAP_KIND: &str = "clap";

/// One module in the saved rack, in playing order.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct RackItemSettings {
    pub kind: String,
    pub enabled: bool,
    pub mix: f32,
    /// Knob values of a native module.
    #[serde(default)]
    pub values: Values,
    /// The plugin of a `clap` item (its own `enabled`/`mix` are unused).
    #[serde(default)]
    pub clap: Option<SlotSettings>,
}

/// A preset saved by the user: the I/O knobs and the whole rack (which modules, their order and
/// their values). Unlike a built-in preset, loading one replaces the rack.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct UserPreset {
    /// Knobs of the I/O stage. A preset saved before presets held the rack has no `rack`, and
    /// these are then also the knobs of the first module of each kind.
    #[serde(flatten)]
    pub knobs: Values,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<Vec<RackItemSettings>>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub monitor_device: Option<String>,
    pub monitor: bool,
    /// Knobs of the I/O stage and of the first module of each kind (what presets act on).
    pub knobs: Values,
    pub user_presets: BTreeMap<String, UserPreset>,
    /// `None` in files written before the rack existed; see [`Settings::rack_items`].
    pub rack: Option<Vec<RackItemSettings>>,
    /// The two fixed plugin slots of older versions; only read, to migrate them into the rack.
    #[serde(skip_serializing)]
    pub slots: Vec<SlotSettings>,
}

/// Where settings live. `SHODAN_CONFIG_DIR` overrides it, to try things without touching the real ones.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SHODAN_CONFIG_DIR") {
        return Some(dir.into());
    }
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
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// The saved rack, or for older files the classic chain built from the saved knobs
    /// (the first built-in preset on a first run) followed by the old plugin slots.
    pub fn rack_items(&self) -> Vec<RackItemSettings> {
        if let Some(rack) = &self.rack {
            return rack.clone();
        }
        let instances = modules::default_instances();
        let params: Vec<&Params> = instances.iter().map(|i| &i.shared.params).collect();
        if self.knobs.0.is_empty() {
            BUILTIN[0].apply(&params);
        } else {
            self.knobs.apply(&params);
        }
        let native = instances.iter().map(|i| RackItemSettings {
            kind: i.kind.id.to_string(),
            enabled: true,
            mix: 1.0,
            values: Values::capture(&[&i.shared.params]),
            clap: None,
        });
        let plugins = self.slots.iter().filter(|s| s.bundle.is_some()).map(|s| RackItemSettings {
            kind: CLAP_KIND.to_string(),
            enabled: s.enabled,
            mix: s.mix,
            values: Values::default(),
            clap: Some(s.clone()),
        });
        native.chain(plugins).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_preset_keys_all_exist() {
        let io = Params::new(crate::params::io::DEFS);
        let rack = modules::default_instances();
        let targets = targets(&io, rack.iter().map(|i| &i.shared.params));
        for preset in BUILTIN {
            for &(key, v) in preset.values {
                assert!(set_key(&targets, key, v), "{}: unknown key '{key}'", preset.name);
            }
        }
    }

    #[test]
    fn user_presets_keep_the_rack_and_old_ones_still_load() {
        let old: UserPreset = serde_json::from_str(r#"{ "base_pitch": 3.0, "out_gain": 4.0 }"#).unwrap();
        assert_eq!(old.rack, None);
        assert_eq!(old.knobs.0["base_pitch"], 3.0);

        let rack = Settings::default().rack_items();
        let new = UserPreset { knobs: Values([("out_gain".to_string(), 4.0)].into()), rack: Some(rack) };
        let reloaded: UserPreset = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!(reloaded, new);
    }

    #[test]
    fn settings_from_before_the_rack_migrate() {
        let old = r#"{
            "input_device": null, "output_device": "wasapi:x", "monitor_device": null, "monitor": false,
            "knobs": { "base_pitch": 3.0, "flange_mix": 0.25, "lofi_bits": 8.0, "out_gain": 4.0 },
            "user_presets": {},
            "slots": [
                { "bundle": "C:/p/Reverb.clap", "plugin_id": "com.x.reverb", "name": "Reverb", "enabled": false, "mix": 0.5, "knobs": [7], "values": { "7": 0.25 } },
                { "bundle": null, "plugin_id": null, "enabled": true, "mix": 1.0, "knobs": [], "values": {} }
            ],
            "hotkey_bypass": true
        }"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        let rack = settings.rack_items();
        assert_eq!(rack.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(), ["shodan_core", "metal", "lofi", "clap"]);
        assert_eq!(rack[0].values.0["base_pitch"], 3.0);
        // No "formant" in the old file: it follows the pitch, as it used to.
        assert_eq!(rack[0].values.0["formant"], 3.0);
        assert_eq!(rack[1].values.0["flange_mix"], 0.25);
        assert_eq!(rack[2].values.0["lofi_bits"], 8.0);
        assert!(!rack[3].enabled);
        assert_eq!(rack[3].mix, 0.5);
        assert_eq!(rack[3].clap.as_ref().unwrap().plugin_id.as_deref(), Some("com.x.reverb"));

        // Once saved, the rack is authoritative and round-trips.
        let saved = Settings { rack: Some(rack.clone()), ..settings };
        let reloaded: Settings = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(reloaded.rack_items(), rack);
    }
}
