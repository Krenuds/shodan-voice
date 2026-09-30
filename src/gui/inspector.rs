//! The inspector: everything about the selected patchbay node, as a panel of the light chassis.
//! MIC (microphone, input gain, noise gate), OUTPUT (output and monitor devices, output gain,
//! latency), or one module (On, Blend, its knobs by group, and what is special to its kind).
//! A help strip at the bottom explains whatever control the pointer is on.

use super::knob::Knob;
use super::param_ui::{self, KnobHits};
#[cfg(feature = "clap-host")]
use super::rack::ClapBody;
use super::rack::{Action, Body, Item, Rack};
use super::state::{Selection, UiState};
use super::theme;
use super::widgets::{self, KeyStyle};
use crate::audio::io::{DeviceInfo, Running};
use crate::audio::module::ModuleId;
use crate::lexicon;
use crate::params::Params;
use crate::presets::Settings;
use crate::shared::Shared;
use crate::speech;
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Layout, Rect, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::sync::atomic::Ordering;

/// The 8 px grid every gap is a multiple of.
const GRID: f32 = theme::GRID;
/// Height of the one-line help strip at the bottom.
const HELP_H: f32 = 5.0 * GRID;
/// Below this window height the help strip is left out, so the knobs keep the room.
const HELP_MIN_WINDOW_H: f32 = 700.0;
/// Room above a key for its LED (see `widgets::key`), so keys without one line up with it.
const LED_ROW: f32 = 9.0;
/// The panel is wide enough for Power, Blend, Level and Remove on one row.
const WIDE_ROW: f32 = 350.0;

/// The inspector's own state between frames.
#[derive(Default)]
pub struct InspectorState {
    /// Help for the control last turned; shown when the pointer is on nothing else.
    touched: Option<Help>,
    /// The selection `touched` belongs to; a new selection forgets it.
    seen: Option<Selection>,
    /// A module whose Remove key was pressed once and waits for confirmation.
    confirm_remove: Option<ModuleId>,
    /// Filter of a CLAP plugin's parameter list.
    #[cfg(feature = "clap-host")]
    param_filter: String,
    /// Filter inside the knob-assign menus.
    #[cfg(feature = "clap-host")]
    assign_filter: String,
}

pub struct InspectorCtx<'a> {
    pub rack: &'a mut Rack,
    pub shared: &'a Shared,
    /// Device choices; changing one sets `InspectorOut::restart_audio`.
    pub settings: &'a mut Settings,
    pub inputs: &'a [DeviceInfo],
    pub outputs: &'a [DeviceInfo],
    pub running: Option<&'a Running>,
}

#[derive(Default)]
pub struct InspectorOut {
    pub action: Option<Action>,
    /// Devices changed or the user asked for it: rebuild the audio stream (and save settings).
    pub restart_audio: bool,
    /// Re-list the audio devices.
    pub refresh_devices: bool,
}

/// One line of the help strip.
#[derive(Clone)]
struct Help {
    title: String,
    text: String,
}

impl Help {
    fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self { title: title.into(), text: text.into() }
    }
}

/// What the controls drawn this frame want the help strip to say.
#[derive(Default)]
struct Hints {
    hover: Option<Help>,
    touched: Option<Help>,
}

impl Hints {
    /// Explain `resp` while the pointer is on it.
    fn on(&mut self, resp: &Response, title: &str, text: &str) {
        if resp.hovered() {
            self.hover = Some(Help::new(title, text));
        }
    }

    /// Take what a row of parameter knobs reported.
    fn knobs(&mut self, params: &Params, hits: KnobHits) {
        let help = |i: usize| {
            let d = &params.defs()[i];
            Help::new(format!("{} / {}", d.group, d.label), format!("{}  Default {}. Drag to turn, Shift for fine, double-click to reset.", d.help, param_ui::format_value(d, d.default)))
        };
        if let Some(i) = hits.hovered {
            self.hover = Some(help(i));
        }
        if let Some(i) = hits.touched {
            self.touched = Some(help(i));
        }
    }
}

