//! Painting and interaction of a laid out document: only visible items are
//! painted; text is selectable (also across paragraphs), links are clickable
//! with a hover preview of their target, images and toggles react to clicks.

use egui::text_selection::LabelSelectionState;
use egui::{Align2, Color32, CursorIcon, FontId, Layout, Popup, PopupKind, SetOpenCommand, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, vec2};
use egui_phosphor::regular as ic;

use super::layout::{ImageContent, Item, LayoutOut, TextItem};
use super::plain::DATE_SCHEME;
use super::tree::{Doc, Link};
use crate::ui::theme::Palette;

/// Something the user did in a document.
#[derive(Clone, Debug, PartialEq)]
pub enum DocEvent {
    /// A link was activated (index into [`Doc::links`]).
    Link(u32),
    /// A collapsible section was toggled.
    Toggle(u64),
    /// An image (not inside a link) was clicked.
    Image(u32),
}

/// Target of a context menu.
pub enum MenuTarget<'a> {
    Link(&'a Link),
    Image(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sel {
    doc: u64,
    anchor: (usize, usize),
    head: (usize, usize),
    dragging: bool,
}

/// Interaction state kept across frames (one per viewer).
#[derive(Default, Debug)]
pub struct DocState {
    sel: Option<Sel>,
    sel_text: Option<String>,
    /// Open link menu: (document, link, position).
    menu: Option<(u64, u32, Pos2)>,
    /// Link under the pointer when a context menu was opened.
    ctx_link: Option<u32>,
}

impl DocState {
    /// Currently selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.sel_text.clone().filter(|s| !s.trim().is_empty())
    }

    pub fn clear_selection(&mut self) {
        self.sel = None;
        self.sel_text = None;
    }
}

/// Does the link open a menu instead of navigating?
fn is_menu_link(href: &str) -> bool {
    href.starts_with(DATE_SCHEME) || href.starts_with("tel:")
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).or_else(|| url.strip_prefix("www.").map(|_| url))?;
    let h = rest.split(['/', '?', '#', ':']).next()?.to_ascii_lowercase();
    Some(h.trim_start_matches("www.").to_string()).filter(|h| h.contains('.'))
}

/// Phishing hint: the visible text is a URL pointing to a different site than the link.
pub fn deceptive_link(link: &Link) -> bool {
    let text = link.text.trim();
    let looks_like_url = text.starts_with("http://") || text.starts_with("https://") || text.starts_with("www.");
    if !looks_like_url {
        return false;
    }
    match (host(text), host(&link.href)) {
        (Some(a), Some(b)) => !(a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))),
        _ => false,
    }
}

fn link_at(t: &TextItem, p: Vec2) -> Option<u32> {
    if t.links.is_empty() {
        return None;
    }
    let row = t.galley.rows.iter().find(|r| {
        let rr = r.rect_without_leading_space();
        rr.min.y <= p.y && p.y < rr.max.y && rr.min.x <= p.x && p.x <= rr.max.x
    })?;
    let _ = row;
    let c = t.galley.cursor_from_pos(p).index.0;
    // the cursor lands between characters; test the character under the pointer
    let probe = |i: usize| t.links.iter().find(|(s, e, _)| *s <= i && i < *e).map(|l| l.2);
    probe(c).or_else(|| c.checked_sub(1).and_then(probe))
}

fn selection_text(lay: &LayoutOut, s: &Sel) -> String {
    let (a, b) = if s.anchor <= s.head { (s.anchor, s.head) } else { (s.head, s.anchor) };
    let mut out = String::new();
    let mut prev: Option<Rect> = None;
    for i in a.0..=b.0.min(lay.items.len().saturating_sub(1)) {
        let Some(Item::Text(t)) = lay.items.get(i) else { continue };
        let chars: Vec<char> = t.galley.text().chars().collect();
        let start = if i == a.0 { a.1.min(chars.len()) } else { 0 };
        let end = if i == b.0 { b.1.min(chars.len()) } else { chars.len() };
        if start >= end {
            continue;
        }
        let r = t.rect();
        if let Some(p) = prev {
            if r.min.y >= p.max.y - 1.0 {
                // A visible gap between blocks is a paragraph break.
                out.push_str(if r.min.y - p.max.y > 4.0 { "\n\n" } else { "\n" });
            } else {
                out.push(' ');
            }
        }
        out.extend(&chars[start..end]);
        prev = Some(r);
    }
    out
}

