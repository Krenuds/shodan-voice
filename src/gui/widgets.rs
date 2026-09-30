//! Hardware-style widgets shared by every panel: keys, LEDs, 7-segment readouts, pixel meters,
//! the speaker grille and the add-module menu. Everything here is painted by hand in the K.O. II
//! look rather than built from egui's stock widgets.

use super::rack::Action;
use super::theme;
use crate::modules;
#[cfg(feature = "clap-host")]
use crate::plugins::{PluginInfo, scan};
use eframe::egui::{self, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStyle {
    Dark,
    Light,
    Orange,
}

struct KeyColors {
    top: Color32,
    bottom: Color32,
    lip: Color32,
    text: Color32,
}

fn key_colors(style: KeyStyle) -> KeyColors {
    match style {
        KeyStyle::Dark => KeyColors { top: Color32::from_rgb(0x42, 0x42, 0x41), bottom: theme::KEY_DARK, lip: theme::KEY_DARK_LOW, text: theme::KEY_LIGHT },
        KeyStyle::Light => KeyColors { top: Color32::WHITE, bottom: Color32::from_rgb(0xE6, 0xE6, 0xE3), lip: Color32::from_rgb(0xB2, 0xB2, 0xAD), text: theme::INK },
        KeyStyle::Orange => KeyColors { top: Color32::from_rgb(0xFF, 0x7A, 0x45), bottom: theme::ORANGE, lip: Color32::from_rgb(0xC2, 0x3E, 0x0C), text: theme::INK },
    }
}

const KEY_MIN: Vec2 = vec2(44.0, 32.0);
const KEY_RADIUS: f32 = 6.0;
const KEY_LIP: f32 = 2.0;
/// Room above a key for its LED.
const LED_ROW: f32 = 9.0;

/// A hardware key: `label` printed on it. `led`: Some(on) draws a small LED above it.
pub fn key(ui: &mut Ui, label: &str, style: KeyStyle, led: Option<bool>) -> Response {
    key_sized(ui, label, style, led, KEY_MIN)
}

/// [`key`] with a minimum size other than the default 44×32 (the LED row comes on top).
pub fn key_sized(ui: &mut Ui, label: &str, style: KeyStyle, led: Option<bool>, min: Vec2) -> Response {
    let font = theme::bold(11.0);
    let galley = ui.painter().layout_no_wrap(label.to_uppercase(), font, Color32::PLACEHOLDER);
    let key_size = vec2((galley.size().x + 2.0 * 14.0).max(min.x), min.y.max(galley.size().y + 12.0));
    let led_h = if led.is_some() { LED_ROW } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(key_size + vec2(0.0, led_h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let enabled = ui.is_enabled();
    let p = ui.painter();
    if let Some(on) = led {
        paint_led(p, pos2(rect.center().x, rect.top() + 3.0), 2.5, on, theme::ORANGE);
    }

    let mut c = key_colors(style);
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    if hovered && !pressed {
        c.top = c.top.lerp_to_gamma(Color32::WHITE, 0.12);
        c.bottom = c.bottom.lerp_to_gamma(Color32::WHITE, 0.08);
    }
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + led_h), rect.max);
    if !enabled {
        // A disabled key is an empty outline printed on the chassis: still there, clearly not live.
        let face = Rect::from_min_max(body.min, body.max - vec2(0.0, KEY_LIP));
        p.rect_stroke(face, KEY_RADIUS, Stroke::new(1.0, theme::INK_DIM.gamma_multiply(0.7)), StrokeKind::Inside);
        let text_pos = face.center() - galley.size() * 0.5;
        p.galley(text_pos, galley, theme::INK_DIM);
        return resp;
    }
    // A soft shadow printed on the chassis under the key.
    p.rect_filled(body.translate(vec2(0.0, 1.0)).expand(0.5), KEY_RADIUS + 0.5, Color32::from_black_alpha(if pressed { 18 } else { 30 }));
    let face = if pressed {
        Rect::from_min_max(body.min + vec2(0.0, 1.0), body.max)
    } else {
        p.rect_filled(body, KEY_RADIUS, c.lip);
        Rect::from_min_max(body.min, body.max - vec2(0.0, KEY_LIP))
    };
    gradient_rect(p, face, KEY_RADIUS, c.top, c.bottom);
    // A thin highlight along the top edge: the lip catching light.
    let edge = if style == KeyStyle::Dark { Color32::from_white_alpha(28) } else { Color32::from_white_alpha(120) };
    p.line_segment([pos2(face.left() + KEY_RADIUS, face.top() + 0.75), pos2(face.right() - KEY_RADIUS, face.top() + 0.75)], Stroke::new(1.0, edge));
    if hovered && style != KeyStyle::Orange {
        p.rect_stroke(face, KEY_RADIUS, Stroke::new(1.0, theme::ORANGE), StrokeKind::Inside);
    }
    let text_pos = face.center() - galley.size() * 0.5;
    p.galley(text_pos, galley, c.text);
    resp
}

/// Fill a rounded rectangle with a vertical gradient from `top` to `bottom`.
pub fn gradient_rect(p: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let mut outline: Vec<Pos2> = Vec::with_capacity(32);
    const STEPS: usize = 6;
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -std::f32::consts::FRAC_PI_2),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), std::f32::consts::FRAC_PI_2),
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
    ];
    for (c, a0) in corners {
        for k in 0..=STEPS {
            let a = a0 + std::f32::consts::FRAC_PI_2 * k as f32 / STEPS as f32;
            outline.push(c + Vec2::angled(a) * r);
        }
    }
    let color = |y: f32| top.lerp_to_gamma(bottom, ((y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0));
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), color(rect.center().y));
    for q in &outline {
        mesh.colored_vertex(*q, color(q.y));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(mesh));
    // The mesh has hard edges; a hairline in the edge colour anti-aliases them.
    p.add(Shape::closed_line(outline.clone(), Stroke::new(0.6, color(rect.center().y).gamma_multiply(0.6))));
}