pub fn show(ui: &mut Ui, c: InspectorCtx, st: &mut UiState) -> InspectorOut {
    let mut out = InspectorOut::default();
    let mut hints = Hints::default();
    if st.inspector.seen != Some(st.selected) {
        st.inspector.seen = Some(st.selected);
        st.inspector.touched = None;
        st.inspector.confirm_remove = None;
    }

    let show_help = ui.ctx().content_rect().height() >= HELP_MIN_WINDOW_H;
    let reserve = if show_help { HELP_H + GRID } else { 0.0 };
    let body_h = (ui.available_height() - reserve).max(120.0);
    egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(GRID, GRID);
        match st.selected {
            Selection::Input => input(ui, c, st, &mut hints, &mut out),
            Selection::Output => output(ui, c, st, &mut hints, &mut out),
            Selection::Module(id) => {
                let place = c.rack.items().iter().position(|i| i.id == id).map(|i| (i + 1, c.rack.items().len()));
                let running = c.running.is_some();
                if let Some(item) = c.rack.item_mut(id) {
                    module(ui, item, place, c.shared, running, st, &mut hints, &mut out);
                }
            }
        }
        ui.add_space(GRID);
    });

    if let Some(t) = hints.touched {
        st.inspector.touched = Some(t);
    }
    if show_help {
        let gap = ui.available_height() - HELP_H;
        if gap > 0.0 {
            ui.add_space(gap);
        }
        help_strip(ui, hints.hover.as_ref().or(st.inspector.touched.as_ref()));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Pieces of the panel.

/// Tracked monospace caps, like a label printed on the chassis.
fn silk(text: &str) -> RichText {
    theme::silk(text)
}

/// A thin printed rule from `x0` to `x1` at `y`.
fn rule(ui: &Ui, x0: f32, x1: f32, y: f32) {
    if x1 > x0 {
        ui.painter().hline(x0..=x1, y, Stroke::new(1.0, theme::RULE));
    }
}

/// A silkscreen section title with a rule running to the right edge.
fn section(ui: &mut Ui, title: &str) {
    ui.add_space(GRID);
    ui.horizontal(|ui| {
        let l = ui.label(silk(title));
        rule(ui, l.rect.right() + GRID, ui.max_rect().right(), l.rect.center().y);
    });
}

/// A section that folds: silkscreen title, rule, optional controls on the right (e.g. "reset"),
/// and its body. Open or closed is remembered per `id`.
fn fold(ui: &mut Ui, id: Id, title: &str, default_open: bool, right: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui)) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    ui.add_space(GRID);
    let toggled = ui
        .horizontal(|ui| {
            let (tri, t_resp) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::click());
            let l = ui.add(egui::Label::new(silk(title)).sense(Sense::click()));
            let hot = t_resp.hovered() || l.hovered();
            paint_triangle(ui, tri, state.openness(ui.ctx()), if hot { theme::ORANGE } else { theme::INK_DIM });
            let right_edge = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                right(ui);
                ui.min_rect().left()
            });
            rule(ui, l.rect.right() + GRID, right_edge.inner - GRID, l.rect.center().y);
            t_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() | l.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
        })
        .inner;
    if toggled {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, body);
}

/// The fold arrow: pointing right when closed, down when open.
fn paint_triangle(ui: &Ui, rect: Rect, openness: f32, color: Color32) {
    let c = rect.center();
    let angle = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| c + egui::emath::Rot2::from_angle(angle) * v;
    let pts = vec![rot(vec2(-3.0, -4.0)), rot(vec2(4.0, 0.0)), rot(vec2(-3.0, 4.0))];
    ui.painter().add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Knobs on the 8 px grid, wrapping to the panel width.
fn knob_grid<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0 * GRID, GRID);
        add(ui)
    })
    .inner
}

/// A small flat text button in silkscreen style, for secondary actions like "reset".
fn small_key(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, egui::Button::new(silk(text)).small().frame_when_inactive(false))
}

/// A lamp that only shows state.
fn lamp(ui: &mut Ui, color: Color32, lit: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    let p = ui.painter();
    if lit {
        p.circle_filled(rect.center(), 6.0, color.gamma_multiply(0.25));
    }
    p.circle_filled(rect.center(), 4.0, if lit { color } else { theme::KNOB_TRACK });
    resp
}

/// Title block: kind colour on a bit of LCD, a silkscreen tag, the big name and one line of what
/// the node does.
fn header(ui: &mut Ui, tag: &str, title: &str, blurb: &str, color: Color32) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 48.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(3), theme::LCD);
        p.rect_filled(Rect::from_center_size(rect.center(), vec2(4.0, rect.height() - 10.0)), CornerRadius::same(1), color);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(silk(tag));
            ui.add(egui::Label::new(RichText::new(title).monospace().size(20.0).strong().color(theme::INK)).wrap());
            ui.add(egui::Label::new(RichText::new(blurb).size(12.0).color(theme::INK_DIM)).wrap());
        });
    });
}

/// A warning line with a lamp.
fn warning(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal_top(|ui| {
        lamp(ui, color, true);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap());
    });
}

/// A dB readout on its own bit of LCD: white, red near clipping.
fn db_readout(ui: &mut Ui, level: f32) -> Response {
    let db = (20.0 * level.max(1e-6).log10()).max(-60.0);
    let color = if db > -1.0 { theme::LCD_RED } else { theme::LCD_WHITE };
    widgets::lcd_frame(ui, |ui| widgets::seg_display(ui, &format!("{db:5.1}"), 14.0, color)).response
}

const LEVEL_HELP: &str = "Peak level in dBFS. Red at the right end means it is close to clipping.";

/// A level meter `width` wide with its peak in dB under it.
fn level(ui: &mut Ui, h: &mut Hints, label: &str, level: f32, width: f32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = GRID / 2.0;
        ui.label(silk(label));
        let m = widgets::pixel_meter(ui, level, vec2(width, 10.0), false);
        let r = db_readout(ui, level);
        h.on(&m, label, LEVEL_HELP);
        h.on(&r, label, LEVEL_HELP);
    });
}

/// The same on one line, filling the width: for narrow panels.
fn level_line(ui: &mut Ui, h: &mut Hints, label: &str, level: f32) {
    ui.horizontal(|ui| {
        ui.label(silk(label));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = db_readout(ui, level);
            let m = widgets::pixel_meter(ui, level, vec2(ui.available_width(), 10.0), false);
            h.on(&m, label, LEVEL_HELP);
            h.on(&r, label, LEVEL_HELP);
        });
    });
}

