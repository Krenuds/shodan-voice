//! The Voice window (devices, meters, presets, the SHODAN knobs) and the Rack window.

use super::knob::{KNOB_WIDTH, meter};
use super::param_ui::{knob_count, param_knobs};
use super::rack::{Live, Rack};
use super::theme;
use crate::audio::engine::{Command, RackLink};
use crate::audio::io::{self, AudioSettings, DeviceInfo, Running};
use crate::audio::module::RackModule;
use crate::modules;
use crate::params::{Params, io::P};
use crate::presets::{self, Settings, Values};
use crate::shared::Shared;
use eframe::egui::{self, RichText};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Knob cards of the Voice window: the I/O stage and the first SHODAN Core in the rack.
const GROUPS: &[&str] = &["Input", "Voice", "Glitch", "Pitch", "Voices", "Output"];

pub struct App {
    shared: Arc<Shared>,
    settings: Settings,
    inputs: Vec<DeviceInfo>,
    outputs: Vec<DeviceInfo>,
    audio: Option<Running>,
    audio_error: Option<String>,
    stream_errors: Vec<String>,
    cmd_tx: Option<rtrb::Producer<Command>>,
    garbage_rx: Option<rtrb::Consumer<RackModule>>,
    rack: Rack,
    preset_label: String,
    new_preset: String,
    glitches_seen: u32,
    glitch_flash: Option<Instant>,
    in_level: f32,
    out_level: f32,
    /// Smoothed end-to-end latency estimate for [main output, monitor], ms.
    latency: [f32; 2],
    _hotkeys: Option<global_hotkey::GlobalHotKeyManager>,
    last_save: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        theme::apply(&cc.egui_ctx);
        let shared = Arc::new(Shared::default());
        shared.speech.enabled.store(true, Ordering::Relaxed);
        let mut settings = Settings::load();
        if settings.knobs.0.is_empty() {
            presets::BUILTIN[0].apply(&[&shared.io]);
        } else {
            settings.knobs.apply(&[&shared.io]);
        }
        let mut rack = Rack::new(Instant::now().elapsed().as_nanos() as u64 ^ std::process::id() as u64);
        rack.restore(&settings.rack_items(), &cc.egui_ctx);

        let outputs = io::list_outputs();
        if settings.knobs.0.is_empty() && settings.output_device.is_none() {
            // First run: send to the virtual cable if it's installed, never straight to speakers by choice.
            settings.output_device = outputs.iter().find(|d| is_cable_playback(&d.name)).map(|d| d.id.clone());
        }

        let hotkeys = settings.hotkey_bypass.then(|| register_hotkey(&shared, &cc.egui_ctx)).flatten();

