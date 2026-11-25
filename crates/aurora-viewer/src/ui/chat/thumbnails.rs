//! The thumbnails of the open conversations (#1134): the Legacy Viewer's IM
//! chiclets (LLChicletPanel with its LLIMP2PChiclet, LLIMGroupChiclet and
//! LLAdHocChiclet, llchiclet.cpp; one per session from
//! LLChicletBar::sessionAdded, none with FSDisableIMChiclets). One square
//! per conversation, initially in the Conversations list's order: a resident's
//! picture with their status, a group's insignia, the conference glyph. Its
//! unread count wears the one red counter, none while the conversation is
//! shown (LLIMChiclet::setCounter from LLChicletPanel::onMessageCountChanged).
//! A click shows the conversation, or puts it away when it is already shown
//! (FSFloaterIM::toggle); a right click opens the conversation's menu, the
//! list's own, which holds those of menu_imchiclet_p2p / group / adhoc.xml
//! (profile, add friend, show, end the session).
//!
//! Aurora's differences, the human's choices: they stand beside the chat
//! bar by default (the Legacy Viewer keeps them at the top right, the other
//! choice of Preferences › Chat); the bottom row can be reordered on its
//! own, independently of the Conversations list; a message flashes the thumbnail like an
//! entry of the list (`flash.rs`: eight soft rises with the Conversations
//! button's mark, then a faint mark until it is read), where the Legacy
//! Viewer flashes its IM well only; where it scrolls the chiclets with two
//! arrows, a "+N" slot lists those that do not fit, initially keeping unread
//! conversations in sight first; a manual dock order then stays fixed as
//! unread counts change. Without room beside the chat bar (a dock aligned against
//! the chat island, a narrow bar) they go to the top right.

use super::{ChatUi, ConvAction, presence, sidebar};
use crate::settings::ChatThumbnails;
use crate::theme::Palette;
use crate::ui::Panels;
use crate::ui::bars::islands;
use crate::world::World;
use egui::{Align2, Pos2, Rect, Sense, Vec2};
use std::collections::HashMap;
use uuid::Uuid;

/// Room between two thumbnails: the counter of one overhangs its right
/// edge by 2 points.
const GAP: f32 = 4.0;
/// A thumbnail in the Legacy Viewer's bottom bar: the height of its buttons.
pub const BAR_SIZE: f32 = 22.0;

/// The thumbnails' state, kept in `ChatUi`.
#[derive(Default)]
pub struct Thumbnails {
    /// Where they stand and whether they flash (Preferences › Chat), and
    /// Reduce motion, given every frame by the app (`prepare`).
    place: ChatThumbnails,
    flash: bool,
    still: bool,
    /// The conversations and unread counts, in the dock's display order.
    items: Vec<(Uuid, usize)>,
    /// Its own saved order: changing it never moves the Conversations list.
    order: Vec<Uuid>,
    manual_order: bool,
    order_loaded: bool,
    /// The room the bottom bar left them this frame (`reserve`).
    slot: Option<Slot>,
    flashes: super::flash::Flashes,
    /// What their right-click menus asked, handed to `chat::show`.
    pub(super) actions: Vec<ConvAction>,
}

/// Where the thumbnails are drawn: a row of `size` squares in `rect`, in
/// a glass island of their own (`island`) or in the Legacy Viewer's bar.
#[derive(Debug, Clone, Copy)]
struct Slot {
    rect: Rect,
    size: f32,
    island: bool,
}

impl Thumbnails {
    /// The thumbnails beside the chat bar: the room they take, a row of
    /// `size` squares with `pad` at each end, at most `room` (0: none, or
    /// no room for one, they go to the top right).
    pub fn bar_width(&self, size: f32, pad: f32, room: f32) -> f32 {
        if self.place != ChatThumbnails::ChatBar || self.items.is_empty() {
            return 0.0;
        }
        match fit(room, size, pad) {
            0 => 0.0,
            n => row_width(n.min(self.items.len()), size, pad),
        }
    }

    /// The bottom bar left them `rect` (`bar_width` wide).
    pub fn reserve(&mut self, rect: Rect, size: f32, island: bool) {
        if rect.width() > 0.0 {
            self.slot = Some(Slot { rect, size, island });
        }
    }

    /// How many thumbnails the top bars make room for (at the top right).
    pub fn top_count(&self) -> usize {
        if self.place == ChatThumbnails::TopRight {
            self.items.len()
        } else {
            0
        }
    }
}

