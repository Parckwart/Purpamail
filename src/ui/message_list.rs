//! The message list, in two layouts: rich rows beside the viewer ("columns")
//! and a sortable table above the viewer ("classic").

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::actions::*;
use super::app::{ListRow, PurpamailApp};
use super::fonts;
use super::theme::Palette;
use super::widgets;
use crate::core::config::Settings;
use crate::core::model::*;

/// Lay out `text` on a single line, truncated with an ellipsis to `max_w`.
fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.replace(['\n', '\r'], " "), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

fn lines(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.replace(['\n', '\r'], " "), font, color, max_w.max(4.0));
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(4.0), max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

impl PurpamailApp {
    fn list_header(&mut self, ui: &mut Ui, s: &Settings) {
        let pal = Palette::of(ui.ctx());
        egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 8, top: 6, bottom: 4 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(egui::Button::new(RichText::new(format!("Sort by {} {}", s.viewing.sort.name(), ic::CARET_DOWN)).size(12.0).color(pal.text_secondary)).frame(false));
                egui::Popup::menu(&r).show(|ui| {
                    let mut st = self.settings.write();
                    for k in SortKey::ALL {
                        if ui.radio(st.viewing.sort == k, k.name()).clicked() {
                            st.viewing.sort = k;
                            self.list_dirty = true;
                        }
                    }
                    ui.separator();
                    if ui.radio(!st.viewing.sort_ascending, if matches!(st.viewing.sort, SortKey::Date | SortKey::Size) { "Newest / Largest first" } else { "Descending" }).clicked() {
                        st.viewing.sort_ascending = false;
                        self.list_dirty = true;
                    }
                    if ui.radio(st.viewing.sort_ascending, if matches!(st.viewing.sort, SortKey::Date | SortKey::Size) { "Oldest / Smallest first" } else { "Ascending" }).clicked() {
                        st.viewing.sort_ascending = true;
                        self.list_dirty = true;
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let on = self.filter.enabled;
                    let label = if on { self.filter_description() } else { "Filter".into() };
                    let col = if on { pal.accent } else { pal.text_secondary };
                    let r = ui.add(egui::Button::new(RichText::new(format!("{} {label}", if on { ic::FUNNEL } else { ic::FUNNEL_SIMPLE })).size(12.0).color(col)).frame(false));
                    if r.clicked() {
                        self.filter.enabled = !on;
                        if !self.filter.any() {
                            self.filter.unread = true;
                        }
                        self.list_dirty = true;
                    }
                    let r2 = ui.add(egui::Button::new(RichText::new(ic::CARET_DOWN).size(10.0).color(col)).frame(false));
                    egui::Popup::menu(&r2).show(|ui| {
                        let f = &mut self.filter;
                        let mut ch = false;
                        ch |= ui.checkbox(&mut f.unread, "Unread").changed();
                        ch |= ui.checkbox(&mut f.flagged, "Flagged").changed();
                        ch |= ui.checkbox(&mut f.to_me, "To: Me").changed();
                        ch |= ui.checkbox(&mut f.cc_me, "Cc: Me").changed();
                        ch |= ui.checkbox(&mut f.with_attachments, "Only with Attachments").changed();
                        ch |= ui.checkbox(&mut f.from_vip, "Only from VIPs").changed();
                        ch |= ui.checkbox(&mut f.today, "Only from Today").changed();
                        if ch {
                            f.enabled = f.any();
                            self.list_dirty = true;
                        }
                    });
                });
            });
        });
        // Categories bar for inboxes
        let inbox_like = matches!(self.sel, SidebarSel::AllRole(MailboxRole::Inbox)) || matches!(self.sel, SidebarSel::Mailbox(id) if self.mailboxes.iter().any(|m| m.id == id && m.role == MailboxRole::Inbox));
        if s.viewing.show_categories && inbox_like && !self.search_active {
            egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 6 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let cats: [(Option<Category>, &str, &str); 5] = [
                        (None, ic::TRAY, "All Mail"),
                        (Some(Category::Primary), ic::USER, "Primary"),
                        (Some(Category::Transactions), ic::SHOPPING_CART, "Transactions"),
                        (Some(Category::Updates), ic::CHAT_CIRCLE_TEXT, "Updates"),
                        (Some(Category::Promotions), ic::MEGAPHONE, "Promotions"),
                    ];
                    for (c, icon, name) in cats {
                        let active = self.filter.category == c;
                        let text = if active { RichText::new(format!("{icon} {name}")).size(12.0).color(pal.accent_text) } else { RichText::new(icon.to_string()).size(14.0).color(pal.text_secondary) };
                        let b = egui::Button::new(text).fill(if active { pal.accent } else { pal.hover_bg }).corner_radius(12.0);
                        if ui.add(b).on_hover_text(name).clicked() {
                            self.filter.category = c;
                            self.list_dirty = true;
                        }
                    }
                });
            });
        }
        widgets::hline(ui);
    }

    fn filter_description(&self) -> String {
        let f = &self.filter;
        let mut v = Vec::new();
        if f.unread {
            v.push("Unread");
        }
        if f.flagged {
            v.push("Flagged");
        }
        if f.to_me {
            v.push("To Me");
        }
        if f.cc_me {
            v.push("Cc Me");
        }
        if f.with_attachments {
            v.push("Attachments");
        }
        if f.from_vip {
            v.push("VIPs");
        }
        if f.today {
            v.push("Today");
        }
        if v.is_empty() { "Filtered".into() } else { v.join(", ") }
    }

    fn row_click(&mut self, m: egui::Modifiers, id: MessageId) {
        if m.command {
            if let Some(p) = self.selected.iter().position(|x| *x == id) {
                self.selected.remove(p);
            } else {
                self.selected.push(id);
            }
        } else if m.shift && self.anchor.is_some() {
            let a = self.rows.iter().position(|r| Some(r.msg.id) == self.anchor);
            let b = self.rows.iter().position(|r| r.msg.id == id);
            if let (Some(a), Some(b)) = (a, b) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                self.selected = self.rows[lo..=hi].iter().map(|r| r.msg.id).collect();
            }
        } else {
            self.selected = vec![id];
            self.anchor = Some(id);
        }
        self.list_has_focus = true;
        self.on_selection_changed();
    }

    /// Start dragging the selection when a row is dragged.
    fn row_drag(&mut self, resp: &egui::Response, id: MessageId) {
        if resp.drag_started() {
            if !self.selected.contains(&id) {
                self.selected = vec![id];
                self.anchor = Some(id);
            }
            let ids = self.selection_ids();
            resp.dnd_set_drag_payload(ids);
        }
        if resp.dragged() {
            let n = self.selection_ids().len();
            egui::Tooltip::always_open(resp.ctx.clone(), resp.layer_id, resp.id.with("drag"), egui::PopupAnchor::Pointer).gap(12.0).show(|ui| {
                ui.label(format!("{} {} {}", ic::ENVELOPE_SIMPLE, n, if n == 1 { "message" } else { "messages" }));
            });
        }
    }

    pub fn message_list(&mut self, ui: &mut Ui) {
        let s = self.settings.read().clone();
        let pal = Palette::of(ui.ctx());
        self.list_header(ui, &s);
        if self.rows.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                let text = if self.search_active { "No Results" } else if self.filter.enabled { "No messages match the filter" } else { "No Messages" };
                ui.label(RichText::new(text).size(16.0).color(pal.text_tertiary));
                if self.search_active && !self.search_all_mailboxes {
                    if ui.link("Search all mailboxes").clicked() {
                        self.search_all_mailboxes = true;
                        self.list_dirty = true;
                    }
                }
            });
            return;
        }
        let list_font = s.fonts.list_font_size.clamp(9.0, 24.0);
        let line_h = list_font + 5.0;
        let preview = s.viewing.list_preview_lines as usize;
        let row_h = 12.0 + line_h * 2.0 + preview as f32 * (line_h - 1.0) + if preview > 0 { 2.0 } else { 0.0 };
        let show_avatar = s.viewing.show_contact_photos;
        let my_addrs: Vec<String> = s.my_addresses();
        let focused = self.list_has_focus;
        let scroll_target = if self.scroll_to_selected { self.selected.last().and_then(|id| self.rows.iter().position(|r| r.msg.id == *id)) } else { None };
        self.scroll_to_selected = false;
        let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("msglist");
        if let Some(i) = scroll_target {
            // keep the selected row visible
            let y = i as f32 * row_h;
            let cur = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("msglist_offset"))).unwrap_or(0.0);
            let h = ui.available_height();
            if y < cur {
                sa = sa.vertical_scroll_offset(y);
            } else if y + row_h > cur + h {
                sa = sa.vertical_scroll_offset(y + row_h - h);
            }
        }
        let rows = self.rows.clone();
        let out = sa.show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let row = &rows[i];
                self.message_row(ui, row, row_h, line_h, list_font, preview, show_avatar, &my_addrs, focused, &s, &pal);
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("msglist_offset"), out.state.offset.y));
    }

    #[allow(clippy::too_many_arguments)]
    fn message_row(&mut self, ui: &mut Ui, row: &ListRow, row_h: f32, line_h: f32, font_size: f32, preview: usize, show_avatar: bool, my_addrs: &[String], focused: bool, s: &Settings, pal: &Palette) {
        let m = &row.msg;
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, row_h), Sense::click_and_drag());
        let selected = self.selected.contains(&m.id);
        if resp.clicked() {
            self.row_click(ui.input(|i| i.modifiers), m.id);
        }
        if resp.double_clicked() {
            let id = m.id;
            let in_drafts = self.mailboxes.iter().any(|b| b.id == m.mailbox_id && b.role == MailboxRole::Drafts);
            if in_drafts {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: id, kind: crate::core::compose::ReplyKind::EditDraft, selected_text: None }));
            } else {
                self.actions.push(Action::OpenMessageWindow(id));
            }
        }
        if resp.secondary_clicked() && !selected {
            self.selected = vec![m.id];
            self.anchor = Some(m.id);
            self.on_selection_changed();
        }
        self.row_drag(&resp, m.id);
        resp.context_menu(|ui| self.message_context_menu(ui));
        if !ui.is_rect_visible(rect) {
            return;
        }
        let p = ui.painter().clone();
        let unread = if row.thread_count > 1 && !row.is_child { row.thread_unread } else { m.is_unread() };
        let (fg, fg2, fg3) = if selected && focused { (pal.selection_text, pal.selection_text.gamma_multiply(0.9), pal.selection_text.gamma_multiply(0.75)) } else { (pal.text, pal.text_secondary, pal.text_tertiary) };
        // background
        let inner = rect.shrink2(Vec2::new(6.0, 1.0));
        if selected {
            p.rect_filled(inner, 8.0, if focused { pal.selection_bg } else { pal.selection_inactive_bg });
        } else if let Some(h) = row.highlight {
            p.rect_filled(inner, 8.0, pal.flag(h).gamma_multiply(0.18));
        } else if resp.hovered() {
            p.rect_filled(inner, 8.0, pal.hover_bg);
        } else if let Some(c) = s.viewing.highlight_conversations_color {
            if row.thread_count > 1 || row.is_child {
                p.rect_filled(inner, 8.0, pal.flag(c).gamma_multiply(0.10));
            }
        }
        if !selected {
            p.hline(rect.x_range().shrink(14.0), rect.bottom() - 0.5, Stroke::new(0.5, pal.separator));
        }
        let child_indent = if row.is_child { 18.0 } else { 0.0 };
        let gutter_x = rect.left() + 14.0 + child_indent;
        let top = rect.top() + 7.0;
        // gutter: unread dot / replied / forwarded / VIP
        let gy = top + line_h / 2.0;
        if unread {
            p.circle_filled(Pos2::new(gutter_x, gy), 4.5, if selected && focused { pal.selection_text } else { pal.unread_dot });
        } else if m.has(flags::ANSWERED) {
            p.text(Pos2::new(gutter_x, gy), Align2::CENTER_CENTER, ic::ARROW_BEND_UP_LEFT, FontId::proportional(11.0), fg3);
        } else if m.has(flags::FORWARDED) {
            p.text(Pos2::new(gutter_x, gy), Align2::CENTER_CENTER, ic::ARROW_BEND_UP_RIGHT, FontId::proportional(11.0), fg3);
        }
        if row.vip {
            p.text(Pos2::new(gutter_x, gy + line_h), Align2::CENTER_CENTER, ic::STAR, FontId::proportional(11.0), if selected && focused { fg } else { pal.vip });
        }
        if m.priority == Priority::High {
            p.text(Pos2::new(gutter_x, gy + line_h * 2.0), Align2::CENTER_CENTER, "!", FontId::new(12.0, fonts::bold()), if selected && focused { fg } else { pal.danger });
        }
        let mut x = gutter_x + 12.0;
        // avatar
        let sent_folder = self.mailboxes.iter().any(|b| b.id == m.mailbox_id && matches!(b.role, MailboxRole::Sent | MailboxRole::Drafts | MailboxRole::Outbox));
        let from_me = my_addrs.iter().any(|a| a.eq_ignore_ascii_case(&m.from.email));
        let who: Address = if (sent_folder || from_me) && !m.to.is_empty() { m.to[0].clone() } else { m.from.clone() };
        if show_avatar {
            let size = 32.0;
            let c = Pos2::new(x + size / 2.0, top + size / 2.0 + 1.0);
            p.circle_filled(c, size / 2.0, widgets::name_color(if who.email.is_empty() { &who.name } else { &who.email }));
            p.text(c, Align2::CENTER_CENTER, widgets::initials(&who.name, &who.email), FontId::new(13.0, fonts::bold()), Color32::WHITE);
            x += size + 10.0;
        }
        let right = rect.right() - 16.0;
        // line 1: sender — date
        let date = widgets::format_list_date(if s.viewing.sort == SortKey::Date { m.date } else { m.date });
        let date_g = p.layout_no_wrap(date, FontId::proportional(font_size - 1.5), fg3);
        let mut rx = right - date_g.size().x;
        p.galley(Pos2::new(rx, top + (line_h - date_g.size().y) / 2.0), date_g, fg3);
        // flag / attachment icons left of date
        if let Some(c) = row.thread_flag.flatten().or(if m.is_flagged() { m.color.or(Some(FlagColor::Red)) } else { None }) {
            rx -= 16.0;
            p.text(Pos2::new(rx + 6.0, top + line_h / 2.0), Align2::CENTER_CENTER, ic::FLAG, FontId::proportional(12.0), if selected && focused { fg } else { pal.flag(c) });
        } else if row.thread_flag == Some(None) || (m.is_flagged() && m.color.is_none()) {
            rx -= 16.0;
            p.text(Pos2::new(rx + 6.0, top + line_h / 2.0), Align2::CENTER_CENTER, ic::FLAG, FontId::proportional(12.0), pal.flag(FlagColor::Red));
        }
        if m.has_attachments {
            rx -= 16.0;
            p.text(Pos2::new(rx + 6.0, top + line_h / 2.0), Align2::CENTER_CENTER, ic::PAPERCLIP, FontId::proportional(12.0), fg3);
        }
        if m.remind_at.is_some() || m.has(flags::REMINDED) {
            rx -= 16.0;
            p.text(Pos2::new(rx + 6.0, top + line_h / 2.0), Align2::CENTER_CENTER, ic::ALARM, FontId::proportional(12.0), if selected && focused { fg } else { pal.flag(FlagColor::Orange) });
        }
        if row.muted {
            rx -= 16.0;
            p.text(Pos2::new(rx + 6.0, top + line_h / 2.0), Align2::CENTER_CENTER, ic::BELL_SLASH, FontId::proportional(12.0), fg3);
        }
        let bold_sender = unread && s.viewing.display_unread_bold;
        let sender_font = FontId::new(font_size + 0.5, if bold_sender { fonts::list_bold() } else { fonts::list_bold() });
        let mut sender = if (sent_folder || from_me) && !m.to.is_empty() {
            let names: Vec<&str> = m.to.iter().map(|a| a.display()).collect();
            if s.viewing.show_to_cc_labels || sent_folder { format!("To: {}", names.join(", ")) } else { names.join(", ") }
        } else if s.viewing.use_smart_addresses {
            m.from.display().to_string()
        } else {
            m.from.to_header()
        };
        if sender.is_empty() {
            sender = "(No Sender)".into();
        }
        let sg = one_line(ui, &sender, sender_font, fg, rx - x - 8.0);
        p.galley(Pos2::new(x, top + (line_h - sg.size().y) / 2.0), sg, fg);
        // line 2: subject (+ count badge)
        let y2 = top + line_h;
        let mut sub_right = right;
        if row.thread_count > 1 && !row.is_child {
            let expanded = self.expanded_threads.contains(&m.thread_id);
            let label = format!("{} {}", row.thread_count, if expanded { ic::CARET_DOWN } else { ic::CARET_RIGHT });
            let bg = p.layout_no_wrap(label, FontId::proportional(11.0), fg2);
            let bw = bg.size().x + 12.0;
            let br = Rect::from_min_size(Pos2::new(right - bw, y2 + (line_h - 16.0) / 2.0), Vec2::new(bw, 16.0));
            p.rect_filled(br, 8.0, if selected && focused { pal.selection_text.gamma_multiply(0.2) } else { pal.hover_bg });
            p.galley(br.center() - bg.size() / 2.0, bg, fg2);
            let br_resp = ui.interact(br, resp.id.with("thread"), Sense::click());
            if br_resp.clicked() {
                if expanded {
                    self.expanded_threads.remove(&m.thread_id);
                } else {
                    self.expanded_threads.insert(m.thread_id);
                }
                self.list_dirty = true;
            }
            sub_right = br.left() - 6.0;
        }
        if s.viewing.show_categories && m.category != Category::Primary && !row.is_child {
            let cg = p.layout_no_wrap(m.category.name().to_string(), FontId::proportional(10.0), fg2);
            let cw = cg.size().x + 10.0;
            let cr = Rect::from_min_size(Pos2::new(sub_right - cw, y2 + (line_h - 15.0) / 2.0), Vec2::new(cw, 15.0));
            p.rect_stroke(cr, 7.0, Stroke::new(1.0, fg3), egui::StrokeKind::Inside);
            p.galley(cr.center() - cg.size() / 2.0, cg, fg2);
            sub_right = cr.left() - 6.0;
        }
        let subject = if m.subject.trim().is_empty() { "(No Subject)".to_string() } else { m.subject.clone() };
        let subj_font = FontId::new(font_size, if bold_sender { fonts::list_bold() } else { fonts::list() });
        let subg = one_line(ui, &subject, subj_font, fg, sub_right - x);
        p.galley(Pos2::new(x, y2 + (line_h - subg.size().y) / 2.0), subg, fg);
        // preview lines
        if preview > 0 {
            let snippet = if m.snippet.is_empty() && !m.has(flags::HAS_BODY) { String::new() } else { m.snippet.clone() };
            let pg = lines(ui, &snippet, FontId::new(font_size - 0.5, fonts::list()), fg2, right - x, preview);
            p.galley(Pos2::new(x, y2 + line_h + 1.0), pg, fg2);
        }
    }

    /// The classic layout: a table with columns.
    pub fn message_table(&mut self, ui: &mut Ui) {
        use egui_extras::{Column, TableBuilder};
        let s = self.settings.read().clone();
        let pal = Palette::of(ui.ctx());
        let cols = s.viewing.classic_columns.clone();
        let rows = self.rows.clone();
        let my_addrs = s.my_addresses();
        let mut tb = TableBuilder::new(ui).striped(false).resizable(true).sense(Sense::click_and_drag()).cell_layout(egui::Layout::left_to_right(egui::Align::Center)).min_scrolled_height(0.0);
        for c in &cols {
            tb = match c.as_str() {
                "unread" | "flag" | "attachment" => tb.column(Column::exact(18.0)),
                "subject" => tb.column(Column::remainder().at_least(160.0).clip(true)),
                "from" | "to" => tb.column(Column::initial(180.0).at_least(80.0).clip(true)),
                "date" => tb.column(Column::initial(120.0).at_least(60.0)),
                "size" => tb.column(Column::initial(70.0)),
                _ => tb.column(Column::initial(110.0).clip(true)),
            };
        }
        if let Some(i) = if self.scroll_to_selected { self.selected.last().and_then(|id| rows.iter().position(|r| r.msg.id == *id)) } else { None } {
            tb = tb.scroll_to_row(i, None);
            self.scroll_to_selected = false;
        }
        let sort_for = |c: &str| -> Option<SortKey> {
            Some(match c {
                "unread" => SortKey::Unread,
                "flag" => SortKey::Flags,
                "attachment" => SortKey::Attachments,
                "from" => SortKey::From,
                "to" => SortKey::To,
                "subject" => SortKey::Subject,
                "date" => SortKey::Date,
                "size" => SortKey::Size,
                "mailbox" => SortKey::Mailbox,
                "category" => SortKey::Category,
                _ => return None,
            })
        };
        let mut new_sort: Option<SortKey> = None;
        tb.header(22.0, |mut h| {
            for c in &cols {
                h.col(|ui| {
                    let label = match c.as_str() {
                        "unread" => ic::CIRCLE.to_string(),
                        "flag" => ic::FLAG.to_string(),
                        "attachment" => ic::PAPERCLIP.to_string(),
                        other => {
                            let mut t = other.to_string();
                            if let Some(f) = t.get_mut(0..1) {
                                f.make_ascii_uppercase();
                            }
                            t
                        }
                    };
                    let active = sort_for(c) == Some(s.viewing.sort);
                    let arrow = if active { if s.viewing.sort_ascending { " ▲" } else { " ▼" } } else { "" };
                    if ui.add(egui::Label::new(RichText::new(format!("{label}{arrow}")).small().strong().color(pal.text_secondary)).sense(Sense::click())).clicked() {
                        new_sort = sort_for(c);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(22.0, rows.len(), |mut trow| {
                let row = &rows[trow.index()];
                let m = &row.msg;
                let selected = self.selected.contains(&m.id);
                trow.set_selected(selected);
                let unread = if row.thread_count > 1 && !row.is_child { row.thread_unread } else { m.is_unread() };
                let txt = |t: String| {
                    let r = RichText::new(t).size(s.fonts.list_font_size);
                    if unread && s.viewing.display_unread_bold { r.family(fonts::list_bold()) } else { r.family(fonts::list()) }
                };
                for c in &cols {
                    trow.col(|ui| match c.as_str() {
                        "unread" => {
                            if unread {
                                widgets::unread_dot(ui, pal.unread_dot, 14.0);
                            } else if m.has(flags::ANSWERED) {
                                ui.label(RichText::new(ic::ARROW_BEND_UP_LEFT).small().color(pal.text_tertiary));
                            }
                        }
                        "flag" => {
                            if m.is_flagged() {
                                ui.label(RichText::new(ic::FLAG).color(pal.flag(m.color.unwrap_or_default())));
                            }
                        }
                        "attachment" => {
                            if m.has_attachments {
                                ui.label(RichText::new(ic::PAPERCLIP).color(pal.text_tertiary));
                            }
                        }
                        "from" => {
                            let indent = if row.is_child { "    " } else { "" };
                            let vip = if row.vip { format!("{} ", ic::STAR) } else { String::new() };
                            ui.label(txt(format!("{indent}{vip}{}", m.from.display())));
                        }
                        "to" => {
                            ui.label(txt(m.to.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ")));
                        }
                        "subject" => {
                            let cnt = if row.thread_count > 1 && !row.is_child { format!("  ({})", row.thread_count) } else { String::new() };
                            ui.label(txt(format!("{}{cnt}", if m.subject.is_empty() { "(No Subject)" } else { &m.subject })));
                        }
                        "date" => {
                            ui.label(txt(widgets::format_list_date(m.date)));
                        }
                        "size" => {
                            ui.label(txt(crate::core::mime::human_size(m.size as u64)));
                        }
                        "mailbox" => {
                            let name = self.mailboxes.iter().find(|b| b.id == m.mailbox_id).map(|b| if b.role != MailboxRole::Custom { format!("{} ({})", b.role.display_name(), self.account_name(&b.account_id)) } else { b.name.clone() }).unwrap_or_default();
                            ui.label(txt(name));
                        }
                        "category" => {
                            ui.label(txt(m.category.name().to_string()));
                        }
                        _ => {}
                    });
                }
                let resp = trow.response();
                if resp.clicked() {
                    self.row_click(resp.ctx.input(|i| i.modifiers), m.id);
                }
                if resp.double_clicked() {
                    self.actions.push(Action::OpenMessageWindow(m.id));
                }
                if resp.secondary_clicked() && !selected {
                    self.selected = vec![m.id];
                    self.anchor = Some(m.id);
                    self.on_selection_changed();
                }
                self.row_drag(&resp, m.id);
                resp.context_menu(|ui| self.message_context_menu(ui));
                let _ = &my_addrs;
            });
        });
        if let Some(k) = new_sort {
            let mut st = self.settings.write();
            if st.viewing.sort == k {
                st.viewing.sort_ascending = !st.viewing.sort_ascending;
            } else {
                st.viewing.sort = k;
            }
            self.list_dirty = true;
        }
    }
}