/// The help strip: one line of LCD at the bottom that explains whatever the pointer is on.
fn help_strip(ui: &mut Ui, help: Option<&Help>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), HELP_H), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(4), theme::LCD);
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let (title, text, text_color) = match help {
        Some(h) => (h.title.to_uppercase(), h.text.as_str(), theme::LCD_WHITE),
        None => ("HELP".to_string(), "Point at any control to see what it does.", theme::LCD_DIM.gamma_multiply(3.0)),
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&title, 0.0, egui::TextFormat::simple(FontId::monospace(10.0), theme::ORANGE));
    job.append(text, GRID, egui::TextFormat::simple(FontId::proportional(12.0), text_color));
    job.wrap = egui::text::TextWrapping { max_width: inner.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('\u{2026}') };
    let galley = p.layout_job(job);
    let elided = galley.elided;
    p.galley(pos2(inner.left(), rect.center().y - galley.size().y / 2.0), galley, text_color);
    if let (true, Some(h)) = (elided, help) {
        resp.on_hover_text(&h.text);
    }
}

// ---------------------------------------------------------------------------------------------
// MIC

fn input(ui: &mut Ui, c: InspectorCtx, st: &mut UiState, h: &mut Hints, out: &mut InspectorOut) {
    header(ui, "Start of the chain", "MIC", "Your microphone. The voice enters the rack here.", theme::LCD_WHITE);

    section(ui, "Device");
    let combo = device_combo(ui, "in_dev", c.inputs, &mut c.settings.input_device);
    out.restart_audio |= combo.0;
    h.on(&combo.1, "Microphone", "The device the voice comes from. Changing it restarts the audio.");
    let r = widgets::key(ui, "REFRESH DEVICES", KeyStyle::Light, None);
    h.on(&r, "Refresh devices", "List the audio devices again, e.g. after plugging in a headset.");
    out.refresh_devices |= r.clicked();

    section(ui, "Level");
    ui.horizontal(|ui| {
        let hits = knob_grid(ui, |ui| param_ui::param_knobs(ui, &c.shared.io, Some("Input")));
        h.knobs(&c.shared.io, hits);
        let width = (ui.available_width() - GRID).clamp(64.0, 160.0);
        level(ui, h, "Input", st.levels.input, width);
    });

    section(ui, "Noise gate");
    let core = c.rack.items().iter().find(|i| i.kind_id() == "shodan_core");
    match core {
        Some(core) => {
            let params = &core.shared.params;
            let id = core.id;
            ui.horizontal_top(|ui| {
                let hits = knob_grid(ui, |ui| param_ui::param_knobs(ui, params, Some("Input")));
                h.knobs(params, hits);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new("The gate lives in SHODAN Core. Raise it until room noise and breaths stop setting off glitches.").size(12.0).color(theme::INK_DIM)).wrap());
                    let r = widgets::key(ui, "SHOW CORE", KeyStyle::Light, None);
                    h.on(&r, "Show core", "Select the SHODAN Core module to see all its knobs.");
                    if r.clicked() {
                        st.selected = Selection::Module(id);
                    }
                });
            });
        }
        None => {
            ui.add(egui::Label::new(RichText::new("No noise gate: add a SHODAN Core to the rack to get one.").size(12.0).color(theme::INK_DIM)).wrap());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// OUTPUT

fn output(ui: &mut Ui, c: InspectorCtx, st: &UiState, h: &mut Hints, out: &mut InspectorOut) {
    header(ui, "End of the chain", "OUTPUT", "Where the finished voice goes, and an optional headphone monitor.", theme::LCD_WHITE);

    section(ui, "Main output");
    let combo = device_combo(ui, "out_dev", c.outputs, &mut c.settings.output_device);
    out.restart_audio |= combo.0;
    h.on(&combo.1, "Output", "Where the processed voice is sent. Pick the virtual cable to use it as a mic in other apps.");
    cable_hints(ui, &c);

    section(ui, "Monitor");
    ui.horizontal(|ui| {
        let on = c.settings.monitor;
        let led = widgets::led_toggle(ui, on, "Also play the result to headphones.");
        let l = ui.add(egui::Label::new(RichText::new(if on { "Headphones on" } else { "Headphones off" }).size(12.0)).sense(Sense::click()));
        if led || l.clicked() {
            c.settings.monitor = !on;
            out.restart_audio = true;
        }
        h.on(&l, "Monitor", "Also play the result to headphones, so you hear what the others hear. Click the lamp to switch.");
    });
    ui.add_enabled_ui(c.settings.monitor, |ui| {
        let combo = device_combo(ui, "mon_dev", c.outputs, &mut c.settings.monitor_device);
        out.restart_audio |= combo.0;
        h.on(&combo.1, "Monitor device", "The headphones the monitor plays to.");
    });

    ui.horizontal(|ui| {
        let r = widgets::key(ui, "REFRESH", KeyStyle::Light, None);
        h.on(&r, "Refresh devices", "List the audio devices again, e.g. after plugging in a headset.");
        out.refresh_devices |= r.clicked();
        let r = widgets::key(ui, "RESTART AUDIO", KeyStyle::Light, None);
        h.on(&r, "Restart audio", "Stop and start the audio stream. Try this if the sound stutters or a device went away.");
        out.restart_audio |= r.clicked();
    });

    section(ui, "Level");
    ui.horizontal(|ui| {
        let hits = knob_grid(ui, |ui| param_ui::param_knobs(ui, &c.shared.io, Some("Output")));
        h.knobs(&c.shared.io, hits);
        let width = (ui.available_width() - GRID).clamp(64.0, 160.0);
        level(ui, h, "Output", st.levels.output, width);
    });
    ui.add(egui::Label::new(RichText::new("After the output gain, a limiter keeps the signal below 0 dBFS, so it never clips.").size(11.0).color(theme::INK_DIM)).wrap());

    section(ui, "Latency");
    match c.running {
        Some(_) => {
            let m = &c.shared.meters;
            let (input, dsp) = (m.input_ms.get(), m.dsp_ms.get());
            let tip = format!(
                "Mic driver {input:.1} ms + rack {dsp:.1} ms (SHODAN Core: Grain/2) + buffered & output driver: main {:.1} ms, monitor {:.1} ms.\nLower the Grain knob to cut the effect's share. Windows shared-mode audio adds ~10 ms per device.",
                m.output_ms[0].get(),
                m.output_ms[1].get()
            );
            let rows: &[(usize, &str)] = if c.settings.monitor { &[(0, "Main"), (1, "Monitor")] } else { &[(0, "Main")] };
            for &(k, name) in rows {
                let total = st.levels.latency[k];
                let color = if total > 60.0 { theme::ORANGE } else { theme::LCD_WHITE };
                let r = ui
                    .horizontal(|ui| {
                        ui.allocate_ui_with_layout(vec2(8.0 * GRID, 3.0 * GRID), Layout::left_to_right(Align::Center), |ui| {
                            ui.set_min_width(8.0 * GRID);
                            ui.label(silk(name));
                        });
                        widgets::lcd_frame(ui, |ui| widgets::seg_display(ui, &format!("{total:3.0}"), 16.0, color));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(RichText::new("ms").monospace().size(11.0).color(theme::INK_DIM));
                            ui.add(egui::Label::new(RichText::new(format!("mic {input:.0} + fx {dsp:.0} + out {:.0}", m.output_ms[k].get())).monospace().size(11.0).color(theme::INK_DIM)).wrap());
                        });
                    })
                    .response
                    .on_hover_text(&tip);
                h.on(&r, "Latency", "How late your voice comes out: microphone driver, then the rack, then the output buffer and driver. Above 60 ms it starts to feel like an echo.");
            }
        }
        None => {
            ui.label(RichText::new("Audio is stopped.").size(12.0).color(theme::INK_DIM));
        }
    }
}

