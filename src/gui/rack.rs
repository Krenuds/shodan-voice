//! The rack: the GUI-thread side of the module chain, and the contents of the Rack window.

use super::knob::Knob;
use super::param_ui::param_knobs;
use super::theme;
use crate::audio::engine::{Command, MAX_MODULES};
use crate::audio::module::{Module, ModuleId, ModuleShared, RackModule};
use crate::modules::{self, ModuleKind};
use crate::params::Params;
#[cfg(feature = "clap-host")]
use crate::plugins::{LoadedPlugin, PluginInfo, scan};
#[cfg(feature = "clap-host")]
use crate::presets::SlotSettings;
use crate::presets::{CLAP_KIND, RackItemSettings, Values};
use crate::shared::Shared;
use crate::speech;
use eframe::egui::{self, RichText};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Assignable knobs shown for a CLAP plugin.
#[cfg(feature = "clap-host")]
const KNOBS: usize = 4;

#[cfg(feature = "clap-host")]
struct ClapBody {
    plugin: LoadedPlugin,
    knobs: [Option<u32>; KNOBS],
}

enum Body {
    Native(&'static ModuleKind),
    #[cfg(feature = "clap-host")]
    Clap(Box<ClapBody>),
    /// A saved module that could not be recreated. Kept so saving does not forget it.
    Missing(Box<RackItemSettings>),
}

struct Item {
    id: ModuleId,
    shared: Arc<ModuleShared>,
    body: Body,
    /// The engine currently holds this item's audio half.
    live: bool,
    error: Option<String>,
}

impl Item {
    fn title(&self) -> String {
        match &self.body {
            Body::Native(kind) => kind.name.to_string(),
            #[cfg(feature = "clap-host")]
            Body::Clap(c) => c.plugin.name.clone(),
            Body::Missing(s) => s.clap.as_ref().and_then(|c| c.name.clone()).unwrap_or_else(|| s.kind.clone()),
        }
    }

    /// Build the audio-thread half at the engine's sample rate.
    fn make(&mut self, sr: f64, seed: u64, app: &Arc<Shared>) -> Option<Box<dyn Module>> {
        match &mut self.body {
            Body::Native(kind) => Some((kind.make)(sr as f32, seed ^ (self.id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15), &self.shared, app)),
            #[cfg(feature = "clap-host")]
            Body::Clap(c) => match c.plugin.activate(sr) {
                Ok(module) => Some(module),
                Err(e) => {
                    self.error = Some(e);
                    None
                }
            },
            Body::Missing(_) => None,
        }
    }
}

/// The running audio engine, as the rack needs it to change modules on the fly.
pub struct Live<'a> {
    pub sr: f64,
    pub tx: &'a mut rtrb::Producer<Command>,
    pub app: &'a Arc<Shared>,
}

