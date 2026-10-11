//! Message viewer: headers, banners, body (HTML / plain), attachments and the
//! conversation view with stacked message cards.
//!
//! Each message is parsed once; its body is turned into a render tree (see
//! [`super::html_render`]) that is laid out once per width and cached until
//! [`MessageViewer::invalidate`] is called.

mod attachments;
mod banners;
mod header;
mod raw;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Color32, Margin, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::actions::{Action, ComposeInit, UiCx};
use super::html_render::{self, BuildOpts, DocEvent, DocState, LayoutOpts, MenuTarget, PlainOpts, Rendered};
use super::theme::Palette;
use crate::core::compose::ReplyKind;
use crate::core::ics::CalendarEvent;
use crate::core::mime::{self, ParsedMessage};
use crate::core::model::*;
use crate::core::store::Store;

pub use raw::raw_source_window;

/// A message attachment prepared for display.
#[derive(Clone)]
struct AttInfo {
    /// Index into [`ParsedMessage::attachments`].
    index: usize,
    name: String,
    mime: String,
    size: usize,
    bytes: Arc<[u8]>,
    uri: String,
    /// Pixel size for images.
    dims: Option<Vec2>,
}

impl AttInfo {
    fn is_image(&self) -> bool {
        self.mime.starts_with("image/") && self.dims.is_some()
    }
}

/// Settings that influence how a body is built.
#[derive(Clone, Debug, PartialEq)]
struct BuildKey {
    /// Message and fixed-width font families (galleys depend on them).
    fonts: (String, String),
    size: f32,
    fixed_size: f32,
    fixed: bool,
    has_body: bool,
}

/// A parsed message with its prepared body.
struct Prepared {
    key: BuildKey,
    parsed: Option<Arc<ParsedMessage>>,
    body: Option<Rendered>,
    /// Plain text of the body (summaries, word counts).
    text: String,
    attachments: Vec<AttInfo>,
    event: Option<CalendarEvent>,
    summary: Option<String>,
}

/// Snapshot of the preferences used while drawing.
#[derive(Clone)]
struct ViewPrefs {
    smart_addresses: bool,
    all_headers: bool,
    load_remote: bool,
    block_all_remote: bool,
    protect_activity: bool,
    show_summary: bool,
    fonts: (String, String),
    font_size: f32,
    fixed_size: f32,
    fixed_plain: bool,
    quote_colors: [Color32; 3],
    quote_text: bool,
    multi_account: bool,
    strip_tracking: bool,
}

impl ViewPrefs {
    fn read(cx: &UiCx, pal: &Palette) -> Self {
        let s = cx.settings.read();
        let qc = |i: usize| super::theme::hex(&s.fonts.quote_colors[i]).unwrap_or(pal.quote_bar);
        ViewPrefs {
            smart_addresses: s.viewing.use_smart_addresses,
            all_headers: s.viewing.show_header_detail_all,
            load_remote: s.viewing.load_remote_content,
            block_all_remote: s.privacy.block_all_remote_content,
            protect_activity: s.privacy.protect_mail_activity,
            show_summary: s.viewing.show_summary,
            fonts: (s.fonts.message_font.clone(), s.fonts.fixed_font.clone()),
            font_size: s.fonts.message_font_size.clamp(8.0, 40.0),
            fixed_size: s.fonts.fixed_font_size.clamp(8.0, 40.0),
            fixed_plain: s.fonts.use_fixed_for_plain_text,
            quote_colors: if s.fonts.color_quoted_text { [qc(0), qc(1), qc(2)] } else { [pal.separator; 3] },
            quote_text: s.fonts.color_quoted_text,
            multi_account: s.accounts.len() > 1,
            strip_tracking: s.privacy.strip_tracking_parameters,
        }
    }
}

/// A contact photo: its `bytes://` URI and the image bytes.
type Photo = (String, Arc<[u8]>);

/// Contacts, VIPs, blocked senders and mailboxes, refreshed every few seconds
/// instead of querying the store for every address on every frame.
#[derive(Default)]
struct People {
    loaded_at: f64,
    contacts_at: f64,
    /// Prepared contact photos by contact id (with the photo size).
    photos: HashMap<i64, (usize, Photo)>,
    vips: HashSet<String>,
    contacts: HashMap<String, (String, Option<Photo>)>,
    blocked: HashSet<String>,
    mailboxes: HashMap<MailboxId, Mailbox>,
    accounts: HashMap<String, String>,
}