fn cable_hints(ui: &mut Ui, c: &InspectorCtx) {
    let has_cable = c.outputs.iter().any(|d| is_cable_playback(&d.name));
    let out_name = c.settings.output_device.as_ref().and_then(|id| c.outputs.iter().find(|d| &d.id == id)).map(|d| d.name.clone()).unwrap_or_default();
    if !has_cable {
        ui.horizontal_top(|ui| {
            lamp(ui, theme::WARN, true);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new("To use SHODAN as a mic in Discord or OBS, install").size(12.0).color(theme::INK));
                ui.hyperlink_to(RichText::new("VB-Audio Virtual Cable").size(12.0), "https://vb-audio.com/Cable/");
                ui.label(RichText::new("then pick it as Output here and \u{201C}CABLE Output\u{201D} as the mic there.").size(12.0).color(theme::INK));
            });
        });
    } else if !is_cable_playback(&out_name) {
        ui.horizontal_top(|ui| {
            lamp(ui, theme::ORANGE, false);
            ui.add(
                egui::Label::new(
                    RichText::new("Tip: set Output to the VB-Audio Virtual Cable device, then select \u{201C}CABLE Output\u{201D} as your mic in Discord or OBS.").size(12.0).color(theme::INK_DIM),
                )
                .wrap(),
            );
        });
    }
}

// ---------------------------------------------------------------------------------------------
// A module