enum Action {
    Add(&'static ModuleKind),
    #[cfg(feature = "clap-host")]
    AddClap(PluginInfo),
    Remove(ModuleId),
    Move(ModuleId, isize),
}

pub struct Rack {
    items: Vec<Item>,
    next_id: ModuleId,
    seed: u64,
    error: Option<String>,
    /// Plugins waiting for the audio thread to hand their processor back before deactivating.
    #[cfg(feature = "clap-host")]
    pending: Vec<LoadedPlugin>,
    #[cfg(feature = "clap-host")]
    catalog: Option<Vec<PluginInfo>>,
    #[cfg(feature = "clap-host")]
    show_instruments: bool,
    #[cfg(feature = "clap-host")]
    filter: String,
}

impl Rack {
    pub fn new(seed: u64) -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            seed,
            error: None,
            #[cfg(feature = "clap-host")]
            pending: Vec::new(),
            #[cfg(feature = "clap-host")]
            catalog: None,
            #[cfg(feature = "clap-host")]
            show_instruments: false,
            #[cfg(feature = "clap-host")]
            filter: String::new(),
        }
    }

    fn new_item(&mut self, body: Body, shared: Arc<ModuleShared>) -> Item {
        let id = self.next_id;
        self.next_id += 1;
        Item { id, shared, body, live: false, error: None }
    }

    /// Recreate the modules saved in the settings file.
    pub fn restore(&mut self, saved: &[RackItemSettings], ctx: &egui::Context) {
        for s in saved.iter().take(MAX_MODULES) {
            let (body, shared, error) = match modules::kind(&s.kind) {
                Some(kind) => {
                    let shared = ModuleShared::new(kind.defs);
                    s.values.apply(&[&shared.params]);
                    (Body::Native(kind), shared, None)
                }
                None => match restore_plugin(s, ctx) {
                    Ok(body) => (body, ModuleShared::new(&[]), None),
                    Err(e) => (Body::Missing(Box::new(s.clone())), ModuleShared::new(&[]), Some(e)),
                },
            };
            shared.enabled.store(s.enabled, Ordering::Relaxed);
            shared.mix.set(s.mix);
            let mut item = self.new_item(body, shared);
            item.error = error;
            self.items.push(item);
        }
    }

    pub fn save(&mut self) -> Vec<RackItemSettings> {
        self.items
            .iter_mut()
            .map(|item| {
                let mut s = RackItemSettings { enabled: item.shared.enabled.load(Ordering::Relaxed), mix: item.shared.mix.get(), ..Default::default() };
                match &mut item.body {
                    Body::Native(kind) => {
                        s.kind = kind.id.to_string();
                        s.values = Values::capture(&[&item.shared.params]);
                    }
                    #[cfg(feature = "clap-host")]
                    Body::Clap(c) => {
                        s.kind = CLAP_KIND.to_string();
                        let p = &mut c.plugin;
                        let ids: Vec<u32> = p.params.iter().map(|m| m.id).collect();
                        s.clap = Some(SlotSettings {
                            bundle: Some(p.bundle.clone()),
                            plugin_id: Some(p.plugin_id.clone()),
                            name: Some(p.name.clone()),
                            enabled: s.enabled,
                            mix: s.mix,
                            knobs: c.knobs.iter().flatten().copied().collect(),
                            values: ids.into_iter().filter_map(|id| Some((id, p.get_value(id)?))).collect(),
                        });
                    }
                    Body::Missing(saved) => s = RackItemSettings { enabled: s.enabled, mix: s.mix, ..(**saved).clone() },
                }
                s
            })
            .collect()
    }

    /// Parameter tables of the modules in rack order, for presets.
    pub fn params(&self) -> impl Iterator<Item = &Params> {
        self.items.iter().map(|i| &i.shared.params)
    }

    /// The first module of a native kind, if the rack has one.
    pub fn first(&self, kind: &ModuleKind) -> Option<&Arc<ModuleShared>> {
        self.items.iter().find(|i| matches!(i.body, Body::Native(k) if k.id == kind.id)).map(|i| &i.shared)
    }

    /// A stream is starting at `sample_rate`: build every module's audio half for the new engine.
    pub fn build(&mut self, sample_rate: f64, app: &Arc<Shared>) -> Vec<RackModule> {
        let seed = self.seed;
        self.items
            .iter_mut()
            .filter_map(|item| {
                let module = item.make(sample_rate, seed, app)?;
                item.live = true;
                Some(RackModule { id: item.id, module, shared: item.shared.clone() })
            })
            .collect()
    }

    /// The audio stream is gone: every module's audio half has been dropped with it.
    pub fn audio_stopped(&mut self) {
        for item in &mut self.items {
            item.live = false;
            #[cfg(feature = "clap-host")]
            if let Body::Clap(c) = &mut item.body {
                c.plugin.deactivate();
            }
        }
        #[cfg(feature = "clap-host")]
        self.pending.clear();
    }

    /// Per-frame housekeeping on the GUI thread.
    pub fn idle(&mut self, garbage: Option<&mut rtrb::Consumer<RackModule>>) {
        if let Some(g) = garbage {
            while let Ok(old) = g.pop() {
                drop(old);
            }
        }
        #[cfg(feature = "clap-host")]
        {
            self.pending.retain_mut(|p| !p.deactivate());
            for item in &mut self.items {
                if let Body::Clap(c) = &mut item.body {
                    c.plugin.idle();
                }
            }
        }
    }

    fn send_order(&self, live: &mut Option<Live>) {
        let Some(live) = live else { return };
        let mut ids = [0; MAX_MODULES];
        for (slot, item) in ids.iter_mut().zip(&self.items) {
            *slot = item.id;
        }
        let _ = live.tx.push(Command::Order(ids));
    }

    fn add(&mut self, body: Body, shared: Arc<ModuleShared>, live: &mut Option<Live>) {
        let mut item = self.new_item(body, shared);
        if let Some(l) = live.as_mut()
            && let Some(module) = item.make(l.sr, self.seed, l.app)
        {
            item.live = l.tx.push(Command::Add(RackModule { id: item.id, module, shared: item.shared.clone() })).is_ok();
        }
        self.items.push(item);
        self.send_order(live);
    }

    fn remove(&mut self, id: ModuleId, live: &mut Option<Live>) {
        let Some(i) = self.items.iter().position(|item| item.id == id) else { return };
        let item = self.items.remove(i);
        if let (true, Some(l)) = (item.live, live.as_mut()) {
            let _ = l.tx.push(Command::Remove(id));
        }
        #[cfg(feature = "clap-host")]
        {
            if let Body::Clap(mut c) = item.body {
                c.plugin.close_editor();
                self.pending.push(c.plugin);
            }
            if live.is_none() {
                self.pending.clear();
            }
        }
    }

    fn apply(&mut self, action: Action, live: &mut Option<Live>, _ctx: &egui::Context) {
        self.error = None;
        let full = self.items.len() >= MAX_MODULES;
        match action {
            Action::Add(_) if full => self.error = Some(format!("The rack holds at most {MAX_MODULES} modules.")),
            Action::Add(kind) => self.add(Body::Native(kind), ModuleShared::new(kind.defs), live),
            #[cfg(feature = "clap-host")]
            Action::AddClap(_) if full => self.error = Some(format!("The rack holds at most {MAX_MODULES} modules.")),
            #[cfg(feature = "clap-host")]
            Action::AddClap(info) => match LoadedPlugin::load(&info.bundle, &info.id, &info.name, _ctx.clone()) {
                Ok(plugin) => {
                    let knobs = std::array::from_fn(|k| plugin.params.get(k).map(|m| m.id));
                    self.add(Body::Clap(Box::new(ClapBody { plugin, knobs })), ModuleShared::new(&[]), live);
                }
                Err(e) => self.error = Some(e),
            },
            Action::Remove(id) => self.remove(id, live),
            Action::Move(id, by) => {
                let Some(i) = self.items.iter().position(|item| item.id == id) else { return };
                let j = i.saturating_add_signed(by).min(self.items.len() - 1);
                self.items.swap(i, j);
                self.send_order(live);
            }
        }
    }

    /// Draw the rack. Returns true if modules were added, removed or moved.
    pub fn ui(&mut self, ui: &mut egui::Ui, mut live: Option<Live>) -> bool {
        let mut action = None;
        self.add_row(ui, &mut action);
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::DANGER).small());
        }

        let flow = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).monospace().size(12.0).color(theme::TEXT_DIM));
        };
        flow(ui, "MIC  ·  input gain");
        let count = self.items.len();
        let app = live.as_ref().map(|l| l.app.clone());
        for (index, item) in self.items.iter_mut().enumerate() {
            ui.push_id(item.id, |ui| strip(ui, item, index, count, app.as_deref(), &mut action));
        }
        if count == 0 {
            ui.label(RichText::new("The rack is empty: your voice passes through unchanged.").color(theme::WARN));
        }
        flow(ui, "OUTPUT  ·  output gain, bypass, limiter");

        let changed = action.is_some();
        if let Some(action) = action {
            self.apply(action, &mut live, ui.ctx());
        }
        changed
    }

    fn add_row(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Add");
            for kind in &modules::NATIVE {
                if ui.button(kind.name).clicked() {
                    *action = Some(Action::Add(kind));
                }
            }
            #[cfg(feature = "clap-host")]
            self.plugin_picker(ui, action);
        });
    }

    #[cfg(feature = "clap-host")]
    fn plugin_picker(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        let Self { catalog, show_instruments, filter, .. } = self;
        egui::ComboBox::from_id_salt("plugin_pick").width(170.0).selected_text("CLAP plugin…").height(420.0).show_ui(ui, |ui| {
            if catalog.is_none() {
                *catalog = Some(scan::scan());
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(filter).hint_text("filter…").desired_width(150.0));
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
            let f = filter.to_lowercase();
            for info in list {
                if (info.is_instrument && !*show_instruments) || (!f.is_empty() && !info.name.to_lowercase().contains(&f) && !info.vendor.to_lowercase().contains(&f)) {
                    continue;
                }
                if ui.selectable_label(false, format!("{}  ·  {}", info.name, info.vendor)).clicked() {
                    *action = Some(Action::AddClap(info.clone()));
                }
            }
        });
    }
}