impl People {
    fn refresh(&mut self, store: &Store, cx: &UiCx, now: f64, force: bool) {
        if !force && now - self.loaded_at < 3.0 && self.loaded_at > 0.0 {
            return;
        }
        let contacts_due = force || self.loaded_at == 0.0 || now - self.contacts_at > 20.0;
        self.loaded_at = now;
        self.vips = store.vip_emails().into_iter().map(|e| e.to_lowercase()).collect();
        self.blocked = store.blocked_senders().into_iter().map(|e| e.to_lowercase()).collect();
        if contacts_due {
            self.contacts_at = now;
            self.contacts.clear();
            let mut photos = HashMap::new();
            for c in store.contacts() {
                let photo = c.photo.as_ref().filter(|p| !p.is_empty()).map(|p| {
                    // Reuse the prepared photo when it did not change.
                    match self.photos.get(&c.id) {
                        Some((len, ph)) if *len == p.len() => ph.clone(),
                        _ => (html_render::tree::bytes_uri(p), Arc::<[u8]>::from(p.as_slice())),
                    }
                });
                if let Some(ph) = &photo {
                    photos.insert(c.id, (ph.1.len(), ph.clone()));
                }
                for e in &c.emails {
                    self.contacts.insert(e.to_lowercase(), (c.display_name(), photo.clone()));
                }
            }
            self.photos = photos;
        }
        self.mailboxes = store.mailboxes(None).into_iter().map(|m| (m.id, m)).collect();
        let s = cx.settings.read();
        self.accounts = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
        self.accounts.insert(LOCAL_ACCOUNT.to_string(), "On This Computer".to_string());
    }

    fn is_vip(&self, email: &str) -> bool {
        self.vips.contains(&email.to_lowercase())
    }

    fn is_contact(&self, email: &str) -> bool {
        self.contacts.contains_key(&email.to_lowercase())
    }

    fn is_blocked(&self, email: &str) -> bool {
        self.blocked.contains(&email.to_lowercase())
    }

    /// Display name for an address ("smart addresses": contact name, else header name).
    fn name_of(&self, a: &Address, smart: bool) -> String {
        if !smart {
            return if a.name.trim().is_empty() { a.email.clone() } else { format!("{} <{}>", a.name, a.email) };
        }
        if let Some((n, _)) = self.contacts.get(&a.email.to_lowercase())
            && !n.trim().is_empty() && !n.contains('@') {
                return n.clone();
            }
        a.display().to_string()
    }

    fn photo(&self, email: &str) -> Option<Photo> {
        self.contacts.get(&email.to_lowercase()).and_then(|(_, p)| p.clone())
    }

    fn mailbox_label(&self, id: MailboxId, with_account: bool) -> Option<String> {
        let m = self.mailboxes.get(&id)?;
        let name = if m.role != MailboxRole::Custom && m.parent.is_none() { m.role.display_name().to_string() } else { m.name.clone() };
        if with_account {
            let acc = self.accounts.get(&m.account_id).cloned().unwrap_or_default();
            Some(if acc.is_empty() { name } else { format!("{name} — {acc}") })
        } else {
            Some(name)
        }
    }
}

/// An in-app preview of an attachment.
struct Preview {
    message: MessageId,
    att: AttInfo,
    open: bool,
}

#[derive(Default)]
pub struct MessageViewer {
    cache: HashMap<MessageId, Prepared>,
    /// Messages whose remote content the user allowed.
    remote_allowed: HashSet<MessageId>,
    /// Conversation cards expanded / collapsed by the user.
    expanded: HashSet<MessageId>,
    collapsed: HashSet<MessageId>,
    expand_all: bool,
    /// Messages that were unread when the conversation was opened (shown expanded).
    initially_unread: HashSet<MessageId>,
    /// Messages with the full recipient list shown.
    details: HashSet<MessageId>,
    /// Expanded collapsible sections (long quotes) per message.
    sections: HashMap<MessageId, HashSet<u64>>,
    /// Open summary cards (message id, or negative thread id for conversations).
    summary_open: HashSet<i64>,
    thread_summary: Option<(i64, usize, String)>,
    doc_state: DocState,
    /// Messages currently shown and the focused one (to detect selection changes).
    shown: Vec<MessageId>,
    focus: Option<MessageId>,
    scroll_to_focus: bool,
    /// The card whose hover toolbar was last shown (kept while its menus are open).
    bar_owner: Option<MessageId>,
    previews: Vec<Preview>,
    people: People,
    /// Invitation responses sent from this viewer.
    rsvp: HashMap<MessageId, crate::core::ics::Rsvp>,
}

