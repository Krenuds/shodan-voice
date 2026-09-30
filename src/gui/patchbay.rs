//! The patchbay: the chain drawn as nodes and cables on the LCD, MIC → modules → OUTPUT.
//!
//! The engine is a strict serial chain, so this is a *visual* patchbay: nodes flow left to right
//! and snake onto new rows, a cable joins each node to the next, a module is moved by dragging
//! it and added by clicking the ⊕ on a cable. Everything it changes goes back as an [`Action`],
//! except a module's On, which is an atomic flag flipped in place.

use super::rack::{Action, Body, Item, Rack};
use super::state::{Selection, UiState};
use super::{theme, widgets};
use crate::audio::engine::MAX_MODULES;
use crate::audio::module::ModuleId;
use crate::params::io::P;
use crate::shared::Shared;
use eframe::egui::{
    self, Align, Align2, CornerRadius, CursorIcon, FontId, Key, LayerId, Layout, Modifiers, Order, Painter, PointerButton, Popup, PopupAnchor, PopupCloseBehavior, PopupKind, Pos2, Rect, Sense,
    Shape, Stroke, StrokeKind, UiBuilder, Vec2, pos2, vec2,
};
use std::sync::atomic::Ordering;

/// A module node's size. Everything on the canvas sits on an 8 px grid.
const NODE: Vec2 = vec2(144.0, 80.0);
/// Largest a node grows to when the whole chain fits on one row with room to spare.
const NODE_MAX: Vec2 = vec2(176.0, 96.0);
/// Narrowest a module node gets to save a row, before the layout goes compact.
const MIN_NODE_X: f32 = 120.0;
/// Narrowest the gap between nodes gets while the keys are full height; the ⊕ still fits.
const MIN_GAP: f32 = 32.0;
/// The MIC / OUTPUT jack's size, drawn centred in its (node-sized) slot.
const END: Vec2 = vec2(112.0, 56.0);
/// Space between nodes, both across and between rows. The ⊕ sits in it.
const GAP: f32 = 48.0;
/// Space between the LCD's edge and the nodes (wrap cables run out into it).
const MARGIN: f32 = 24.0;
/// The LCD's dot grid; the chain is snapped onto it.
const DOT: f32 = 16.0;
/// How close the pointer must be to a cable to show its ⊕.
const CABLE_HOVER: f32 = 12.0;
const PLUS_RADIUS: f32 = 10.0;

/// Node size, gaps (across, between rows) and margin for one layout.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    node: Vec2,
    gap: Vec2,
    margin: f32,
}

const fn fit_of(w: f32, h: f32, gx: f32, gy: f32, margin: f32) -> Fit {
    Fit { node: vec2(w, h), gap: vec2(gx, gy), margin }
}

/// Full-height layouts, largest first: the one with the fewest rows wins.
const ROOMY: [Fit; 4] = [fit_of(NODE.x, NODE.y, GAP, GAP, MARGIN), fit_of(NODE.x, NODE.y, MIN_GAP, 40.0, MARGIN), fit_of(128.0, NODE.y, MIN_GAP, 40.0, MARGIN), fit_of(MIN_NODE_X, NODE.y, MIN_GAP, 40.0, MARGIN)];
/// Compact layouts for a chain that would not fit the LCD's height otherwise, largest first.
/// The last one (18 nodes in a 900×560 window) scrolls if even it does not fit.
const COMPACT: [Fit; 4] = [fit_of(120.0, 64.0, MIN_GAP, 32.0, MARGIN), fit_of(112.0, 56.0, 24.0, 24.0, MARGIN), fit_of(104.0, 48.0, 24.0, 24.0, 16.0), fit_of(96.0, 48.0, 24.0, 16.0, 16.0)];

/// The canvas's own state between frames.
#[derive(Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    /// The add menu is open for this chain position, anchored here.
    add_at: Option<(usize, Pos2)>,
    /// Last frame's node rects (MIC, items, OUTPUT) and cable midpoints, in screen space.
    slots: Vec<Rect>,
    cable_mids: Vec<Pos2>,
}

/// A module being dragged to a new place.
struct Drag {
    id: ModuleId,
    from: usize,
    /// Pointer position relative to the node's top-left corner when the drag started.
    grab: Vec2,
}

pub struct PatchbayCtx<'a> {
    /// Toggling a module's On flips its `shared.enabled`; opening a CLAP editor needs `&mut`.
    pub rack: &'a mut Rack,
    pub shared: &'a Shared,
    /// Audio is running (a module that isn't live then shows "not running").
    pub running: bool,
}