#[cfg(feature = "clap-host")]
fn restore_plugin(s: &RackItemSettings, ctx: &egui::Context) -> Result<Body, String> {
    let clap = s.clap.as_ref().filter(|_| s.kind == CLAP_KIND).ok_or_else(|| format!("unknown module kind '{}'", s.kind))?;
    let (Some(bundle), Some(id)) = (&clap.bundle, &clap.plugin_id) else { return Err("no plugin saved for this module".into()) };
    let name = clap.name.clone().unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_string());
    let mut plugin = LoadedPlugin::load(bundle, id, &name, ctx.clone())?;
    for (&pid, &v) in &clap.values {
        plugin.set_value(pid, v);
    }
    let knobs = std::array::from_fn(|k| match clap.knobs.is_empty() {
        true => plugin.params.get(k).map(|m| m.id),
        false => clap.knobs.get(k).copied(),
    });
    Ok(Body::Clap(Box::new(ClapBody { plugin, knobs })))
}

#[cfg(not(feature = "clap-host"))]
fn restore_plugin(s: &RackItemSettings, _ctx: &egui::Context) -> Result<Body, String> {
    Err(if s.kind == CLAP_KIND { "this build cannot host CLAP plugins".into() } else { format!("unknown module kind '{}'", s.kind) })
}

