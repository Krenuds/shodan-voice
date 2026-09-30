//! The rack: the GUI-thread side of the module chain. Only the model; the patchbay and the
//! inspector draw it and change it through [`Action`]s.

use crate::audio::engine::{Command, MAX_MODULES};
use crate::audio::module::{Module, ModuleId, ModuleShared, RackModule};
use crate::modules::{self, ModuleKind};
use crate::params::Params;
#[cfg(feature = "clap-host")]
use crate::plugins::{LoadedPlugin, PluginInfo};
#[cfg(feature = "clap-host")]
use crate::presets::SlotSettings;
use crate::presets::{CLAP_KIND, RackItemSettings, Values};
use crate::shared::Shared;
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Assignable knobs shown for a CLAP plugin.
#[cfg(feature = "clap-host")]
pub const KNOBS: usize = 4;

#[cfg(feature = "clap-host")]
pub struct ClapBody {
    pub plugin: LoadedPlugin,
    /// Plugin parameter ids on the assignable knobs.
    pub knobs: [Option<u32>; KNOBS],
}

pub enum Body {
    Native(&'static ModuleKind),
    #[cfg(feature = "clap-host")]
    Clap(Box<ClapBody>),
    /// A saved module that could not be recreated. Kept so saving does not forget it.
    Missing(Box<RackItemSettings>),
}

pub struct Item {
    pub id: ModuleId,
    pub shared: Arc<ModuleShared>,
    pub body: Body,
    /// The engine currently holds this item's audio half.
    pub live: bool,
    pub error: Option<String>,
}

impl Item {
    pub fn title(&self) -> String {
        match &self.body {
            Body::Native(kind) => kind.name.to_string(),
            #[cfg(feature = "clap-host")]
            Body::Clap(c) => c.plugin.name.clone(),
            Body::Missing(s) => s.clap.as_ref().and_then(|c| c.name.clone()).unwrap_or_else(|| s.kind.clone()),
        }
    }

    /// The native kind's id, `"clap"`, or the saved kind of a missing module.
    pub fn kind_id(&self) -> &str {
        match &self.body {
            Body::Native(kind) => kind.id,
            #[cfg(feature = "clap-host")]
            Body::Clap(_) => CLAP_KIND,
            Body::Missing(s) => &s.kind,
        }
    }

    /// Whether this item is the module `s` describes, whatever its values.
    fn holds(&self, s: &RackItemSettings) -> bool {
        match &self.body {
            Body::Native(kind) => kind.id == s.kind,
            #[cfg(feature = "clap-host")]
            Body::Clap(c) => {
                s.kind == CLAP_KIND && s.clap.as_ref().is_some_and(|clap| clap.bundle.as_ref() == Some(&c.plugin.bundle) && clap.plugin_id.as_ref() == Some(&c.plugin.plugin_id))
            }
            Body::Missing(_) => false,
        }
    }