fn paint_border(p: &egui::Painter, r: Rect, w: [f32; 4], c: [Color32; 4], radius: f32) {
    if w.iter().all(|x| *x == w[0]) && c.iter().all(|x| *x == c[0]) {
        p.rect_stroke(r, radius, Stroke::new(w[0], c[0]), StrokeKind::Inside);
        return;
    }
    if w[0] > 0.0 {
        p.rect_filled(Rect::from_min_max(r.min, egui::pos2(r.max.x, r.min.y + w[0])), 0.0, c[0]);
    }
    if w[2] > 0.0 {
        p.rect_filled(Rect::from_min_max(egui::pos2(r.min.x, r.max.y - w[2]), r.max), 0.0, c[2]);
    }
    if w[3] > 0.0 {
        p.rect_filled(Rect::from_min_max(r.min, egui::pos2(r.min.x + w[3], r.max.y)), 0.0, c[3]);
    }
    if w[1] > 0.0 {
        p.rect_filled(Rect::from_min_max(egui::pos2(r.max.x - w[1], r.min.y), r.max), 0.0, c[1]);
    }
}

fn link_tooltip(ui: &mut Ui, link: &Link) {
    let pal = Palette::of(ui.ctx());
    ui.set_max_width(420.0);
    let href = link.href.as_str();
    let shown = if let Some(ts) = href.strip_prefix(DATE_SCHEME) {
        ts.parse::<i64>().ok().map(crate::ui::widgets::format_full_date).unwrap_or_else(|| "Date".into())
    } else {
        href.to_string()
    };
    ui.label(RichText::new(shown).size(12.0).color(pal.text_secondary));
    if deceptive_link(link) {
        ui.add_space(2.0);
        ui.label(RichText::new(format!("{} The link text shows a different address than where this link goes.", ic::WARNING)).size(12.0).color(pal.danger));
    }
}

