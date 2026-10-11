//! Message headers: sender, subject, recipients with address menus, badges,
//! all raw headers, and the hover toolbar with message actions.

use egui::{Align2, Color32, FontId, Layout, Popup, Rect, RichText, Sense, Stroke, Ui, UiBuilder, Vec2};
use egui_phosphor::regular as ic;

use super::{MessageViewer, People, ViewPrefs};
use crate::core::compose::ReplyKind;
use crate::core::model::*;
use crate::ui::actions::{Action, ComposeInit, UiCx};
use crate::ui::theme::Palette;
use crate::ui::widgets;

/// Which header variant to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum HeaderKind {
    /// Single message: large avatar, subject.
    Full,
    /// Card in a conversation.
    Card { show_subject: bool, show_mailbox: bool },
}

/// Paint a round avatar (contact photo or initials) into `rect`.
pub(super) fn paint_avatar(ui: &Ui, rect: Rect, a: &Address, photo: Option<&super::Photo>) {
    let size = rect.width();
    if let Some((uri, bytes)) = photo {
        egui::Image::from_bytes(uri.clone(), egui::load::Bytes::Shared(bytes.clone())).corner_radius(size / 2.0).paint_at(ui, rect);
        return;
    }
    let c = widgets::name_color(if a.email.is_empty() { &a.name } else { &a.email });
    let p = ui.painter();
    p.circle_filled(rect.center(), size / 2.0, c);
    p.circle_filled(rect.center() - Vec2::new(0.0, size * 0.04), size / 2.0 - size * 0.04, c.gamma_multiply(1.08));
    p.text(rect.center(), Align2::CENTER_CENTER, widgets::initials(&a.name, &a.email), FontId::new(size * 0.38, crate::ui::fonts::bold()), Color32::WHITE);
}

/// Menu for an address; returns true when contacts / VIPs / blocked senders changed.
fn address_menu(ui: &mut Ui, actions: &mut Vec<Action>, people: &People, a: &Address) -> bool {
    let pal = Palette::of(ui.ctx());
    let mut changed = false;
    ui.set_min_width(220.0);
    let name = people.name_of(a, true);
    ui.label(RichText::new(&name).strong().color(pal.text));
    if name != a.email {
        ui.label(RichText::new(&a.email).size(12.0).color(pal.text_secondary));
    }
    ui.separator();
    if ui.button(format!("{}  Copy Address", ic::COPY)).clicked() {
        ui.ctx().copy_text(if a.name.is_empty() { a.email.clone() } else { a.to_header() });
        ui.close();
    }
    if ui.button(format!("{}  New Message to {}", ic::PENCIL_SIMPLE, a.display())).clicked() {
        actions.push(Action::Compose(ComposeInit::New { to: vec![a.clone()], cc: vec![], subject: String::new(), body: String::new(), attachments: vec![] }));
        ui.close();
    }
    if ui.button(format!("{}  Search for Messages from {}", ic::MAGNIFYING_GLASS, a.display())).clicked() {
        actions.push(Action::Search(format!("from:{}", a.email)));
        ui.close();
    }
    ui.separator();
    if people.is_contact(&a.email) {
        if ui.button(format!("{}  Show in Contacts", ic::ADDRESS_BOOK)).clicked() {
            actions.push(Action::ShowContacts);
            ui.close();
        }
    } else if ui.button(format!("{}  Add to Contacts", ic::USER_PLUS)).clicked() {
        actions.push(Action::AddToContacts(a.clone()));
        changed = true;
        ui.close();
    }
    let vip = people.is_vip(&a.email);
    if ui.button(if vip { format!("{}  Remove from VIPs", ic::STAR_HALF) } else { format!("{}  Add to VIPs", ic::STAR) }).clicked() {
        actions.push(Action::SetVip { address: a.clone(), vip: !vip });
        changed = true;
        ui.close();
    }
    let blocked = people.is_blocked(&a.email);
    if ui.button(if blocked { format!("{}  Unblock Contact", ic::CHECK_CIRCLE) } else { format!("{}  Block Contact", ic::PROHIBIT) }).clicked() {
        actions.push(Action::BlockSender { email: a.email.clone(), blocked: !blocked });
        changed = true;
        ui.close();
    }
    changed
}

