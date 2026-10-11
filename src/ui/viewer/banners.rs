//! Banners above the message body (junk, remote content, mailing lists, not
//! downloaded, reminders, ...), the calendar invitation card and summaries.

use egui::{Color32, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::{MessageViewer, ViewPrefs};
use crate::core::ics::{self, CalendarEvent, Rsvp};
use crate::core::model::*;
use crate::core::summary;
use crate::ui::actions::{Action, UiCx};
use crate::ui::theme::Palette;
use crate::ui::widgets;

/// Minimum body length for which a summary is offered.
const SUMMARY_MIN_CHARS: usize = 600;

/// Parse a text/calendar part into an event worth showing.
pub(super) fn parse_event(ics_text: &str) -> Option<CalendarEvent> {
    ics::parse(ics_text).filter(|e| !e.summary.trim().is_empty() || e.start.is_some())
}

/// Human readable time range of an event in local time.
fn event_time(ev: &CalendarEvent) -> String {
    use chrono::{DateTime, Duration, Local};
    let Some(start) = ev.start.and_then(|s| DateTime::from_timestamp(s, 0)).map(|d| d.with_timezone(&Local)) else { return String::new() };
    let end = ev.end.and_then(|e| DateTime::from_timestamp(e, 0)).map(|d| d.with_timezone(&Local));
    if ev.all_day {
        let last = end.map(|e| e - Duration::days(1)).filter(|e| e.date_naive() > start.date_naive());
        return match last {
            Some(l) => format!("{} – {} (all day)", start.format("%-d %B %Y"), l.format("%-d %B %Y")),
            None => format!("{} (all day)", start.format("%A, %-d %B %Y")),
        };
    }
    match end {
        Some(e) if e.date_naive() == start.date_naive() => format!("{}, {} – {}", start.format("%A, %-d %B %Y"), start.format("%H:%M"), e.format("%H:%M")),
        Some(e) => format!("{} – {}", start.format("%-d %b %Y, %H:%M"), e.format("%-d %b %Y, %H:%M")),
        None => format!("{}, {}", start.format("%A, %-d %B %Y"), start.format("%H:%M")),
    }
}

/// A banner button.
struct Btn<'a> {
    label: &'a str,
    primary: bool,
}