/// One line on what a module kind does.
fn blurb(item: &Item) -> String {
    match &item.body {
        Body::Native(kind) => match kind.id {
            "shodan_core" => "The SHODAN voice itself: glitches, pitch and formants.".into(),
            "metal" => "A flanger: the hollow, metallic sweep.".into(),
            "lofi" => "Bit and sample-rate crush: cheap-speaker grit.".into(),
            "titobot" => "Robot language from speech: it hears your words and answers in beeps.".into(),
            _ => "A built-in module.".into(),
        },
        #[cfg(feature = "clap-host")]
        Body::Clap(c) => {
            let file = c.plugin.bundle.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            format!("CLAP plugin  ·  {file}")
        }
        Body::Missing(_) => "Could not be loaded.".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn module(ui: &mut Ui, item: &mut Item, place: Option<(usize, usize)>, shared: &Shared, running: bool, st: &mut UiState, h: &mut Hints, out: &mut InspectorOut) {
    let id = item.id;
    let tag = match place {
        Some((n, of)) => format!("Module {n:02} / {of:02}"),
        None => "Module".into(),
    };
    header(ui, &tag, &item.title().to_uppercase(), &blurb(item), theme::kind_color(item.kind_id()));

    let missing = matches!(item.body, Body::Missing(_));
    if !missing {
        top_row(ui, item, st, h, out);
        if running && !item.live {
            warning(ui, theme::DANGER, "Not running: the audio engine does not have this module, so it is skipped. Restart audio to try again.");
        }
        if let Some(e) = &item.error {
            warning(ui, theme::DANGER, e);
        }
    }

    match &mut item.body {
        Body::Native(kind) => {
            let params = &item.shared.params;
            for g in param_ui::groups(params) {
                let changed = param_ui::group_changed(params, g);
                let mut reset: Option<Response> = None;
                fold(
                    ui,
                    Id::new(("insp_group", kind.id, g)),
                    g,
                    true,
                    |ui| reset = Some(small_key(ui, "reset", changed)),
                    |ui| {
                        let hits = knob_grid(ui, |ui| param_ui::param_knobs(ui, params, Some(g)));
                        h.knobs(params, hits);
                    },
                );
                if let Some(r) = reset {
                    h.on(&r, &format!("Reset {g}"), &format!("Put every {g} knob of this module back to its default."));
                    if r.clicked() {
                        param_ui::reset_group(params, g);
                    }
                }
            }
            if kind.id == "titobot" {
                speech_status(ui, shared, running, h);
                word_list(ui, id, shared, running, h);
            }
        }
        #[cfg(feature = "clap-host")]
        Body::Clap(c) => clap(ui, id, c, &mut item.error, st, h),
        Body::Missing(saved) => {
            section(ui, "Not loaded");
            ui.add(
                egui::Label::new(
                    RichText::new("This module could not be loaded, so it makes no sound. It stays in the saved rack, with its settings, until you remove it; if its plugin comes back, it loads again next start.")
                        .size(12.0)
                        .color(theme::INK),
                )
                .wrap(),
            );
            let what = match &saved.clap {
                Some(clap) => clap.bundle.as_ref().map(|b| b.display().to_string()).unwrap_or_else(|| "(no plugin saved)".into()),
                None => format!("kind \u{201C}{}\u{201D}", saved.kind),
            };
            ui.add(egui::Label::new(RichText::new(what).monospace().size(11.0).color(theme::INK_DIM)).wrap());
            if let Some(e) = &item.error {
                warning(ui, theme::DANGER, e);
            }
            remove_control(ui, id, &mut st.inspector.confirm_remove, h, out);
        }
    }
}

/// Power, Blend, the level after the module, and Remove at the far end. On a narrow panel the
/// level gets a line of its own under them.
fn top_row(ui: &mut Ui, item: &Item, st: &mut UiState, h: &mut Hints, out: &mut InspectorOut) {
    let id = item.id;
    let wide = ui.available_width() >= WIDE_ROW;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = if wide { 2.0 * GRID } else { GRID };
        let sh = &item.shared;
        let on = sh.enabled.load(Ordering::Relaxed);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = GRID / 2.0;
            ui.label(silk("Power"));
            let r = widgets::key_sized(ui, if on { "ON" } else { "OFF" }, if on { KeyStyle::Dark } else { KeyStyle::Light }, Some(on), vec2(6.0 * GRID, 4.0 * GRID));
            let help =
                if on { "On: the module is working. Click to switch it off and pass the sound through unchanged." } else { "Off: the sound passes through unchanged. Click to switch the module on." };
            h.on(&r, "Power", help);
            if r.clicked() {
                sh.enabled.store(!on, Ordering::Relaxed);
            }
        });
        let mut mix = sh.mix.get();
        let r = Knob::new(&mut mix, 0.0, 1.0, 1.0, "Blend").format(|v| format!("{:.0}%", v * 100.0)).help("Blend of this module's output with its input.").show(ui);
        if r.changed() {
            sh.mix.set(mix);
        }
        let blend_help = Help::new("Blend", "How much of this module you hear: 0% is its input untouched, 100% is only its output.");
        if r.hovered() || r.dragged() {
            h.hover = Some(blend_help.clone());
        }
        if r.changed() {
            h.touched = Some(blend_help);
        }
        if wide {
            level(ui, h, "Level", st.levels.module(id), 11.0 * GRID);
        }
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| remove_control(ui, id, &mut st.inspector.confirm_remove, h, out));
    });
    if !wide {
        level_line(ui, h, "Level", st.levels.module(id));
    }
}

/// A small light Remove key, level with the Power key, that asks once more before it takes the
/// module out of the rack.
fn remove_control(ui: &mut Ui, id: ModuleId, confirm: &mut Option<ModuleId>, h: &mut Hints, out: &mut InspectorOut) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(GRID / 2.0, GRID / 2.0);
        let asking = *confirm == Some(id);
        ui.label(silk(if asking { "Remove?" } else { " " }));
        ui.add_space(LED_ROW);
        ui.horizontal(|ui| {
            if asking {
                let yes = widgets::key(ui, "YES", KeyStyle::Orange, None);
                h.on(&yes, "Remove", "Take this module out of the rack. Its settings are lost.");
                if yes.clicked() {
                    out.action = Some(Action::Remove(id));
                    *confirm = None;
                }
                let no = widgets::key(ui, "NO", KeyStyle::Light, None);
                h.on(&no, "Keep", "Keep the module.");
                if no.clicked() {
                    *confirm = None;
                }
            } else {
                let r = widgets::key(ui, "REMOVE", KeyStyle::Light, None);
                h.on(&r, "Remove", "Take this module out of the rack. Asks once more before it does.");
                if r.clicked() {
                    *confirm = Some(id);
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------------------------
// TitoBot

/// What the speech recogniser behind TitoBot is doing.
fn speech_status(ui: &mut Ui, app: &Shared, running: bool, h: &mut Hints) {
    section(ui, "Listener");
    let s = &app.speech;
    let state = s.state.load(Ordering::Relaxed);
    let blink = (ui.input(|i| i.time) * 3.0).fract() < 0.5;
    let (color, lit, text) = match (running, state) {
        (false, _) => (theme::KNOB_TRACK, false, "Audio stopped: not listening".to_string()),
        (true, speech::LOADING) => (theme::ORANGE, blink, "Loading the speech model\u{2026}".to_string()),
        (true, speech::LISTENING) => (theme::ORANGE, true, "Listening".to_string()),
        (true, speech::FAILED) => (theme::DANGER, true, "Not listening".to_string()),
        _ => (theme::KNOB_TRACK, false, "Idle".to_string()),
    };
    if state == speech::LOADING {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
    }
    let r = ui
        .horizontal(|ui| {
            lamp(ui, color, lit);
            ui.label(RichText::new(text.to_uppercase()).monospace().size(12.0).color(if state == speech::FAILED && running { theme::DANGER } else { theme::INK }));
        })
        .response;
    h.on(&r, "Listener", "TitoBot listens with Whisper. Blinking: the model is loading. Lit: listening. Red: it could not start, see why below.");

    if !running {
        return;
    }
    match state {
        speech::LISTENING => {
            let heard = s.text();
            egui::Frame::new().fill(theme::LCD).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("HEARD").monospace().size(10.0).color(theme::ORANGE));
                    let text = if heard.is_empty() { "\u{2026}".to_string() } else { heard };
                    ui.label(RichText::new(text).monospace().size(12.0).color(theme::LCD_WHITE));
                });
            });
            let r = ui.label(RichText::new(format!("pass {:.0} ms  ·  word out {:.0} ms after the voice", s.ms.get(), s.delay_ms.get())).monospace().size(11.0).color(theme::INK_DIM));
            h.on(&r, "Speed", "Pass: how long one run of the recogniser takes. Word out: how long after you stop talking the last word reached the robot.");
        }
        speech::FAILED => {
            let raw = s.text();
            ui.add(egui::Label::new(RichText::new(plain_failure(&raw)).size(12.0).color(theme::DANGER)).wrap()).on_hover_text(raw);
        }
        _ => {}
    }
}