/// "Remind me" choices: (label, unix time).
pub(super) fn remind_options() -> Vec<(String, i64)> {
    use chrono::{Datelike, Duration, Local, TimeZone, Weekday};
    let now = Local::now();
    let at = |h: u32, days: i64| -> i64 {
        let d = now.date_naive() + Duration::days(days);
        d.and_hms_opt(h, 0, 0).and_then(|t| Local.from_local_datetime(&t).single()).map(|t| t.timestamp()).unwrap_or(now_ts() + 86_400)
    };
    let mut v = vec![("In 1 Hour".to_string(), now_ts() + 3600)];
    if now.time() < chrono::NaiveTime::from_hms_opt(20, 0, 0).unwrap_or_default() {
        v.push(("Tonight (21:00)".into(), at(21, 0)));
    }
    v.push(("Tomorrow (8:00)".into(), at(8, 1)));
    let to_sat = (Weekday::Sat.num_days_from_monday() as i64 - now.weekday().num_days_from_monday() as i64 + 7) % 7;
    v.push(("This Weekend".into(), at(9, if to_sat == 0 { 7 } else { to_sat })));
    let to_mon = (7 - now.weekday().num_days_from_monday() as i64) % 7;
    v.push(("Next Week".into(), at(8, if to_mon == 0 { 7 } else { to_mon })));
    v
}

fn role_icon(role: MailboxRole) -> &'static str {
    match role {
        MailboxRole::Inbox => ic::TRAY,
        MailboxRole::Drafts => ic::FILE_TEXT,
        MailboxRole::Sent => ic::PAPER_PLANE_TILT,
        MailboxRole::Junk => ic::PROHIBIT,
        MailboxRole::Trash => ic::TRASH,
        MailboxRole::Archive => ic::ARCHIVE,
        MailboxRole::Outbox => ic::TRAY_ARROW_UP,
        MailboxRole::Custom => ic::FOLDER_SIMPLE,
    }
}