/// A rounded banner with an icon, a message and buttons; returns the clicked button.
fn banner(ui: &mut Ui, icon: &str, icon_color: Color32, fill: Color32, text: &str, buttons: &[Btn], spinner: bool) -> Option<usize> {
    let pal = Palette::of(ui.ctx());
    let mut clicked = None;
    egui::Frame::new().fill(fill).corner_radius(8).inner_margin(Margin { left: 12, right: 8, top: 7, bottom: 7 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let font = egui::FontId::proportional(12.5);
        let buttons_w: f32 = buttons.iter().map(|b| ui.fonts_mut(|f| f.layout_no_wrap(b.label.to_string(), font.clone(), Color32::WHITE).size().x) + 24.0).sum::<f32>() + 8.0 * buttons.len() as f32;
        ui.horizontal(|ui| {
            if spinner {
                ui.add(egui::Spinner::new().size(14.0).color(icon_color));
            } else {
                ui.label(RichText::new(icon).size(16.0).color(icon_color));
            }
            ui.add_space(4.0);
            let text_w = (ui.available_width() - buttons_w - 12.0).max(80.0);
            ui.scope(|ui| {
                ui.set_max_width(text_w);
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(pal.text)).wrap());
            });
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                for (i, b) in buttons.iter().enumerate().rev() {
                    let btn = if b.primary {
                        egui::Button::new(RichText::new(b.label).size(12.5).color(pal.accent_text)).fill(pal.accent)
                    } else {
                        egui::Button::new(RichText::new(b.label).size(12.5).color(pal.text)).fill(if pal.dark { Color32::from_white_alpha(14) } else { Color32::from_white_alpha(200) }).stroke(Stroke::new(1.0, pal.separator))
                    };
                    if ui.add(btn.corner_radius(6).min_size(Vec2::new(0.0, 24.0))).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
    });
    ui.add_space(8.0);
    clicked
}

impl MessageViewer {
    pub(super) fn banners(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        let neutral = if pal.dark { Color32::from_rgb(0x2a, 0x26, 0x33) } else { Color32::from_rgb(0xf4, 0xf1, 0xf9) };
        let amber = pal.warning_bg;
        let red_soft = if pal.dark { Color32::from_rgb(0x3d, 0x22, 0x25) } else { Color32::from_rgb(0xfd, 0xec, 0xec) };

        if !m.has(flags::HAS_BODY)
            && banner(ui, ic::CLOUD_ARROW_DOWN, pal.accent, neutral, "This message hasn't been downloaded from the server yet.", &[Btn { label: "Download", primary: false }], true).is_some() {
                cx.act(Action::FetchBody(m.id));
            }
        let junk_mb = self.people.mailboxes.get(&m.mailbox_id).is_some_and(|b| b.role == MailboxRole::Junk);
        if (m.has(flags::JUNK) || junk_mb) && !m.has(flags::NOT_JUNK)
            && banner(ui, ic::WARNING_OCTAGON, Color32::from_rgb(0xd0, 0x8a, 0x1a), amber, "Purpamail thinks this message is junk mail.", &[Btn { label: "Not Junk", primary: false }], false).is_some() {
                cx.act(Action::MarkJunk { ids: vec![m.id], junk: false });
            }
        if self.people.is_blocked(&m.from.email) {
            let text = format!("You blocked {}. Messages from this sender go to the Trash.", m.from.display());
            if banner(ui, ic::PROHIBIT, pal.danger, red_soft, &text, &[Btn { label: "Unblock", primary: false }], false).is_some() {
                cx.act(Action::BlockSender { email: m.from.email.clone(), blocked: false });
                self.people.loaded_at = 0.0;
            }
        }
        let has_remote = self.cache.get(&m.id).and_then(|p| p.body.as_ref()).is_some_and(|b| b.doc.has_remote);
        if has_remote && !self.remote_allowed(prefs, m.id) {
            let text = if prefs.block_all_remote { "Remote content in this message is blocked by your privacy settings." } else { "Remote content is blocked to protect your privacy." };
            if banner(ui, ic::SHIELD_CHECK, pal.accent, neutral, text, &[Btn { label: "Load Remote Content", primary: false }], false).is_some() {
                self.remote_allowed.insert(m.id);
            }
        }
        let list_unsub = !m.list_unsubscribe.is_empty() || self.cache.get(&m.id).and_then(|p| p.parsed.as_ref()).is_some_and(|p| !p.list_unsubscribe.is_empty());
        if list_unsub {
            let list = self.cache.get(&m.id).and_then(|p| p.parsed.as_ref()).map(|p| p.list_id.clone()).unwrap_or_default();
            let name = list.split('<').next().unwrap_or("").trim().trim_matches('"').to_string();
            let text = if name.is_empty() { "This message is from a mailing list.".to_string() } else { format!("This message is from the mailing list “{name}”.") };
            if banner(ui, ic::MEGAPHONE_SIMPLE, pal.text_secondary, neutral, &text, &[Btn { label: "Unsubscribe", primary: false }], false).is_some() {
                cx.act(Action::Unsubscribe(m.id));
            }
        }
        if let Some(t) = m.remind_at {
            let text = format!("Reminder set for {}.", widgets::format_full_date(t));
            if banner(ui, ic::BELL, pal.accent, pal.accent_soft, &text, &[Btn { label: "Cancel Reminder", primary: false }], false).is_some() {
                cx.act(Action::Remind { ids: vec![m.id], at: None });
            }
        }
        if let Some(ev) = self.cache.get(&m.id).and_then(|p| p.event.clone()) {
            self.calendar_card(ui, cx, m, &ev);
        }
        let mut info = Vec::new();
        if m.has(flags::ANSWERED) {
            info.push((ic::ARROW_BEND_UP_LEFT, "You replied to this message."));
        }
        if m.has(flags::FORWARDED) {
            info.push((ic::ARROW_BEND_UP_RIGHT, "You forwarded this message."));
        }
        if m.has(flags::REDIRECTED) {
            info.push((ic::ARROW_ELBOW_UP_RIGHT, "You redirected this message."));
        }
        if !info.is_empty() {
            ui.horizontal(|ui| {
                for (icon, t) in info {
                    ui.label(RichText::new(format!("{icon}  {t}")).size(12.0).color(pal.text_tertiary));
                    ui.add_space(10.0);
                }
            });
            ui.add_space(8.0);
        }
    }

    fn calendar_card(&mut self, ui: &mut Ui, cx: &mut UiCx, m: &MessageSummary, ev: &CalendarEvent) {
        let pal = Palette::of(ui.ctx());
        let method = ev.method.to_ascii_uppercase();
        let cancelled = method == "CANCEL";
        let card = if pal.dark { Color32::from_rgb(0x26, 0x23, 0x2e) } else { Color32::from_rgb(0xfb, 0xfa, 0xfd) };
        egui::Frame::new().fill(card).stroke(Stroke::new(1.0, pal.separator)).corner_radius(12).inner_margin(14).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                // Calendar page tile.
                let (r, _) = ui.allocate_exact_size(Vec2::new(54.0, 58.0), Sense::hover());
                let p = ui.painter();
                let tile_bg = if pal.dark { Color32::from_rgb(0x1e, 0x1c, 0x23) } else { Color32::WHITE };
                p.rect_filled(r, 9.0, tile_bg);
                p.rect_stroke(r, 9.0, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
                let band = egui::Rect::from_min_size(r.min, Vec2::new(r.width(), 18.0));
                p.rect_filled(band, egui::CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, if cancelled { pal.danger } else { pal.accent });
                let start = ev.start.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|d| d.with_timezone(&chrono::Local));
                let (mon, day) = start.map(|d| (d.format("%b").to_string().to_uppercase(), d.format("%-d").to_string())).unwrap_or(("—".into(), "?".into()));
                p.text(band.center(), egui::Align2::CENTER_CENTER, mon, egui::FontId::new(10.5, crate::ui::fonts::bold()), Color32::WHITE);
                p.text(egui::pos2(r.center().x, band.max.y + (r.max.y - band.max.y) / 2.0), egui::Align2::CENTER_CENTER, day, egui::FontId::new(22.0, crate::ui::fonts::bold()), pal.text);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let title = if ev.summary.trim().is_empty() { "Event" } else { ev.summary.as_str() };
                    let title_rt = RichText::new(title).size(15.0).family(crate::ui::fonts::bold()).color(pal.text);
                    ui.add(egui::Label::new(if cancelled { title_rt.strikethrough() } else { title_rt }).wrap());
                    let when = event_time(ev);
                    if !when.is_empty() {
                        ui.label(RichText::new(format!("{}  {when}", ic::CLOCK)).size(12.5).color(pal.text_secondary));
                    }
                    if !ev.location.trim().is_empty() {
                        ui.add(egui::Label::new(RichText::new(format!("{}  {}", ic::MAP_PIN, ev.location.trim())).size(12.5).color(pal.text_secondary)).wrap());
                    }
                    if !ev.organizer.trim().is_empty() {
                        let org_email = ev.organizer.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:");
                        let org = if org_email.contains('@') { self.people.name_of(&Address::new("", org_email), true) } else { org_email.to_string() };
                        let n = ev.attendees.len();
                        let extra = if n > 0 { format!(" · {n} invitee{}", if n == 1 { "" } else { "s" }) } else { String::new() };
                        ui.label(RichText::new(format!("{}  Organizer: {org}{extra}", ic::USER)).size(12.5).color(pal.text_secondary));
                    }
                    ui.add_space(4.0);
                    if cancelled {
                        ui.label(RichText::new(format!("{}  This event has been cancelled.", ic::X_CIRCLE)).size(12.5).color(pal.danger));
                    } else if method == "REPLY" {
                        ui.label(RichText::new(format!("{}  This is a response to an invitation.", ic::INFO)).size(12.5).color(pal.text_tertiary));
                    } else if let Some(r) = self.rsvp.get(&m.id) {
                        let t = match r {
                            Rsvp::Accept => "You accepted this invitation.",
                            Rsvp::Tentative => "You replied “Maybe”.",
                            Rsvp::Decline => "You declined this invitation.",
                        };
                        ui.label(RichText::new(format!("{}  {t}", ic::CHECK_CIRCLE)).size(12.5).color(pal.accent));
                    } else {
                        ui.horizontal(|ui| {
                            let choices = [(Rsvp::Accept, format!("{}  Accept", ic::CHECK), true), (Rsvp::Tentative, format!("{}  Maybe", ic::QUESTION), false), (Rsvp::Decline, format!("{}  Decline", ic::X), false)];
                            for (r, label, primary) in choices {
                                let b = if primary {
                                    egui::Button::new(RichText::new(label).size(12.5).color(pal.accent_text)).fill(pal.accent)
                                } else {
                                    egui::Button::new(RichText::new(label).size(12.5).color(pal.text)).stroke(Stroke::new(1.0, pal.separator))
                                };
                                if ui.add(b.corner_radius(6).min_size(Vec2::new(78.0, 26.0))).clicked() {
                                    cx.act(Action::Rsvp { message: m.id, response: r });
                                    self.rsvp.insert(m.id, r);
                                }
                            }
                        });
                    }
                    let ics_index = self.cache.get(&m.id).and_then(|p| p.attachments.iter().find(|a| a.mime == "text/calendar" || a.name.to_ascii_lowercase().ends_with(".ics")).map(|a| a.index));
                    if let Some(index) = ics_index
                        && ui.add(egui::Button::new(RichText::new(format!("{}  Add to Calendar", ic::CALENDAR_PLUS)).size(12.0).color(pal.accent)).frame(false)).clicked() {
                            cx.act(Action::OpenAttachment { message: m.id, index });
                        }
                });
            });
        });
        ui.add_space(10.0);
    }

    /// Collapsible summary of a single long message.
    pub(super) fn summary_card(&mut self, ui: &mut Ui, m: &MessageSummary) {
        let Some(p) = self.cache.get(&m.id) else { return };
        if p.text.chars().count() < SUMMARY_MIN_CHARS {
            return;
        }
        let open = self.summary_open.contains(&m.id);
        if open && p.summary.is_none() {
            let s = summary::summarize(&p.text, 3);
            if let Some(p) = self.cache.get_mut(&m.id) {
                p.summary = Some(s);
            }
        }
        let text = self.cache.get(&m.id).and_then(|p| p.summary.clone());
        if summary_frame(ui, "Summarize", open, text.as_deref()) {
            if open {
                self.summary_open.remove(&m.id);
            } else {
                self.summary_open.insert(m.id);
            }
        }
    }

    /// Collapsible summary of a whole conversation.
    pub(super) fn thread_summary_card(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, msgs: &[MessageSummary]) {
        let thread = msgs.first().map(|m| m.thread_id).unwrap_or(0);
        let key = -(thread.max(1));
        let approx: usize = msgs.iter().map(|m| m.snippet.len()).sum();
        if approx < SUMMARY_MIN_CHARS / 2 && msgs.len() < 3 {
            return;
        }
        let open = self.summary_open.contains(&key);
        if open && self.thread_summary.as_ref().is_none_or(|(t, n, _)| *t != thread || *n != msgs.len()) {
            let mut texts: Vec<String> = Vec::new();
            for m in msgs {
                let p = self.prepare(cx, prefs, m);
                let body = strip_quotes(&p.text);
                if !body.trim().is_empty() {
                    texts.push(body);
                }
            }
            let s = summary::summarize(&texts.join("\n\n"), 4);
            self.thread_summary = Some((thread, msgs.len(), s));
        }
        let text = self.thread_summary.as_ref().filter(|(t, _, _)| *t == thread).map(|(_, _, s)| s.clone());
        if summary_frame(ui, "Summarize Conversation", open, text.as_deref()) {
            if open {
                self.summary_open.remove(&key);
            } else {
                self.summary_open.insert(key);
            }
        }
    }
}