/// Paint `lay` into `rect`; returns what the user did.
pub fn paint(ui: &mut Ui, rect: Rect, lay: &LayoutOut, doc: &Doc, key: u64, st: &mut DocState, menu: &mut dyn FnMut(&mut Ui, MenuTarget)) -> Vec<DocEvent> {
    let mut events = Vec::new();
    let ctx = ui.ctx().clone();
    let pal = Palette::of(&ctx);
    let id = ui.id().with(("pm-doc", key));
    let clip = ui.clip_rect().intersect(rect.expand2(vec2(1.0, 4.0)));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("pm-doc-ui", key)));
    child.set_clip_rect(clip);
    let origin = rect.min.to_vec2();
    let (pressed, released, press_origin, pointer, escape) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.press_origin(), i.pointer.latest_pos(), i.key_pressed(egui::Key::Escape)));
    let mut any_hovered = false;
    let sel_items: Option<(usize, usize)> = st.sel.filter(|s| s.doc == key).map(|s| (s.anchor.0, s.head.0));
    let mut sel_changed = false;
    let painter = child.painter().clone();

    for (i, item) in lay.items.iter().enumerate() {
        let r = item.rect().translate(origin);
        let visible = r.intersects(clip);
        let involved = sel_items.is_some_and(|(a, h)| a == i || h == i);
        if !visible && !involved {
            continue;
        }
        match item {
            Item::Rect { fill, radius, .. } => {
                if r.is_positive() {
                    painter.rect_filled(r, *radius, *fill);
                }
            }
            Item::Border { widths, colors, radius, .. } => paint_border(&painter, r, *widths, *colors, *radius),
            Item::Bar { color, .. } => {
                painter.rect_filled(r, 1.5, *color);
            }
            Item::Text(t) => {
                let pos = t.pos + origin;
                let resp = child.interact(r, id.with(("t", i)), Sense::click_and_drag());
                let hover_link = resp.hover_pos().and_then(|p| link_at(t, p - pos));
                if resp.hovered() {
                    any_hovered = true;
                    ctx.set_cursor_icon(if hover_link.is_some() { CursorIcon::PointingHand } else { CursorIcon::Text });
                }
                // Our own selection bookkeeping (egui paints the selection).
                if resp.drag_started() {
                    let from = press_origin.unwrap_or(pos);
                    let c = t.galley.cursor_from_pos(from - pos).index.0;
                    st.sel = Some(Sel { doc: key, anchor: (i, c), head: (i, c), dragging: true });
                    sel_changed = true;
                }
                if let (Some(s), Some(p)) = (st.sel.as_mut(), pointer)
                    && s.dragging && s.doc == key && resp.contains_pointer() {
                        let c = t.galley.cursor_from_pos(p - pos).index.0;
                        if s.head != (i, c) {
                            s.head = (i, c);
                            sel_changed = true;
                        }
                    }
                LabelSelectionState::label_text_selection(&child, &resp, pos, t.galley.clone(), pal.text, Stroke::NONE);
                for (ur, c) in &t.underlines {
                    painter.rect_filled(ur.translate(pos.to_vec2()), 0.0, *c);
                }
                if resp.clicked() {
                    if let Some(l) = hover_link {
                        let href = doc.links.get(l as usize).map(|l| l.href.as_str()).unwrap_or("");
                        if is_menu_link(href) {
                            st.menu = Some((key, l, pointer.unwrap_or(r.center())));
                            Popup::open_id(&ctx, id.with("link-menu"));
                        } else {
                            events.push(DocEvent::Link(l));
                        }
                    }
                    if st.sel.is_some_and(|s| s.doc == key) {
                        st.clear_selection();
                    }
                }
                if resp.secondary_clicked() {
                    st.ctx_link = hover_link;
                }
                if let Some(l) = hover_link.and_then(|l| doc.links.get(l as usize)) {
                    resp.clone().on_hover_ui_at_pointer(|ui| link_tooltip(ui, l));
                }
                let has_sel = st.selected_text().is_some();
                let ctx_link = st.ctx_link;
                let sel_text = st.selected_text();
                resp.context_menu(|ui| {
                    if let Some(l) = ctx_link.and_then(|l| doc.links.get(l as usize)) {
                        menu(ui, MenuTarget::Link(l));
                        if has_sel {
                            ui.separator();
                        }
                    }
                    if has_sel && ui.button(format!("{}  Copy", ic::COPY)).clicked() {
                        if let Some(t) = &sel_text {
                            ui.ctx().copy_text(t.clone());
                        }
                        ui.close();
                    }
                    if ctx_link.is_none() && !has_sel {
                        ui.label(RichText::new("Select text to copy it").color(Palette::of(ui.ctx()).text_tertiary));
                    }
                });
            }
            Item::Image { img, link, content, .. } => {
                match content {
                    ImageContent::Bytes { uri, bytes } => {
                        egui::Image::from_bytes(uri.clone(), egui::load::Bytes::Shared(bytes.clone())).paint_at(&child, r);
                    }
                    ImageContent::Placeholder { alt, loading } => {
                        painter.rect_filled(r, 4.0, lay.placeholder);
                        painter.rect_stroke(r, 4.0, Stroke::new(1.0, lay.muted.gamma_multiply(0.25)), StrokeKind::Inside);
                        if r.width() >= 18.0 && r.height() >= 14.0 {
                            let icon = if *loading { ic::HOURGLASS_MEDIUM } else { ic::IMAGE };
                            let icon_size = (r.height() * 0.5).clamp(10.0, 22.0);
                            if !alt.is_empty() && r.width() > 60.0 {
                                let x = r.min.x + 8.0;
                                painter.text(egui::pos2(x, r.center().y), Align2::LEFT_CENTER, icon, FontId::proportional(icon_size.min(16.0)), lay.muted);
                                let mut job = egui::text::LayoutJob::simple_singleline(alt.clone(), FontId::proportional(12.0), lay.muted);
                                job.wrap = egui::text::TextWrapping::truncate_at_width((r.width() - 34.0).max(10.0));
                                let g = painter.layout_job(job);
                                painter.galley(egui::pos2(x + 20.0, r.center().y - g.size().y / 2.0), g, lay.muted);
                            } else {
                                painter.text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(icon_size), lay.muted);
                            }
                        }
                    }
                }
                let resp = child.interact(r, id.with(("img", i)), Sense::click());
                if resp.hovered() {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                if let Some(l) = link.and_then(|l| doc.links.get(l as usize)) {
                    resp.clone().on_hover_ui_at_pointer(|ui| link_tooltip(ui, l));
                }
                if resp.clicked() {
                    events.push(match link {
                        Some(l) => DocEvent::Link(*l),
                        None => DocEvent::Image(*img),
                    });
                }
                let link = *link;
                let img = *img;
                resp.context_menu(|ui| {
                    if let Some(l) = link.and_then(|l| doc.links.get(l as usize)) {
                        menu(ui, MenuTarget::Link(l));
                        ui.separator();
                    }
                    menu(ui, MenuTarget::Image(img));
                });
            }
            Item::Toggle { key: tkey, open, label, .. } => {
                let resp = child.interact(r, id.with(("toggle", i)), Sense::click());
                let fill = if resp.hovered() { pal.hover_bg } else { lay.placeholder };
                painter.rect_filled(r, r.height() / 2.0, fill);
                painter.rect_stroke(r, r.height() / 2.0, Stroke::new(1.0, pal.separator), StrokeKind::Inside);
                let icon = if *open { ic::CARET_UP } else { ic::DOTS_THREE };
                painter.text(egui::pos2(r.min.x + 14.0, r.center().y), Align2::CENTER_CENTER, icon, FontId::proportional(14.0), lay.muted);
                painter.text(egui::pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.0), lay.muted);
                if resp.hovered() {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    events.push(DocEvent::Toggle(*tkey));
                }
            }
        }
    }

    // Link menu (dates, phone numbers) opened by a left click.
    if let Some((mdoc, l, at)) = st.menu
        && mdoc == key {
            let pid = id.with("link-menu");
            if let Some(link) = doc.links.get(l as usize) {
                Popup::new(pid, ctx.clone(), at, child.layer_id())
                    .kind(PopupKind::Menu)
                    .layout(Layout::top_down_justified(egui::Align::Min))
                    .style(egui::containers::menu::menu_style)
                    .open_memory(None::<SetOpenCommand>)
                    .show(|ui| menu(ui, MenuTarget::Link(link)));
            }
            if !Popup::is_id_open(&ctx, pid) {
                st.menu = None;
            }
        }

    if let Some(s) = st.sel.as_mut()
        && s.doc == key {
            if released {
                s.dragging = false;
            }
            // A press elsewhere ends the selection (but not one inside a menu, e.g. "Copy").
            if escape || (pressed && !any_hovered && !Popup::is_any_open(&ctx)) {
                st.sel = None;
                st.sel_text = None;
            } else if sel_changed {
                let s = *s;
                st.sel_text = Some(selection_text(lay, &s));
            }
        }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phishing_hint() {
        let l = |text: &str, href: &str| Link { href: href.into(), text: text.into() };
        assert!(deceptive_link(&l("https://mybank.com/login", "https://evil.example/login")));
        assert!(!deceptive_link(&l("https://mybank.com/login", "https://www.mybank.com/x")));
        assert!(!deceptive_link(&l("www.mybank.com", "https://secure.mybank.com/")));
        assert!(!deceptive_link(&l("Click here", "https://evil.example/")));
    }
}
