//! "K.O. II hardware" look: a light grey chassis with a lighter tray for the patchbay, small black
//! LCD windows, chunky keys and one hot orange. Every colour the GUI uses is a constant here; panels never hard-code colours.
//!
//! Text is Space Mono (SIL OFL 1.1, `assets/fonts/OFL.txt`), with egui's own fonts behind it as
//! fallbacks for symbols and emoji.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Shadow, Stroke, TextStyle, Vec2, Visuals};
use std::sync::Arc;

// Chassis (printed-on-plastic surfaces).
pub const CHASSIS: Color32 = Color32::from_rgb(0xD9, 0xD9, 0xD6);
pub const CHASSIS_LIGHT: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE5);
pub const SPEAKER: Color32 = Color32::from_rgb(0xC9, 0xC9, 0xC5);
pub const INK: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const INK_DIM: Color32 = Color32::from_rgb(0x55, 0x55, 0x4F);

// Keys.
pub const KEY_DARK: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
pub const KEY_DARK_LOW: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);
pub const KEY_LIGHT: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF0);
pub const ORANGE: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x1F);

// The patchbay tray: a lighter panel set into the chassis, with a printed dot grid. Dark keys,
// grey cables and dark jacks sit on it.
pub const TRAY: Color32 = Color32::from_rgb(0xF3, 0xF3, 0xF0);
/// The tray's recessed top edge, and the floor of an empty (missing-module) socket.
pub const TRAY_SHADE: Color32 = Color32::from_rgb(0xE2, 0xE2, 0xDE);
pub const TRAY_DOT: Color32 = Color32::from_rgb(0xC6, 0xC6, 0xC1);
/// A silent cable on the tray (it lights up orange with signal).
pub const CABLE: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x87);
/// The soft shadow a key casts on the tray.
pub const KEY_SHADOW: Color32 = Color32::from_black_alpha(34);
/// The lip under a light key (MIC / OUTPUT).
pub const KEY_LIGHT_LOW: Color32 = Color32::from_rgb(0xC4, 0xC4, 0xBF);

// The small LCD windows (latency readout, help strip, meter strips) and the colours lit in them.
pub const LCD: Color32 = Color32::from_rgb(0x0E, 0x0E, 0x0E);
pub const LCD_DIM: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const LCD_RED: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x30);
pub const LCD_BLUE: Color32 = Color32::from_rgb(0x3D, 0x7B, 0xFF);
pub const LCD_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xC4, 0x00);
pub const LCD_WHITE: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);
pub const LCD_GREEN: Color32 = Color32::from_rgb(0x3D, 0xDC, 0x84);
/// SHODAN's own colour: the cyan of her cyberspace. Orange stays the "state" colour.
pub const LCD_CYAN: Color32 = Color32::from_rgb(0x2E, 0xE6, 0xD6);
/// Secondary text on dark keys and LCDs (node numbers, OFF).
pub const LCD_TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x96);

// Older names, kept for widgets that still use them.
#[allow(dead_code)]
pub const BG: Color32 = CHASSIS;
#[allow(dead_code)]
pub const PANEL: Color32 = CHASSIS_LIGHT;
#[allow(dead_code)]
pub const CARD: Color32 = CHASSIS_LIGHT;
pub const KNOB_BODY: Color32 = KEY_LIGHT;
pub const KNOB_TRACK: Color32 = Color32::from_rgb(0xBD, 0xBD, 0xB8);
pub const ACCENT: Color32 = ORANGE;
pub const ACCENT_HOT: Color32 = Color32::from_rgb(0xFF, 0x7E, 0x4D);
#[allow(dead_code)]
pub const TEXT: Color32 = INK;
#[allow(dead_code)]
pub const TEXT_DIM: Color32 = INK_DIM;
pub const WARN: Color32 = Color32::from_rgb(0xD9, 0x8A, 0x00);
pub const DANGER: Color32 = Color32::from_rgb(0xD6, 0x28, 0x1E);

/// A hairline printed on the chassis (section rules, separators).
pub const RULE: Color32 = Color32::from_rgb(0xB4, 0xB4, 0xAF);

/// The 8 px grid everything is spaced on.
pub const GRID: f32 = 8.0;

/// Size of silkscreen (printed small caps) labels.
pub const SILK_SIZE: f32 = 11.0;

/// The bold cut of the embedded font, for wordmarks and key legends.
pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, bold_family())
}

const REGULAR: &str = "space_mono";
const BOLD: &str = "space_mono_bold";