        let mut app = Self {
            shared,
            inputs: io::list_inputs(),
            outputs,
            audio: None,
            audio_error: None,
            stream_errors: Vec::new(),
            cmd_tx: None,
            garbage_rx: None,
            rack,
            preset_label: if settings.knobs.0.is_empty() { presets::BUILTIN[0].name.to_string() } else { "(last session)".into() },
            new_preset: String::new(),
            glitches_seen: 0,
            glitch_flash: None,
            in_level: 0.0,
            out_level: 0.0,
            latency: [0.0; 2],
            _hotkeys: hotkeys,
            last_save: Instant::now(),
            settings,
        };
        app.restart_audio();
        app
    }

    fn restart_audio(&mut self) {
        self.audio = None;
        self.cmd_tx = None;
        self.garbage_rx = None;
        self.rack.audio_stopped();

        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(64);
        let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(64);
        let link = RackLink { commands: cmd_rx, garbage: garbage_tx };
        let cfg = AudioSettings {
            input: self.settings.input_device.clone(),
            output: self.settings.output_device.clone(),
            monitor: self.settings.monitor.then(|| self.settings.monitor_device.clone()),
        };
        let (rack, shared) = (&mut self.rack, &self.shared);
        match io::start(&cfg, shared.clone(), Some(link), |sample_rate| rack.build(sample_rate, shared)) {
            Ok(running) => {
                self.audio_error = None;
                self.cmd_tx = Some(cmd_tx);
                self.garbage_rx = Some(garbage_rx);
                self.audio = Some(running);
            }
            Err(e) => {
                // Modules built for an engine that never started were dropped with it.
                self.rack.audio_stopped();
                self.audio_error = Some(e);
            }
        }
    }

    /// What presets and the saved knobs act on.
    fn targets(&self) -> Vec<&Params> {
        presets::targets(&self.shared.io, self.rack.params())
    }

    fn save_settings(&mut self) {
        self.settings.knobs = Values::capture(&self.targets());
        self.settings.rack = Some(self.rack.save());
        if let Err(e) = self.settings.save() {
            self.stream_errors.push(format!("could not save settings: {e}"));
        }
        self.last_save = Instant::now();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("S.H.O.D.A.N.").monospace().size(22.0).strong().color(theme::ACCENT));
            ui.label(RichText::new("voice").monospace().size(14.0).color(theme::TEXT_DIM));
            ui.add_space(16.0);

            ui.label("Preset");
            egui::ComboBox::from_id_salt("preset").width(170.0).selected_text(&self.preset_label).show_ui(ui, |ui| {
                for p in presets::BUILTIN {
                    if ui.selectable_label(self.preset_label == p.name, p.name).clicked() {
                        p.apply(&self.targets());
                        self.preset_label = p.name.to_string();
                    }
                }
                if !self.settings.user_presets.is_empty() {
                    ui.separator();
                }
                let mut delete = None;
                for (name, values) in &self.settings.user_presets {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(&self.preset_label == name, name).clicked() {
                            values.apply(&presets::targets(&self.shared.io, self.rack.params()));
                            self.preset_label = name.clone();
                        }
                        if ui.small_button("🗙").on_hover_text("Delete preset").clicked() {
                            delete = Some(name.clone());
                        }
                    });
                }
                if let Some(d) = delete {
                    self.settings.user_presets.remove(&d);
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.new_preset).hint_text("new preset name").desired_width(130.0));
            if ui.add_enabled(!self.new_preset.trim().is_empty(), egui::Button::new("Save")).clicked() {
                let name = self.new_preset.trim().to_string();
                let values = Values::capture(&self.targets());
                self.settings.user_presets.insert(name.clone(), values);
                self.preset_label = name;
                self.new_preset.clear();
                self.save_settings();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let bypass = self.shared.io.get(P::Bypass) > 0.5;
                let (text, color) = if bypass { ("BYPASSED", theme::WARN) } else { ("ACTIVE", theme::ACCENT) };
                let btn = egui::Button::new(RichText::new(text).monospace().strong().color(egui::Color32::BLACK)).fill(color).min_size(egui::vec2(110.0, 28.0));
                let hint = if self._hotkeys.is_some() { "Toggle effect (F8 works globally)" } else { "Toggle effect" };
                if ui.add(btn).on_hover_text(hint).clicked() {
                    self.shared.io.set(P::Bypass, if bypass { 0.0 } else { 1.0 });
                }
                ui.add_space(8.0);
                if ui.selectable_label(self.settings.rack_open, "Rack").on_hover_text("Show the module rack: add, remove and reorder effects").clicked() {
                    self.settings.rack_open = !self.settings.rack_open;
                }
            });
        });
    }

    fn devices_card(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, "Routing", |ui| {
            let mut changed = false;
            egui::Grid::new("devices").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Microphone");
                changed |= device_combo(ui, "in_dev", &self.inputs, &mut self.settings.input_device);
                ui.end_row();
                ui.label("Output");
                changed |= device_combo(ui, "out_dev", &self.outputs, &mut self.settings.output_device);
                ui.end_row();
                ui.label("Monitor");
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut self.settings.monitor, "").on_hover_text("Also play the result to headphones").changed();
                    ui.add_enabled_ui(self.settings.monitor, |ui| {
                        changed |= device_combo(ui, "mon_dev", &self.outputs, &mut self.settings.monitor_device);
                    });
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Refresh devices").clicked() {
                    self.inputs = io::list_inputs();
                    self.outputs = io::list_outputs();
                    changed |= self.audio.is_none();
                }
                if ui.button("Restart audio").clicked() {
                    changed = true;
                }
            });
            if changed {
                self.restart_audio();
                self.save_settings();
            }

            let has_cable = self.outputs.iter().any(|d| is_cable_playback(&d.name));
            let out_name = self.settings.output_device.as_ref().and_then(|id| self.outputs.iter().find(|d| &d.id == id)).map(|d| d.name.to_lowercase()).unwrap_or_default();
            if !has_cable {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("To use SHODAN as a mic in Discord/OBS, install").small().color(theme::WARN));
                    ui.hyperlink_to(RichText::new("VB-Audio Virtual Cable").small(), "https://vb-audio.com/Cable/");
                    ui.label(RichText::new("then pick it as Output here and “CABLE Output” as the mic there.").small().color(theme::WARN));
                });
            } else if !is_cable_playback(&out_name) {
                ui.label(RichText::new("Tip: set Output to the VB-Audio Virtual Cable device, then select “CABLE Output” as your mic in Discord/OBS.").small().color(theme::TEXT_DIM));
            }
        });
    }

    fn meters_card(&mut self, ui: &mut egui::Ui) {
        let m = &self.shared.meters;
        self.in_level = m.input_peak.take().max(self.in_level * 0.85);
        self.out_level = m.output_peak.take().max(self.out_level * 0.85);
        let glitches = m.glitches.load(Ordering::Relaxed);
        if glitches != self.glitches_seen {
            self.glitches_seen = glitches;
            self.glitch_flash = Some(Instant::now());
        }
        theme::card(ui, "Signal", |ui| {
            meter(ui, "in", self.in_level, 220.0);
            meter(ui, "out", self.out_level, 220.0);
            ui.horizontal(|ui| {
                let flash = self.glitch_flash.is_some_and(|t| t.elapsed() < Duration::from_millis(180));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, if flash { theme::ACCENT_HOT } else { theme::KNOB_TRACK });
                ui.label(RichText::new(format!("glitches {glitches}")).monospace().small());
                ui.label(RichText::new(format!("pitch {:+.1} st", m.pitch.get())).monospace().small());
                ui.label(RichText::new(format!("tape lag {:.0} ms", m.lag_ms.get())).monospace().small());
            });
            if self.audio.is_some() {
                let (input, dsp) = (m.input_ms.get(), m.dsp_ms.get());
                for k in 0..2 {
                    self.latency[k] += (input + dsp + m.output_ms[k].get() - self.latency[k]) * 0.1;
                }
                let mut text = format!("latency ≈ {:.0} ms", self.latency[0]);
                if self.settings.monitor {
                    text += &format!(" · monitor ≈ {:.0} ms", self.latency[1]);
                }
                text += &format!("  (mic {input:.0} + fx {dsp:.0} + out {:.0})", m.output_ms[if self.settings.monitor { 1 } else { 0 }].get());
                let color = if self.latency[if self.settings.monitor { 1 } else { 0 }] > 60.0 { theme::WARN } else { theme::TEXT_DIM };
                ui.label(RichText::new(text).monospace().small().color(color)).on_hover_text(format!(
                    "Mic driver {input:.1} ms + rack {dsp:.1} ms (SHODAN Core: Grain/2) + buffered & output driver: main {:.1} ms, monitor {:.1} ms.\nLower the Grain knob to cut the effect's share. Windows shared-mode audio adds ~10 ms per device.",
                    m.output_ms[0].get(),
                    m.output_ms[1].get()
                ));
            }
        });
    }

    /// The parameter tables that have knobs in a Voice window card.
    fn group_params(&self, group: &str) -> Vec<&Params> {
        let core = self.rack.first(&modules::shodan::KIND).map(|m| &m.params);
        std::iter::once(&self.shared.io).chain(core).filter(|p| knob_count(p, Some(group)) > 0).collect()
    }

    fn knob_group(&self, ui: &mut egui::Ui, group: &str) {
        theme::card(ui, group, |ui| {
            ui.horizontal(|ui| {
                for params in self.group_params(group) {
                    param_knobs(ui, params, Some(group));
                }
            });
        });
    }

    fn rack_window(&mut self, ctx: &egui::Context) {
        let builder = egui::ViewportBuilder::default().with_title("SHODAN Rack").with_inner_size([820.0, 720.0]).with_min_inner_size([480.0, 320.0]);
        let changed = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("rack"), builder, |ui, _class| {
            if ui.input(|i| i.viewport().close_requested()) {
                self.settings.rack_open = false;
            }
            let mut changed = false;
            egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                    let live = match (&self.audio, self.cmd_tx.as_mut()) {
                        (Some(audio), Some(tx)) => Some(Live { sr: audio.sample_rate, tx, app: &self.shared }),
                        _ => None,
                    };
                    changed = self.rack.ui(ui, live);
                });
            });
            changed
        });
        if changed {
            self.save_settings();
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(errors) = self.audio.as_ref().map(|a| a.errors.clone())
            && let Ok(mut e) = errors.lock() {
                self.stream_errors.append(&mut e);
            }
        let keep = self.stream_errors.len().saturating_sub(3);
        self.stream_errors.drain(..keep);
        ui.horizontal(|ui| {
            match (&self.audio, &self.audio_error) {
                (_, Some(e)) => {
                    ui.label(RichText::new(format!("Audio error: {e}")).color(theme::DANGER));
                }
                (Some(a), None) => {
                    ui.label(RichText::new(&a.description).small().color(theme::TEXT_DIM));
                    let u = self.shared.meters.underruns.load(Ordering::Relaxed);
                    if u > 0 {
                        ui.label(RichText::new(format!("· {u} dropouts")).small().color(theme::WARN));
                    }
                }
                (None, None) => {
                    ui.label("Audio stopped");
                }
            }
            if let Some(e) = self.stream_errors.last() {
                ui.label(RichText::new(e).small().color(theme::WARN));
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.rack.idle(self.garbage_rx.as_mut());
        if self.settings.rack_open {
            self.rack_window(&ui.ctx().clone());
        }
        if self.last_save.elapsed() > Duration::from_secs(30) {
            self.save_settings();
        }

        egui::Panel::top("top").frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 4))).show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(12))).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                if ui.available_width() > 960.0 {
                    ui.horizontal(|ui| {
                        self.devices_card(ui);
                        self.meters_card(ui);
                    });
                } else {
                    self.devices_card(ui);
                    self.meters_card(ui);
                }
                // Cards don't know their size before layout, so pack them into rows by hand.
                let spacing = ui.spacing().item_spacing.x;
                let card_width = |g: &str| {
                    let n = self.group_params(g).iter().map(|p| knob_count(p, Some(g))).sum::<usize>() as f32;
                    n * KNOB_WIDTH + (n - 1.0) * spacing + 20.0
                };
                let mut rows: Vec<Vec<&str>> = vec![Vec::new()];
                let mut used = 0.0;
                for g in GROUPS.iter().filter(|g| !self.group_params(g).is_empty()) {
                    let w = card_width(g) + spacing;
                    if used + w > ui.available_width() && !rows.last().unwrap().is_empty() {
                        rows.push(Vec::new());
                        used = 0.0;
                    }
                    rows.last_mut().unwrap().push(g);
                    used += w;
                }
                for row in rows {
                    ui.horizontal(|ui| {
                        for g in row {
                            self.knob_group(ui, g);
                        }
                    });
                }
                if self.rack.first(&modules::shodan::KIND).is_none() {
                    ui.label(RichText::new("There is no SHODAN Core in the rack, so its knobs are hidden. Add one in the Rack window.").color(theme::TEXT_DIM));
                }
            });
        });
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    fn on_exit(&mut self) {
        self.save_settings();
        // Stop audio before plugins are torn down.
        self.audio = None;
    }
}