    /// Take the saved On, Blend and values.
    fn set(&mut self, s: &RackItemSettings) {
        self.shared.enabled.store(s.enabled, Ordering::Relaxed);
        self.shared.mix.set(s.mix);
        match &mut self.body {
            Body::Native(_) => s.values.apply(&[&self.shared.params]),
            #[cfg(feature = "clap-host")]
            Body::Clap(c) => {
                let Some(clap) = &s.clap else { return };
                for (&pid, &v) in &clap.values {
                    c.plugin.set_value(pid, v);
                }
                if !clap.knobs.is_empty() {
                    c.knobs = std::array::from_fn(|k| clap.knobs.get(k).copied());
                }
            }
            Body::Missing(_) => {}
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

/// An edit of the chain. Positions are indices into [`Rack::items`]; `None` or past the end
/// means the end of the chain.
pub enum Action {
    Add(&'static ModuleKind, Option<usize>),
    #[cfg(feature = "clap-host")]
    AddClap(PluginInfo, Option<usize>),
    Remove(ModuleId),
    /// Move a module so that it ends up at this index.
    MoveTo(ModuleId, usize),
}

pub struct Rack {
    items: Vec<Item>,
    next_id: ModuleId,
    seed: u64,
    error: Option<String>,
    /// Plugins waiting for the audio thread to hand their processor back before deactivating.
    #[cfg(feature = "clap-host")]
    pending: Vec<LoadedPlugin>,
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
        }
    }

    /// The modules in playing order.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [Item] {
        &mut self.items
    }

    pub fn item(&self, id: ModuleId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ModuleId) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Why the last edit could not be done (e.g. the rack is full).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn new_item(&mut self, body: Body, shared: Arc<ModuleShared>) -> Item {
        let id = self.next_id;
        self.next_id += 1;
        Item { id, shared, body, live: false, error: None }
    }

    /// Recreate one saved module.
    fn restore_item(&mut self, s: &RackItemSettings, ctx: &egui::Context) -> Item {
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
        item
    }

    /// Recreate the modules saved in the settings file.
    pub fn restore(&mut self, saved: &[RackItemSettings], ctx: &egui::Context) {
        for s in saved.iter().take(MAX_MODULES) {
            let item = self.restore_item(s, ctx);
            self.items.push(item);
        }
    }

    /// Replace the rack with a saved one (a user preset). A module the saved rack also has keeps
    /// running and only takes the saved values, so presets over the same chain switch seamlessly.
    pub fn load(&mut self, saved: &[RackItemSettings], mut live: Option<Live>, ctx: &egui::Context) {
        self.error = None;
        let saved = &saved[..saved.len().min(MAX_MODULES)];
        let mut old: Vec<Option<Item>> = std::mem::take(&mut self.items).into_iter().map(Some).collect();
        let kept: Vec<Option<Item>> = saved
            .iter()
            .map(|s| {
                let slot = old.iter_mut().find(|o| o.as_ref().is_some_and(|item| item.holds(s)))?;
                let mut item = slot.take()?;
                item.set(s);
                Some(item)
            })
            .collect();
        // The engine must let go of the old modules before the new ones arrive, or it could be full.
        for item in old.into_iter().flatten() {
            self.discard(item, &mut live);
        }
        for (s, kept) in saved.iter().zip(kept) {
            let item = match kept {
                Some(item) => item,
                None => {
                    let mut item = self.restore_item(s, ctx);
                    self.go_live(&mut item, &mut live);
                    item
                }
            };
            self.items.push(item);
        }
        self.send_order(&mut live);
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

    /// Hand a new item's audio half to the running engine, if there is one.
    fn go_live(&self, item: &mut Item, live: &mut Option<Live>) {
        if let Some(l) = live.as_mut()
            && let Some(module) = item.make(l.sr, self.seed, l.app)
        {
            item.live = l.tx.push(Command::Add(RackModule { id: item.id, module, shared: item.shared.clone() })).is_ok();
        }
    }

    fn add(&mut self, body: Body, shared: Arc<ModuleShared>, at: Option<usize>, live: &mut Option<Live>) -> ModuleId {
        let mut item = self.new_item(body, shared);
        let id = item.id;
        self.go_live(&mut item, live);
        let at = at.unwrap_or(usize::MAX).min(self.items.len());
        self.items.insert(at, item);
        self.send_order(live);
        id
    }

    fn remove(&mut self, id: ModuleId, live: &mut Option<Live>) {
        let Some(i) = self.items.iter().position(|item| item.id == id) else { return };
        let item = self.items.remove(i);
        self.discard(item, live);
    }

    /// Take an item that has left the list out of the engine.
    fn discard(&mut self, item: Item, live: &mut Option<Live>) {
        let id = item.id;
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

    /// Do an edit, on the running engine too if there is one. Returns the id of an added module.
    pub fn apply(&mut self, action: Action, live: &mut Option<Live>, _ctx: &egui::Context) -> Option<ModuleId> {
        self.error = None;
        let full = self.items.len() >= MAX_MODULES;
        match action {
            Action::Add(..) if full => self.error = Some(format!("The rack holds at most {MAX_MODULES} modules.")),
            Action::Add(kind, at) => return Some(self.add(Body::Native(kind), ModuleShared::new(kind.defs), at, live)),
            #[cfg(feature = "clap-host")]
            Action::AddClap(..) if full => self.error = Some(format!("The rack holds at most {MAX_MODULES} modules.")),
            #[cfg(feature = "clap-host")]
            Action::AddClap(info, at) => match LoadedPlugin::load(&info.bundle, &info.id, &info.name, _ctx.clone()) {
                Ok(plugin) => {
                    let knobs = std::array::from_fn(|k| plugin.params.get(k).map(|m| m.id));
                    return Some(self.add(Body::Clap(Box::new(ClapBody { plugin, knobs })), ModuleShared::new(&[]), at, live));
                }
                Err(e) => self.error = Some(e),
            },
            Action::Remove(id) => self.remove(id, live),
            Action::MoveTo(id, to) => {
                let i = self.items.iter().position(|item| item.id == id)?;
                let item = self.items.remove(i);
                self.items.insert(to.min(self.items.len()), item);
                self.send_order(live);
            }
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::engine::{Engine, RackLink};
    use crate::presets::Settings;

    /// Drive the rack the way the patchbay does, against a real engine.
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
        edit(&mut rack, Action::Add(modules::kind("lofi").unwrap(), None));
        edit(&mut rack, Action::MoveTo(4, 0));
        assert_eq!(ids(&rack), [4, 1, 2, 3]);
        assert_eq!(run(&mut engine), [4, 1, 2, 3]);
        // Inserted in the middle, then dragged past the end.
        assert_eq!(edit(&mut rack, Action::Add(modules::kind("metal").unwrap(), Some(2))), Some(5));
        assert_eq!(ids(&rack), [4, 1, 5, 2, 3]);
        edit(&mut rack, Action::MoveTo(5, 99));
        assert_eq!(run(&mut engine), [4, 1, 2, 3, 5]);
        edit(&mut rack, Action::Remove(5));
        rack.idle(Some(&mut garbage_rx));

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

        // Loading a preset's rack keeps the modules both racks have (here 2 and 4, the second
        // taking the preset's values) and swaps the rest.
        let mut preset = Settings::default().rack_items();
        preset[2].values.0.insert("lofi_bits".into(), 7.0);
        rack.load(&preset, Some(Live { sr: 48000.0, tx: &mut tx, app: &app }), &ctx);
        assert_eq!(ids(&rack), [6, 2, 4]);
        assert_eq!(run(&mut engine), [6, 2, 4]);
        assert!(rack.items.iter().all(|i| i.live));
        assert_eq!(rack.items[2].shared.params.get_index(1), 7.0);
        assert_eq!(rack.save(), preset);
        rack.idle(Some(&mut garbage_rx));

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