/// Paint a round LED. On: lit with a glow. Off: a dull grey dome.
pub fn paint_led(p: &Painter, center: Pos2, radius: f32, on: bool, color: Color32) {
    if on {
        p.circle_filled(center, radius * 2.6, color.gamma_multiply(0.12));
        p.circle_filled(center, radius * 1.7, color.gamma_multiply(0.28));
        p.circle_filled(center, radius, color);
        p.circle_filled(center - vec2(radius * 0.3, radius * 0.3), radius * 0.35, Color32::from_white_alpha(170));
    } else {
        p.circle_filled(center, radius + 0.6, Color32::from_black_alpha(40));
        p.circle_filled(center, radius, Color32::from_rgb(0xA6, 0xA6, 0xA1));
        p.circle_filled(center - vec2(radius * 0.3, radius * 0.3), radius * 0.35, Color32::from_white_alpha(70));
    }
}

/// A small round LED you can click. Returns true when it was clicked (the caller flips the state).
#[allow(dead_code)] // shared API for the inspector and patchbay
pub fn led_toggle(ui: &mut Ui, on: bool, hint: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    if resp.hovered() {
        ui.painter().circle_stroke(rect.center(), 6.5, Stroke::new(1.0, theme::ORANGE));
    }
    paint_led(ui.painter(), rect.center(), 4.0, on, theme::ORANGE);
    let resp = if hint.is_empty() { resp } else { resp.on_hover_text(hint) };
    resp.clicked()
}

// 7-segment bits: a (top), b (top right), c (bottom right), d (bottom), e (bottom left),
// f (top left), g (middle).
const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 4;
const D: u8 = 8;
const E: u8 = 16;
const F: u8 = 32;
const G: u8 = 64;

fn segments(ch: char) -> u8 {
    match ch {
        '0' | 'O' => A | B | C | D | E | F,
        '1' | 'I' => B | C,
        '2' | 'Z' | 'z' => A | B | D | E | G,
        '3' => A | B | C | D | G,
        '4' => B | C | F | G,
        '5' | 'S' | 's' => A | C | D | F | G,
        '6' => A | C | D | E | F | G,
        '7' => A | B | C,
        '8' | 'B' => A | B | C | D | E | F | G,
        '9' | 'g' => A | B | C | D | F | G,
        '-' => G,
        '_' => D,
        'E' | 'e' => A | D | E | F | G,
        'r' | 'R' => E | G,
        'o' => C | D | E | G,
        'F' | 'f' => A | E | F | G,
        'n' | 'N' => C | E | G,
        'P' | 'p' => A | B | E | F | G,
        'A' | 'a' => A | B | C | E | F | G,
        'b' => C | D | E | F | G,
        'd' | 'D' => B | C | D | E | G,
        'c' => D | E | G,
        'C' => A | D | E | F,
        't' | 'T' => D | E | F | G,
        'L' | 'l' => D | E | F,
        'H' => B | C | E | F | G,
        'h' => C | E | F | G,
        'U' => B | C | D | E | F,
        'u' | 'v' => C | D | E,
        'y' | 'Y' => B | C | D | F | G,
        'J' | 'j' => B | C | D | E,
        _ => 0,
    }
}