/// VB-Cable's playback end: "CABLE Input (VB-Audio Virtual Cable)" on older drivers,
/// "Speakers (VB-Audio Virtual Cable)" on newer ones. The 16-channel variant is skipped.
fn is_cable_playback(name: &str) -> bool {
    let n = name.to_lowercase();
    (n.contains("cable input") || n.contains("vb-audio virtual cable")) && !n.contains("16 ch")
}

fn device_combo(ui: &mut egui::Ui, id: &str, devices: &[DeviceInfo], selected: &mut Option<String>) -> bool {
    let label = match selected {
        None => "System default".to_string(),
        Some(sel) => devices.iter().find(|d| &d.id == sel).map(|d| d.name.clone()).unwrap_or_else(|| "(missing device)".into()),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).width(260.0).selected_text(label).show_ui(ui, |ui| {
        if ui.selectable_label(selected.is_none(), "System default").clicked() {
            *selected = None;
            changed = true;
        }
        for d in devices {
            if ui.selectable_label(selected.as_deref() == Some(&d.id), &d.name).clicked() {
                *selected = Some(d.id.clone());
                changed = true;
            }
        }
    });
    changed
}

/// F8 toggles bypass from anywhere, even while a game has focus.
fn register_hotkey(shared: &Arc<Shared>, ctx: &egui::Context) -> Option<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::hotkey::{Code, HotKey};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    let manager = GlobalHotKeyManager::new().ok()?;
    let hotkey = HotKey::new(None, Code::F8);
    manager.register(hotkey).ok()?;
    let shared = shared.clone();
    let ctx = ctx.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.id == hotkey.id() && e.state == HotKeyState::Pressed {
            let on = shared.io.get(P::Bypass) > 0.5;
            shared.io.set(P::Bypass, if on { 0.0 } else { 1.0 });
            ctx.request_repaint();
        }
    }));
    Some(manager)
}