/// Text without quoted lines and signature (for conversation summaries).
fn strip_quotes(text: &str) -> String {
    let mut out = String::new();
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with('>') {
            continue;
        }
        if l == "-- " || l == "--" {
            break;
        }
        if t.starts_with("On ") && t.ends_with("wrote:") {
            break;
        }
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// The summary card; returns true when its header was clicked.
fn summary_frame(ui: &mut Ui, title: &str, open: bool, text: Option<&str>) -> bool {
    let pal = Palette::of(ui.ctx());
    let mut clicked = false;
    let fill = if pal.dark { Color32::from_rgb(0x2b, 0x25, 0x3d) } else { Color32::from_rgb(0xf5, 0xf0, 0xfd) };
    egui::Frame::new().fill(fill).corner_radius(10).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let r = ui
            .horizontal(|ui| {
                ui.label(RichText::new(format!("{}  {title}", ic::SPARKLE)).size(12.5).family(crate::ui::fonts::bold()).color(pal.accent));
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(if open { ic::CARET_UP } else { ic::CARET_DOWN }).color(pal.accent));
                });
            })
            .response;
        let r = ui.interact(r.rect, ui.id().with(("summary", title)), Sense::click());
        if r.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        clicked = r.clicked();
        if open {
            ui.add_space(4.0);
            match text {
                Some(t) if !t.trim().is_empty() => {
                    ui.add(egui::Label::new(RichText::new(t).size(13.0).color(pal.text)).wrap().selectable(true));
                }
                Some(_) => {
                    ui.label(RichText::new("No summary is available for this message.").size(12.5).color(pal.text_tertiary));
                }
                None => {
                    ui.add(egui::Spinner::new().size(14.0));
                }
            }
        }
    });
    ui.add_space(10.0);
    clicked
}