impl MessageViewer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show one message, or a whole conversation when `msgs` has several entries
    /// (in the order given). `focus` is the message the user selected.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut UiCx, msgs: &[MessageSummary], focus: Option<MessageId>) {
        let pal = Palette::of(ui.ctx());
        let prefs = ViewPrefs::read(cx, &pal);
        let now = ui.input(|i| i.time);
        self.people.refresh(cx.store, cx, now, false);

        let ids: Vec<MessageId> = msgs.iter().map(|m| m.id).collect();
        let changed = ids != self.shown || focus != self.focus;
        if changed {
            let same_conversation = !ids.is_empty() && ids.iter().any(|i| self.shown.contains(i));
            if !same_conversation {
                self.expanded.clear();
                self.collapsed.clear();
                self.expand_all = false;
                self.details.clear();
                self.previews.clear();
            }
            if !same_conversation {
                self.initially_unread = msgs.iter().filter(|m| m.is_unread()).map(|m| m.id).collect();
            } else {
                self.initially_unread.extend(msgs.iter().filter(|m| m.is_unread()).map(|m| m.id));
            }
            self.doc_state.clear_selection();
            self.scroll_to_focus = msgs.len() > 1;
            self.shown = ids;
            self.focus = focus;
            // Keep the cache bounded.
            if self.cache.len() > 64 {
                let keep: HashSet<MessageId> = self.shown.iter().copied().collect();
                self.cache.retain(|k, _| keep.contains(k));
            }
        }

        let scroll_id = if msgs.len() > 1 { "viewer-conversation" } else { "viewer-message" };
        let mut scroll = egui::ScrollArea::vertical().id_salt((scroll_id, msgs.first().map(|m| m.id))).auto_shrink([false, false]);
        if changed {
            // A newly selected message always starts at the top.
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            let side = if ui.available_width() > 700.0 { 28 } else { 16 };
            egui::Frame::new().inner_margin(Margin { left: side, right: side, top: 18, bottom: 28 }).show(ui, |ui| {
                if msgs.len() == 1 {
                    self.single(ui, cx, &prefs, &msgs[0]);
                } else {
                    self.conversation(ui, cx, &prefs, msgs, focus);
                }
            });
        });
        self.previews_ui(ui.ctx(), cx);
    }

    /// Forget cached rendering for a message (body arrived, flags changed, ...).
    pub fn invalidate(&mut self, id: MessageId) {
        self.cache.remove(&id);
        if self.thread_summary.is_some() {
            self.thread_summary = None;
        }
    }

    /// Text currently selected by the user in the viewer (for "quote selected text" replies).
    pub fn selected_text(&self) -> Option<String> {
        self.doc_state.selected_text()
    }

    // ------------------------------------------------------------------ preparation

    fn prepare(&mut self, cx: &UiCx, prefs: &ViewPrefs, m: &MessageSummary) -> &mut Prepared {
        let key = BuildKey { fonts: prefs.fonts.clone(), size: prefs.font_size, fixed_size: prefs.fixed_size, fixed: prefs.fixed_plain, has_body: m.has(flags::HAS_BODY) };
        let stale = self.cache.get(&m.id).is_none_or(|p| p.key != key);
        if stale {
            let p = build_prepared(cx.store, key, m.id);
            self.cache.insert(m.id, p);
        }
        self.cache.get_mut(&m.id).expect("just inserted")
    }

    fn remote_allowed(&self, prefs: &ViewPrefs, id: MessageId) -> bool {
        self.remote_allowed.contains(&id) || (prefs.load_remote && !prefs.block_all_remote)
    }

    // ------------------------------------------------------------------ single message

    fn single(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        self.prepare(cx, prefs, m);
        let top = ui.cursor().min;
        self.header(ui, cx, prefs, m, header::HeaderKind::Full);
        let header_rect = Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y));
        self.action_bar(ui, cx, m, header_rect);
        ui.add_space(12.0);
        super::widgets::hline(ui);
        ui.add_space(12.0);
        self.banners(ui, cx, prefs, m);
        if prefs.show_summary {
            self.summary_card(ui, m);
        }
        self.body(ui, cx, prefs, m, pal.viewer_bg);
        self.attachments(ui, cx, m);
    }

    // ------------------------------------------------------------------ conversation

    fn is_expanded(&self, m: &MessageSummary, focus: Option<MessageId>) -> bool {
        if self.collapsed.contains(&m.id) {
            return false;
        }
        self.expand_all || self.expanded.contains(&m.id) || Some(m.id) == focus || self.initially_unread.contains(&m.id) || m.is_unread() || m.has(flags::DRAFT)
    }

    fn conversation(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, msgs: &[MessageSummary], focus: Option<MessageId>) {
        let pal = Palette::of(ui.ctx());
        // Conversation title.
        let subject = msgs.iter().find(|m| !m.subject.trim().is_empty()).map(|m| normalize_subject(&m.subject)).unwrap_or_else(|| "(No Subject)".into());
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(&subject).size(20.0).family(super::fonts::bold()).color(pal.text)).wrap());
        });
        ui.horizontal(|ui| {
            let people: HashSet<&str> = msgs.iter().map(|m| m.from.email.as_str()).collect();
            ui.label(RichText::new(format!("{} messages · {} {}", msgs.len(), people.len(), if people.len() == 1 { "person" } else { "people" })).size(12.0).color(pal.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let all = msgs.iter().all(|m| self.is_expanded(m, focus));
                let label = if all { format!("{} Collapse All", ic::ARROWS_IN_LINE_VERTICAL) } else { format!("{} Expand All", ic::ARROWS_OUT_LINE_VERTICAL) };
                if ui.add(egui::Button::new(RichText::new(label).size(12.0).color(pal.accent)).frame(false)).clicked() {
                    if all {
                        self.expand_all = false;
                        self.expanded.clear();
                        self.collapsed = msgs.iter().filter(|m| Some(m.id) != focus).map(|m| m.id).collect();
                    } else {
                        self.expand_all = true;
                        self.collapsed.clear();
                        let unread: Vec<MessageId> = msgs.iter().filter(|m| m.is_unread()).map(|m| m.id).collect();
                        if !unread.is_empty() {
                            cx.act(Action::SetFlags { ids: unread, add: flags::SEEN, remove: 0 });
                        }
                    }
                }
            });
        });
        ui.add_space(10.0);
        if prefs.show_summary {
            self.thread_summary_card(ui, cx, prefs, msgs);
        }
        let focus_mailbox = focus.and_then(|f| msgs.iter().find(|m| m.id == f)).map(|m| m.mailbox_id);
        let mut focus_rect = None;
        for m in msgs {
            let expanded = self.is_expanded(m, focus);
            let r = self.card(ui, cx, prefs, m, expanded, &subject, focus_mailbox, Some(m.id) == focus);
            if Some(m.id) == focus {
                focus_rect = Some(r);
            }
            ui.add_space(10.0);
        }
        if self.scroll_to_focus {
            if let Some(r) = focus_rect {
                ui.scroll_to_rect(r.with_max_y(r.min.y + 60.0), None);
            }
            self.scroll_to_focus = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn card(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary, expanded: bool, subject: &str, focus_mailbox: Option<MailboxId>, focused: bool) -> Rect {
        let pal = Palette::of(ui.ctx());
        let card_bg = if pal.dark { pal.window_bg } else { pal.viewer_bg };
        let stroke = if focused && expanded { Stroke::new(1.0, pal.accent.gamma_multiply(0.55)) } else { Stroke::new(1.0, pal.separator) };
        let frame = egui::Frame::new()
            .fill(card_bg)
            .stroke(stroke)
            .corner_radius(12)
            .inner_margin(Margin { left: 16, right: 16, top: 12, bottom: if expanded { 16 } else { 12 } })
            .shadow(egui::Shadow { offset: [0, 1], blur: 6, spread: 0, color: Color32::from_black_alpha(if pal.dark { 60 } else { 14 }) });
        let resp = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !expanded {
                self.collapsed_row(ui, cx, prefs, m);
                return;
            }
            self.prepare(cx, prefs, m);
            let top = ui.cursor().min;
            let other_mailbox = focus_mailbox.is_some_and(|f| f != m.mailbox_id);
            let show_subject = normalize_subject(&m.subject) != subject && !m.subject.trim().is_empty();
            let kind = header::HeaderKind::Card { show_subject, show_mailbox: other_mailbox };
            self.header(ui, cx, prefs, m, kind);
            let header_rect = Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y));
            self.action_bar(ui, cx, m, header_rect);
            ui.add_space(10.0);
            self.banners(ui, cx, prefs, m);
            if prefs.show_summary {
                self.summary_card(ui, m);
            }
            self.body(ui, cx, prefs, m, card_bg);
            self.attachments(ui, cx, m);
        });
        resp.response.rect
    }

    /// One-line header + snippet of a collapsed card; a click expands it.
    fn collapsed_row(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::click());
        let painter = ui.painter_at(rect.expand(2.0));
        let photo = self.people.photo(&m.from.email);
        let av = Rect::from_center_size(egui::pos2(rect.min.x + 15.0, rect.center().y), Vec2::splat(30.0));
        header::paint_avatar(ui, av, &m.from, photo.as_ref());
        let name = self.people.name_of(&m.from, prefs.smart_addresses);
        let x = av.max.x + 10.0;
        let date = super::widgets::format_list_date(m.date);
        let date_g = painter.layout_no_wrap(date, egui::FontId::proportional(12.0), pal.text_tertiary);
        let date_w = date_g.size().x;
        painter.galley(egui::pos2(rect.max.x - date_w, rect.min.y + 1.0), date_g, pal.text_tertiary);
        let name_font = egui::FontId::new(13.5, super::fonts::bold());
        let mut job = egui::text::LayoutJob::simple_singleline(name, name_font, pal.text);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.max.x - date_w - x - 12.0).max(20.0));
        let g = painter.layout_job(job);
        let name_w = g.size().x;
        painter.galley(egui::pos2(x, rect.min.y), g, pal.text);
        let mut ix = x + name_w + 6.0;
        if self.people.is_vip(&m.from.email) {
            painter.text(egui::pos2(ix, rect.min.y + 1.0), egui::Align2::LEFT_TOP, ic::STAR, egui::FontId::proportional(12.0), pal.vip);
            ix += 16.0;
        }
        if m.is_flagged() {
            let c = pal.flag(m.color.unwrap_or_default());
            painter.text(egui::pos2(ix, rect.min.y + 1.0), egui::Align2::LEFT_TOP, ic::FLAG, egui::FontId::proportional(12.0), c);
        }
        let snippet_src = if m.snippet.contains('<') && m.snippet.contains('>') { mime::make_snippet(&mime::html_to_text(&m.snippet)) } else { m.snippet.clone() };
        let snippet = if snippet_src.is_empty() { if m.has(flags::HAS_BODY) { "No preview".to_string() } else { "Not downloaded yet".to_string() } } else { snippet_src };
        let mut job = egui::text::LayoutJob::simple_singleline(snippet, egui::FontId::proportional(12.5), pal.text_secondary);
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.max.x - x - 4.0).max(20.0));
        painter.galley(egui::pos2(x, rect.min.y + 18.0), painter.layout_job(job), pal.text_secondary);
        if m.is_unread() {
            painter.circle_filled(egui::pos2(rect.min.x - 8.0, rect.center().y), 3.5, pal.unread_dot);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.collapsed.remove(&m.id);
            self.expanded.insert(m.id);
            if m.is_unread() {
                cx.act(Action::SetFlags { ids: vec![m.id], add: flags::SEEN, remove: 0 });
            }
        }
        resp.context_menu(|ui| {
            if ui.button(format!("{}  Expand", ic::ARROWS_OUT_SIMPLE)).clicked() {
                self.expanded.insert(m.id);
                self.collapsed.remove(&m.id);
                ui.close();
            }
            if ui.button(format!("{}  Open in New Window", ic::ARROW_SQUARE_OUT)).clicked() {
                cx.act(Action::OpenMessageWindow(m.id));
                ui.close();
            }
            if ui.button(format!("{}  Reply", ic::ARROW_BEND_UP_LEFT)).clicked() {
                cx.act(Action::Compose(ComposeInit::Respond { message: m.id, kind: ReplyKind::Reply, selected_text: None }));
                ui.close();
            }
        });
    }

    // ------------------------------------------------------------------ body

    fn body(&mut self, ui: &mut Ui, cx: &mut UiCx, prefs: &ViewPrefs, m: &MessageSummary, canvas: Color32) {
        let pal = Palette::of(ui.ctx());
        let allow_remote = self.remote_allowed(prefs, m.id);
        let expanded = self.sections.get(&m.id).cloned().unwrap_or_default();
        let ppp = ui.ctx().pixels_per_point();
        let Some(p) = self.cache.get_mut(&m.id) else { return };
        let Some(r) = p.body.as_mut() else {
            if m.has(flags::HAS_BODY) && p.attachments.is_empty() {
                ui.label(RichText::new("This message has no content.").italics().color(pal.text_tertiary));
            }
            return;
        };
        if allow_remote && !prefs.protect_activity {
            // Without activity protection, remote trackers load like any other image.
            for img in r.doc.images.iter().filter(|i| i.tracking) {
                if let html_render::tree::ImgSrc::Remote(u) = &img.src {
                    let _ = html_render::remote::get(u, ui.ctx());
                }
            }
        }
        let doc = r.doc.clone();
        // Designed HTML in dark mode is shown on a light "paper" card; everything
        // else adapts its colors to the theme.
        let paper = pal.dark && doc.designed;
        let page_bg = doc.bg.filter(|_| !pal.dark || paper);
        let card = paper || page_bg.is_some();
        let pad = if card { 16.0 } else { 0.0 };
        let width = (ui.available_width() - 2.0 * pad).max(60.0);
        let (canvas, text, secondary, link, rule, placeholder) = if paper || (!pal.dark && page_bg.is_some()) {
            let bg = page_bg.unwrap_or(Color32::WHITE);
            (bg, Color32::from_rgb(0x1d, 0x1b, 0x22), Color32::from_rgb(0x6b, 0x67, 0x75), Color32::from_rgb(0x5b, 0x36, 0xc4), Color32::from_rgb(0xdd, 0xd8, 0xe4), Color32::from_rgb(0xf1, 0xee, 0xf5))
        } else {
            (canvas, pal.text, pal.text_secondary, pal.accent, pal.separator, if pal.dark { Color32::from_rgb(0x2a, 0x27, 0x31) } else { Color32::from_rgb(0xf3, 0xf0, 0xf8) })
        };
        let opts = LayoutOpts {
            width,
            adapt_dark: pal.dark && !paper,
            canvas,
            text,
            text_secondary: secondary,
            link,
            quote_colors: prefs.quote_colors,
            quote_text: prefs.quote_text,
            rule,
            placeholder_bg: placeholder,
            allow_remote,
            expanded,
            line_height: 1.42,
            pixels_per_point: ppp,
        };
        let lay = r.layout(ui.ctx(), &opts);
        let size = Vec2::new(ui.available_width(), lay.size.y + 2.0 * pad);
        let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
        if card {
            ui.painter().rect_filled(outer, 10.0, canvas);
            if pal.dark {
                ui.painter().rect_stroke(outer, 10.0, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
            }
        }
        let rect = Rect::from_min_size(outer.min + Vec2::splat(pad), lay.size);
        let atts = p.attachments.clone();
        let cid_targets: Vec<(u32, Option<usize>)> = doc
            .images
            .iter()
            .enumerate()
            .map(|(i, img)| {
                let idx = match &img.src {
                    html_render::tree::ImgSrc::Cid(c) => p.parsed.as_ref().and_then(|pm| pm.attachments.iter().position(|a| a.content_id.as_deref().map(|x| x.trim_matches(['<', '>'])) == Some(c.trim_matches(['<', '>'])))),
                    _ => None,
                };
                (i as u32, idx)
            })
            .collect();
        let parsed = p.parsed.clone();
        let id = m.id;
        let actions = &mut *cx.actions;
        let previews = &mut self.previews;
        let mut menu = |ui: &mut Ui, target: MenuTarget| match target {
            MenuTarget::Link(l) => link_menu(ui, actions, id, &l.href, &l.text),
            MenuTarget::Image(i) => {
                let att = cid_targets.iter().find(|(k, _)| *k == i).and_then(|(_, a)| *a);
                match (att, doc.images.get(i as usize)) {
                    (Some(index), _) => {
                        if ui.button(format!("{}  Quick Look", ic::EYE)).clicked() {
                            if let Some(a) = parsed.as_ref().and_then(|pm| att_info(pm, index)) {
                                previews.push(Preview { message: id, att: a, open: true });
                            }
                            ui.close();
                        }
                        if ui.button(format!("{}  Open Image", ic::ARROW_SQUARE_OUT)).clicked() {
                            actions.push(Action::OpenAttachment { message: id, index });
                            ui.close();
                        }
                        if ui.button(format!("{}  Save Image…", ic::DOWNLOAD_SIMPLE)).clicked() {
                            actions.push(Action::SaveAttachment { message: id, index });
                            ui.close();
                        }
                    }
                    (None, Some(img)) => {
                        if let html_render::tree::ImgSrc::Remote(u) = &img.src {
                            if ui.button(format!("{}  Copy Image Address", ic::LINK)).clicked() {
                                ui.ctx().copy_text(u.clone());
                                ui.close();
                            }
                        } else {
                            ui.label(RichText::new("Embedded image").color(Palette::of(ui.ctx()).text_tertiary));
                        }
                    }
                    _ => {}
                }
            }
        };
        let events = html_render::paint::paint(ui, rect, &lay, &doc, id as u64, &mut self.doc_state, &mut menu);
        for ev in events {
            match ev {
                DocEvent::Link(i) => {
                    if let Some(l) = doc.links.get(i as usize) {
                        let href = l.href.trim().to_string();
                        let lower = href.to_ascii_lowercase();
                        if lower.starts_with("http://") || lower.starts_with("https://") {
                            cx.act(Action::OpenUrl(if prefs.strip_tracking { strip_tracking_params(&href) } else { href }));
                        } else if lower.starts_with("mailto:") || lower.starts_with("ftp://") {
                            cx.act(Action::OpenUrl(href));
                        } else if lower.starts_with('#') || lower.is_empty() {
                        } else if lower.starts_with("www.") {
                            cx.act(Action::OpenUrl(format!("https://{href}")));
                        } else {
                            cx.act(Action::Toast(format!("Can't open “{href}”")));
                        }
                    }
                }
                DocEvent::Toggle(k) => {
                    let set = self.sections.entry(m.id).or_default();
                    if !set.remove(&k) {
                        set.insert(k);
                    }
                }
                DocEvent::Image(i) => {
                    let att = cid_targets.iter().find(|(k, _)| *k == i).and_then(|(_, a)| *a);
                    if let Some(a) = att.and_then(|index| atts.iter().find(|a| a.index == index).cloned().or_else(|| self.cache.get(&m.id).and_then(|p| p.parsed.as_ref()).and_then(|pm| att_info(pm, index)))) {
                        self.previews.push(Preview { message: m.id, att: a, open: true });
                    }
                }
            }
        }
    }
}