/// Why the recogniser failed, in words for the person using the app. The raw message goes in
/// the tooltip.
fn plain_failure(raw: &str) -> &'static str {
    let r = raw.to_lowercase();
    if r.contains("no speech recognition") {
        "Speech recognition isn't included in this version. TitoBot still plays the words you click below."
    } else if r.contains("model not found") {
        "The speech model file is missing, so TitoBot can't listen. It still plays the words you click below."
    } else {
        "Speech recognition stopped with an error. Restart audio to try again."
    }
}

/// The lexicon's word classes, as its comments group them. A word missing here is shown under
/// "Other", so the list cannot lose any.
const CLASSES: &[(&str, &[&str])] = &[
    ("Answers", &["yes", "no", "ok", "maybe", "don't know"]),
    ("Social", &["hello", "bye", "thanks", "sorry"]),
    ("People", &["me", "you"]),
    ("Actions", &["go", "come", "stop", "wait", "help", "look", "want"]),
    ("Directions", &["left", "right"]),
    ("Warnings", &["warning", "danger", "stay back", "run", "enemy"]),
    ("Places", &["here", "there"]),
    ("Qualities", &["good", "bad"]),
    ("Questions", &["what", "where"]),
    ("Time", &["now"]),
];

/// Every word TitoBot knows, as chips by class. Hover for the other forms that mean it; click to
/// hear it.
fn word_list(ui: &mut Ui, id: ModuleId, shared: &Shared, running: bool, h: &mut Hints) {
    let title = format!("Words  {}", lexicon::WORDS.len());
    fold(
        ui,
        Id::new(("insp_words", id)),
        &title,
        false,
        |_| {},
        |ui| {
            let others: Vec<&str> = lexicon::WORDS.iter().map(|w| w.text).filter(|t| !CLASSES.iter().any(|(_, ws)| ws.contains(t))).collect();
            let rows = CLASSES.iter().map(|&(c, ws)| (c, ws.to_vec())).chain((!others.is_empty()).then_some(("Other", others)));
            for (class, words) in rows {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(vec2(88.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_width(88.0);
                        ui.label(silk(class));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for text in words {
                            let Some(i) = lexicon::WORDS.iter().position(|w| w.text == text) else {
                                continue;
                            };
                            let word = &lexicon::WORDS[i];
                            let r = ui.add(egui::Button::new(RichText::new(word.text).monospace().size(11.0).color(theme::INK)).fill(theme::KEY_LIGHT).corner_radius(CornerRadius::same(9)).small());
                            let also = if word.also.is_empty() { "Only this word.".to_string() } else { format!("Also heard as: {}.", word.also.join(", ")) };
                            let click = if running { " Click to hear it." } else { "" };
                            let r = r.on_hover_text(&also);
                            h.on(&r, &format!("{class} / {}", word.text), &format!("{also}{click}"));
                            if r.clicked() && running {
                                shared.words.push(i as u32);
                            }
                        }
                    });
                });
            }
        },
    );
}

// ---------------------------------------------------------------------------------------------
// CLAP

#[cfg(feature = "clap-host")]
fn clap(ui: &mut Ui, id: ModuleId, body: &mut ClapBody, error: &mut Option<String>, st: &mut UiState, h: &mut Hints) {
    let p = &mut body.plugin;
    if p.has_editor() {
        let open = p.window.is_some();
        let r = widgets::key(ui, if open { "CLOSE EDITOR" } else { "OPEN EDITOR" }, if open { KeyStyle::Dark } else { KeyStyle::Orange }, Some(open));
        h.on(&r, "Editor", "Open the plugin's own window with all its controls.");
        if r.clicked() {
            if open {
                p.close_editor();
            } else if let Err(e) = p.open_editor() {
                *error = Some(e);
            }
        }
    }

    fold(
        ui,
        Id::new(("insp_clap_knobs", id)),
        "Knobs",
        true,
        |_| {},
        |ui| {
            knob_grid(ui, |ui| plugin_knobs(ui, body, &mut st.inspector.assign_filter, h));
        },
    );

    let n = body.plugin.params.len();
    fold(ui, Id::new(("insp_clap_params", id)), &format!("All parameters  {n}"), false, |_| {}, |ui| all_params(ui, id, body, &mut st.inspector.param_filter, h));
}