/// What the speech recogniser behind TitoBot is doing.
fn speech_status(ui: &mut egui::Ui, app: &Shared) {
    let s = &app.speech;
    let (text, color) = match s.state.load(Ordering::Relaxed) {
        speech::LOADING => ("Loading the speech model…".to_string(), theme::TEXT_DIM),
        speech::LISTENING => match s.text() {
            heard if heard.is_empty() => ("Listening".to_string(), theme::TEXT_DIM),
            heard => (format!("Heard: {heard}  ·  pass {:.0} ms  ·  word out {:.0} ms after the voice", s.ms.get(), s.delay_ms.get()), theme::TEXT_DIM),
        },
        speech::FAILED => (format!("Not listening: {}", s.text()), theme::WARN),
        _ => return,
    };
    ui.label(RichText::new(text).small().color(color));
}

/// One module: reorder/remove header, then On, Mix and the module's own knobs. `app` is there
/// while audio is running.
fn strip(ui: &mut egui::Ui, item: &mut Item, index: usize, count: usize, app: Option<&Shared>, action: &mut Option<Action>) {
    let running = app.is_some();
    let width = ui.available_width() - 20.0;
    egui::Frame::new().fill(theme::CARD).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            if ui.add_enabled(index > 0, egui::Button::new("⏶").small()).on_hover_text("Earlier in the chain").clicked() {
                *action = Some(Action::Move(item.id, -1));
            }
            if ui.add_enabled(index + 1 < count, egui::Button::new("⏷").small()).on_hover_text("Later in the chain").clicked() {
                *action = Some(Action::Move(item.id, 1));
            }
            ui.label(RichText::new(format!("{}  {}", index + 1, item.title().to_uppercase())).monospace().size(12.0).color(theme::ACCENT));
            if running && !item.live {
                ui.label(RichText::new("not running").small().color(theme::WARN));
            }
            #[cfg(feature = "clap-host")]
            if let Body::Clap(c) = &mut item.body {
                let p = &mut c.plugin;
                if p.has_editor() && ui.small_button(if p.window.is_some() { "Close editor" } else { "Editor" }).clicked() {
                    if p.window.is_some() {
                        p.close_editor();
                    } else if let Err(e) = p.open_editor() {
                        item.error = Some(e);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("🗙").on_hover_text("Remove from the rack").clicked() {
                    *action = Some(Action::Remove(item.id));
                }
            });
        });
        if let Some(e) = &item.error {
            ui.label(RichText::new(e).color(theme::DANGER).small());
        }
        if let (Body::Native(kind), Some(app)) = (&item.body, app)
            && kind.id == modules::titobot::KIND.id
        {
            speech_status(ui, app);
        }

        ui.horizontal_wrapped(|ui| {
            let sh = &item.shared;
            let mut enabled = sh.enabled.load(Ordering::Relaxed);
            if ui.checkbox(&mut enabled, "On").changed() {
                sh.enabled.store(enabled, Ordering::Relaxed);
            }
            let mut mix = sh.mix.get();
            if Knob::new(&mut mix, 0.0, 1.0, 1.0, "Blend").format(|v| format!("{:.0}%", v * 100.0)).help("Blend of this module's output with its input.").show(ui).changed() {
                sh.mix.set(mix);
            }
            ui.separator();
            match &mut item.body {
                Body::Native(_) => param_knobs(ui, &sh.params, None),
                #[cfg(feature = "clap-host")]
                Body::Clap(c) => plugin_knobs(ui, c),
                Body::Missing(_) => {
                    ui.label(RichText::new("Not loaded. It stays in the saved rack until you remove it.").small().color(theme::TEXT_DIM));
                }
            }
        });
    });
}