/// Remove well-known click-tracking query parameters (`utm_*`, `fbclid`, ...) from a URL.
fn strip_tracking_params(url: &str) -> String {
    let (base, frag) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let Some((path, query)) = base.split_once('?') else { return url.to_string() };
    let tracking = |k: &str| {
        let k = k.to_ascii_lowercase();
        k.starts_with("utm_") || ["fbclid", "gclid", "dclid", "msclkid", "mc_cid", "mc_eid", "_hsenc", "_hsmi", "mkt_tok", "yclid", "igshid", "oly_enc_id", "oly_anon_id", "vero_id"].contains(&k.as_str())
    };
    let kept: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !tracking(p.split('=').next().unwrap_or(""))).collect();
    let mut out = path.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Menu for a link inside a message body.
fn link_menu(ui: &mut Ui, actions: &mut Vec<Action>, message: MessageId, href: &str, text: &str) {
    use super::html_render::DATE_SCHEME;
    if let Some(ts) = href.strip_prefix(DATE_SCHEME) {
        let ts: Option<i64> = ts.parse().ok();
        if let Some(t) = ts {
            ui.label(RichText::new(super::widgets::format_full_date(t)).strong());
            ui.separator();
            if t > now_ts() && ui.button(format!("{}  Remind Me Then", ic::BELL)).clicked() {
                actions.push(Action::Remind { ids: vec![message], at: Some(t) });
                ui.close();
            }
            if ui.button(format!("{}  Remind Me the Day Before", ic::BELL_SIMPLE)).clicked() {
                actions.push(Action::Remind { ids: vec![message], at: Some((t - 86_400).max(now_ts() + 60)) });
                ui.close();
            }
        }
        if ui.button(format!("{}  Copy", ic::COPY)).clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
        return;
    }
    if let Some(num) = href.strip_prefix("tel:") {
        ui.label(RichText::new(text).strong());
        ui.separator();
        if ui.button(format!("{}  Call", ic::PHONE)).clicked() {
            actions.push(Action::OpenUrl(href.to_string()));
            ui.close();
        }
        if ui.button(format!("{}  Copy Phone Number", ic::COPY)).clicked() {
            ui.ctx().copy_text(num.to_string());
            ui.close();
        }
        return;
    }
    if let Some(addr) = href.strip_prefix("mailto:") {
        let email = addr.split('?').next().unwrap_or(addr).to_string();
        if ui.button(format!("{}  New Message", ic::PENCIL_SIMPLE)).clicked() {
            actions.push(Action::Compose(ComposeInit::Mailto(href.to_string())));
            ui.close();
        }
        if ui.button(format!("{}  Copy Address", ic::COPY)).clicked() {
            ui.ctx().copy_text(email.clone());
            ui.close();
        }
        if ui.button(format!("{}  Add to Contacts", ic::USER_PLUS)).clicked() {
            actions.push(Action::AddToContacts(Address::new("", email)));
            ui.close();
        }
        return;
    }
    if ui.button(format!("{}  Open Link", ic::ARROW_SQUARE_OUT)).clicked() {
        actions.push(Action::OpenUrl(href.to_string()));
        ui.close();
    }
    if ui.button(format!("{}  Copy Link", ic::LINK)).clicked() {
        ui.ctx().copy_text(href.to_string());
        ui.close();
    }
}