/// "Module / name" of a plugin parameter.
#[cfg(feature = "clap-host")]
fn full_name(m: &crate::plugins::slot::ParamMeta) -> String {
    if m.module.is_empty() { m.name.clone() } else { format!("{} / {}", m.module, m.name) }
}

#[cfg(feature = "clap-host")]
fn plugin_knobs(ui: &mut Ui, body: &mut ClapBody, filter: &mut String, h: &mut Hints) {
    let ClapBody { plugin, knobs } = body;
    for (k, slot) in knobs.iter_mut().enumerate() {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let meta = slot.and_then(|id| plugin.params.iter().find(|m| m.id == id)).cloned();
            match &meta {
                Some(m) => {
                    let short: String = m.name.chars().take(10).collect();
                    let full = full_name(m);
                    let mut v = plugin.get_value(m.id).unwrap_or(m.default) as f32;
                    let resp = Knob::new(&mut v, m.min as f32, m.max as f32, m.default as f32, &short)
                        .format(|v| plugin.value_text(m.id, v as f64).unwrap_or_else(|| format!("{v:.2}")).chars().take(10).collect())
                        .stepped(m.stepped)
                        .help(&full)
                        .show(ui);
                    if resp.changed() {
                        plugin.set_value(m.id, v as f64);
                    }
                    let help = Help::new(format!("Knob {} / {}", k + 1, full), "A plugin parameter on a knob. Drag to turn, double-click to reset. Pick another with the menu below.");
                    if resp.hovered() || resp.dragged() {
                        h.hover = Some(help.clone());
                    }
                    if resp.changed() {
                        h.touched = Some(help);
                    }
                }
                None => {
                    let (rect, r) = ui.allocate_exact_size(vec2(super::knob::KNOB_WIDTH, 80.0), Sense::hover());
                    ui.painter().rect_stroke(rect.shrink(6.0), CornerRadius::same(6), Stroke::new(1.0, theme::KNOB_TRACK), egui::StrokeKind::Inside);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("KNOB {}", k + 1), FontId::monospace(10.0), theme::INK_DIM);
                    h.on(&r, &format!("Knob {}", k + 1), "Empty. Put a plugin parameter on it with the menu below, or right-click one in All parameters.");
                }
            }
            let label = meta.as_ref().map_or("assign", |_| "change");
            let combo = egui::ComboBox::from_id_salt(("knob_assign", k)).width(super::knob::KNOB_WIDTH - 6.0).selected_text(silk(label)).height(360.0).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(filter).hint_text("filter\u{2026}").desired_width(200.0));
                if ui.selectable_label(slot.is_none(), RichText::new("(empty)").italics()).clicked() {
                    *slot = None;
                }
                let f = filter.to_lowercase();
                for m in plugin.params.iter().filter(|m| f.is_empty() || full_name(m).to_lowercase().contains(&f)) {
                    if ui.selectable_label(*slot == Some(m.id), full_name(m)).clicked() {
                        *slot = Some(m.id);
                    }
                }
            });
            h.on(&combo.response, &format!("Knob {}", k + 1), "Choose which plugin parameter this knob turns.");
        });
    }
}