/// Draw the patchbay into the whole of `ui`. Returns an edit of the chain, if one was made.
pub fn show(ui: &mut egui::Ui, mut c: PatchbayCtx, st: &mut UiState) -> Option<Action> {
    let (lcd, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    paint_lcd(ui.painter(), lcd);

    let inner = lcd.shrink(4.0);
    let mut action = None;
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(inner);
    // The layout shrinks to fit, so this only scrolls as a last resort: then with a solid bar and
    // no fade (the fade is painted in the chassis colour, a grey band across the LCD).
    let scroll = &mut child.spacing_mut().scroll;
    *scroll = egui::style::ScrollStyle::solid();
    scroll.fade.strength = 0.0;
    egui::ScrollArea::vertical().auto_shrink(false).show(&mut child, |ui| {
        action = canvas(ui, &mut c, st, inner.height(), lcd);
    });
    if let Some((text, t)) = st.notice() {
        paint_notice(ui.ctx(), lcd, text, t);
    }
    action.or_else(|| keyboard(ui, &c, st))
}

/// A brief message across the top of the LCD, fading out at the end of its time.
fn paint_notice(ctx: &egui::Context, lcd: Rect, text: &str, t: f32) {
    ctx.request_repaint();
    let mut p = ctx.layer_painter(LayerId::new(Order::Foreground, egui::Id::new("patch_notice"))).with_clip_rect(lcd);
    p.multiply_opacity(((1.0 - t) / 0.2).min(1.0));
    let font = FontId::monospace(11.0);
    let galley = p.layout_no_wrap(text.to_uppercase(), font, theme::ORANGE);
    let rect = Rect::from_center_size(pos2(lcd.center().x, lcd.top() + 24.0), galley.size() + vec2(24.0, 12.0));
    p.rect(rect, CornerRadius::same(2), theme::KEY_DARK_LOW, Stroke::new(1.0, theme::ORANGE), StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, theme::ORANGE);
}

/// Everything inside the LCD: cables, nodes, ⊕ buttons, the drag, the add menu.
fn canvas(ui: &mut egui::Ui, c: &mut PatchbayCtx, st: &mut UiState, view_height: f32, lcd: Rect) -> Option<Action> {
    let mut action = None;
    let ctx = ui.ctx().clone();
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let hover_pos = ctx.pointer_hover_pos().filter(|p| ui.clip_rect().contains(*p));
    let n = c.rack.items().len();
    let empty = n == 0;

    // Layout: MIC, the items (or the empty-rack target), OUTPUT.
    let view = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), view_height));
    let n_nodes = if empty { 3 } else { n + 2 };
    let f = fit(n_nodes, view.size(), !empty);
    let (node, gap) = (f.node, f.gap);
    let area = view.shrink(f.margin);
    let mut slots = if empty { empty_layout(area, f) } else { layout(n_nodes, area, node, gap) };
    // Onto the dot grid (a nudge of at most half a dot, which the margin takes).
    let origin = lattice_origin(lcd);
    let first = slots[0].min;
    let snapped = origin + ((first - origin) / DOT).round() * DOT;
    for s in &mut slots {
        *s = s.translate(snapped - first);
    }
    // Scroll only when a key itself would be cut off (not for the margin or the snap's nudge).
    let lowest = slots.iter().map(|r| r.bottom()).fold(area.top(), f32::max);
    let bottom = if lowest > view.bottom() - 2.0 { lowest + f.margin } else { view.bottom() };
    ui.advance_cursor_after_rect(Rect::from_min_max(view.min, pos2(view.right(), bottom)));
    let last = n_nodes - 1;
    // What is drawn in each slot: the ends are smaller jacks.
    let end = end_size(node);
    let shapes: Vec<Rect> = slots.iter().enumerate().map(|(i, s)| if i == 0 || i == last { Rect::from_center_size(s.center(), end.min(s.size())) } else { *s }).collect();
    let item_slots = &slots[1..last];

    let painter = ui.painter().clone();
    let menu_id = ui.id().with("patch_add_menu");
    let drag_id = st.canvas.drag.as_ref().map(|d| d.id);
    let full = n >= MAX_MODULES;

    // Cables.
    let cables: Vec<Cable> = if empty {
        Vec::new()
    } else {
        (0..last)
            .map(|i| {
                let level = if i == 0 { st.levels.input } else { st.levels.module(c.rack.items()[i - 1].id) };
                Cable::new(&slots, &shapes, i, gap.x, level)
            })
            .collect()
    };
    let over_node = hover_pos.is_some_and(|p| shapes.iter().any(|r| r.expand(4.0).contains(p)));
    let hovered_cable = match (hover_pos, st.canvas.drag.is_some() || full || over_node) {
        (Some(p), false) => cables.iter().map(|c| (c.index, c.distance(p))).filter(|(_, d)| *d < CABLE_HOVER).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i),
        _ => None,
    };
    let menu_open_at = st.canvas.add_at.filter(|_| Popup::is_id_open(&ctx, menu_id)).map(|(i, _)| i);
    for cable in &cables {
        cable.paint(&painter, hovered_cable == Some(cable.index) || menu_open_at == Some(cable.index));
    }

    // The ends.
    let bypass = c.shared.io.get(P::Bypass) > 0.5;
    for (i, sel, label) in [(0, Selection::Input, "MIC"), (last, Selection::Output, "OUTPUT")] {
        let r = ui.interact(shapes[i], ui.id().with(label), Sense::click());
        if r.clicked() {
            st.selected = sel;
        }
        let (level, note) = match sel {
            Selection::Input => (st.levels.input, None),
            _ => (st.levels.output, bypass.then_some("BYPASS")),
        };
        paint_end(&painter, shapes[i], label, level, note, i == 0, st.selected == sel, r.hovered());
        let help = match sel {
            Selection::Input => "Microphone: where the chain starts. Click for input settings.",
            _ if bypass => "Output (BYPASS is on: you hear the dry mic). Click for output settings.",
            _ => "Output: where the chain ends. Click for output settings.",
        };
        r.on_hover_text(help);
    }

    // The empty-rack target.
    if empty {
        let target = shapes[1];
        let r = ui.interact(target, ui.id().with("first_module"), Sense::click());
        for i in 0..2 {
            Cable::new(&slots, &shapes, i, gap.x, 0.0).paint_dashed(&painter);
        }
        paint_first_target(&painter, target, r.hovered() || menu_open_at == Some(0));
        if r.hovered() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        if r.clicked() {
            st.canvas.add_at = Some((0, target.center_bottom()));
            Popup::open_id(&ctx, menu_id);
        }
        r.on_hover_text("Add a module to the chain");
    }

    // Module nodes.
    for k in 0..n {
        let slot = slots[k + 1];
        let id = c.rack.items()[k].id;
        let r = ui.interact(slot, ui.id().with(("node", id)), Sense::click_and_drag());
        if r.clicked() {
            st.selected = Selection::Module(id);
        }
        if r.drag_started_by(PointerButton::Primary)
            && let Some(p) = r.interact_pointer_pos()
        {
            st.selected = Selection::Module(id);
            st.canvas.drag = Some(Drag { id, from: k, grab: p - slot.min });
        }
        if r.drag_stopped_by(PointerButton::Primary)
            && let Some(d) = st.canvas.drag.take()
            && let Some(p) = pointer
        {
            let to = drop_index(p, item_slots, d.from);
            if to != d.from {
                action = Some(Action::MoveTo(d.id, to));
            }
        }
        if r.dragged_by(PointerButton::Primary) {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if r.hovered() {
            ctx.set_cursor_icon(CursorIcon::Grab);
        }

        let item = &c.rack.items()[k];
        let look = NodeLook {
            index: k,
            level: st.levels.module(id),
            selected: st.selected == Selection::Module(id),
            hovered: r.hovered(),
            pressed: r.is_pointer_button_down_on() && drag_id.is_none(),
            running: c.running,
        };
        let mut p = painter.clone();
        if drag_id == Some(id) {
            p.multiply_opacity(0.25);
        }
        let led = paint_module(&p, slot, item, &look);

        let missing = matches!(item.body, Body::Missing(_));
        if !missing {
            let on = item.shared.enabled.load(Ordering::Relaxed);
            let led_r = ui.interact(Rect::from_center_size(led, Vec2::splat(18.0)), ui.id().with(("led", id)), Sense::click());
            if led_r.clicked() {
                item.shared.enabled.store(!on, Ordering::Relaxed);
            }
            led_r.on_hover_text(if on { "On: click to switch this module off" } else { "Off: click to switch this module on" });
        }

        let tip = match missing {
            true => format!("Unknown module '{}': this build cannot load it. It is kept so saving does not lose it.\nDrag to reorder · right-click for more", item.kind_id()),
            false => format!("{}\nDrag to reorder · right-click for more", item.title()),
        };
        let r = if st.canvas.drag.is_none() { r.on_hover_text(tip) } else { r };
        r.context_menu(|ui| {
            if let Some(a) = node_menu(ui, c.rack, k) {
                action = Some(a);
            }
        });
    }

    // The ⊕ on the hovered cable (or the one whose menu is open).
    if let Some(i) = hovered_cable.or(menu_open_at).filter(|_| !empty && !full)
        && let Some(cable) = cables.get(i)
    {
        let r = ui.interact(Rect::from_center_size(cable.mid, Vec2::splat(PLUS_RADIUS * 2.0 + 4.0)), ui.id().with(("plus", i)), Sense::click());
        paint_plus(&painter, cable.mid, r.hovered() || menu_open_at == Some(i));
        if r.hovered() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        if r.clicked() {
            st.canvas.add_at = Some((i, cable.mid + vec2(0.0, PLUS_RADIUS)));
            Popup::open_id(&ctx, menu_id);
        }
        r.on_hover_text("Insert a module here");
    }

    // The drag: the node follows the pointer, a bar marks where it will land.
    if let Some(d) = &st.canvas.drag
        && let Some(p) = pointer
        && let Some(item) = c.rack.item(d.id)
    {
        let (k, right) = nearest_side(p, item_slots);
        let at = k + right as usize;
        if at != d.from && at != d.from + 1 {
            let s = item_slots[k];
            let x = if right { s.right() + gap.x / 2.0 } else { s.left() - gap.x / 2.0 };
            let bar = Rect::from_min_max(pos2(x - 2.0, s.top() - 6.0), pos2(x + 2.0, s.bottom() + 6.0));
            painter.rect_filled(bar, CornerRadius::same(2), theme::ORANGE);
        }
        let mut fg = ctx.layer_painter(LayerId::new(Order::Tooltip, ui.id().with("patch_drag"))).with_clip_rect(lcd);
        fg.multiply_opacity(0.8);
        let look = NodeLook { index: d.from, level: st.levels.module(d.id), selected: true, hovered: false, pressed: false, running: c.running };
        paint_module(&fg, Rect::from_min_size(p - d.grab, node), item, &look);
    }
    // A drag whose node went away (or a release we missed) ends here.
    if st.canvas.drag.as_ref().is_some_and(|d| c.rack.item(d.id).is_none()) || (st.canvas.drag.is_some() && !ui.input(|i| i.pointer.any_down())) {
        st.canvas.drag = None;
    }

    st.canvas.slots = slots;
    st.canvas.cable_mids = cables.iter().map(|c| c.mid).collect();

    // The add menu.
    if let Some((at, pos)) = st.canvas.add_at {
        let shown = Popup::new(menu_id, ctx.clone(), PopupAnchor::Position(pos), ui.layer_id())
            .kind(PopupKind::Menu)
            .layout(Layout::top_down_justified(Align::Min))
            .style(egui::containers::menu::menu_style)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .open_memory(None)
            .show(|ui| widgets::add_module_menu(ui, &mut st.catalog, Some(at)));
        match shown {
            Some(r) => action = action.or(r.inner),
            None => st.canvas.add_at = None,
        }
    }
    action
}

