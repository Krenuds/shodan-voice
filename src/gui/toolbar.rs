//! The toolbar across the top, read left to right like the device's top edge: the wordmark,
//! presets, add module, the latency LCD, the cable warning, bypass, and the speaker grille.

use super::rack::Action;
use super::state::{Selection, UiState};
use super::theme;
use super::widgets::{self, KeyStyle};
use crate::params::io::P;
use crate::presets::{self, Settings};
use crate::shared::Shared;
use eframe::egui::{self, Color32, CornerRadius, Popup, PopupCloseBehavior, RichText, Sense, Stroke, StrokeKind, pos2, vec2};

/// The toolbar's own state between frames.
#[derive(Default)]
pub struct ToolbarState {
    /// Name typed for a new user preset.
    pub new_preset: String,
    /// The user dismissed the "no virtual cable" badge.
    pub cable_hint_dismissed: bool,
}

pub struct ToolbarCtx<'a> {
    pub shared: &'a Shared,
    /// Read for the user preset list; the app does the loading, saving and deleting.
    pub settings: &'a Settings,
    /// Name of the preset last loaded or saved.
    pub preset_label: &'a str,
    /// The knobs differ from what that preset set.
    pub preset_dirty: bool,
    /// The rack holds `MAX_MODULES`: adding would fail.
    pub rack_full: bool,
    /// F8 is registered as a global hotkey.
    pub hotkey: bool,
    pub running: bool,
    /// No VB-Cable (or similar) output device is installed.
    pub cable_missing: bool,
}

#[derive(Default)]
pub struct ToolbarOut {
    /// Index into `presets::BUILTIN`.
    pub builtin: Option<usize>,
    pub load_user: Option<String>,
    pub delete_user: Option<String>,
    /// Save the current knobs and rack as a user preset of this name (new, or Update).
    pub save_as: Option<String>,
    pub action: Option<Action>,
}

/// Height of the keys and fields in the toolbar.
const ROW: f32 = 32.0;
/// Width of the speaker grille at the right edge.
const GRILLE: f32 = 88.0;
/// Width of the ACTIVE / BYPASSED key.
const EFFECT_W: f32 = 112.0;
/// Width of the latency LCD: four 7-segment cells in a framed window.
const LAT_W: f32 = 80.0;
/// Width of the cable badge, full and folded down to its LED and ×.
const BADGE_W: f32 = 196.0;
const BADGE_SMALL_W: f32 = 56.0;
/// Space between the groups on the right.
const RIGHT_GAP: f32 = 2.0 * theme::GRID;

/// A control group: its name printed small above it, the controls in a row underneath.
/// Works in both left-to-right and right-to-left rows; the label is always flush left.
fn group<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let align = if ui.layout().prefer_right_to_left() { egui::Align::Max } else { egui::Align::Min };
    ui.with_layout(egui::Layout::top_down(align), |ui| {
        let galley = ui.painter().layout_no_wrap(label.to_uppercase(), egui::FontId::monospace(theme::SILK_SIZE), theme::INK_DIM);
        let label_h = galley.size().y + 3.0;
        ui.add_space(label_h);
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GRID;
            add(ui)
        });
        ui.painter().galley(pos2(row.response.rect.left(), row.response.rect.top() - label_h), galley, theme::INK_DIM);
        row.inner
    })
    .inner
}

fn wordmark(ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GRID;
            ui.label(RichText::new("S.H.O.D.A.N.").font(theme::bold(22.0)).color(theme::INK).extra_letter_spacing(0.5));
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 22.0), Sense::hover());
            ui.painter().rect_filled(egui::Rect::from_center_size(pos2(r.center().x, r.bottom() - 7.0), egui::Vec2::splat(7.0)), 0.0, theme::ORANGE);
        });
        ui.label(theme::silk("voice unit"));
    });
}