/// The colour a module kind is drawn with (its key's stripe on the patchbay, the inspector chip).
pub fn kind_color(kind_id: &str) -> Color32 {
    match kind_id {
        "shodan_core" => LCD_CYAN,
        "metal" => LCD_BLUE,
        "lofi" => LCD_YELLOW,
        "titobot" => LCD_GREEN,
        "clap" => LCD_WHITE,
        _ => LCD_DIM,
    }
}

fn fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    f.font_data.insert(REGULAR.into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/SpaceMono-Regular.ttf"))));
    f.font_data.insert(BOLD.into(), Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/SpaceMono-Bold.ttf"))));
    // egui's defaults stay behind Space Mono so symbols and emoji still have a glyph.
    let fallbacks = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for family in [FontFamily::Monospace, FontFamily::Proportional] {
        f.families.entry(family).or_default().insert(0, REGULAR.into());
    }
    let mut bold = vec![BOLD.to_string()];
    bold.extend(fallbacks);
    f.families.insert(bold_family(), bold);
    ctx.set_fonts(f);
}

pub fn apply(ctx: &egui::Context) {
    fonts(ctx);

    let mut v = Visuals::light();
    v.panel_fill = CHASSIS;
    v.window_fill = CHASSIS_LIGHT;
    v.window_stroke = Stroke::new(1.0, RULE);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(40) };
    v.popup_shadow = Shadow { offset: [0, 3], blur: 10, spread: 0, color: Color32::from_black_alpha(36) };
    v.extreme_bg_color = KEY_LIGHT;
    v.text_edit_bg_color = Some(KEY_LIGHT);
    v.faint_bg_color = CHASSIS_LIGHT;
    v.code_bg_color = CHASSIS_LIGHT;
    v.selection.bg_fill = ORANGE.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ORANGE);
    v.hyperlink_color = ORANGE;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.override_text_color = Some(INK);
    v.text_cursor.stroke = Stroke::new(2.0, ORANGE);
    v.striped = false;
    v.indent_has_left_vline = false;

    let radius = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CHASSIS;
    w.noninteractive.weak_bg_fill = CHASSIS;
    w.noninteractive.bg_stroke = Stroke::new(1.0, RULE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, INK);

    w.inactive.bg_fill = KEY_LIGHT;
    w.inactive.weak_bg_fill = KEY_LIGHT;
    w.inactive.bg_stroke = Stroke::new(1.0, RULE);
    w.inactive.fg_stroke = Stroke::new(1.0, INK);

    w.hovered.bg_fill = Color32::WHITE;
    w.hovered.weak_bg_fill = Color32::WHITE;
    w.hovered.bg_stroke = Stroke::new(1.0, ORANGE);
    w.hovered.fg_stroke = Stroke::new(1.5, INK);
    w.hovered.expansion = 0.0;

    w.active.bg_fill = ORANGE;
    w.active.weak_bg_fill = ORANGE.linear_multiply(0.35);
    w.active.bg_stroke = Stroke::new(1.5, ORANGE);
    w.active.fg_stroke = Stroke::new(1.5, INK);
    w.active.expansion = 0.0;

    w.open.bg_fill = Color32::WHITE;
    w.open.weak_bg_fill = Color32::WHITE;
    w.open.bg_stroke = Stroke::new(1.0, ORANGE);
    w.open.fg_stroke = Stroke::new(1.0, INK);

    for w in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        w.corner_radius = radius;
    }

    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(GRID, GRID);
        s.button_padding = Vec2::new(GRID, 4.0);
        s.interact_size = Vec2::new(4.0 * GRID, 3.0 * GRID);
        s.window_margin = Margin::same(12);
        s.menu_margin = Margin::same(8);
        s.combo_width = 20.0 * GRID;
        s.text_edit_width = 20.0 * GRID;
        s.slider_width = 20.0 * GRID;
        s.icon_width = 14.0;
        s.icon_width_inner = 8.0;
        s.icon_spacing = 6.0;
        s.menu_spacing = 4.0;
        s.combo_height = 320.0;
        s.scroll.bar_width = 6.0;
        s.scroll.floating = true;

        use FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Small, FontId::new(10.0, Proportional)),
            (TextStyle::Body, FontId::new(12.5, Proportional)),
            (TextStyle::Button, FontId::new(12.5, Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, Monospace)),
            (TextStyle::Heading, FontId::new(18.0, bold_family())),
        ]
        .into();
        style.interaction.tooltip_delay = 0.35;
    });
}

/// A label printed on the chassis in small caps, like the silkscreen above a key.
pub fn silkscreen(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Label::new(silk(text)).wrap_mode(egui::TextWrapMode::Extend))
}

/// Small caps printed text, for building labels by hand.
pub fn silk(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(FontId::monospace(SILK_SIZE)).color(INK_DIM).extra_letter_spacing(0.8)
}