/// The width of a row of `n` thumbnails of `size` with `pad` at each end,
/// within `room` (0: none): the top bars' room for them.
pub fn top_width(n: usize, size: f32, pad: f32, room: f32) -> f32 {
    match fit(room, size, pad).min(n) {
        0 => 0.0,
        n => row_width(n, size, pad),
    }
}

fn top_id() -> egui::Id {
    egui::Id::unique("conversation_thumbnails_top")
}

/// The top bars left the thumbnails `rect` this frame: an island of the
/// top row, or room in the Legacy Viewer's navigation bar.
pub fn reserve_top(ctx: &egui::Context, rect: Rect, size: f32, island: bool) {
    // (the frame read outside: the context is locked within `data_mut`)
    let frame = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
    ctx.data_mut(|d| d.insert_temp(top_id(), (frame, Slot { rect, size, island })));
}

/// What `reserve_top` left this frame.
fn top_slot(ctx: &egui::Context) -> Option<Slot> {
    let frame = ctx.cumulative_frame_nr_for(egui::ViewportId::ROOT);
    ctx.data(|d| d.get_temp::<(u64, Slot)>(top_id()))
        .filter(|(f, _)| *f == frame)
        .map(|(_, s)| s)
}

/// How many `size` squares a row `room` wide holds, with `pad` at each end.
fn fit(room: f32, size: f32, pad: f32) -> usize {
    ((room - pad * 2.0 + GAP) / (size + GAP)).floor().max(0.0) as usize
}

/// The width of a row of `n` squares of `size`, with `pad` at each end.
fn row_width(n: usize, size: f32, pad: f32) -> f32 {
    pad * 2.0 + n as f32 * size + n.saturating_sub(1) as f32 * GAP
}

/// What a row of `fit` squares shows of `items` (the list's order, with
/// their unread counts): all of them when they fit; else `fit - 1` of them
/// and the "+N" slot, the unread conversations kept in sight first, in the
/// list's order. Returns the indices shown and those left to the "+N" slot.
fn arrange(items: &[(Uuid, usize)], fit: usize) -> (Vec<usize>, Vec<usize>) {
    if items.len() <= fit {
        return ((0..items.len()).collect(), Vec::new());
    }
    let room = fit.saturating_sub(1);
    let unread = items.iter().enumerate().filter(|(_, (_, n))| *n > 0).map(|(i, _)| i);
    let read = items.iter().enumerate().filter(|(_, (_, n))| *n == 0).map(|(i, _)| i);
    let mut shown: Vec<usize> = unread.chain(read).take(room).collect();
    shown.sort_unstable();
    let rest = (0..items.len()).filter(|i| !shown.contains(i)).collect();
    (shown, rest)
}

/// The manual dock order is stable even when an unread count changes. The
/// overflow slot lists the rest in that same horizontal order.
fn arrange_in_order(items: &[(Uuid, usize)], fit: usize) -> (Vec<usize>, Vec<usize>) {
    if items.len() <= fit {
        return ((0..items.len()).collect(), Vec::new());
    }
    let shown = (0..fit.saturating_sub(1)).collect();
    let rest = (fit.saturating_sub(1)..items.len()).collect();
    (shown, rest)
}

/// AURORA_DEMO_THUMBNAILS=bar|top|off: where the thumbnails stand for the
/// run, the settings left alone.
pub fn demo_place() -> Option<ChatThumbnails> {
    match std::env::var("AURORA_DEMO_THUMBNAILS").ok()?.as_str() {
        "bar" => Some(ChatThumbnails::ChatBar),
        "top" => Some(ChatThumbnails::TopRight),
        "off" => Some(ChatThumbnails::Off),
        _ => None,
    }
}

/// The conversations of this frame and the settings, before the bars
/// (they reserve the thumbnails' room).
pub fn prepare(ctx: &egui::Context, st: &mut ChatUi, world: &World, place: ChatThumbnails, flash: bool, still: bool) {
    st.reorder.sync_order(ctx, world);
    let sessions = st.reorder.sessions(world);
    let t = &mut st.thumbnails;
    t.place = place;
    t.flash = flash;
    t.still = still;
    t.slot = None;
    t.sync_order(ctx, &sessions);
    if place == ChatThumbnails::Off {
        t.items.clear();
    }
}