#[cfg(feature = "clap-host")]
fn plugin_knobs(ui: &mut egui::Ui, body: &mut ClapBody) {
    let ClapBody { plugin, knobs } = body;
    for k in 0..KNOBS {
        ui.vertical(|ui| {
            let meta = knobs[k].and_then(|id| plugin.params.iter().find(|m| m.id == id)).cloned();
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
            egui::ComboBox::from_id_salt(("knob_assign", k)).width(62.0).selected_text(RichText::new("assign").small()).height(360.0).show_ui(ui, |ui| {
                for m in &plugin.params {
                    let name = if m.module.is_empty() { m.name.clone() } else { format!("{} / {}", m.module, m.name) };
                    if ui.selectable_label(knobs[k] == Some(m.id), name).clicked() {
                        knobs[k] = Some(m.id);
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::engine::{Engine, RackLink};
    use crate::presets::Settings;

    /// Drive the rack the way the Rack window does, against a real engine.
    #[test]
    fn rack_edits_reach_the_running_engine() {
        let ctx = egui::Context::default();
        let app = Arc::new(Shared::default());
        let mut rack = Rack::new(1);
        rack.restore(&Settings::default().rack_items(), &ctx);
        let ids = |rack: &Rack| rack.items.iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(ids(&rack), [1, 2, 3]);

        let (mut tx, commands) = rtrb::RingBuffer::new(64);
        let (garbage, mut garbage_rx) = rtrb::RingBuffer::new(64);
        let mut engine = Engine::new(48000.0, app.clone(), rack.build(48000.0, &app), Some(RackLink { commands, garbage }));
        assert!(rack.items.iter().all(|i| i.live));
        let run = |engine: &mut Engine| {
            let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
            engine.process(&[0.1; 256], &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            engine.order()
        };
        assert_eq!(run(&mut engine), [1, 2, 3]);

        let mut edit = |rack: &mut Rack, action| rack.apply(action, &mut Some(Live { sr: 48000.0, tx: &mut tx, app: &app }), &ctx);
        // A second Lo-fi, moved to the front of the chain.
        edit(&mut rack, Action::Add(modules::kind("lofi").unwrap()));
        edit(&mut rack, Action::Move(4, -1));
        edit(&mut rack, Action::Move(4, -1));
        edit(&mut rack, Action::Move(4, -1));
        assert_eq!(ids(&rack), [4, 1, 2, 3]);
        assert_eq!(run(&mut engine), [4, 1, 2, 3]);

        edit(&mut rack, Action::Remove(1));
        assert_eq!(run(&mut engine), [4, 2, 3]);
        rack.idle(Some(&mut garbage_rx));
        assert!(garbage_rx.is_empty());

        // Two modules of one kind save and restore with their own knob values.
        rack.items[0].shared.params.set_key("lofi_bits", 5.0);
        let saved = rack.save();
        assert_eq!(saved.iter().map(|s| s.kind.as_str()).collect::<Vec<_>>(), ["lofi", "metal", "lofi"]);
        let mut again = Rack::new(1);
        again.restore(&saved, &ctx);
        assert_eq!(again.items[0].shared.params.get_index(1), 5.0);
        assert_eq!(again.items[2].shared.params.get_index(1), rack.items[2].shared.params.get_index(1));

        // A module this build cannot recreate is kept, not silently dropped.
        let mut unknown = saved.clone();
        unknown[1].kind = "from_the_future".into();
        let mut kept = Rack::new(1);
        kept.restore(&unknown, &ctx);
        assert!(kept.items[1].error.is_some());
        assert_eq!(kept.save()[1].kind, "from_the_future");
        assert_eq!(kept.build(48000.0, &app).len(), 2);
    }
}