pub fn toolbar(ui: &mut egui::Ui, t: ToolbarCtx, st: &mut UiState) -> ToolbarOut {
    let mut out = ToolbarOut::default();
    let full = ui.max_rect();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0 * theme::GRID;
        ui.spacing_mut().interact_size.y = ROW;
        wordmark(ui);

        group(ui, "preset", |ui| {
            preset_combo(ui, &t, &mut out);
            save_key(ui, &t, &mut st.toolbar, &mut out);
        });

        group(ui, "module", |ui| {
            let add = ui.add_enabled_ui(!t.rack_full, |ui| widgets::key(ui, "+ add", KeyStyle::Dark, None)).inner;
            let add = add.on_hover_text("Add a module at the end of the chain").on_disabled_hover_text("The rack is full (16 modules)");
            Popup::menu(&add).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                out.action = widgets::add_module_menu(ui, &mut st.catalog, None);
            });
        });

        // The right-hand cluster gets what the left side left over. When that is not enough the
        // speaker grille goes first, then the cable badge folds to its LED, then the latency LCD.
        let avail = ui.available_width() - theme::GRID;
        let badge = t.cable_missing && !st.toolbar.cable_hint_dismissed;
        let mut need = EFFECT_W;
        let lat_fits = need + RIGHT_GAP + LAT_W <= avail;
        if lat_fits {
            need += RIGHT_GAP + LAT_W;
        }
        let badge_full = badge && need + RIGHT_GAP + BADGE_W <= avail;
        if badge {
            need += RIGHT_GAP + if badge_full { BADGE_W } else { BADGE_SMALL_W };
        }
        let grille = need + RIGHT_GAP + GRILLE <= avail;

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
            ui.spacing_mut().item_spacing.x = RIGHT_GAP;
            if grille {
                // Speaker grille in the corner, bleeding into the panel margin like the real one.
                let (g, _) = ui.allocate_exact_size(vec2(GRILLE, ui.available_height()), Sense::hover());
                let area = egui::Rect::from_min_max(pos2(g.left(), full.top() - 6.0), pos2(full.right() + 10.0, full.bottom() + 6.0));
                widgets::paint_speaker_grille(&ui.painter_at(area.expand(1.0)), area);
            }

            let bypass = t.shared.io.get(P::Bypass) > 0.5;
            group(ui, if t.hotkey { "effect · f8" } else { "effect" }, |ui| {
                let hint = if t.hotkey { "Toggle the effect (F8 works globally, even in a game)" } else { "Toggle the effect" };
                let (text, style) = if bypass { ("bypassed", KeyStyle::Light) } else { ("active", KeyStyle::Orange) };
                if widgets::key_sized(ui, text, style, None, vec2(EFFECT_W, ROW)).on_hover_text(hint).clicked() {
                    t.shared.io.set(P::Bypass, if bypass { 0.0 } else { 1.0 });
                }
            });

            if lat_fits {
                group(ui, "lat ms", |ui| latency(ui, &t, st));
            }

            if badge {
                group(ui, "route", |ui| {
                    if cable_badge(ui, &mut st.toolbar.cable_hint_dismissed, badge_full) {
                        st.selected = Selection::Output;
                    }
                });
            }
        });
    });
    out
}

/// The preset list: built-ins, then the user's own with a delete key each. `*` marks edits.
fn preset_combo(ui: &mut egui::Ui, t: &ToolbarCtx, out: &mut ToolbarOut) {
    let shown = if t.preset_dirty { format!("{} *", t.preset_label) } else { t.preset_label.to_string() };
    let combo = egui::ComboBox::from_id_salt("preset").width(176.0).height(420.0).selected_text(RichText::new(shown).strong());
    let r = combo.show_ui(ui, |ui| {
        ui.set_min_width(240.0);
        ui.label(theme::silk("built-in"));
        for (i, p) in presets::BUILTIN.iter().enumerate() {
            if ui.selectable_label(t.preset_label == p.name, p.name).clicked() {
                out.builtin = Some(i);
            }
        }
        if !t.settings.user_presets.is_empty() {
            ui.separator();
            ui.label(theme::silk("yours"));
        }
        for name in t.settings.user_presets.keys() {
            let row = ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = theme::GRID;
                let del = widgets::key_sized(ui, "×", KeyStyle::Light, None, vec2(24.0, 22.0)).on_hover_text(format!("Delete \"{name}\""));
                let pick = ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| ui.selectable_label(t.preset_label == name, name)).inner;
                (del.clicked(), pick.clicked())
            });
            let (del, pick) = row.inner;
            if del {
                out.delete_user = Some(name.clone());
            } else if pick {
                out.load_user = Some(name.clone());
            }
        }
    });
    if t.preset_dirty {
        r.response.on_hover_text("Changed since the preset was loaded");
    }
}

/// `SAVE…`: a popup with a name field, "Save new" and "Update <loaded preset>".
fn save_key(ui: &mut egui::Ui, t: &ToolbarCtx, tb: &mut ToolbarState, out: &mut ToolbarOut) {
    let key = widgets::key(ui, "save…", KeyStyle::Light, None).on_hover_text("Save the knobs and the rack as a preset");
    Popup::menu(&key).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(248.0);
        ui.spacing_mut().item_spacing = vec2(theme::GRID, theme::GRID);
        ui.label(theme::silk("new preset"));
        let edit = ui.add(egui::TextEdit::singleline(&mut tb.new_preset).hint_text("name").desired_width(f32::INFINITY));
        if key.clicked() {
            edit.request_focus();
        }
        let name = tb.new_preset.trim().to_string();
        let taken = t.settings.user_presets.contains_key(&name);
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let save = ui.add_enabled_ui(!name.is_empty(), |ui| widgets::key(ui, if taken { "replace" } else { "save new" }, KeyStyle::Orange, None)).inner;
        let save = if taken { save.on_hover_text(format!("\"{name}\" exists: this overwrites it")) } else { save };
        if (save.clicked() || enter) && !name.is_empty() {
            out.save_as = Some(name);
            tb.new_preset.clear();
            ui.close();
        }
        ui.separator();
        let loaded = t.settings.user_presets.contains_key(t.preset_label);
        let label = if loaded { format!("update {}", fit(t.preset_label, 18)) } else { "update".to_string() };
        let update = ui.add_enabled_ui(loaded, |ui| widgets::key(ui, &label, KeyStyle::Light, None)).inner;
        let update = update.on_hover_text(format!("Save the current knobs and rack into \"{}\"", t.preset_label)).on_disabled_hover_text("Load one of your presets to update it");
        if update.clicked() && loaded {
            out.save_as = Some(t.preset_label.to_string());
            ui.close();
        }
    });
}