fn att_info(pm: &ParsedMessage, index: usize) -> Option<AttInfo> {
    let a = pm.attachments.get(index)?;
    let bytes: Arc<[u8]> = Arc::from(a.data.as_slice());
    let dims = if a.is_image() { html_render::tree::image_dims(&a.data) } else { None };
    Some(AttInfo { index, name: a.filename.clone(), mime: a.mime_type.clone(), size: a.data.len(), uri: html_render::tree::bytes_uri(&a.data), bytes, dims })
}

/// Some messages carry their HTML source in the text slot.
fn looks_like_html(t: &str) -> bool {
    let l = t.trim_start().get(..64).unwrap_or(t.trim_start()).to_ascii_lowercase();
    l.starts_with("<!doctype") || l.starts_with("<html") || (l.starts_with('<') && t.contains("</") && (t.contains("<body") || t.contains("<div") || t.contains("<p")))
}

/// Parse a message and build its render tree.
fn build_prepared(store: &Store, key: BuildKey, id: MessageId) -> Prepared {
    let empty = |key: BuildKey| Prepared { key, parsed: None, body: None, text: String::new(), attachments: Vec::new(), event: None, summary: None };
    let Some(raw) = store.raw(id) else { return empty(key) };
    let Some(pm) = mime::parse(&raw) else { return empty(key) };
    let resolve = |cid: &str| pm.find_cid(cid).map(|a| Arc::<[u8]>::from(a.data.as_slice()));
    let mut body = None;
    let mut referenced: HashSet<String> = HashSet::new();
    if let Some(html) = pm.html.as_deref().filter(|h| !h.trim().is_empty()) {
        let doc = html_render::build_html(html, &BuildOpts { base_size: key.size, resolve_cid: &resolve });
        for img in &doc.images {
            if let html_render::tree::ImgSrc::Cid(c) = &img.src {
                referenced.insert(c.trim_matches(['<', '>']).to_string());
            }
        }
        if doc.text_len > 0 || !doc.images.is_empty() || pm.text.as_deref().is_none_or(|t| t.trim().is_empty()) {
            body = Some(Rendered::new(doc));
        }
    }
    if body.is_none()
        && let Some(text) = pm.text.as_deref().filter(|t| !t.trim().is_empty() && !looks_like_html(t)) {
            let size = if key.fixed { key.fixed_size } else { key.size };
            body = Some(Rendered::new(html_render::build_plain(text, &PlainOpts { size, fixed: key.fixed, collapse_over: 10 })));
        }
    let attachments: Vec<AttInfo> = pm
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, a)| !a.content_id.as_deref().is_some_and(|c| referenced.contains(c.trim_matches(['<', '>']))))
        .filter_map(|(i, _)| att_info(&pm, i))
        .collect();
    let event = pm.calendar.as_deref().and_then(banners::parse_event);
    let text = match (&pm.text, &pm.html) {
        (Some(t), Some(h)) if looks_like_html(t) => mime::html_to_text(h),
        (Some(t), None) if looks_like_html(t) => mime::html_to_text(t),
        _ => pm.plain_text(),
    };
    Prepared { key, parsed: Some(Arc::new(pm)), body, text, attachments, event, summary: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_in_text_slot() {
        assert!(looks_like_html("<!DOCTYPE html><html><body>x</body></html>"));
        assert!(looks_like_html("  <html><p>x</p></html>"));
        assert!(looks_like_html("<div>Tuesday works</div><div><br></div>"));
        assert!(!looks_like_html("Hello <world>, how are you?"));
        assert!(!looks_like_html("> quoted\nreply"));
    }

    #[test]
    fn tracking_parameters() {
        assert_eq!(strip_tracking_params("https://x.org/a?utm_source=n&id=3&fbclid=abc#top"), "https://x.org/a?id=3#top");
        assert_eq!(strip_tracking_params("https://x.org/a?utm_medium=email"), "https://x.org/a");
        assert_eq!(strip_tracking_params("https://x.org/a"), "https://x.org/a");
    }

    #[test]
    fn people_names() {
        let mut p = People::default();
        p.contacts.insert("alice@example.org".into(), ("Alice Martin".into(), None));
        let a = Address::new("alice", "Alice@Example.org");
        assert_eq!(p.name_of(&a, true), "Alice Martin");
        assert_eq!(p.name_of(&a, false), "alice <Alice@Example.org>");
        assert_eq!(p.name_of(&Address::new("", "x@y.z"), true), "x@y.z");
        assert!(p.is_contact("ALICE@example.org"));
    }

    #[test]
    fn prepared_message_with_cid_and_attachments() {
        let store = Store::open_in_memory().unwrap();
        let inbox = store.ensure_mailbox(LOCAL_ACCOUNT, "Inbox", MailboxRole::Inbox).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfake";
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let raw = format!(
            "From: A <a@x.org>\r\nTo: b@x.org\r\nSubject: T\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=M\r\n\r\n--M\r\nContent-Type: multipart/related; boundary=R\r\n\r\n--R\r\nContent-Type: text/html\r\n\r\n<p>Hi <img src=\"cid:logo\"></p>\r\n--R\r\nContent-Type: image/png\r\nContent-ID: <logo>\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\n{b64}\r\n--R--\r\n--M\r\nContent-Type: application/pdf; name=a.pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\n\r\n%PDF\r\n--M--\r\n"
        );
        let ids = crate::core::ops::import_raw(&store, inbox, &[raw.into_bytes()]).unwrap();
        let key = BuildKey { fonts: Default::default(), size: 14.0, fixed_size: 13.0, fixed: false, has_body: true };
        let p = build_prepared(&store, key, ids[0]);
        let body = p.body.expect("body");
        // the cid image is part of the body, not listed as an attachment
        assert_eq!(body.doc.images.len(), 1);
        assert!(body.doc.images[0].bytes.is_some());
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].name, "a.pdf");
        assert!(p.text.contains("Hi"));
    }
}