/// A 7-segment readout of `text` (digits, '.', '-', ' ' and a few letters), `height` px tall,
/// lit in `color`. Unlit segments show faintly, like a real LCD.
///
/// The ghost of the unlit segments follows `color`: a bright colour is taken to sit on the black
/// LCD and gets `LCD_DIM` ghosts; a dark colour (e.g. `INK` on the chassis) gets barely-there
/// ghosts, so the value always reads. Use [`seg_display_ghost`] to choose, or [`seg_lcd`] for a
/// readout in its own LCD window.
pub fn seg_display(ui: &mut Ui, text: &str, height: f32, color: Color32) -> Response {
    seg_display_ghost(ui, text, height, color, auto_ghost(color))
}

/// The unlit-segment colour that keeps a readout lit in `color` legible.
pub fn auto_ghost(color: Color32) -> Color32 {
    let luma = 0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
    if luma > 110.0 { theme::LCD_DIM } else { Color32::from_black_alpha(14) }
}

/// A 7-segment readout set into its own small black LCD window (bright digits, dim ghosts).
#[allow(dead_code)] // shared API for the inspector
pub fn seg_lcd(ui: &mut Ui, text: &str, height: f32, color: Color32) -> Response {
    lcd_frame(ui, |ui| seg_display_ghost(ui, text, height, color, theme::LCD_DIM)).response
}

/// [`seg_display`] with an explicit colour for the unlit segments (`Color32::TRANSPARENT` hides them).
pub fn seg_display_ghost(ui: &mut Ui, text: &str, height: f32, color: Color32, ghost: Color32) -> Response {
    let h = height.max(6.0);
    let w = h * 0.5;
    let t = (h * 0.13).max(1.5);
    let gap = t * 1.2;
    let cells: Vec<(char, bool)> = {
        let mut v: Vec<(char, bool)> = Vec::new();
        for ch in text.chars() {
            if ch == '.' || ch == ',' {
                match v.last_mut() {
                    Some(last) if !last.1 => last.1 = true,
                    _ => v.push((' ', true)),
                }
            } else {
                v.push((ch, false));
            }
        }
        v
    };
    let skew = h * 0.08;
    let n = cells.len().max(1) as f32;
    let size = vec2(n * w + (n - 1.0) * gap + skew + t, h);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    // Lean the digits forward a little, like most 7-segment glass.
    let lean = |q: Pos2| pos2(q.x + (rect.bottom() - q.y) / h * skew, q.y);
    for (i, (ch, dot)) in cells.iter().enumerate() {
        let x0 = rect.left() + i as f32 * (w + gap);
        paint_digit(p, Rect::from_min_size(pos2(x0, rect.top()), vec2(w, h)), t, segments(*ch), color, ghost, &lean);
        let dc = lean(pos2(x0 + w + gap * 0.5, rect.bottom() - t * 0.5));
        p.rect_filled(Rect::from_center_size(dc, Vec2::splat(t)), 0.0, if *dot { color } else { ghost });
    }
    resp
}

fn paint_digit(p: &Painter, r: Rect, t: f32, bits: u8, lit: Color32, ghost: Color32, lean: &impl Fn(Pos2) -> Pos2) {
    let (l, rr, top, bot) = (r.left() + t * 0.5, r.right() - t * 0.5, r.top() + t * 0.5, r.bottom() - t * 0.5);
    let mid = (top + bot) * 0.5;
    let inset = t * 0.6;
    let ht = t * 0.5;
    let horiz = |y: f32| vec![pos2(l + inset - ht, y), pos2(l + inset, y - ht), pos2(rr - inset, y - ht), pos2(rr - inset + ht, y), pos2(rr - inset, y + ht), pos2(l + inset, y + ht)];
    let vert = |x: f32, y0: f32, y1: f32| vec![pos2(x, y0 + inset - ht), pos2(x + ht, y0 + inset), pos2(x + ht, y1 - inset), pos2(x, y1 - inset + ht), pos2(x - ht, y1 - inset), pos2(x - ht, y0 + inset)];
    let segs: [(u8, Vec<Pos2>); 7] = [(A, horiz(top)), (B, vert(rr, top, mid)), (C, vert(rr, mid, bot)), (D, horiz(bot)), (E, vert(l, mid, bot)), (F, vert(l, top, mid)), (G, horiz(mid))];
    for (bit, pts) in segs {
        let color = if bits & bit != 0 { lit } else { ghost };
        if color.a() == 0 {
            continue;
        }
        p.add(Shape::convex_polygon(pts.into_iter().map(lean).collect(), color, Stroke::NONE));
    }
}