impl MessageViewer {
    /// Draw the header block of a message.
    pub(super) fn header(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary, kind: HeaderKind) {
        let pal = Palette::of(ui.ctx());
        let full = kind == HeaderKind::Full;
        let avatar = if full { 46.0 } else { 36.0 };
        let mut people_changed = false;
        ui.horizontal_top(|ui| {
            // Avatar (click: sender menu).
            let (av, av_resp) = ui.allocate_exact_size(Vec2::splat(avatar), Sense::click());
            let photo = self.people.photo(&m.from.email);
            paint_avatar(ui, av, &m.from, photo.as_ref());
            if av_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            Popup::menu(&av_resp).show(|ui| people_changed |= address_menu(ui, cx.actions, &self.people, &m.from));
            ui.add_space(if full { 12.0 } else { 10.0 });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                // Sender line with the date on the right.
                ui.horizontal(|ui| {
                    let name = self.people.name_of(&m.from, prefs.smart_addresses);
                    let name = if name.is_empty() { "(Unknown Sender)".to_string() } else { name };
                    let r = ui.add(egui::Label::new(RichText::new(name).size(if full { 15.5 } else { 14.0 }).family(crate::ui::fonts::bold()).color(pal.text)).sense(Sense::click()).truncate());
                    let r = r.on_hover_text(&m.from.email);
                    Popup::menu(&r).show(|ui| people_changed |= address_menu(ui, cx.actions, &self.people, &m.from));
                    if self.people.is_vip(&m.from.email) {
                        ui.label(RichText::new(ic::STAR).color(pal.vip).size(13.0)).on_hover_text("VIP");
                    }
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(widgets::format_full_date(m.date)).size(12.0).color(pal.text_tertiary));
                        if m.is_flagged() {
                            let c = pal.flag(m.color.unwrap_or_default());
                            ui.label(RichText::new(ic::FLAG).color(c).size(13.0)).on_hover_text(format!("Flagged ({})", m.color.unwrap_or_default().name()));
                        }
                        if m.has_attachments {
                            ui.label(RichText::new(ic::PAPERCLIP).color(pal.text_tertiary).size(13.0));
                        }
                        match m.priority {
                            Priority::High => {
                                ui.label(RichText::new("!!").strong().color(pal.danger)).on_hover_text("High priority");
                            }
                            Priority::Low => {
                                ui.label(RichText::new(ic::ARROW_DOWN).color(pal.text_tertiary)).on_hover_text("Low priority");
                            }
                            Priority::Normal => {}
                        }
                    });
                });
                // Subject.
                let show_subject = match kind {
                    HeaderKind::Full => true,
                    HeaderKind::Card { show_subject, .. } => show_subject,
                };
                if show_subject {
                    let subject = if m.subject.trim().is_empty() { "(No Subject)" } else { m.subject.as_str() };
                    ui.add(egui::Label::new(RichText::new(subject).size(if full { 15.0 } else { 13.5 }).color(pal.text)).wrap());
                }
                self.recipients(ui, cx, prefs, m, &mut people_changed);
                self.badges(ui, prefs, m, kind);
                if prefs.all_headers {
                    self.all_headers(ui, m);
                }
                if m.has(flags::DRAFT) {
                    ui.add_space(4.0);
                    if widgets::primary_button(ui, &format!("{}  Edit Draft", ic::PENCIL_SIMPLE)).clicked() {
                        cx.act(Action::Compose(ComposeInit::Respond { message: m.id, kind: ReplyKind::EditDraft, selected_text: None }));
                    }
                }
            });
        });
        if people_changed {
            self.people.loaded_at = 0.0;
        }
    }

    fn recipients(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary, changed: &mut bool) {
        let pal = Palette::of(ui.ctx());
        let pm = self.cache.get(&m.id).and_then(|p| p.parsed.clone());
        let (to, cc, bcc, reply_to) = match &pm {
            Some(p) => (p.to.clone(), p.cc.clone(), p.bcc.clone(), p.reply_to.clone()),
            None => (m.to.clone(), m.cc.clone(), m.bcc.clone(), m.reply_to.clone()),
        };
        let reply_to: Vec<Address> = reply_to.into_iter().filter(|r| !r.email.eq_ignore_ascii_case(&m.from.email)).collect();
        let total = to.len() + cc.len() + bcc.len();
        let details = self.details.contains(&m.id);
        let has_more = total > 3 || !cc.is_empty() || !bcc.is_empty() || !reply_to.is_empty();
        let mut toggle = false;
        let label_w: f32 = if details && !reply_to.is_empty() { 62.0 } else { 30.0 };
        let mut line = |ui: &mut Ui, label: &str, list: &[Address], limit: Option<usize>, extra: usize, show_toggle: bool, toggle_label: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_sized([label_w.min(ui.available_width()), 18.0], egui::Label::new(RichText::new(label).size(12.5).color(pal.text_tertiary)).halign(egui::Align::Max));
                ui.add_space(6.0);
                let n = limit.unwrap_or(list.len()).min(list.len());
                for (i, a) in list.iter().take(n).enumerate() {
                    let name = self.people.name_of(a, prefs.smart_addresses);
                    let vip = self.people.is_vip(&a.email);
                    let text = if vip { format!("{} {name}", ic::STAR) } else { name };
                    let r = ui.add(egui::Label::new(RichText::new(text).size(12.5).color(pal.text_secondary)).sense(Sense::click()));
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let r = r.on_hover_text(&a.email);
                    Popup::menu(&r).show(|ui| *changed |= address_menu(ui, cx.actions, &self.people, a));
                    if i + 1 < n {
                        ui.label(RichText::new(", ").size(12.5).color(pal.text_secondary));
                    }
                }
                if extra > 0 {
                    ui.label(RichText::new(format!(" & {extra} more")).size(12.5).color(pal.text_secondary));
                }
                if show_toggle {
                    ui.add_space(8.0);
                    let r = ui.add(egui::Label::new(RichText::new(toggle_label).size(12.0).color(pal.accent)).sense(Sense::click()));
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if r.clicked() {
                        toggle = true;
                    }
                }
            });
        };
        if !details {
            let all: Vec<Address> = to.iter().chain(cc.iter()).chain(bcc.iter()).cloned().collect();
            if !all.is_empty() {
                let shown = all.len().min(3);
                line(ui, "To:", &all, Some(shown), all.len() - shown, has_more, "Details");
            }
        } else {
            let lines: Vec<(&str, &Vec<Address>)> = [("To:", &to), ("Cc:", &cc), ("Bcc:", &bcc), ("Reply-To:", &reply_to)].into_iter().filter(|(_, l)| !l.is_empty()).collect();
            let n = lines.len();
            for (i, (label, list)) in lines.into_iter().enumerate() {
                line(ui, label, list, None, 0, i + 1 == n, "Hide");
            }
            if n == 0 {
                line(ui, "To:", &[], None, 0, true, "Hide");
            }
        }
        if toggle {
            if details {
                self.details.remove(&m.id);
            } else {
                self.details.insert(m.id);
            }
        }
    }

    fn badges(&mut self, ui: &mut Ui, prefs: &ViewPrefs, m: &MessageSummary, kind: HeaderKind) {
        let pal = Palette::of(ui.ctx());
        let pm = self.cache.get(&m.id).and_then(|p| p.parsed.clone());
        let signed = pm.as_ref().is_some_and(|p| p.signed);
        let encrypted = pm.as_ref().is_some_and(|p| p.encrypted);
        let trackers = prefs.protect_activity && self.cache.get(&m.id).and_then(|p| p.body.as_ref()).is_some_and(|b| b.doc.has_trackers);
        let show_mailbox = matches!(kind, HeaderKind::Card { show_mailbox: true, .. });
        let mailbox = if show_mailbox { self.people.mailbox_label(m.mailbox_id, prefs.multi_account) } else { None };
        if !(signed || encrypted || trackers || mailbox.is_some()) {
            return;
        }
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let soft = if pal.dark { Color32::from_rgb(0x2e, 0x2a, 0x38) } else { Color32::from_rgb(0xf1, 0xed, 0xf8) };
            if let Some(mb) = mailbox {
                let role = self.people.mailboxes.get(&m.mailbox_id).map(|b| b.role).unwrap_or_default();
                widgets::chip(ui, &format!("{} {mb}", role_icon(role)), soft, pal.text_secondary).on_hover_text("Mailbox");
            }
            if signed {
                widgets::chip(ui, &format!("{} Signed", ic::SEAL_CHECK), soft, Color32::from_rgb(0x3c, 0xa5, 0x4a)).on_hover_text("This message carries a digital signature.");
            }
            if encrypted {
                widgets::chip(ui, &format!("{} Encrypted", ic::LOCK_SIMPLE), soft, pal.accent).on_hover_text("This message was encrypted.");
            }
            if trackers {
                widgets::chip(ui, &format!("{} Trackers blocked", ic::SHIELD_CHECK), soft, pal.text_secondary).on_hover_text("Invisible tracking images in this message were not loaded.");
            }
        });
    }

    fn all_headers(&mut self, ui: &mut Ui, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        let Some(pm) = self.cache.get(&m.id).and_then(|p| p.parsed.clone()) else { return };
        ui.add_space(6.0);
        egui::Frame::new().fill(if pal.dark { Color32::from_rgb(0x24, 0x21, 0x2b) } else { Color32::from_rgb(0xf7, 0xf5, 0xfa) }).corner_radius(8).inner_margin(10).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            for (k, v) in &pm.headers {
                let mut job = egui::text::LayoutJob::default();
                let mono = FontId::new(11.5, crate::ui::fonts::fixed());
                job.append(&format!("{k}: "), 0.0, egui::TextFormat::simple(FontId::new(11.5, crate::ui::fonts::bold()), pal.accent));
                job.append(v, 0.0, egui::TextFormat::simple(mono, pal.text_secondary));
                ui.add(egui::Label::new(job).wrap().selectable(true));
            }
        });
    }

    /// Hover toolbar at the top-right of a message header.
    pub(super) fn action_bar(&mut self, ui: &mut Ui, cx: &mut UiCx, m: &MessageSummary, header_rect: Rect) {
        let pal = Palette::of(ui.ctx());
        let hovered = ui.rect_contains_pointer(header_rect.expand(6.0));
        if hovered {
            self.bar_owner = Some(m.id);
        }
        let keep = self.bar_owner == Some(m.id) && Popup::is_any_open(ui.ctx());
        if !(hovered || keep) {
            return;
        }
        let junk = m.has(flags::JUNK) || self.people.mailboxes.get(&m.mailbox_id).is_some_and(|b| b.role == MailboxRole::Junk);
        let n = 7.0;
        let w = n * 34.0 + 10.0;
        let rect = Rect::from_min_size(egui::pos2(header_rect.max.x - w, header_rect.min.y - 4.0), Vec2::new(w, 36.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(egui::Align::Center)));
        let frame_fill = if pal.dark { pal.toolbar_bg } else { pal.window_bg };
        egui::Frame::new()
            .fill(frame_fill)
            .stroke(Stroke::new(1.0, pal.separator))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(4, 3))
            .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if pal.dark { 90 } else { 28 }) })
            .show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if widgets::tool_button(ui, ic::ARROW_BEND_UP_LEFT, "Reply", true).clicked() {
                    cx.act(Action::Compose(ComposeInit::Respond { message: m.id, kind: ReplyKind::Reply, selected_text: self.doc_state.selected_text() }));
                }
                if widgets::tool_button(ui, ic::ARROW_BEND_DOUBLE_UP_LEFT, "Reply All", true).clicked() {
                    cx.act(Action::Compose(ComposeInit::Respond { message: m.id, kind: ReplyKind::ReplyAll, selected_text: self.doc_state.selected_text() }));
                }
                if widgets::tool_button(ui, ic::ARROW_BEND_UP_RIGHT, "Forward", true).clicked() {
                    cx.act(Action::Compose(ComposeInit::Respond { message: m.id, kind: ReplyKind::Forward, selected_text: None }));
                }
                if widgets::tool_button(ui, ic::TRASH, "Move to Trash", true).clicked() {
                    cx.act(Action::Delete(vec![m.id]));
                }
                let (icon, tip) = if junk { (ic::ENVELOPE_SIMPLE_OPEN, "Not Junk") } else { (ic::PROHIBIT, "Move to Junk") };
                if widgets::tool_button(ui, icon, tip, true).clicked() {
                    cx.act(Action::MarkJunk { ids: vec![m.id], junk: !junk });
                }
                let fr = widgets::tool_button(ui, ic::FLAG, "Flag", true);
                if m.is_flagged() {
                    let c = pal.flag(m.color.unwrap_or_default());
                    ui.painter().circle_filled(fr.rect.right_top() + Vec2::new(-8.0, 8.0), 3.0, c);
                }
                Popup::menu(&fr).show(|ui| self.flag_menu(ui, cx, m));
                let mr = widgets::tool_button(ui, ic::DOTS_THREE_CIRCLE, "More", true);
                Popup::menu(&mr).show(|ui| self.more_menu(ui, cx, m));
            });
    }

    fn flag_menu(&mut self, ui: &mut Ui, cx: &mut UiCx, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        let names = cx.settings.read().flag_names.clone();
        for c in FlagColor::ALL {
            let name = names.get(c.name()).cloned().unwrap_or_else(|| c.name().to_string());
            let current = m.is_flagged() && m.color.unwrap_or_default() == c;
            let mut job = egui::text::LayoutJob::default();
            job.append(ic::FLAG, 0.0, egui::TextFormat::simple(FontId::proportional(14.0), pal.flag(c)));
            job.append(&format!("  {name}"), 0.0, egui::TextFormat::simple(FontId::proportional(13.5), pal.text));
            if current {
                job.append(&format!("  {}", ic::CHECK), 0.0, egui::TextFormat::simple(FontId::proportional(13.0), pal.accent));
            }
            if ui.button(job).clicked() {
                cx.act(Action::SetFlagColor { ids: vec![m.id], color: Some(c) });
                ui.close();
            }
        }
        if m.is_flagged() {
            ui.separator();
            if ui.button(format!("{}  Clear Flag", ic::FLAG_BANNER)).clicked() {
                cx.act(Action::SetFlagColor { ids: vec![m.id], color: None });
                ui.close();
            }
        }
    }

    fn more_menu(&mut self, ui: &mut Ui, cx: &mut UiCx, m: &MessageSummary) {
        ui.set_min_width(210.0);
        if m.is_unread() {
            if ui.button(format!("{}  Mark as Read", ic::ENVELOPE_OPEN)).clicked() {
                cx.act(Action::SetFlags { ids: vec![m.id], add: flags::SEEN, remove: 0 });
                ui.close();
            }
        } else if ui.button(format!("{}  Mark as Unread", ic::ENVELOPE_SIMPLE)).clicked() {
            cx.act(Action::SetFlags { ids: vec![m.id], add: 0, remove: flags::SEEN });
            ui.close();
        }
        ui.menu_button(format!("{}  Remind Me", ic::BELL), |ui| {
            for (label, t) in remind_options() {
                if ui.button(label).clicked() {
                    cx.act(Action::Remind { ids: vec![m.id], at: Some(t) });
                    ui.close();
                }
            }
            if m.remind_at.is_some() {
                ui.separator();
                if ui.button("Cancel Reminder").clicked() {
                    cx.act(Action::Remind { ids: vec![m.id], at: None });
                    ui.close();
                }
            }
        });
        let mut targets: Vec<&Mailbox> = self.people.mailboxes.values().filter(|b| b.account_id == m.account_id && b.selectable && b.id != m.mailbox_id && b.role != MailboxRole::Outbox).collect();
        targets.sort_by_key(|b| (b.role == MailboxRole::Custom, b.path.to_lowercase()));
        if !targets.is_empty() {
            ui.menu_button(format!("{}  Move To", ic::FOLDER_SIMPLE_DASHED), |ui| {
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for b in targets {
                        let name = if b.role != MailboxRole::Custom && b.parent.is_none() { b.role.display_name().to_string() } else { b.path.clone() };
                        if ui.button(format!("{}  {name}", role_icon(b.role))).clicked() {
                            cx.act(Action::Move { ids: vec![m.id], dest: b.id });
                            ui.close();
                        }
                    }
                });
            });
        }
        if ui.button(format!("{}  Archive", ic::ARCHIVE)).clicked() {
            cx.act(Action::Archive(vec![m.id]));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}  Print…", ic::PRINTER)).clicked() {
            cx.act(Action::Print(m.id));
            ui.close();
        }
        if ui.button(format!("{}  Save As…", ic::FLOPPY_DISK)).clicked() {
            cx.act(Action::SaveMessageAs(m.id));
            ui.close();
        }
        if ui.button(format!("{}  Export as PDF…", ic::FILE_PDF)).clicked() {
            cx.act(Action::ExportPdf(m.id));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}  Show Raw Source", ic::CODE)).clicked() {
            cx.act(Action::ShowRawSource(m.id));
            ui.close();
        }
        let all = cx.settings.read().viewing.show_header_detail_all;
        if ui.button(if all { format!("{}  Hide All Headers", ic::LIST_DASHES) } else { format!("{}  Show All Headers", ic::LIST_DASHES) }).clicked() {
            cx.settings.write().viewing.show_header_detail_all = !all;
            cx.act(Action::SettingsChanged);
            ui.close();
        }
        if m.has_attachments && ui.button(format!("{}  Remove Attachments", ic::PAPERCLIP_HORIZONTAL)).clicked() {
            cx.act(Action::RemoveAttachments(vec![m.id]));
            ui.close();
        }
        if ui.button(format!("{}  Open in New Window", ic::ARROW_SQUARE_OUT)).clicked() {
            cx.act(Action::OpenMessageWindow(m.id));
            ui.close();
        }
        if m.thread_id != 0 {
            let muted = cx.store.is_muted(m.thread_id);
            if ui.button(if muted { format!("{}  Unmute Conversation", ic::BELL_RINGING) } else { format!("{}  Mute Conversation", ic::BELL_SLASH) }).clicked() {
                cx.act(Action::Mute { thread: m.thread_id, muted: !muted });
                ui.close();
            }
        }
    }
}