/// Every parameter of the plugin as a compact slider, with a filter. Right-click a row to put it
/// on one of the knobs.
#[cfg(feature = "clap-host")]
fn all_params(ui: &mut Ui, id: ModuleId, body: &mut ClapBody, filter: &mut String, h: &mut Hints) {
    const ROW_H: f32 = 20.0;
    let ClapBody { plugin, knobs } = body;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(filter).hint_text("filter parameters\u{2026}").desired_width(ui.available_width() - 60.0));
        h.on(&r, "Filter", "Type part of a parameter's name to find it.");
        if small_key(ui, "clear", !filter.is_empty()).clicked() {
            filter.clear();
        }
    });
    let f = filter.to_lowercase();
    let rows: Vec<usize> = plugin.params.iter().enumerate().filter(|(_, m)| f.is_empty() || full_name(m).to_lowercase().contains(&f)).map(|(i, _)| i).collect();
    if rows.is_empty() {
        ui.label(RichText::new(if plugin.params.is_empty() { "This plugin has no parameters." } else { "Nothing matches." }).size(12.0).color(theme::INK_DIM));
        return;
    }
    egui::ScrollArea::vertical().id_salt(("insp_clap_rows", id)).max_height(320.0).auto_shrink([false, true]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for &i in &rows[range] {
            let m = plugin.params[i].clone();
            let v = plugin.get_value(m.id).unwrap_or(m.default);
            let text = plugin.value_text(m.id, v).unwrap_or_else(|| format!("{v:.2}"));
            let on_knob = knobs.iter().position(|k| *k == Some(m.id));
            ui.horizontal(|ui| {
                let name_w = (ui.available_width() * 0.42).min(150.0);
                let name = RichText::new(&m.name).size(11.5).color(theme::INK);
                ui.add_sized([name_w, ROW_H - 2.0], egui::Label::new(name).truncate()).on_hover_text(full_name(&m));
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H - 4.0), Sense::click_and_drag());
                let range = (m.max - m.min).max(1e-9);
                let mut nv = v;
                if resp.dragged() {
                    let fine = ui.input(|i| i.modifiers.shift);
                    nv += resp.drag_delta().x as f64 / rect.width() as f64 * range / if fine { 8.0 } else { 1.0 };
                }
                if resp.double_clicked() {
                    nv = m.default;
                }
                nv = nv.clamp(m.min, m.max);
                if m.stepped {
                    nv = nv.round();
                }
                if nv != v {
                    plugin.set_value(m.id, nv);
                }
                let hot = resp.hovered() || resp.dragged();
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(3), theme::KEY_LIGHT);
                let frac = ((nv - m.min) / range).clamp(0.0, 1.0) as f32;
                let mut fill = rect;
                fill.set_width(rect.width() * frac);
                p.rect_filled(fill, CornerRadius::same(3), if hot { theme::ACCENT_HOT } else { theme::KNOB_TRACK });
                if hot {
                    p.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, theme::ORANGE), egui::StrokeKind::Inside);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                let shown = if nv == v { text.clone() } else { plugin.value_text(m.id, nv).unwrap_or_else(|| format!("{nv:.2}")) };
                p.with_clip_rect(rect).text(rect.right_center() - vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, shown, FontId::monospace(10.5), theme::INK);
                if let Some(k) = on_knob {
                    p.text(rect.left_center() + vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, format!("K{}", k + 1), FontId::monospace(10.0), theme::ORANGE);
                }
                let place = match on_knob {
                    Some(k) => format!(" It is on knob {}.", k + 1),
                    None => String::new(),
                };
                h.on(&resp, &full_name(&m), &format!("Drag sideways to change, Shift for fine, double-click to reset. Right-click to put it on a knob.{place}"));
                resp.context_menu(|ui| {
                    ui.label(silk("Put on knob"));
                    for (k, slot) in knobs.iter_mut().enumerate() {
                        let current = slot.and_then(|id| plugin.params.iter().find(|p| p.id == id)).map_or("empty".to_string(), |p| p.name.clone());
                        if ui.button(format!("Knob {}  ({current})", k + 1)).clicked() {
                            *slot = Some(m.id);
                            ui.close();
                        }
                    }
                });
            });
        }
    });
}

// ---------------------------------------------------------------------------------------------

/// VB-Cable's playback end: "CABLE Input (VB-Audio Virtual Cable)" on older drivers,
/// "Speakers (VB-Audio Virtual Cable)" on newer ones. The 16-channel variant is skipped.
pub fn is_cable_playback(name: &str) -> bool {
    let n = name.to_lowercase();
    (n.contains("cable input") || n.contains("vb-audio virtual cable")) && !n.contains("16 ch")
}

/// A device picker as wide as the panel. Returns whether the choice changed, and the response
/// of the combo button.
fn device_combo(ui: &mut Ui, id: &str, devices: &[DeviceInfo], selected: &mut Option<String>) -> (bool, Response) {
    let label = match selected {
        None => "System default".to_string(),
        Some(sel) => devices.iter().find(|d| &d.id == sel).map(|d| d.name.clone()).unwrap_or_else(|| "(missing device)".into()),
    };
    let mut changed = false;
    let r = egui::ComboBox::from_id_salt(id).width(ui.available_width() - 4.0).truncate().selected_text(label).show_ui(ui, |ui| {
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
    (changed, r.response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::RackItemSettings;

    /// Draw the inspector headless for every kind of node, a few frames each, including a
    /// module that could not be loaded; and check that "reset" puts a group back.
    #[test]
    fn every_selection_draws() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let shared = Shared::default();
        let mut settings = Settings::default();
        let mut rack = Rack::new(1);
        let mut saved = settings.rack_items();
        saved.push(RackItemSettings { kind: "titobot".into(), enabled: true, mix: 1.0, ..Default::default() });
        saved.push(RackItemSettings { kind: "from_the_future".into(), enabled: true, mix: 1.0, ..Default::default() });
        rack.restore(&saved, &ctx);
        let mut st = UiState::default();
        let mut selections = vec![Selection::Input, Selection::Output];
        selections.extend(rack.items().iter().map(|i| Selection::Module(i.id)));
        for sel in selections {
            st.selected = sel;
            for _ in 0..3 {
                let mut full = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(360.0, 900.0))), ..Default::default() }, |ui| {
                    let c = InspectorCtx { rack: &mut rack, shared: &shared, settings: &mut settings, inputs: &[], outputs: &[], running: None };
                    let out = show(ui, c, &mut st);
                    assert!(out.action.is_none() && !out.restart_audio && !out.refresh_devices);
                });
                full.textures_delta.clear();
            }
        }

        let core = &rack.items()[0].shared.params;
        assert!(param_ui::group_changed(core, "Glitch"), "the first preset changes the glitch knobs");
        param_ui::reset_group(core, "Glitch");
        assert!(!param_ui::group_changed(core, "Glitch"));
    }

    /// Every word the class table names is in the lexicon, once.
    #[test]
    fn word_classes_match_the_lexicon() {
        let mut seen = Vec::new();
        for (_, words) in CLASSES {
            for w in *words {
                assert!(lexicon::WORDS.iter().any(|x| x.text == *w), "{w} is not in the lexicon");
                assert!(!seen.contains(w), "{w} is in two classes");
                seen.push(*w);
            }
        }
    }
}