fn order_id() -> egui::Id {
    egui::Id::unique("conversation_dock_manual_order")
}

fn manual_order_id() -> egui::Id {
    egui::Id::unique("conversation_dock_manual_order_enabled")
}

fn unique_order(order: Vec<Uuid>) -> Vec<Uuid> {
    let mut seen = std::collections::HashSet::new();
    order.into_iter().filter(|id| seen.insert(*id)).collect()
}

impl Thumbnails {
    fn load_saved_order(&mut self, ctx: &egui::Context, sessions: &[(Uuid, usize)]) -> bool {
        if self.order_loaded {
            return false;
        }
        let saved = ctx.data_mut(|d| d.get_persisted::<Vec<Uuid>>(order_id()));
        let first_load = saved.is_none();
        self.order = saved.map(unique_order).unwrap_or_else(|| sessions.iter().map(|(id, _)| *id).collect());
        self.manual_order = ctx.data_mut(|d| d.get_persisted::<bool>(manual_order_id())).unwrap_or(false);
        self.order_loaded = true;
        first_load
    }

    fn sync_order(&mut self, ctx: &egui::Context, sessions: &[(Uuid, usize)]) {
        let first_load = self.load_saved_order(ctx, sessions);

        let before = self.order.clone();
        let mut known: std::collections::HashSet<_> = self.order.iter().copied().collect();
        for (id, _) in sessions {
            if known.insert(*id) {
                self.order.push(*id);
            }
        }
        let live: std::collections::HashSet<_> = sessions.iter().map(|(id, _)| *id).collect();
        trim_dock_order(&mut self.order, &live);
        if first_load || self.order != before {
            self.save_order(ctx);
        }

        let unread: HashMap<_, _> = sessions.iter().copied().collect();
        self.items = self.order.iter().filter_map(|id| unread.get(id).map(|n| (*id, *n))).collect();
    }

    fn save_order(&self, ctx: &egui::Context) {
        ctx.data_mut(|d| {
            d.insert_persisted(order_id(), self.order.clone());
            d.insert_persisted(manual_order_id(), self.manual_order);
        });
    }

    fn move_to(&mut self, ctx: &egui::Context, source: Uuid, target: Uuid, after: bool) -> bool {
        if !move_dock_item(&mut self.order, source, target, after) {
            return false;
        }
        self.manual_order = true;
        self.save_order(ctx);
        true
    }

    pub(super) fn rekey(&mut self, ctx: &egui::Context, from: Uuid, to: Uuid) {
        self.load_saved_order(ctx, &[]);
        if !rekey_dock_order(&mut self.order, from, to) {
            return;
        }
        let mut unread = HashMap::new();
        for (id, count) in std::mem::take(&mut self.items) {
            let id = if id == from { to } else { id };
            unread.entry(id).or_insert(count);
        }
        self.items = self.order.iter().filter_map(|id| unread.get(id).map(|count| (*id, *count))).collect();
        self.save_order(ctx);
    }
}

fn trim_dock_order(order: &mut Vec<Uuid>, live: &std::collections::HashSet<Uuid>) {
    const LIMIT: usize = 512;
    while order.len() > LIMIT {
        let Some(stale) = order.iter().position(|id| !live.contains(id)) else { break };
        order.remove(stale);
    }
}

fn move_dock_item(order: &mut Vec<Uuid>, source: Uuid, target: Uuid, after: bool) -> bool {
    if source == target {
        return false;
    }
    let Some(from) = order.iter().position(|id| *id == source) else {
        return false;
    };
    order.remove(from);
    let Some(to) = order.iter().position(|id| *id == target).map(|to| to + usize::from(after)) else {
        order.insert(from, source);
        return false;
    };
    if to == from {
        order.insert(from, source);
        return false;
    }
    order.insert(to, source);
    true
}

fn rekey_dock_order(order: &mut Vec<Uuid>, from: Uuid, to: Uuid) -> bool {
    if from == to {
        return false;
    }
    let Some(at) = order.iter().position(|id| *id == from) else {
        return false;
    };
    order.remove(at);
    if let Some(existing) = order.iter().position(|id| *id == to) {
        order.remove(existing);
    }
    let insertion = at.min(order.len());
    order.insert(insertion, to);
    true
}