/// Level meter made of square pixels, -60..0 dBFS, on its own little LCD strip.
/// `vertical` stacks the pixels upwards.
pub fn pixel_meter(ui: &mut Ui, level: f32, size: Vec2, vertical: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, 2.0, theme::LCD);
        paint_pixel_meter(p, rect.shrink(2.0), level, vertical);
    }
    resp
}

/// The colour of the meter pixel at `db` (its top edge).
fn meter_color(db: f32) -> Color32 {
    if db > -3.0 {
        theme::LCD_RED
    } else if db > -9.0 {
        theme::LCD_YELLOW
    } else if db > -24.0 {
        theme::LCD_WHITE
    } else {
        theme::LCD_BLUE
    }
}

/// Paint a pixel meter into `rect` (for custom-painted places like patchbay nodes). Only the
/// pixels are drawn, lit or `LCD_DIM`; the background is the caller's (normally the LCD).
pub fn paint_pixel_meter(p: &Painter, rect: Rect, level: f32, vertical: bool) {
    let (long, short) = if vertical { (rect.height(), rect.width()) } else { (rect.width(), rect.height()) };
    if long < 2.0 || short < 1.0 {
        return;
    }
    // Tiny meters are one row of pixels as tall as the strip; bigger ones get a grid.
    let px = if short <= 8.0 { short } else { (short / 2.0 - 1.0).clamp(3.0, 6.0) };
    let pitch = px + 1.0;
    let rows = (((short + 1.0) / pitch).floor() as usize).max(1);
    let cols = (((long + 1.0) / pitch).floor() as usize).max(1);
    let used_long = cols as f32 * pitch - 1.0;
    let used_short = rows as f32 * pitch - 1.0;
    let db = 20.0 * level.max(1e-6).log10();
    let frac = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let lit = (frac * cols as f32).round() as usize;
    let off_long = ((long - used_long) * 0.5).floor();
    let off_short = ((short - used_short) * 0.5).floor();
    for i in 0..cols {
        let color = if i < lit { meter_color(-60.0 + 60.0 * (i + 1) as f32 / cols as f32) } else { theme::LCD_DIM };
        for j in 0..rows {
            let a = off_long + i as f32 * pitch;
            let b = off_short + j as f32 * pitch;
            let min = if vertical { pos2(rect.left() + b, rect.bottom() - a - px) } else { pos2(rect.left() + a, rect.top() + b) };
            p.rect_filled(Rect::from_min_size(min, Vec2::splat(px)), 0.0, color);
        }
    }
}

/// The speaker grille: a field of small holes punched in the chassis, filling `rect`.
pub fn paint_speaker_grille(p: &Painter, rect: Rect) {
    const PITCH: f32 = 7.0;
    let cols = ((rect.width() - 2.0) / PITCH).floor() as i32;
    let rows = ((rect.height() - 2.0) / PITCH).floor() as i32;
    if cols < 1 || rows < 1 {
        return;
    }
    let x0 = rect.right() - (cols as f32 - 0.5) * PITCH - 1.0;
    let y0 = rect.center().y - (rows as f32 - 1.0) * 0.5 * PITCH;
    for j in 0..rows {
        for i in 0..cols {
            let c = pos2(x0 + i as f32 * PITCH, y0 + j as f32 * PITCH);
            p.circle_filled(c + vec2(0.0, 0.6), 2.1, Color32::from_white_alpha(110));
            p.circle_filled(c, 2.0, theme::SPEAKER.lerp_to_gamma(theme::INK, 0.28));
        }
    }
}

/// A black LCD window set into the chassis, with the contents drawn inside it.
pub fn lcd_frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(theme::LCD)
        .corner_radius(CornerRadius::same(4))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0xB8, 0xB8, 0xB3)))
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, add)
}