/// Cut `s` to `n` characters with an ellipsis.
fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

/// The text of the latency LCD: always four cells, so the window never changes width, and
/// `--.-` while audio is stopped.
fn latency_text(running: bool, ms: f32) -> String {
    if !running {
        " --.-".to_string()
    } else if ms < 99.95 {
        format!("{ms:5.1}")
    } else {
        format!("{:4.0}", ms.min(9999.0))
    }
}

/// The latency LCD: mic to main output in ms.
fn latency(ui: &mut egui::Ui, t: &ToolbarCtx, st: &UiState) {
    let text = latency_text(t.running, st.levels.latency[0]);
    ui.allocate_ui_with_layout(vec2(LAT_W, ROW), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let r = widgets::lcd_frame(ui, |ui| {
            ui.set_min_size(vec2(LAT_W - 18.0, ROW - 12.0));
            widgets::seg_display_ghost(ui, &text, 20.0, theme::LCD_WHITE, theme::LCD_DIM);
        })
        .response;
        if t.running {
            r.on_hover_text(format!("Mic to output: {:.1} ms\nMic to monitor: {:.1} ms", st.levels.latency[0], st.levels.latency[1]));
        } else {
            r.on_hover_text("Audio is stopped");
        }
    });
}

/// A warning chip printed like a label with a blinking LED; clicking it opens the output
/// settings, its × dismisses it. Folded (`!full`) it is only the LED and the ×. Returns true when the chip itself was clicked.
fn cable_badge(ui: &mut egui::Ui, dismissed: &mut bool, full: bool) -> bool {
    let font = theme::bold(10.5);
    let galley = ui.painter().layout_no_wrap(if full { "NO VIRTUAL CABLE" } else { "" }.into(), font, theme::INK);
    let body_w = if full { BADGE_W - 24.0 } else { BADGE_SMALL_W - 24.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(body_w + 24.0, ROW), Sense::hover());
    let body = egui::Rect::from_min_size(rect.min, vec2(body_w, ROW));
    let close = egui::Rect::from_min_max(pos2(body.right(), rect.top()), rect.max);
    let body_resp = ui.interact(body, ui.id().with("cable_badge"), Sense::click()).on_hover_text("No VB-Cable (or similar) output was found. Click to open the output settings.");
    let close_resp = ui.interact(close, ui.id().with("cable_badge_x"), Sense::click()).on_hover_text("Hide this hint");

    let p = ui.painter();
    let fill = if body_resp.hovered() || close_resp.hovered() { Color32::WHITE } else { theme::CHASSIS_LIGHT };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.5, theme::WARN), StrokeKind::Inside);
    let blink = (ui.input(|i| i.time) * 2.0).fract() < 0.6;
    widgets::paint_led(p, pos2(body.left() + 13.0, body.center().y), 3.0, blink, theme::WARN);
    p.galley(pos2(body.left() + 24.0, body.center().y - galley.size().y * 0.5), galley, theme::INK);
    p.line_segment([pos2(close.left(), rect.top() + 7.0), pos2(close.left(), rect.bottom() - 7.0)], Stroke::new(1.0, theme::RULE));
    let x = close.center();
    let ink = if close_resp.hovered() { theme::ORANGE } else { theme::INK_DIM };
    for d in [vec2(4.0, 4.0), vec2(4.0, -4.0)] {
        p.line_segment([x - d, x + d], Stroke::new(1.5, ink));
    }
    if close_resp.clicked() {
        *dismissed = true;
    }
    body_resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four 7-segment cells whatever the value (a '.' shares its digit's cell).
    fn cells(s: &str) -> usize {
        s.chars().filter(|c| *c != '.').count()
    }

    #[test]
    fn latency_lcd_keeps_its_width() {
        for (running, ms) in [(false, 0.0), (true, 0.0), (true, 9.94), (true, 99.94), (true, 99.96), (true, 123.4), (true, 1e6)] {
            assert_eq!(cells(&latency_text(running, ms)), 4, "{running} {ms}");
        }
        assert_eq!(latency_text(false, 12.0), " --.-");
    }

    #[test]
    fn fit_cuts_long_names() {
        assert_eq!(fit("short", 18), "short");
        assert_eq!(fit("abcdefghijklmnopqrstuvwxyz", 5).chars().count(), 5);
    }
}