/// Is `id` on screen: the conversation shown in the Conversations window,
/// or its own window (`popout.rs`) shown?
fn shown(ctx: &egui::Context, st: &ChatUi, conversations_open: bool, id: Uuid) -> bool {
    if st.popped.contains(Some(id)) {
        super::popout::in_sight(ctx, &st.popped).any(|conversation| conversation == Some(id))
    } else {
        conversations_open && !crate::ui::widgets::floater_minimized(ctx, "conversations") && st.selected == Some(id) && !st.contacts
    }
}

/// A click on the thumbnail of `id`: the conversation shown, with its field
/// taking the keyboard, or put away when it already is (FSFloaterIM::toggle).
fn toggle(ctx: &egui::Context, world: &mut World, st: &mut ChatUi, panels: &mut Panels, id: Uuid, on_screen: bool) {
    let apart = st.popped.contains(Some(id));
    match (on_screen, apart) {
        (true, true) => {
            st.popped.set_hidden(Some(id), true);
        }
        (true, false) => crate::ui::widgets::toggle_floater(ctx, "conversations", &mut panels.chat),
        (false, true) => {
            st.popped.raise(Some(id), true);
        }
        (false, false) => {
            world.focus_conversation(id, true);
            crate::ui::notifications::open_conversations(ctx, &mut panels.chat);
        }
    }
}

/// The thumbnails, after the bars: in the room the bars left them, else at
/// the top right of `work` (the windows' work area) when `top` (the top
/// bars are shown).
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    p: &Palette,
    world: &mut World,
    st: &mut ChatUi,
    panels: &mut Panels,
    pics: &HashMap<Uuid, egui::TextureHandle>,
    images: &HashMap<Uuid, egui::TextureHandle>,
    work: Rect,
    top: bool,
) {
    let ctx = ui.ctx().clone();
    let t = &st.thumbnails;
    if t.items.is_empty() {
        return;
    }
    let slot = match (t.place, t.slot, top_slot(&ctx)) {
        (ChatThumbnails::Off, _, _) => return,
        (ChatThumbnails::ChatBar, Some(slot), _) => slot,
        (ChatThumbnails::TopRight, _, Some(slot)) => slot,
        // the chat bar without room for one: the top right of the windows'
        // area
        _ if !top => return,
        _ => {
            let size = islands::SLOT;
            let height = islands::HEIGHT;
            let pad = (height - size) / 2.0;
            let n = fit(work.width() / 2.0, size, pad).min(t.items.len());
            if n == 0 {
                return;
            }
            let width = row_width(n, size, pad);
            let rect = Rect::from_min_size(Pos2::new(work.right() - width, work.top()), Vec2::new(width, height));
            Slot { rect, size, island: true }
        }
    };
    let now = ctx.input(|i| i.time);
    st.thumbnails.flashes.begin(now);
    let mut moving = false;
    if slot.island {
        islands::area(&ctx, p, "conversation_thumbnails", slot.rect, |ui| {
            moving = row(ui, p, world, st, panels, pics, images, slot, now);
        });
    } else {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(slot.rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // the counters overhang the bar's thumbnails
        child.set_clip_rect(slot.rect.expand(8.0));
        moving = row(&mut child, p, world, st, panels, pics, images, slot, now);
    }
    if moving {
        ctx.request_repaint();
    }
}

/// The row of thumbnails in `slot`; returns whether a flash moves.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    p: &Palette,
    world: &mut World,
    st: &mut ChatUi,
    panels: &mut Panels,
    pics: &HashMap<Uuid, egui::TextureHandle>,
    images: &HashMap<Uuid, egui::TextureHandle>,
    slot: Slot,
    now: f64,
) -> bool {
    let ctx = ui.ctx().clone();
    let pad = (slot.rect.height() - slot.size) / 2.0;
    let fits = fit(slot.rect.width(), slot.size, pad);
    // what is unread in a conversation on screen is being read
    let items: Vec<(Uuid, usize)> = st
        .thumbnails
        .items
        .iter()
        .map(|&(id, n)| (id, if shown(&ctx, st, panels.chat, id) { 0 } else { n }))
        .collect();
    let (in_sight, rest) = if st.thumbnails.manual_order {
        arrange_in_order(&items, fits)
    } else {
        arrange(&items, fits)
    };
    let square = |i: usize| {
        Rect::from_min_size(
            Pos2::new(slot.rect.left() + pad + i as f32 * (slot.size + GAP), slot.rect.top() + pad),
            Vec2::splat(slot.size),
        )
    };
    let mut moving = false;
    let reorderable = slot.rect.center().y >= ctx.content_rect().center().y;
    let visible: Vec<_> = in_sight.iter().map(|&i| items[i].0).collect();
    for (at, &i) in in_sight.iter().enumerate() {
        let (id, unread) = items[i];
        let on_screen = shown(&ctx, st, panels.chat, id);
        let rect = square(at);
        let (wash, flashing) = st.thumbnails.flashes.mark(id, unread, now, st.thumbnails.still);
        let wash = if st.thumbnails.flash { wash } else { 0.0 };
        moving |= flashing && st.thumbnails.flash;
        let resp = thumbnail(ui, p, world, st, pics, images, id, unread, on_screen, wash, rect, reorderable);
        let was_dragging = resp.dragged() || resp.drag_stopped();
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if reorderable
            && resp.drag_stopped()
            && let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()).filter(|pos| slot.rect.contains(*pos))
            && let Some((target, after)) = dock_drop_target(&visible, slot, pointer)
        {
            st.thumbnails.move_to(&ctx, id, target, after);
        }
        super::conv_menu::attach(&resp, p, world, &mut st.thumbnails.actions, Some(id));
        if resp.clicked() && !was_dragging {
            toggle(&ctx, world, st, panels, id, on_screen);
        }
    }
    // the conversations left out keep their flash's timing
    for &i in &rest {
        let (id, unread) = items[i];
        st.thumbnails.flashes.mark(id, unread, now, st.thumbnails.still);
    }
    if !rest.is_empty() {
        let rect = square(in_sight.len());
        if let Some(id) = more(ui, p, world, &items, &rest, rect) {
            toggle(&ctx, world, st, panels, id, false);
        }
    }
    moving
}