/// The CLAP plugin list for the add menu, scanned the first time the menu opens.
#[derive(Default)]
pub struct Catalog {
    #[cfg(feature = "clap-host")]
    plugins: Option<Vec<PluginInfo>>,
    #[cfg(feature = "clap-host")]
    show_instruments: bool,
    #[cfg(feature = "clap-host")]
    filter: String,
}

/// One line about what a native module does, for the add menu.
pub fn kind_blurb(kind_id: &str) -> &'static str {
    match kind_id {
        "shodan_core" => "the SHODAN voice itself",
        "metal" => "flanger",
        "lofi" => "bit / sample-rate crush",
        "titobot" => "robot language from speech",
        _ => "",
    }
}

const MENU_WIDTH: f32 = 300.0;

/// A menu row: a colour swatch in a small LCD square, a name and a printed description.
fn menu_row(ui: &mut Ui, swatch: Color32, name: &str, blurb: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(MENU_WIDTH), 38.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 5.0, Color32::WHITE);
        p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(3.0, rect.height() - 12.0)), 1.5, theme::ORANGE);
    }
    let chip = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0));
    p.rect_filled(chip, 3.0, theme::LCD);
    p.rect_filled(Rect::from_center_size(chip.center(), Vec2::splat(10.0)), 0.0, swatch);
    let x = chip.right() + 12.0;
    let name_g = p.layout_no_wrap(name.to_string(), theme::bold(12.5), theme::INK);
    let name_h = name_g.size().y;
    p.galley(pos2(x, rect.center().y - name_h + 2.0), name_g, theme::INK);
    let blurb_g = p.layout(blurb.to_uppercase(), egui::FontId::monospace(9.5), theme::INK_DIM, rect.right() - x - 8.0);
    p.galley(pos2(x, rect.center().y + 2.0), blurb_g, theme::INK_DIM);
    resp
}

fn menu_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(theme::silk(text));
}

/// The contents of an add-module menu (call inside a menu or popup). `at` is where the new
/// module goes in the chain; `None` is the end.
pub fn add_module_menu(ui: &mut Ui, catalog: &mut Catalog, at: Option<usize>) -> Option<Action> {
    let mut action = None;
    ui.set_min_width(MENU_WIDTH);
    ui.spacing_mut().item_spacing.y = 2.0;
    menu_heading(ui, "Modules");
    for kind in &modules::NATIVE {
        if menu_row(ui, theme::kind_color(kind.id), kind.name, kind_blurb(kind.id)).clicked() {
            action = Some(Action::Add(kind, at));
        }
    }
    #[cfg(feature = "clap-host")]
    {
        ui.add_space(6.0);
        ui.separator();
        menu_heading(ui, "CLAP plugins");
        let Catalog { plugins, show_instruments, filter } = catalog;
        if plugins.is_none() {
            *plugins = Some(scan::scan());
        }
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(filter).hint_text("filter…").desired_width(120.0));
            ui.checkbox(show_instruments, egui::RichText::new("INSTR").size(10.0)).on_hover_text("Also list instruments and synths");
            if key_sized(ui, "rescan", KeyStyle::Light, None, vec2(44.0, 24.0)).on_hover_text("Search the CLAP folders again").clicked() {
                *plugins = Some(scan::scan());
            }
        });
        let list = plugins.as_deref().unwrap_or_default();
        if list.is_empty() {
            ui.label(egui::RichText::new("No CLAP plugins found.").color(theme::WARN));
            ui.label(egui::RichText::new("Install e.g. Airwindows Consolidated or Surge XT,\nthen press rescan. Searched:").small());
            for p in scan::search_paths() {
                ui.label(egui::RichText::new(p.display().to_string()).small().monospace().color(theme::INK_DIM));
            }
        }
        let f = filter.to_lowercase();
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for info in list {
                if (info.is_instrument && !*show_instruments) || (!f.is_empty() && !info.name.to_lowercase().contains(&f) && !info.vendor.to_lowercase().contains(&f)) {
                    continue;
                }
                if menu_row(ui, theme::kind_color("clap"), &info.name, &info.vendor).clicked() {
                    action = Some(Action::AddClap(info.clone(), at));
                }
            }
        });
    }
    #[cfg(not(feature = "clap-host"))]
    let _ = catalog;
    if action.is_some() {
        ui.close();
    }
    action
}
