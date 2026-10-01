//! The main window: toolbar, patchbay, inspector and status bar. `App` owns the settings, the
//! audio stream and the rack; the panels only draw and report what the user did.

use super::inspector::{self, InspectorCtx, is_cable_playback};
use super::patchbay::{self, PatchbayCtx};
use super::rack::{Action, Live, Rack};
use super::state::{Selection, UiState};
use super::statusbar::{self, StatusCtx};
use super::theme;
use super::toolbar::{self, ToolbarCtx};
use crate::audio::engine::{Command, MAX_MODULES, RackLink};
use crate::audio::io::{self, AudioSettings, DeviceInfo, Running};
use crate::audio::module::RackModule;
use crate::params::Params;
use crate::presets::{self, Settings, UserPreset, Values};
use crate::shared::Shared;
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How long a stream error stays in the status bar.
const ERROR_SHOWN: Duration = Duration::from_secs(10);

pub struct App {
    shared: Arc<Shared>,
    settings: Settings,
    inputs: Vec<DeviceInfo>,
    outputs: Vec<DeviceInfo>,
    audio: Option<Running>,
    audio_error: Option<String>,
    stream_errors: Vec<String>,
    /// When the last stream error arrived; they are dropped a while after.
    error_at: Instant,
    cmd_tx: Option<rtrb::Producer<Command>>,
    garbage_rx: Option<rtrb::Consumer<RackModule>>,
    rack: Rack,
    preset_label: String,
    /// The knobs as the preset left them, to show when they have been changed since.
    preset_values: Values,
    ui_state: UiState,
    last_save: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        theme::apply(&cc.egui_ctx);
        let shared = Arc::new(Shared::default());
        shared.speech.enabled.store(true, Ordering::Relaxed);
        start_trace(&shared);
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

        let mut ui_state = UiState::default();
        // Open on the voice itself when there is one.
        if let Some(first) = rack.items().first() {
            ui_state.selected = Selection::Module(first.id);
        }