/// One thumbnail at `rect`, its picture, flash and counter.
#[allow(clippy::too_many_arguments)]
fn thumbnail(
    ui: &mut egui::Ui,
    p: &Palette,
    world: &World,
    st: &mut ChatUi,
    pics: &HashMap<Uuid, egui::TextureHandle>,
    images: &HashMap<Uuid, egui::TextureHandle>,
    id: Uuid,
    unread: usize,
    on_screen: bool,
    wash: f32,
    rect: Rect,
    reorderable: bool,
) -> egui::Response {
    let sense = if reorderable {
        Sense::click_and_drag().union(Sense::click())
    } else {
        Sense::click()
    };
    let resp = ui.interact(rect, egui::Id::unique(("conversation_thumbnail", id)), sense);
    islands::slot_face(ui, p, rect, resp.hovered(), on_screen);
    if wash > 0.0 {
        crate::ui::widgets::attention_mark(ui.painter(), p, rect, ui.visuals().window_corner_radius, wash);
    }
    let kind = sidebar::kind(world, &id);
    let name = sidebar::entry_name(world, &id, kind);
    let icon = match kind {
        sidebar::Kind::Private => {
            st.wanted_pics.insert(id);
            sidebar::Icon::Avatar(pics.get(&id), presence::of(world, &id), world.social.is_typing(&id))
        }
        sidebar::Kind::Group => {
            let insignia = sidebar::group_insignia(world, &id);
            if let Some(i) = insignia {
                st.wanted_images.insert(i);
            }
            sidebar::Icon::Group(insignia.and_then(|i| images.get(&i)))
        }
        sidebar::Kind::Conference => sidebar::Icon::Glyph("users", p.muted),
    };
    // a call rings there: the picture pulses, as its entry in the list
    if world.calls.invite_for(&id).is_some() {
        let pulse = ((ui.input(|i| i.time) as f32) * 3.0).sin().abs();
        ui.painter().rect_stroke(
            rect.expand(1.0 + pulse * 2.0),
            ui.visuals().window_corner_radius,
            egui::Stroke::new(2.0, p.success.gamma_multiply(1.0 - pulse * 0.7)),
            egui::StrokeKind::Outside,
        );
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    let inset = slot_inset(rect.width()).max(1.0);
    sidebar::paint_picture(ui, p, &icon, &name, rect.shrink(inset), p.bar);
    if unread > 0 {
        islands::count(ui, p, rect, unread, 1.0, false);
    }
    let tip = if unread > 0 {
        aurora_i18n::tr!("ui-bars-unread", value0 = name.clone(), unread = unread.to_string()).to_string()
    } else {
        name
    };
    resp.on_hover_text(tip)
}

fn dock_drop_target(visible: &[Uuid], slot: Slot, pointer: egui::Pos2) -> Option<(Uuid, bool)> {
    let pad = (slot.rect.height() - slot.size) / 2.0;
    visible
        .iter()
        .enumerate()
        .find(|(at, _)| {
            let rect = Rect::from_min_size(
                Pos2::new(slot.rect.left() + pad + *at as f32 * (slot.size + GAP), slot.rect.top() + pad),
                Vec2::splat(slot.size),
            );
            pointer.x < rect.center().x
        })
        .map(|(_, id)| (*id, false))
        .or_else(|| visible.last().copied().map(|id| (id, true)))
}

/// Room between a thumbnail's face and its picture: the flash's outline
/// shows around it.
fn slot_inset(size: f32) -> f32 {
    (size * 0.1).round()
}

/// The "+N" slot at `rect`: the conversations left out (`rest`) and their
/// unread messages; a click lists them. Returns the one picked.
fn more(ui: &mut egui::Ui, p: &Palette, world: &World, items: &[(Uuid, usize)], rest: &[usize], rect: Rect) -> Option<Uuid> {
    let resp = ui.interact(rect, egui::Id::unique("conversation_thumbnails_more"), Sense::click());
    islands::slot_face(ui, p, rect, resp.hovered(), false);
    let unread: usize = rest.iter().map(|&i| items[i].1).sum();
    // under the counter of their unread messages, the number at the bottom
    // left of the square
    let (at, align) = if unread > 0 {
        (rect.left_bottom() + Vec2::new(3.0, -2.0), Align2::LEFT_BOTTOM)
    } else {
        (rect.center(), Align2::CENTER_CENTER)
    };
    ui.painter().text(
        at,
        align,
        format!("+{}", rest.len()),
        egui::FontId::new(11.0, crate::ui::fonts::semibold(ui.ctx())),
        if resp.hovered() { p.ink } else { p.muted },
    );
    if unread > 0 {
        islands::count(ui, p, rect, unread, 1.0, false);
    }
    let resp = resp.on_hover_text(aurora_i18n::tr!("ui-chat-thumbnails-more", n = rest.len() as i64));
    let mut picked = None;
    // above the bottom bar, below the top right's island
    let below = rect.center().y < ui.ctx().content_rect().center().y;
    let opens = if below {
        egui::RectAlign::BOTTOM_END
    } else {
        egui::RectAlign::TOP_END
    };
    crate::ui::menu::dropdown_placed(&resp, p, egui::PopupCloseBehavior::CloseOnClick, Some((opens, 8.0)), |ui| {
        for &i in rest {
            let (id, n) = items[i];
            let kind = sidebar::kind(world, &id);
            let name = sidebar::entry_name(world, &id, kind);
            let icon = match kind {
                sidebar::Kind::Private => "user-filled",
                sidebar::Kind::Group => "users-group",
                sidebar::Kind::Conference => "users",
            };
            let label = if n > 0 {
                aurora_i18n::tr!("ui-bars-unread", value0 = name, unread = n.to_string()).to_string()
            } else {
                name
            };
            if crate::ui::menu::item(ui, p, icon, &label) {
                picked = Some(id);
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(unread: &[usize]) -> Vec<(Uuid, usize)> {
        unread
            .iter()
            .enumerate()
            .map(|(i, &n)| (Uuid::from_u128(i as u128 + 1), n))
            .collect()
    }

    #[test]
    fn a_row_holds_whole_squares_between_its_pads() {
        assert_eq!(fit(row_width(3, 28.0, 4.0), 28.0, 4.0), 3);
        assert_eq!(fit(row_width(3, 28.0, 4.0) - 1.0, 28.0, 4.0), 2);
        assert_eq!(fit(10.0, 28.0, 4.0), 0);
        assert_eq!(row_width(1, 22.0, 0.0), 22.0);
    }

    /// Everything fits: the list's order. Too many: the "+N" slot takes
    /// the last square and the unread conversations stay in sight, still
    /// in the list's order.
    #[test]
    fn the_unread_conversations_stay_in_sight_when_the_row_is_full() {
        assert_eq!(arrange(&items(&[0, 2, 0]), 3), (vec![0, 1, 2], vec![]));
        assert_eq!(arrange(&items(&[0, 0, 0, 5, 1]), 3), (vec![3, 4], vec![0, 1, 2]));
        assert_eq!(arrange(&items(&[0, 0, 0, 5]), 3), (vec![0, 3], vec![1, 2]));
        // one square: the "+N" slot alone
        assert_eq!(arrange(&items(&[1, 1]), 1), (vec![], vec![0, 1]));
    }

    #[test]
    fn manual_dock_order_is_independent_and_survives_restart() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let d = Uuid::from_u128(4);
        let conversation_order = vec![a, b, c];
        let ctx = egui::Context::default();
        let mut dock = Thumbnails::default();
        dock.sync_order(&ctx, &[(a, 0), (b, 0), (c, 0)]);
        assert!(dock.move_to(&ctx, a, c, true));
        assert_eq!(dock.order, vec![b, c, a]);
        assert_eq!(conversation_order, vec![a, b, c], "the dock has its own order");

        let saved = ron::to_string(&ctx.memory(|m| m.clone())).expect("serialize UI preferences");
        let memory: egui::Memory = ron::from_str(&saved).expect("restore UI preferences");
        let restored_ctx = egui::Context::default();
        restored_ctx.memory_mut(|m| *m = memory);
        let mut restored = Thumbnails::default();
        // The list is now in another order and a new session has arrived.
        restored.sync_order(&restored_ctx, &[(c, 2), (a, 0), (b, 1), (d, 5)]);
        assert_eq!(restored.order, vec![b, c, a, d]);
        assert_eq!(restored.items, vec![(b, 1), (c, 2), (a, 0), (d, 5)]);
    }

    #[test]
    fn manual_dock_order_keeps_unread_icons_in_their_positions() {
        let rows = items(&[0, 0, 0, 5]);
        assert_eq!(arrange(&rows, 3), (vec![0, 3], vec![1, 2]));
        assert_eq!(arrange_in_order(&rows, 3), (vec![0, 1], vec![2, 3]));
    }

    #[test]
    fn a_horizontal_drop_targets_the_nearest_icon_edge() {
        let slot = Slot {
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 32.0)),
            size: 28.0,
            island: true,
        };
        let ids = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
        assert_eq!(dock_drop_target(&ids, slot, egui::pos2(47.0, 16.0)), Some((ids[1], false)));
        assert_eq!(dock_drop_target(&ids, slot, egui::pos2(90.0, 16.0)), Some((ids[2], true)));
    }

    #[test]
    fn a_rekeyed_conference_keeps_its_manual_dock_position_and_unread_count() {
        let ctx = egui::Context::default();
        let (a, conference, c, assigned) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3), Uuid::from_u128(4));
        let mut dock = Thumbnails {
            order: vec![a, conference, c],
            manual_order: true,
            order_loaded: true,
            items: vec![(a, 0), (conference, 3), (c, 0)],
            ..Thumbnails::default()
        };
        dock.save_order(&ctx);
        dock.rekey(&ctx, conference, assigned);
        assert_eq!(dock.order, vec![a, assigned, c]);
        assert_eq!(dock.items, vec![(a, 0), (assigned, 3), (c, 0)]);
        assert_eq!(ctx.data_mut(|d| d.get_persisted::<Vec<Uuid>>(order_id())), Some(dock.order.clone()));

        let mut order = vec![a, conference, c, assigned];
        assert!(rekey_dock_order(&mut order, conference, assigned));
        assert_eq!(order, vec![a, assigned, c]);
    }

    #[test]
    fn the_bar_reserves_nothing_without_a_conversation_or_off() {
        let mut t = Thumbnails {
            place: ChatThumbnails::ChatBar,
            ..Default::default()
        };
        assert_eq!(t.bar_width(22.0, 0.0, 500.0), 0.0);
        t.items = items(&[0, 1]);
        assert_eq!(t.bar_width(22.0, 0.0, 500.0), row_width(2, 22.0, 0.0));
        // room for one square: it becomes the "+N" slot
        assert_eq!(t.bar_width(22.0, 0.0, 30.0), 22.0);
        assert_eq!(t.bar_width(22.0, 0.0, 10.0), 0.0);
        t.place = ChatThumbnails::TopRight;
        assert_eq!(t.bar_width(22.0, 0.0, 500.0), 0.0);
    }
}
