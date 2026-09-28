//! The two CLAP plugin slots: picker, editor button, enable/mix, and four assignable knobs.

use super::knob::Knob;
use super::theme;
use crate::audio::engine::{Command, SlotProcessor};
use crate::plugins::{LoadedPlugin, PluginInfo, scan};
use crate::presets::SlotSettings;
use crate::shared::{SLOT_COUNT, Shared};
use eframe::egui::{self, RichText};
use std::sync::atomic::Ordering;

const KNOBS: usize = 4;

#[derive(Default)]
struct Slot {
    plugin: Option<LoadedPlugin>,
    knobs: [Option<u32>; KNOBS],
    error: Option<String>,
    filter: String,
}

#[derive(Default)]
pub struct Slots {
    slots: [Slot; SLOT_COUNT],
    /// Plugins waiting for the audio thread to hand their processor back before deactivating.
    pending: Vec<LoadedPlugin>,
    catalog: Option<Vec<PluginInfo>>,
    show_instruments: bool,
}

impl Slots {
    /// Recreate plugins saved in the settings file.
    pub fn restore(&mut self, saved: &[SlotSettings], shared: &Shared, ctx: &egui::Context) {
        for (i, s) in saved.iter().take(SLOT_COUNT).enumerate() {
            shared.slots[i].enabled.store(s.enabled, Ordering::Relaxed);
            shared.slots[i].mix.set(s.mix);
            let (Some(bundle), Some(id)) = (&s.bundle, &s.plugin_id) else { continue };
            let name = s.name.clone().unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_string());
            match LoadedPlugin::load(bundle, id, &name, ctx.clone()) {
                Ok(mut p) => {
                    for (&pid, &v) in &s.values {
                        p.set_value(pid, v);
                    }
                    let slot = &mut self.slots[i];
                    slot.knobs = std::array::from_fn(|k| match s.knobs.is_empty() {
                        true => p.params.get(k).map(|m| m.id),
                        false => s.knobs.get(k).copied(),
                    });
                    slot.plugin = Some(p);
                }
                Err(e) => self.slots[i].error = Some(e),
            }
        }
    }

    pub fn save(&mut self, shared: &Shared) -> Vec<SlotSettings> {
        self.slots
            .iter_mut()
            .enumerate()
            .map(|(i, slot)| {
                let mut s = SlotSettings {
                    enabled: shared.slots[i].enabled.load(Ordering::Relaxed),
                    mix: shared.slots[i].mix.get(),
                    knobs: slot.knobs.iter().flatten().copied().collect(),
                    ..Default::default()
                };
                if let Some(p) = slot.plugin.as_mut() {
                    s.bundle = Some(p.bundle.clone());
                    s.plugin_id = Some(p.plugin_id.clone());
                    s.name = Some(p.name.clone());
                    let ids: Vec<u32> = p.params.iter().map(|m| m.id).collect();
                    for id in ids {
                        if let Some(v) = p.get_value(id) {
                            s.values.insert(id, v);
                        }
                    }
                }
                s
            })
            .collect()
    }

    /// The audio stream is gone: every processor has been dropped with it.
    pub fn audio_stopped(&mut self) {
        for p in self.slots.iter_mut().filter_map(|s| s.plugin.as_mut()) {
            p.deactivate();
        }
        self.pending.clear();
    }

    /// A new stream is running at `sample_rate`: activate plugins and hand them to the engine.
    pub fn audio_started(&mut self, sample_rate: f64, tx: &mut rtrb::Producer<Command>) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(p) = slot.plugin.as_mut() else { continue };
            match p.activate(sample_rate) {
                Ok(proc) => {
                    let _ = tx.push(Command::SetSlot(i, Some(proc)));
                }
                Err(e) => slot.error = Some(e),
            }
        }
    }

    /// Per-frame housekeeping on the GUI thread.
    pub fn idle(&mut self, garbage: Option<&mut rtrb::Consumer<Box<dyn SlotProcessor>>>) {
        if let Some(g) = garbage {
            while let Ok(old) = g.pop() {
                drop(old);
            }
        }
        self.pending.retain_mut(|p| !p.deactivate());
        for p in self.slots.iter_mut().filter_map(|s| s.plugin.as_mut()) {
            p.idle();
        }
    }

    fn set_plugin(&mut self, i: usize, new: Option<LoadedPlugin>, sample_rate: Option<f64>, tx: Option<&mut rtrb::Producer<Command>>) {
        let slot = &mut self.slots[i];
        slot.error = None;
        let mut new = new;
        let mut proc = None;
        if let (Some(p), Some(sr)) = (new.as_mut(), sample_rate) {
            match p.activate(sr) {
                Ok(pr) => proc = Some(pr),
                Err(e) => {
                    slot.error = Some(e);
                    return;
                }
            }
        }
        if let Some(p) = new.as_ref() {
            slot.knobs = std::array::from_fn(|k| p.params.get(k).map(|m| m.id));
        }
        if let Some(mut old) = std::mem::replace(&mut slot.plugin, new) {
            old.close_editor();
            self.pending.push(old);
        }
        match tx {
            Some(tx) => {
                let _ = tx.push(Command::SetSlot(i, proc));
            }
            None => self.pending.iter_mut().for_each(|p| {
                p.deactivate();
            }),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &Shared, sample_rate: Option<f64>, mut tx: Option<&mut rtrb::Producer<Command>>) {
        let side_by_side = ui.available_width() > 2.0 * 470.0;
        let mut layout = |ui: &mut egui::Ui| {
            for i in 0..SLOT_COUNT {
                let title = format!("Plugin slot {}", ["A", "B"][i]);
                let mut action: Option<Option<PluginInfo>> = None;
                theme::card(ui, &title, |ui| {
                    ui.set_width(430.0);
                    action = self.slot_ui(ui, i, shared);
                });
                if let Some(choice) = action {
                    let loaded = match choice {
                        Some(info) => match LoadedPlugin::load(&info.bundle, &info.id, &info.name, ui.ctx().clone()) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                self.slots[i].error = Some(e);
                                continue;
                            }
                        },
                        None => None,
                    };
                    self.set_plugin(i, loaded, sample_rate, tx.as_deref_mut());
                }
            }
        };
        if side_by_side {
            ui.horizontal(|ui| layout(ui));
        } else {
            layout(ui);
        }
    }

    /// Returns `Some(choice)` when the user picked a plugin (or `Some(None)` to remove it).
    fn slot_ui(&mut self, ui: &mut egui::Ui, i: usize, shared: &Shared) -> Option<Option<PluginInfo>> {
        let mut action = None;
        let catalog = &mut self.catalog;
        let show_instruments = &mut self.show_instruments;
        let slot = &mut self.slots[i];
        let current = slot.plugin.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "(empty)".into());

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("plugin_pick", i)).width(220.0).selected_text(current).height(420.0).show_ui(ui, |ui| {
                if catalog.is_none() {
                    *catalog = Some(scan::scan());
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut slot.filter).hint_text("filter…").desired_width(150.0));
                    ui.checkbox(show_instruments, "instruments");
                    if ui.small_button("rescan").clicked() {
                        *catalog = Some(scan::scan());
                    }
                });
                let list = catalog.as_deref().unwrap_or_default();
                if list.is_empty() {
                    ui.label(RichText::new("No CLAP plugins found.").color(theme::WARN));
                    ui.label("Install e.g. Airwindows Consolidated or Surge XT,\nthen press rescan. Searched:");
                    for p in scan::search_paths() {
                        ui.label(RichText::new(p.display().to_string()).small().monospace());
                    }
                }
                let f = slot.filter.to_lowercase();
                for info in list {
                    if (info.is_instrument && !*show_instruments) || (!f.is_empty() && !info.name.to_lowercase().contains(&f) && !info.vendor.to_lowercase().contains(&f)) {
                        continue;
                    }
                    if ui.selectable_label(false, format!("{}  ·  {}", info.name, info.vendor)).clicked() {
                        action = Some(Some(info.clone()));
                    }
                }
            });
            if let Some(p) = slot.plugin.as_mut() {
                if p.has_editor() && ui.button(if p.window.is_some() { "Close editor" } else { "Editor" }).clicked() {
                    if p.window.is_some() {
                        p.close_editor();
                    } else if let Err(e) = p.open_editor() {
                        slot.error = Some(e);
                    }
                }
                if ui.button("Remove").clicked() {
                    action = Some(None);
                }
            }
        });

        if let Some(e) = &slot.error {
            ui.label(RichText::new(e).color(theme::DANGER).small());
        }
        let Some(plugin) = slot.plugin.as_mut() else {
            ui.label(RichText::new("Runs after the SHODAN chain. Good picks: a plate/hall reverb, ring mod, or frequency shifter.").small().color(theme::TEXT_DIM));
            return action;
        };

        ui.horizontal(|ui| {
            let sh = &shared.slots[i];
            let mut enabled = sh.enabled.load(Ordering::Relaxed);
            ui.vertical(|ui| {
                ui.add_space(14.0);
                if ui.checkbox(&mut enabled, "On").changed() {
                    sh.enabled.store(enabled, Ordering::Relaxed);
                }
            });
            let mut mix = sh.mix.get();
            if Knob::new(&mut mix, 0.0, 1.0, 1.0, "Mix").format(|v| format!("{:.0}%", v * 100.0)).help("Blend of the plugin's output with its input.").show(ui).changed() {
                sh.mix.set(mix);
            }
            ui.separator();
            for k in 0..KNOBS {
                ui.vertical(|ui| {
                    let meta = slot.knobs[k].and_then(|id| plugin.params.iter().find(|m| m.id == id)).cloned();
                    let label = meta.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "—".into());
                    let short: String = label.chars().take(10).collect();
                    match meta {
                        Some(m) => {
                            let mut v = plugin.get_value(m.id).unwrap_or(m.default) as f32;
                            let resp = Knob::new(&mut v, m.min as f32, m.max as f32, m.default as f32, &short)
                                .format(|v| plugin.value_text(m.id, v as f64).unwrap_or_else(|| format!("{v:.2}")).chars().take(10).collect())
                                .stepped(m.stepped)
                                .help(&label)
                                .show(ui);
                            if resp.changed() {
                                plugin.set_value(m.id, v as f64);
                            }
                        }
                        None => {
                            ui.add_space(80.0);
                        }
                    }
                    egui::ComboBox::from_id_salt(("knob_assign", i, k)).width(62.0).selected_text(RichText::new("assign").small()).height(360.0).show_ui(ui, |ui| {
                        for m in &plugin.params {
                            let name = if m.module.is_empty() { m.name.clone() } else { format!("{} / {}", m.module, m.name) };
                            if ui.selectable_label(slot.knobs[k] == Some(m.id), name).clicked() {
                                slot.knobs[k] = Some(m.id);
                            }
                        }
                    });
                });
            }
        });
        action
    }
}
