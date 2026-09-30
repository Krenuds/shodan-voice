//! GUI state shared by the panels of the main window: what is selected, meter levels held
//! between frames, and each panel's own state.

use super::rack::Rack;
use super::{inspector, patchbay, toolbar, widgets};
use crate::audio::module::ModuleId;
use crate::shared::Shared;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// What the inspector shows. MIC and OUTPUT are the fixed ends of the patchbay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Selection {
    #[default]
    Input,
    Output,
    Module(ModuleId),
}

#[derive(Default)]
pub struct UiState {
    pub selected: Selection,
    pub levels: Levels,
    pub catalog: widgets::Catalog,
    pub canvas: patchbay::CanvasState,
    pub inspector: inspector::InspectorState,
    pub toolbar: toolbar::ToolbarState,
    notice: Option<(String, Instant)>,
}

/// How long a notice (e.g. "the rack is full") stays up.
const NOTICE_SHOWN: Duration = Duration::from_secs(4);

impl UiState {
    /// Tell the user something brief, e.g. why an edit could not be done. The patchbay shows it.
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    /// The current notice and how far through its time it is (0..1), if one is up.
    pub fn notice(&self) -> Option<(&str, f32)> {
        let (text, at) = self.notice.as_ref()?;
        let t = at.elapsed().as_secs_f32() / NOTICE_SHOWN.as_secs_f32();
        (t < 1.0).then_some((text.as_str(), t))
    }

    /// Keep the selection pointing at something that exists.
    pub fn fix_selection(&mut self, rack: &Rack) {
        if let Selection::Module(id) = self.selected
            && rack.item(id).is_none()
        {
            self.selected = Selection::Output;
        }
    }
}

/// Meter readings, taken from the atomics once per frame and held with a decay so they read
/// well at 30 fps. Everything that draws a meter reads these, never the atomics.
#[derive(Default)]
pub struct Levels {
    pub input: f32,
    pub output: f32,
    /// After each module, by id.
    pub modules: HashMap<ModuleId, f32>,
    /// Smoothed end-to-end latency for [main output, monitor], ms.
    pub latency: [f32; 2],
    pub glitches: u32,
    glitch_at: Option<Instant>,
}

const DECAY: f32 = 0.85;

impl Levels {
    pub fn update(&mut self, shared: &Shared, rack: &Rack, running: bool) {
        let m = &shared.meters;
        self.input = m.input_peak.take().max(self.input * DECAY);
        self.output = m.output_peak.take().max(self.output * DECAY);
        self.modules.retain(|id, _| rack.item(*id).is_some());
        for item in rack.items() {
            let held = self.modules.entry(item.id).or_default();
            *held = item.shared.out_peak.take().max(*held * DECAY);
        }
        let glitches = m.glitches.load(Ordering::Relaxed);
        if glitches != self.glitches {
            self.glitches = glitches;
            self.glitch_at = Some(Instant::now());
        }
        if running {
            let base = m.input_ms.get() + m.dsp_ms.get();
            for k in 0..2 {
                self.latency[k] += (base + m.output_ms[k].get() - self.latency[k]) * 0.1;
            }
        }
    }

    pub fn module(&self, id: ModuleId) -> f32 {
        self.modules.get(&id).copied().unwrap_or(0.0)
    }

    /// A glitch fired in the last moment: for a flashing LED.
    pub fn glitch_flash(&self) -> bool {
        self.glitch_at.is_some_and(|t| t.elapsed() < Duration::from_millis(180))
    }
}