/// Right-click menu of module `k`.
fn node_menu(ui: &mut egui::Ui, rack: &mut Rack, k: usize) -> Option<Action> {
    let n = rack.items().len();
    let item = &mut rack.items_mut()[k];
    let id = item.id;
    let mut action = None;
    let on = item.shared.enabled.load(Ordering::Relaxed);
    if ui.button(if on { "Switch off" } else { "Switch on" }).clicked() {
        item.shared.enabled.store(!on, Ordering::Relaxed);
        ui.close();
    }
    ui.separator();
    if ui.add_enabled(k > 0, egui::Button::new("Move left")).clicked() {
        action = Some(Action::MoveTo(id, k - 1));
        ui.close();
    }
    if ui.add_enabled(k + 1 < n, egui::Button::new("Move right")).clicked() {
        action = Some(Action::MoveTo(id, k + 1));
        ui.close();
    }
    #[cfg(feature = "clap-host")]
    if let Body::Clap(cb) = &mut item.body
        && cb.plugin.has_editor()
    {
        ui.separator();
        let open = cb.plugin.window.is_some();
        if ui.button(if open { "Close editor" } else { "Open editor" }).clicked() {
            if open {
                cb.plugin.close_editor();
            } else if let Err(e) = cb.plugin.open_editor() {
                item.error = Some(e);
            }
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Remove").clicked() {
        action = Some(Action::Remove(id));
        ui.close();
    }
    action
}

/// Delete, Space, ←/→ and Ctrl+←/→ on the selection, unless something else has the keyboard.
fn keyboard(ui: &egui::Ui, c: &PatchbayCtx, st: &mut UiState) -> Option<Action> {
    let ctx = ui.ctx();
    if ctx.egui_wants_keyboard_input() || Popup::is_any_open(ctx) || st.canvas.drag.is_some() {
        return None;
    }
    let items = c.rack.items();
    let (move_l, move_r, left, right, delete, space) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::COMMAND, Key::ArrowLeft),
            i.consume_key(Modifiers::COMMAND, Key::ArrowRight),
            i.consume_key(Modifiers::NONE, Key::ArrowLeft),
            i.consume_key(Modifiers::NONE, Key::ArrowRight),
            i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
            i.consume_key(Modifiers::NONE, Key::Space),
        )
    });
    let chain: Vec<Selection> = std::iter::once(Selection::Input).chain(items.iter().map(|i| Selection::Module(i.id))).chain(std::iter::once(Selection::Output)).collect();
    let pos = chain.iter().position(|s| *s == st.selected).unwrap_or(0);
    if left && pos > 0 {
        st.selected = chain[pos - 1];
    }
    if right && pos + 1 < chain.len() {
        st.selected = chain[pos + 1];
    }
    let Selection::Module(id) = st.selected else { return None };
    let k = items.iter().position(|i| i.id == id)?;
    if space {
        let e = &items[k].shared.enabled;
        e.store(!e.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    if move_l && k > 0 {
        return Some(Action::MoveTo(id, k - 1));
    }
    if move_r && k + 1 < items.len() {
        return Some(Action::MoveTo(id, k + 1));
    }
    if delete {
        // Keep a module selected: the next one, else the one before, else OUTPUT.
        st.selected = items.get(k + 1).or(k.checked_sub(1).and_then(|p| items.get(p))).map_or(Selection::Output, |i| Selection::Module(i.id));
        return Some(Action::Remove(id));
    }
    None
}

/// Sizes for `n_nodes` in an LCD view of `view`:
/// - the whole chain on one row at full size (`grow`): keys grow into the room, up to `NODE_MAX`;
/// - otherwise full-height keys, narrowing the gaps and then the keys while that saves a row;
/// - and if those rows do not fit the height, compact keys, scrolling only when even the
///   smallest do not fit.
fn fit(n_nodes: usize, view: Vec2, grow: bool) -> Fit {
    let n = n_nodes.max(1);
    let area = |f: &Fit| view - Vec2::splat(2.0 * f.margin);
    let per_row = |f: &Fit| (((area(f).x + f.gap.x) / (f.node.x + f.gap.x)).floor() as usize).clamp(1, n);
    let rows = |f: &Fit| n.div_ceil(per_row(f));
    let fits = |f: &Fit| rows(f) as f32 * f.node.y + (rows(f) - 1) as f32 * f.gap.y <= area(f).y;
    let full = ROOMY[0];
    let one_row = n as f32 * full.node.x + (n - 1) as f32 * full.gap.x;
    if grow && one_row <= area(&full).x {
        let s = (area(&full).x / one_row).min(NODE_MAX.x / NODE.x);
        let g = snap8(GAP * s);
        let grown = Fit { node: vec2(snap8(NODE.x * s), snap8(NODE.y * s).min(NODE_MAX.y)), gap: vec2(g, g), margin: MARGIN };
        if fits(&grown) && per_row(&grown) == n {
            return grown;
        }
    }
    // `min_by_key` keeps the first of equals: the largest keys.
    if let Some(f) = ROOMY.iter().filter(|f| fits(f)).min_by_key(|f| rows(f)) {
        return *f;
    }
    *COMPACT.iter().find(|f| fits(f)).unwrap_or(&COMPACT[COMPACT.len() - 1])
}

fn snap8(v: f32) -> f32 {
    (v / 8.0).floor() * 8.0
}

/// The MIC / OUTPUT jack for a node size: grows with grown keys, fills compact ones.
fn end_size(node: Vec2) -> Vec2 {
    if node.y > NODE.y { vec2(snap8(END.x * node.y / NODE.y), snap8(END.y * node.y / NODE.y)) } else { END.min(node) }
}

/// An empty rack: MIC and OUTPUT at key size with a wider "add your first module" target
/// between them, all on one row when they fit.
fn empty_layout(area: Rect, f: Fit) -> Vec<Rect> {
    let (node, gap) = (f.node, f.gap);
    let target = (area.width() - 2.0 * node.x - 2.0 * gap.x).clamp(node.x, 2.0 * node.x);
    let width = 2.0 * node.x + 2.0 * gap.x + target;
    if width > area.width() {
        return layout(3, area, node, gap);
    }
    let (x0, cy) = (area.center().x - width / 2.0, area.center().y);
    let y = cy - node.y / 2.0;
    vec![
        Rect::from_min_size(pos2(x0, y), node),
        Rect::from_center_size(pos2(x0 + node.x + gap.x + target / 2.0, cy), vec2(target, node.y + 16.0)),
        Rect::from_min_size(pos2(x0 + width - node.x, y), node),
    ]
}

/// Place `n_nodes` nodes of `node_size` left to right, wrapping onto new rows when `avail` is too
/// narrow, in reading order. The block is centred across, and down too when it fits.
fn layout(n_nodes: usize, avail: Rect, node_size: Vec2, gap: Vec2) -> Vec<Rect> {
    if n_nodes == 0 {
        return Vec::new();
    }
    let per_row = (((avail.width() + gap.x) / (node_size.x + gap.x)).floor() as usize).clamp(1, n_nodes);
    let rows = n_nodes.div_ceil(per_row);
    let width = per_row as f32 * node_size.x + (per_row - 1) as f32 * gap.x;
    let height = rows as f32 * node_size.y + (rows - 1) as f32 * gap.y;
    let x0 = if width <= avail.width() { avail.center().x - width / 2.0 } else { avail.left() };
    let y0 = if height <= avail.height() { avail.center().y - height / 2.0 } else { avail.top() };
    (0..n_nodes)
        .map(|i| {
            let (row, col) = (i / per_row, i % per_row);
            Rect::from_min_size(pos2(x0 + col as f32 * (node_size.x + gap.x), y0 + row as f32 * (node_size.y + gap.y)), node_size)
        })
        .collect()
}

/// The slot nearest to `pointer`, and whether the pointer is on its right half.
fn nearest_side(pointer: Pos2, slots: &[Rect]) -> (usize, bool) {
    let k = (0..slots.len()).min_by(|&a, &b| slots[a].distance_sq_to_pos(pointer).total_cmp(&slots[b].distance_sq_to_pos(pointer))).unwrap_or(0);
    (k, slots.get(k).is_some_and(|s| pointer.x > s.center().x))
}

/// Where a module dragged from `dragged_from` lands when dropped at `pointer`: its index in the
/// item list after it has been taken out. `slots` are the items' rects in chain order.
fn drop_index(pointer: Pos2, slots: &[Rect], dragged_from: usize) -> usize {
    if slots.is_empty() {
        return 0;
    }
    let (k, right) = nearest_side(pointer, slots);
    let before = k + right as usize;
    let to = if before > dragged_from { before - 1 } else { before };
    to.min(slots.len() - 1)
}

/// The cable from node `index` to the next one, as a polyline.
struct Cable {
    index: usize,
    points: Vec<Pos2>,
    mid: Pos2,
    level: f32,
}

impl Cable {
    fn new(slots: &[Rect], shapes: &[Rect], index: usize, gap: f32, level: f32) -> Self {
        let (a, b) = (shapes[index], shapes[index + 1]);
        let from = a.right_center();
        let to = b.left_center();
        let mut points = Vec::with_capacity(64);
        let mid = if (slots[index].center().y - slots[index + 1].center().y).abs() < 1.0 {
            // Same row: a short cable that sags a little.
            let d = (to.x - from.x) / 3.0;
            let sag = 7.0;
            bezier(&mut points, [from, from + vec2(d, sag), to + vec2(-d, sag), to]);
            points[points.len() / 2]
        } else {
            // Row wrap: out to the right, down into the gap, back left under the row, and in.
            let y = (slots[index].bottom() + slots[index + 1].top()) / 2.0;
            let right = pos2(slots[index].right() + gap * 0.5, y);
            let left = pos2(slots[index + 1].left() - gap * 0.5, y);
            let drop = (y - from.y) * 0.9;
            bezier(&mut points, [from, from + vec2(right.x - from.x, 0.0), right + vec2(0.0, -drop * 0.4), right]);
            let rise = (to.y - y) * 0.9;
            let mut back = Vec::new();
            bezier(&mut back, [left, left + vec2(0.0, rise * 0.4), to + vec2(left.x - to.x, 0.0), to]);
            points.extend(back);
            pos2((right.x + left.x) / 2.0, y)
        };
        Self { index, points, mid, level }
    }

    fn distance(&self, p: Pos2) -> f32 {
        self.points.windows(2).map(|w| segment_distance(p, w[0], w[1])).fold(f32::INFINITY, f32::min)
    }

    fn paint(&self, p: &Painter, hot: bool) {
        // Grey when silent, lighting up orange with the signal that flows through it.
        let lit = if hot { 1.0 } else { meter_fraction(self.level).powf(1.5) };
        p.add(Shape::line(self.points.clone(), Stroke::new(3.0, theme::INK_DIM.gamma_multiply(0.7))));
        if lit > 0.01 {
            p.add(Shape::line(self.points.clone(), Stroke::new(3.0, theme::ORANGE.gamma_multiply(lit))));
        }
    }

    /// A cable that isn't there yet (the empty rack's placeholder).
    fn paint_dashed(&self, p: &Painter) {
        p.extend(Shape::dashed_line(&self.points, Stroke::new(2.0, theme::INK_DIM), 6.0, 5.0));
    }
}

fn bezier(out: &mut Vec<Pos2>, [p0, p1, p2, p3]: [Pos2; 4]) {
    const STEPS: usize = 24;
    for s in 0..=STEPS {
        let t = s as f32 / STEPS as f32;
        let u = 1.0 - t;
        let v = p0.to_vec2() * (u * u * u) + p1.to_vec2() * (3.0 * u * u * t) + p2.to_vec2() * (3.0 * u * t * t) + p3.to_vec2() * (t * t * t);
        out.push(v.to_pos2());
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// A peak level as 0..1 over -60..0 dBFS, like the pixel meters.
fn meter_fraction(level: f32) -> f32 {
    ((20.0 * level.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Where the LCD's dot lattice starts.
fn lattice_origin(lcd: Rect) -> Pos2 {
    (lcd.min + Vec2::splat(DOT / 2.0)).round()
}

fn paint_lcd(p: &Painter, lcd: Rect) {
    p.rect_filled(lcd.expand(3.0), CornerRadius::same(13), theme::SPEAKER);
    p.rect_filled(lcd, CornerRadius::same(10), theme::LCD);
    // Inner shadow: a faint rim just inside the bezel.
    p.rect_stroke(lcd.shrink(1.0), CornerRadius::same(9), Stroke::new(1.0, theme::LCD_DIM), StrokeKind::Inside);
    let o = lattice_origin(lcd);
    let dot = Vec2::splat(1.5);
    let mut y = o.y + DOT;
    while y < lcd.bottom() - DOT / 2.0 {
        let mut x = o.x + DOT;
        while x < lcd.right() - DOT / 2.0 {
            p.rect_filled(Rect::from_center_size(pos2(x, y), dot), CornerRadius::ZERO, theme::LCD_DIM);
            x += DOT;
        }
        y += DOT;
    }
}

/// How a module node is drawn this frame.
struct NodeLook {
    index: usize,
    level: f32,
    selected: bool,
    hovered: bool,
    pressed: bool,
    running: bool,
}

/// Draw a module node as a dark key. Returns the centre of its On LED. Scales from compact
/// (48 px) to grown (96 px) keys: a top line (number, status), the title, the meter.
fn paint_module(p: &Painter, rect: Rect, item: &Item, look: &NodeLook) -> Pos2 {
    const LIP: f32 = 2.0;
    let missing = matches!(item.body, Body::Missing(_));
    let on = item.shared.enabled.load(Ordering::Relaxed) && !missing;
    let compact = rect.height() < 64.0;
    let pad = if compact { 6.0 } else { 8.0 };
    let radius = CornerRadius::same(8);
    if look.selected {
        p.rect_stroke(rect.expand(4.0), CornerRadius::same(11), Stroke::new(2.0, theme::ORANGE), StrokeKind::Outside);
    }
    let body = Rect::from_min_max(rect.min + vec2(0.0, if look.pressed { LIP } else { 0.0 }), rect.max - vec2(0.0, LIP));
    if missing {
        // Not a real key: a hollow, dashed outline.
        p.rect_filled(rect, radius, theme::KEY_DARK_LOW);
        p.extend(Shape::dashed_line(&rounded_path(rect.shrink(1.0), 8.0), Stroke::new(1.5, theme::LCD_RED.gamma_multiply(0.8)), 5.0, 4.0));
    } else {
        p.rect_filled(rect, radius, theme::KEY_DARK_LOW);
        p.rect_filled(body, radius, theme::KEY_DARK);
    }
    if look.hovered && !look.selected {
        p.rect_stroke(body, radius, Stroke::new(1.0, theme::INK_DIM), StrokeKind::Inside);
    }

    let color = theme::kind_color(item.kind_id());
    if !missing {
        let stripe = Rect::from_min_max(body.left_top() + vec2(8.0, pad + 2.0), pos2(body.left() + 11.0, body.bottom() - pad - 2.0));
        p.rect_filled(stripe, CornerRadius::same(2), if on { color } else { color.gamma_multiply(0.35) });
    }

    let x = body.left() + 18.0;
    let led_r = if compact { 4.0 } else { 5.0 };
    let led = pos2(body.right() - pad - led_r, body.top() + pad + led_r);
    let text_right = led.x - led_r - 6.0;
    let small = FontId::monospace(9.0);
    let title_font = FontId::monospace(if compact || missing { 11.0 } else if rect.height() >= 88.0 { 13.0 } else { 12.0 });
    let char_w = title_font.size * 0.6;

    // Top line: position, then what is wrong (or OFF).
    let top = body.top() + pad;
    p.text(pos2(x, top), Align2::LEFT_TOP, format!("{:02}", look.index + 1), small.clone(), theme::LCD_TEXT_DIM);
    let status = if missing {
        Some(("MISSING".to_string(), theme::LCD_RED))
    } else if let Some(e) = &item.error {
        Some((e.to_uppercase(), theme::LCD_RED))
    } else if look.running && !item.live {
        Some(("NOT RUNNING".to_string(), theme::LCD_YELLOW))
    } else if !on {
        Some(("OFF".to_string(), theme::LCD_TEXT_DIM))
    } else {
        None
    };
    if let Some((text, c)) = status {
        let sx = x + 18.0;
        p.text(pos2(sx, top), Align2::LEFT_TOP, fit_text(&text, ((text_right - sx) / 5.4) as usize), small.clone(), c);
    }

    // Title.
    let title = if missing { "UNKNOWN MODULE".to_string() } else { item.title().to_uppercase() };
    let title_y = top + if compact { 11.0 } else { 14.0 };
    p.text(pos2(x, title_y), Align2::LEFT_TOP, fit_text(&title, ((body.right() - pad - x) / char_w) as usize), title_font, if on || missing { theme::LCD_WHITE } else { theme::LCD_TEXT_DIM });

    // Bottom: the meter, or for an unknown module the kind it was saved as.
    let bottom = body.bottom() - pad;
    if missing {
        let kind = item.kind_id();
        let name = item.title();
        let what = if name != kind { format!("{kind} · {name}") } else { kind.to_string() };
        p.text(pos2(x, bottom), Align2::LEFT_BOTTOM, fit_text(&what, ((body.right() - pad - x) / 5.4) as usize), small, theme::LCD_TEXT_DIM);
    } else {
        let meter = Rect::from_min_max(pos2(x, bottom - 6.0), pos2(body.right() - pad - 2.0, bottom));
        p.rect_filled(meter.expand(2.0), CornerRadius::same(2), theme::LCD);
        widgets::paint_pixel_meter(p, meter, if on { look.level } else { 0.0 }, false);
    }

    if on {
        p.circle_filled(led, led_r + 3.0, theme::ORANGE.gamma_multiply(0.22));
        p.circle_filled(led, led_r, theme::ORANGE);
    } else if missing {
        p.circle(led, led_r, theme::KEY_DARK_LOW, Stroke::new(1.5, theme::LCD_RED));
    } else {
        p.circle(led, led_r, theme::KEY_DARK_LOW, Stroke::new(1.0, theme::LCD_TEXT_DIM));
    }

    jack(p, pos2(rect.left(), body.center().y));
    jack(p, pos2(rect.right(), body.center().y));
    led
}

/// A rounded rectangle's outline as a closed polyline (for dashing).
fn rounded_path(r: Rect, radius: f32) -> Vec<Pos2> {
    let corners = [(r.right_top() + vec2(-radius, radius), -90.0f32), (r.right_bottom() + vec2(-radius, -radius), 0.0), (r.left_bottom() + vec2(radius, -radius), 90.0), (r.left_top() + vec2(radius, radius), 180.0)];
    let mut path: Vec<Pos2> = corners
        .iter()
        .flat_map(|&(c, start)| (0..=4).map(move |k| c + Vec2::angled((start + 22.5 * k as f32).to_radians()) * radius))
        .collect();
    path.push(path[0]);
    path
}

/// Draw MIC or OUTPUT: a rounder key with a socket on its outer side.
#[allow(clippy::too_many_arguments)]
fn paint_end(p: &Painter, rect: Rect, label: &str, level: f32, note: Option<&str>, is_input: bool, selected: bool, hovered: bool) {
    let r = (rect.height() / 2.0) as u8;
    if selected {
        p.rect_stroke(rect.expand(4.0), CornerRadius::same(r + 4), Stroke::new(2.0, theme::ORANGE), StrokeKind::Outside);
    }
    p.rect_filled(rect, CornerRadius::same(r), theme::KEY_DARK_LOW);
    let body = Rect::from_min_max(rect.min, rect.max - vec2(0.0, 2.0));
    p.rect_filled(body, CornerRadius::same(r), theme::KEY_DARK);
    if hovered && !selected {
        p.rect_stroke(body, CornerRadius::same(r), Stroke::new(1.0, theme::INK_DIM), StrokeKind::Inside);
    }

    // The socket, on the outer side.
    let h = body.height();
    let socket_r = (h * 0.24).clamp(9.0, 16.0);
    let sx = if is_input { body.left() + h / 2.0 } else { body.right() - h / 2.0 };
    let socket = pos2(sx, body.center().y);
    p.circle(socket, socket_r, theme::LCD, Stroke::new(2.0, theme::LCD_WHITE));
    p.circle_filled(socket, socket_r * 0.4, theme::KEY_DARK_LOW);

    let compact = h < 56.0;
    let gap = socket_r + 7.0;
    let (x0, x1) = if is_input { (sx + gap, body.right() - 14.0) } else { (body.left() + 14.0, sx - gap) };
    let top = body.top() + if compact { 7.0 } else { 10.0 };
    p.text(pos2(x0, top), Align2::LEFT_TOP, label, FontId::monospace(if compact { 11.0 } else { 12.0 }), theme::LCD_WHITE);
    let bottom = body.bottom() - if compact { 8.0 } else { 10.0 };
    match note {
        // Compact: the note takes the meter's place.
        Some(note) if compact => {
            p.text(pos2(x0, bottom), Align2::LEFT_BOTTOM, note, FontId::monospace(9.0), theme::ORANGE);
        }
        _ => {
            if let Some(note) = note {
                p.text(pos2(x0, top + 14.0), Align2::LEFT_TOP, note, FontId::monospace(9.0), theme::ORANGE);
            }
            let meter = Rect::from_min_max(pos2(x0, bottom - 6.0), pos2(x1, bottom));
            p.rect_filled(meter.expand(2.0), CornerRadius::same(2), theme::LCD);
            widgets::paint_pixel_meter(p, meter, level, false);
        }
    }

    jack(p, if is_input { pos2(rect.right(), body.center().y) } else { pos2(rect.left(), body.center().y) });
}

/// Where a cable plugs in.
fn jack(p: &Painter, at: Pos2) {
    p.circle(at, 4.5, theme::LCD_WHITE, Stroke::new(1.5, theme::KEY_DARK_LOW));
}

/// The round ⊕ on a hovered cable.
fn paint_plus(p: &Painter, at: Pos2, hot: bool) {
    let (fill, ink) = if hot { (theme::ORANGE, theme::LCD) } else { (theme::KEY_DARK, theme::ORANGE) };
    p.circle(at, PLUS_RADIUS, fill, Stroke::new(1.5, theme::ORANGE));
    plus_sign(p, at, 5.0, Stroke::new(2.0, ink));
}

fn plus_sign(p: &Painter, at: Pos2, arm: f32, stroke: Stroke) {
    p.line_segment([at - vec2(arm, 0.0), at + vec2(arm, 0.0)], stroke);
    p.line_segment([at - vec2(0.0, arm), at + vec2(0.0, arm)], stroke);
}

/// The dashed "add your first module" box of an empty rack.
fn paint_first_target(p: &Painter, rect: Rect, hot: bool) {
    let color = if hot { theme::ORANGE } else { theme::LCD_TEXT_DIM };
    if hot {
        p.rect_filled(rect, CornerRadius::same(10), theme::ORANGE.gamma_multiply(0.08));
    }
    let r = rect.shrink(1.0);
    p.extend(Shape::dashed_line(&rounded_path(r, 8.0), Stroke::new(2.0, color), 8.0, 6.0));
    let c = rect.center();
    plus_sign(p, c - vec2(0.0, 14.0), 10.0, Stroke::new(3.0, color));
    let text = if rect.width() >= 200.0 { "ADD YOUR FIRST MODULE" } else { "ADD A MODULE" };
    p.text(c + vec2(0.0, 14.0), Align2::CENTER_CENTER, text, FontId::monospace(12.0), if hot { theme::ORANGE } else { theme::LCD_WHITE });
}

/// Cut `s` to at most `max` characters, marking the cut.
fn fit_text(s: &str, max: usize) -> String {
    let max = max.max(3);
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 2).collect::<String>() + ".." }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.intersects(*b) && a.intersect(*b).area() > 0.0
    }

    fn rows_of(n: usize, view: Vec2, f: Fit) -> usize {
        let area = Rect::from_min_size(Pos2::ZERO, view).shrink(f.margin);
        layout(n, area, f.node, f.gap).iter().map(|r| r.top() as i32).collect::<std::collections::BTreeSet<_>>().len()
    }

    #[test]
    fn narrower_nodes_save_a_row() {
        // MIC, three modules, OUTPUT beside a docked inspector: one row, not a lone OUTPUT below.
        let view = vec2(790.0, 450.0);
        let f = fit(5, view, true);
        assert!(f.node.x >= MIN_NODE_X && f.node.x < NODE.x && f.node.y == NODE.y);
        assert_eq!(rows_of(5, view, f), 1);
    }

    #[test]
    fn keys_grow_on_a_wide_lcd() {
        let f = fit(5, vec2(1300.0, 700.0), true);
        assert!(f.node.x > NODE.x && f.node.x <= NODE_MAX.x && f.node.y <= NODE_MAX.y);
        assert_eq!(f.node.x % 8.0, 0.0);
        assert_eq!(rows_of(5, vec2(1300.0, 700.0), f), 1);
        // Never for the empty rack, and never past the maximum.
        assert_eq!(fit(3, vec2(1300.0, 700.0), false), ROOMY[0]);
        assert_eq!(fit(2, vec2(5000.0, 700.0), true).node, NODE_MAX);
    }

    #[test]
    fn a_full_rack_fits_a_small_window() {
        // 16 modules + MIC + OUTPUT in the LCD of a 900×560 window.
        let view = vec2(490.0, 378.0);
        let f = fit(18, view, true);
        let area = Rect::from_min_size(Pos2::ZERO, view).shrink(f.margin);
        let r = layout(18, area, f.node, f.gap);
        assert!(r.iter().all(|s| area.expand(0.5).contains_rect(*s)), "{f:?}");
        // A short chain in the same window stays full height.
        assert_eq!(fit(5, view, true).node.y, NODE.y);
    }

    #[test]
    fn empty_rack_is_one_row_with_a_wide_target() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(700.0, 400.0));
        let r = empty_layout(area, ROOMY[0]);
        assert_eq!(r[0].size(), NODE);
        assert_eq!(r[2].size(), NODE);
        assert!(r[1].width() > NODE.x && r[1].width() <= 2.0 * NODE.x);
        assert!((r[0].center().y - r[2].center().y).abs() < 1e-3);
        assert!(r[0].right() < r[1].left() && r[1].right() < r[2].left());
        assert!(r.iter().all(|s| area.contains_rect(*s)));
    }

    #[test]
    fn layout_fits_one_row_and_centres() {
        let avail = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let r = layout(4, avail, vec2(150.0, 80.0), vec2(50.0, 50.0));
        assert_eq!(r.len(), 4);
        assert!(r.iter().all(|x| (x.top() - r[0].top()).abs() < 1e-3));
        assert!(r.iter().all(|x| avail.contains_rect(*x)));
        assert!((r[0].center().y - avail.center().y).abs() < 1e-3);
        let (left, right) = (r[0].left() - avail.left(), avail.right() - r[3].right());
        assert!((left - right).abs() < 1e-3);
    }

    #[test]
    fn layout_snakes_in_reading_order_without_overlap() {
        let node = vec2(150.0, 80.0);
        for width in [160.0, 350.0, 400.0, 560.0, 777.0, 1200.0] {
            let avail = Rect::from_min_size(pos2(10.0, 20.0), vec2(width, 300.0));
            let r = layout(18, avail, node, vec2(50.0, 50.0));
            assert_eq!(r.len(), 18);
            for i in 0..r.len() {
                assert!(r[i].left() >= avail.left() - 1e-3 && r[i].right() <= avail.right() + 1e-3, "width {width}: node {i} does not fit");
                for j in i + 1..r.len() {
                    assert!(!overlaps(&r[i], &r[j]), "width {width}: {i} and {j} overlap");
                }
                if i > 0 {
                    let (a, b) = (r[i - 1], r[i]);
                    assert!(b.top() > a.top() + 1e-3 || (b.top() == a.top() && b.left() > a.left()), "width {width}: {i} is out of order");
                }
            }
            // Taller than the view: starts at the top instead of being centred.
            assert!(r[0].top() >= avail.top() - 1e-3);
        }
        assert!(layout(0, Rect::EVERYTHING, node, vec2(50.0, 50.0)).is_empty());
    }

    #[test]
    fn drop_index_counts_the_removed_module() {
        let slots: Vec<Rect> = (0..4).map(|i| Rect::from_min_size(pos2(i as f32 * 200.0, 0.0), vec2(150.0, 80.0))).collect();
        let at = |x: f32| pos2(x, 40.0);
        // Dragged from the front to just right of the third: ends third.
        assert_eq!(drop_index(at(2.0 * 200.0 + 140.0), &slots, 0), 2);
        // Past the end.
        assert_eq!(drop_index(at(2000.0), &slots, 0), 3);
        assert_eq!(drop_index(at(2000.0), &slots, 3), 3);
        // To the very front.
        assert_eq!(drop_index(at(-50.0), &slots, 3), 0);
        assert_eq!(drop_index(at(10.0), &slots, 2), 0);
        // Dropped on its own place or its neighbours' near edges: stays.
        assert_eq!(drop_index(at(1.0 * 200.0 + 10.0), &slots, 1), 1);
        assert_eq!(drop_index(at(1.0 * 200.0 + 140.0), &slots, 1), 1);
        assert_eq!(drop_index(at(0.0 * 200.0 + 140.0), &slots, 1), 1);
        // Left half of the second: goes before it.
        assert_eq!(drop_index(at(1.0 * 200.0 + 10.0), &slots, 3), 1);
        assert_eq!(drop_index(at(0.0), &[], 0), 0);
    }

    #[test]
    fn drop_index_across_rows() {
        // Two rows of two.
        let slots: Vec<Rect> = (0..4).map(|i| Rect::from_min_size(pos2((i % 2) as f32 * 200.0, (i / 2) as f32 * 130.0), vec2(150.0, 80.0))).collect();
        // Onto the right half of the lower-left node, from the top-left: lands after it.
        assert_eq!(drop_index(pos2(120.0, 170.0), &slots, 0), 2);
        // Onto the left half of the top-right node, from the bottom-right.
        assert_eq!(drop_index(pos2(210.0, 40.0), &slots, 3), 1);
    }

    /// Drives `show` headlessly: one egui frame per call, with the given input events.
    struct Harness {
        ctx: egui::Context,
        rack: Rack,
        shared: Shared,
        st: UiState,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut rack = Rack::new(1);
            rack.restore(&crate::presets::Settings::default().rack_items(), &ctx);
            let mut h = Self { ctx, rack, shared: Shared::default(), st: UiState::default(), time: 0.0 };
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Option<Action> {
            self.time += 1.0 / 30.0;
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))), time: Some(self.time), events, ..Default::default() };
            let mut action = None;
            let Self { ctx, rack, shared, st, .. } = self;
            ctx.run_ui(raw, |ui| action = show(ui, PatchbayCtx { rack, shared, running: false }, st)).textures_delta.clear();
            action
        }

        fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) -> Option<Action> {
            self.frame(vec![egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }])
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) -> Option<Action> {
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }])
        }

        fn ids(&self) -> Vec<ModuleId> {
            self.rack.items().iter().map(|i| i.id).collect()
        }
    }

    #[test]
    fn dragging_a_node_moves_it() {
        let mut h = Harness::new();
        assert_eq!(h.ids(), [1, 2, 3]);
        let (from, to) = (h.st.canvas.slots[3].center(), h.st.canvas.slots[1].left_center() + vec2(10.0, 0.0));
        h.frame(vec![egui::Event::PointerMoved(from)]);
        h.button(from, PointerButton::Primary, true);
        for s in 1..=12 {
            let p = from + (to - from) * (s as f32 / 12.0);
            assert!(h.frame(vec![egui::Event::PointerMoved(p)]).is_none());
        }
        assert!(h.st.canvas.drag.is_some());
        let action = h.button(to, PointerButton::Primary, false);
        assert!(matches!(action, Some(Action::MoveTo(3, 0))));
        assert!(h.st.canvas.drag.is_none());
        assert_eq!(h.st.selected, Selection::Module(3));
    }

    #[test]
    fn click_selects_and_right_click_does_not_drag() {
        let mut h = Harness::new();
        let metal = h.st.canvas.slots[2].center();
        h.frame(vec![egui::Event::PointerMoved(metal)]);
        h.button(metal, PointerButton::Primary, true);
        h.button(metal, PointerButton::Primary, false);
        assert_eq!(h.st.selected, Selection::Module(2));
        let out = h.st.canvas.slots[4].center();
        h.frame(vec![egui::Event::PointerMoved(out)]);
        h.button(out, PointerButton::Secondary, true);
        h.button(out, PointerButton::Secondary, false);
        assert!(h.st.canvas.drag.is_none());
    }

    #[test]
    fn clicking_the_plus_on_a_cable_opens_the_add_menu() {
        let mut h = Harness::new();
        let mid = h.st.canvas.cable_mids[2];
        h.frame(vec![egui::Event::PointerMoved(mid)]);
        h.frame(vec![]);
        h.button(mid, PointerButton::Primary, true);
        h.button(mid, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(h.st.canvas.add_at.map(|(i, _)| i), Some(2));
    }

    #[test]
    fn empty_rack_offers_the_first_module() {
        let mut h = Harness::new();
        for id in h.ids() {
            let mut live = None;
            h.rack.apply(Action::Remove(id), &mut live, &h.ctx.clone());
        }
        h.frame(vec![]);
        assert_eq!(h.st.canvas.slots.len(), 3);
        assert!(h.st.canvas.cable_mids.is_empty());
        let target = h.st.canvas.slots[1].center();
        h.frame(vec![egui::Event::PointerMoved(target)]);
        h.button(target, PointerButton::Primary, true);
        h.button(target, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(h.st.canvas.add_at.map(|(i, _)| i), Some(0));
    }

    #[test]
    fn keyboard_walks_moves_toggles_and_removes() {
        let mut h = Harness::new();
        h.st.selected = Selection::Input;
        h.key(Key::ArrowRight, Modifiers::NONE);
        h.key(Key::ArrowRight, Modifiers::NONE);
        assert_eq!(h.st.selected, Selection::Module(2));
        h.key(Key::Space, Modifiers::NONE);
        assert!(!h.rack.item(2).unwrap().shared.enabled.load(Ordering::Relaxed));
        assert!(matches!(h.key(Key::ArrowLeft, Modifiers::COMMAND), Some(Action::MoveTo(2, 0))));
        assert!(matches!(h.key(Key::Delete, Modifiers::NONE), Some(Action::Remove(2))));
        assert_eq!(h.st.selected, Selection::Module(3));
        h.key(Key::ArrowLeft, Modifiers::NONE);
        h.key(Key::ArrowLeft, Modifiers::NONE);
        h.key(Key::ArrowLeft, Modifiers::NONE);
        assert_eq!(h.st.selected, Selection::Input);
        assert!(h.key(Key::Delete, Modifiers::NONE).is_none());
    }

    #[test]
    fn a_notice_keeps_the_canvas_repainting() {
        let mut h = Harness::new();
        h.st.notify("The rack holds at most 16 modules.");
        h.frame(vec![]);
        assert!(h.ctx.has_requested_repaint());
    }

    #[test]
    fn cable_hover_distance() {
        let slots = layout(3, Rect::from_min_size(Pos2::ZERO, vec2(800.0, 200.0)), NODE, vec2(GAP, GAP));
        let c = Cable::new(&slots, &slots, 0, GAP, 0.0);
        assert!(c.distance(c.mid) < 1.0);
        assert!(c.distance(c.mid + vec2(0.0, 40.0)) > CABLE_HOVER);
    }
}