        let mut app = Self {
            shared,
            inputs: io::list_inputs(),
            outputs,
            audio: None,
            audio_error: None,
            stream_errors: Vec::new(),
            error_at: Instant::now(),
            preset_values: Values::default(),
            cmd_tx: None,
            garbage_rx: None,
            rack,
            preset_label: if settings.knobs.0.is_empty() { presets::BUILTIN[0].name.to_string() } else { "(last session)".into() },
            ui_state,
            last_save: Instant::now(),
            settings,
        };
        app.preset_values = Values::capture(&app.targets());
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
            self.error_at = Instant::now();
        }
        self.last_save = Instant::now();
    }

    /// A user preset brings its own rack; one saved before presets held the rack only sets knobs.
    fn load_user_preset(&mut self, name: &str, ctx: &egui::Context) {
        let Some(preset) = self.settings.user_presets.get(name) else { return };
        match preset.rack.clone() {
            Some(saved) => {
                preset.knobs.apply(&[&self.shared.io]);
                self.rack.load(&saved, live(&self.audio, &mut self.cmd_tx, &self.shared), ctx);
                self.ui_state.fix_selection(&self.rack);
            }
            None => preset.knobs.apply(&presets::targets(&self.shared.io, self.rack.params())),
        }
        self.preset_label = name.to_string();
        self.preset_values = Values::capture(&self.targets());
        self.save_settings();
    }

    /// Save the I/O knobs and the whole rack under `name`, replacing a preset of that name.
    fn save_user_preset(&mut self, name: String) {
        let preset = UserPreset { knobs: Values::capture(&[&self.shared.io]), rack: Some(self.rack.save()) };
        self.settings.user_presets.insert(name.clone(), preset);
        self.preset_label = name;
        self.preset_values = Values::capture(&self.targets());
        self.save_settings();
    }

    /// Do an edit of the chain, select what it added, and save.
    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        // Removing the selected module selects its neighbour, however it was removed.
        if let Action::Remove(id) = action
            && self.ui_state.selected == Selection::Module(id)
        {
            let items = self.rack.items();
            let i = items.iter().position(|it| it.id == id).unwrap_or(0);
            self.ui_state.selected = match items.get(i + 1).or(i.checked_sub(1).and_then(|p| items.get(p))) {
                Some(next) => Selection::Module(next.id),
                None => Selection::Output,
            };
        }
        let added = self.rack.apply(action, &mut live(&self.audio, &mut self.cmd_tx, &self.shared), ctx);
        if let Some(id) = added {
            self.ui_state.selected = Selection::Module(id);
        }
        if let Some(e) = self.rack.error() {
            self.ui_state.notify(e);
        }
        self.ui_state.fix_selection(&self.rack);
        self.save_settings();
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let t = ToolbarCtx {
            shared: &self.shared,
            settings: &self.settings,
            preset_label: &self.preset_label,
            preset_dirty: Values::capture(&self.targets()) != self.preset_values,
            rack_full: self.rack.items().len() >= MAX_MODULES,
            running: self.audio.is_some(),
            cable_missing: !self.outputs.iter().any(|d| is_cable_playback(&d.name)),
        };
        let out = toolbar::toolbar(ui, t, &mut self.ui_state);
        if let Some(i) = out.builtin {
            let p = &presets::BUILTIN[i];
            p.apply(&self.targets());
            self.preset_label = p.name.to_string();
            self.preset_values = Values::capture(&self.targets());
            self.save_settings();
        }
        if let Some(name) = out.load_user {
            self.load_user_preset(&name, ui.ctx());
        }
        if let Some(name) = out.delete_user {
            self.settings.user_presets.remove(&name);
            self.save_settings();
        }
        if let Some(name) = out.save_as {
            self.save_user_preset(name);
        }
        if let Some(action) = out.action {
            self.apply(action, ui.ctx());
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let c = InspectorCtx { rack: &mut self.rack, shared: &self.shared, settings: &mut self.settings, inputs: &self.inputs, outputs: &self.outputs, running: self.audio.as_ref() };
        let out = inspector::show(ui, c, &mut self.ui_state);
        if out.refresh_devices {
            self.inputs = io::list_inputs();
            self.outputs = io::list_outputs();
        }
        if out.restart_audio || (out.refresh_devices && self.audio.is_none()) {
            self.restart_audio();
            self.save_settings();
        }
        if let Some(action) = out.action {
            self.apply(action, ui.ctx());
        }
    }

    fn patchbay(&mut self, ui: &mut egui::Ui) {
        let c = PatchbayCtx { rack: &mut self.rack, shared: &self.shared, running: self.audio.is_some() };
        if let Some(action) = patchbay::show(ui, c, &mut self.ui_state) {
            self.apply(action, ui.ctx());
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(errors) = self.audio.as_ref().map(|a| a.errors.clone())
            && let Ok(mut e) = errors.lock()
            && !e.is_empty()
        {
            self.stream_errors.append(&mut e);
            self.error_at = Instant::now();
        }
        let keep = if self.error_at.elapsed() > ERROR_SHOWN { 0 } else { 3 };
        let drop = self.stream_errors.len().saturating_sub(keep);
        self.stream_errors.drain(..drop);
        let has_core = self.rack.items().iter().any(|i| i.kind_id() == crate::modules::shodan::KIND.id);
        let s = StatusCtx { shared: &self.shared, running: self.audio.as_ref(), audio_error: self.audio_error.as_deref(), stream_errors: &self.stream_errors, levels: &self.ui_state.levels, has_core };
        statusbar::status_bar(ui, s);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shared.trace.gui_frames.fetch_add(1, Ordering::Relaxed);
        self.rack.idle(self.garbage_rx.as_mut());
        self.ui_state.levels.update(&self.shared, &self.rack, self.audio.is_some());
        self.ui_state.fix_selection(&self.rack);
        if self.last_save.elapsed() > Duration::from_secs(30) {
            self.save_settings();
        }

        egui::Panel::top("toolbar").frame(egui::Frame::new().fill(theme::CHASSIS).inner_margin(egui::Margin::symmetric(16, 8))).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(theme::CHASSIS).inner_margin(egui::Margin::symmetric(16, 8))).show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size((ui.available_width() * 0.3).clamp(280.0, 400.0))
            .min_size(280.0)
            .max_size(520.0)
            .frame(egui::Frame::new().fill(theme::CHASSIS_LIGHT).inner_margin(egui::Margin::same(16)))
            .show(ui, |ui| self.inspector(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CHASSIS).inner_margin(egui::Margin::same(16))).show(ui, |ui| self.patchbay(ui));
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    fn on_exit(&mut self) {
        self.save_settings();
        // Stop audio before plugins are torn down.
        self.audio = None;
    }
}

/// The running engine, as the rack needs it to change modules on the fly.
fn live<'a>(audio: &Option<Running>, tx: &'a mut Option<rtrb::Producer<Command>>, app: &'a Arc<Shared>) -> Option<Live<'a>> {
    match (audio, tx.as_mut()) {
        (Some(audio), Some(tx)) => Some(Live { sr: audio.sample_rate, tx, app }),
        _ => None,
    }
}

/// Write `trace.log` in the config folder: a summary line every `trace::SUMMARY`, and the window
/// in front whenever it changes. The thread ends with the app.
fn start_trace(shared: &Arc<Shared>) {
    let Some(dir) = presets::config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    shared.trace.open(&dir.join("trace.log"));
    let weak = Arc::downgrade(shared);
    let _ = std::thread::Builder::new().name("trace".into()).spawn(move || {
        let (mut title, mut underruns) = (String::new(), 0);
        while let Some(shared) = weak.upgrade() {
            let front = crate::trace::foreground_title();
            if front != title {
                shared.trace.event(format_args!("front: \"{front}\""));
                title = front;
            }
            let u = shared.meters.underruns.load(Ordering::Relaxed);
            shared.trace.summary(u.wrapping_sub(underruns));
            underruns = u;
            drop(shared);
            std::thread::sleep(crate::trace::SUMMARY);
        }
    });
}
