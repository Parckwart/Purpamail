//! Contacts window: address book (with VIPs and photos), previous recipients
//! and blocked senders.

use std::collections::HashSet;

use egui::{RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as ic;

use super::actions::{Action, ComposeInit, UiCx};
use super::prefs::form::{self, Form};
use super::theme::Palette;
use super::widgets;
use crate::core::model::Address;
use crate::core::store::{Contact, PreviousRecipient};

/// Seconds without further edits before a contact is saved.
const SAVE_DELAY: f64 = 0.6;
/// Edge length of stored contact photos.
const PHOTO_SIZE: u32 = 256;
const F: Form = Form::new(110.0);

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContactsTab {
    #[default]
    Contacts,
    PreviousRecipients,
    Blocked,
}

#[derive(Default)]
pub struct ContactsWindow {
    pub open: bool,
    pub tab: ContactsTab,
    contacts: Vec<Contact>,
    loaded: bool,
    last_reload: f64,
    search: String,
    selected: Option<i64>,
    /// Working copy of the selected contact.
    edit: Option<Contact>,
    edited_at: Option<f64>,
    focus_first_name: bool,
    confirm_delete: Option<i64>,
    pr_search: String,
    pr_selected: Option<String>,
    new_blocked: String,
    blocked_selected: Option<String>,
    status: Option<String>,
}

/// Square PNG thumbnail (center-cropped) of an image file's bytes.
pub fn photo_png(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(bytes)?;
    let thumb = img.resize_to_fill(PHOTO_SIZE, PHOTO_SIZE, image::imageops::FilterType::Lanczos3);
    let mut out = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Whether a contact matches a search string (name, nickname, organization, emails, phones).
pub fn contact_matches(c: &Contact, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let hay = [c.first_name.as_str(), c.last_name.as_str(), c.nickname.as_str(), c.organization.as_str()];
    hay.iter().any(|h| h.to_lowercase().contains(&q)) || c.display_name().to_lowercase().contains(&q) || c.emails.iter().chain(c.phones.iter()).any(|e| e.to_lowercase().contains(&q))
}

/// Sort key: last name, then first name (organization / email for contacts without a name).
pub fn sort_key(c: &Contact) -> String {
    let key = if c.last_name.trim().is_empty() { c.display_name() } else { format!("{} {}", c.last_name, c.first_name) };
    key.trim().to_lowercase()
}

/// Stable image URI for a contact photo (changes when the photo changes).
fn photo_uri(c: &Contact) -> Option<String> {
    let p = c.photo.as_ref()?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.iter().take(4096).chain(p.iter().rev().take(4096)) {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    Some(format!("bytes://contact-photo-{}-{}-{h:x}.png", c.id, p.len()))
}

fn contact_avatar(ui: &mut Ui, c: &Contact, size: f32) -> egui::Response {
    let uri = photo_uri(c);
    let photo = match (&uri, &c.photo) {
        (Some(u), Some(p)) => Some((u.as_str(), p.as_slice())),
        _ => None,
    };
    widgets::avatar(ui, &c.display_name(), &c.primary_email(), size, photo)
}

impl ContactsWindow {
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let vid = egui::ViewportId::from_hash_of("purpamail-contacts");
        let builder = egui::ViewportBuilder::default().with_title("Contacts").with_inner_size([820.0, 560.0]).with_min_inner_size([640.0, 420.0]);
        let mut close = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            let now = ui.input(|i| i.time);
            form::window_style(ui);
            if !self.loaded || (now - self.last_reload > 5.0 && self.edited_at.is_none()) {
                self.reload(cx, now);
            }
            let pal = Palette::of(ui.ctx());
            egui::Panel::top("contacts_tabs").frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    form::segmented(ui, &mut self.tab, &[(ContactsTab::Contacts, "Contacts"), (ContactsTab::PreviousRecipients, "Previous Recipients"), (ContactsTab::Blocked, "Blocked")]);
                });
            });
            match self.tab {
                ContactsTab::Contacts => self.contacts_tab(ui, cx, now),
                ContactsTab::PreviousRecipients => {
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window_bg).inner_margin(16)).show(ui, |ui| self.recipients_tab(ui, cx));
                }
                ContactsTab::Blocked => {
                    egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window_bg).inner_margin(16)).show(ui, |ui| self.blocked_tab(ui, cx));
                }
            }
            self.autosave(cx, now, false);
            if self.edited_at.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            if ui.input(|i| i.viewport().close_requested() || (i.key_pressed(egui::Key::W) && i.modifiers.command)) {
                self.autosave(cx, now, true);
                close = true;
            }
        });
        if close {
            self.open = false;
            self.loaded = false;
        }
    }

    fn reload(&mut self, cx: &UiCx, now: f64) {
        self.contacts = cx.store.contacts();
        self.contacts.sort_by_cached_key(sort_key);
        self.loaded = true;
        self.last_reload = now;
        if self.selected.is_some_and(|id| !self.contacts.iter().any(|c| c.id == id)) {
            self.selected = None;
            self.edit = None;
        }
        if let (Some(id), None) = (self.selected, &self.edit) {
            self.edit = self.contacts.iter().find(|c| c.id == id).cloned();
        }
    }

    /// Save the working copy when it differs from the stored contact.
    fn autosave(&mut self, cx: &mut UiCx, now: f64, force: bool) {
        let Some(t) = self.edited_at else { return };
        if !force && now - t < SAVE_DELAY {
            return;
        }
        self.edited_at = None;
        let Some(edit) = &self.edit else { return };
        let mut c = edit.clone();
        c.emails.retain(|e| !e.trim().is_empty());
        c.phones.retain(|p| !p.trim().is_empty());
        for e in &mut c.emails {
            *e = e.trim().to_string();
        }
        let stored = self.contacts.iter().find(|x| x.id == c.id);
        if stored == Some(&c) {
            return;
        }
        let vip_changed = stored.is_some_and(|s| s.vip != c.vip);
        match cx.store.save_contact(&c) {
            Ok(_) => {
                if vip_changed
                    && let Some(e) = c.emails.first() {
                        cx.act(Action::SetVip { address: Address::new(c.display_name(), e.clone()), vip: c.vip });
                    }
                if let Some(x) = self.contacts.iter_mut().find(|x| x.id == c.id) {
                    *x = c;
                }
            }
            Err(e) => self.status = Some(format!("Could not save contact: {e}")),
        }
    }

    fn select(&mut self, cx: &mut UiCx, id: Option<i64>, now: f64) {
        self.autosave(cx, now, true);
        self.selected = id;
        self.edit = id.and_then(|id| self.contacts.iter().find(|c| c.id == id).cloned());
    }

    // ------------------------------------------------------------------ contacts

    fn contacts_tab(&mut self, ui: &mut Ui, cx: &mut UiCx, now: f64) {
        let pal = Palette::of(ui.ctx());
        egui::Panel::left("contacts_list")
            .resizable(false)
            .exact_size(290.0)
            .frame(egui::Frame::new().fill(pal.sidebar_bg).inner_margin(egui::Margin::symmetric(10, 10)))
            .show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search", ic::MAGNIFYING_GLASS)).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                let list_h = ui.available_height() - 34.0;
                let mut clicked = None;
                egui::ScrollArea::vertical().id_salt("contacts_scroll").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                    let shown: Vec<&Contact> = self.contacts.iter().filter(|c| contact_matches(c, &self.search)).collect();
                    if shown.is_empty() {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(if self.contacts.is_empty() { "No contacts yet" } else { "No matches" }).color(pal.text_tertiary));
                        });
                    }
                    let mut last_letter: Option<char> = None;
                    for c in shown {
                        let letter = sort_key(c).chars().next().filter(|ch| ch.is_alphabetic()).map(|ch| ch.to_uppercase().next().unwrap_or(ch)).unwrap_or('#');
                        if self.search.is_empty() && last_letter != Some(letter) {
                            ui.add_space(2.0);
                            ui.label(RichText::new(letter.to_string()).size(11.0).family(super::fonts::bold()).color(pal.text_tertiary));
                            last_letter = Some(letter);
                        }
                        if self.contact_row(ui, c).clicked() {
                            clicked = Some(c.id);
                        }
                    }
                });
                if let Some(id) = clicked {
                    self.select(cx, Some(id), now);
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if form::icon_button(ui, ic::PLUS, "New contact", true).clicked() {
                        self.autosave(cx, now, true);
                        match cx.store.save_contact(&Contact::default()) {
                            Ok(id) => {
                                self.search.clear();
                                self.reload(cx, now);
                                self.select(cx, Some(id), now);
                                self.focus_first_name = true;
                            }
                            Err(e) => self.status = Some(e.to_string()),
                        }
                    }
                    if form::icon_button(ui, ic::MINUS, "Delete contact", self.selected.is_some()).clicked() {
                        self.confirm_delete = self.selected;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if form::icon_button(ui, ic::EXPORT, "Export all contacts as vCard…", !self.contacts.is_empty()).clicked() {
                            self.export(cx);
                        }
                        if form::icon_button(ui, ic::DOWNLOAD_SIMPLE, "Import vCard…", true).clicked() {
                            self.import(cx, now);
                        }
                        ui.label(RichText::new(match self.contacts.len() {
                            1 => "1 contact".to_string(),
                            n => format!("{n} contacts"),
                        }).size(11.0).color(pal.text_tertiary));
                    });
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window_bg).inner_margin(egui::Margin::symmetric(22, 16))).show(ui, |ui| {
            if let Some(s) = self.status.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ic::INFO).color(pal.accent));
                    ui.add(egui::Label::new(RichText::new(&s).size(12.0).color(pal.text_secondary)).wrap());
                    if form::icon_button(ui, ic::X, "Dismiss", true).clicked() {
                        self.status = None;
                    }
                });
                ui.add_space(6.0);
            }
            if self.edit.is_some() {
                self.detail(ui, cx, now);
            } else {
                ui.add_space(100.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(ic::ADDRESS_BOOK).size(48.0).color(pal.text_tertiary));
                    ui.label(RichText::new("No contact selected").family(super::fonts::bold()).size(15.0));
                    ui.label(RichText::new("Select a contact, or click + to add one.").color(pal.text_secondary));
                });
            }
        });
        self.delete_dialog(ui, cx, now);
    }

    fn contact_row(&self, ui: &mut Ui, c: &Contact) -> egui::Response {
        let pal = Palette::of(ui.ctx());
        let sel = self.selected == Some(c.id);
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 42.0), Sense::click());
        if ui.is_rect_visible(rect) {
            if sel {
                ui.painter().rect_filled(rect, 6.0, pal.selection_bg);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, pal.hover_bg);
            }
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            contact_avatar(&mut child, c, 30.0);
            let (fg, fg2) = if sel { (pal.selection_text, pal.selection_text.gamma_multiply(0.85)) } else { (pal.text, pal.text_secondary) };
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let name = c.display_name();
                ui.add(egui::Label::new(RichText::new(if name.is_empty() { "No Name".into() } else { name }).family(super::fonts::bold()).color(fg)).truncate());
                let sub = if c.emails.is_empty() { c.organization.clone() } else { c.primary_email() };
                ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(fg2)).truncate());
            });
            if c.vip {
                ui.painter().text(rect.right_center() - vec2(14.0, 0.0), egui::Align2::CENTER_CENTER, ic::STAR, egui::FontId::proportional(14.0), if sel { fg } else { pal.vip });
            }
        }
        resp
    }

    fn detail(&mut self, ui: &mut Ui, cx: &mut UiCx, now: f64) {
        let pal = Palette::of(ui.ctx());
        let Some(c) = self.edit.as_mut() else { return };
        let before = c.clone();
        let mut photo_status = None;
        egui::ScrollArea::vertical().id_salt(("contact_detail", c.id)).auto_shrink([false, false]).show(ui, |ui| {
            // ---------------------------------------------- header
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    contact_avatar(ui, c, 76.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if ui.small_button("Choose…").on_hover_text("Choose a photo").clicked()
                            && let Some(p) = rfd::FileDialog::new().set_title("Choose a Photo").add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).pick_file() {
                                match std::fs::read(&p).map_err(anyhow::Error::from).and_then(|b| photo_png(&b)) {
                                    Ok(png) => c.photo = Some(png),
                                    Err(e) => photo_status = Some(format!("Could not use this image: {e}")),
                                }
                            }
                        if c.photo.is_some() && ui.small_button("Remove").clicked() {
                            c.photo = None;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    let name = c.display_name();
                    ui.label(RichText::new(if name.is_empty() { "No Name".to_string() } else { name }).family(super::fonts::bold()).size(20.0));
                    if !c.organization.is_empty() {
                        ui.label(RichText::new(&c.organization).color(pal.text_secondary));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let star = RichText::new(format!("{} VIP", ic::STAR)).color(if c.vip { pal.vip } else { pal.text_secondary });
                        if ui.add(egui::Button::selectable(c.vip, star)).on_hover_text("VIP messages appear in the VIPs mailbox").clicked() {
                            c.vip = !c.vip;
                        }
                        let email = c.emails.iter().find(|e| !e.trim().is_empty()).cloned();
                        if ui.add_enabled(email.is_some(), egui::Button::new(format!("{} New Message", ic::NOTE_PENCIL))).clicked()
                            && let Some(e) = &email {
                                cx.act(Action::Compose(ComposeInit::New { to: vec![Address::new(c.display_name(), e.clone())], cc: vec![], subject: String::new(), body: String::new(), attachments: vec![] }));
                            }
                        if ui.add_enabled(email.is_some(), egui::Button::new(format!("{} Show Messages", ic::MAGNIFYING_GLASS))).clicked()
                            && let Some(e) = &email {
                                cx.act(Action::Search(format!("from:{e}")));
                            }
                    });
                });
            });
            ui.add_space(8.0);
            form::section(ui);
            // ---------------------------------------------- fields
            F.row(ui, "First Name:", |ui| {
                let r = form::text_field(ui, &mut c.first_name, 260.0, "First");
                if self.focus_first_name {
                    r.request_focus();
                    self.focus_first_name = false;
                }
            });
            F.row(ui, "Last Name:", |ui| form::text_field(ui, &mut c.last_name, 260.0, "Last"));
            F.row(ui, "Nickname:", |ui| form::text_field(ui, &mut c.nickname, 260.0, ""));
            F.row(ui, "Organization:", |ui| form::text_field(ui, &mut c.organization, 260.0, "Company"));
            ui.add_space(4.0);
            F.row(ui, "Email:", |ui| list_editor(ui, "emails", &mut c.emails, "name@example.com", "Add Email"));
            F.row(ui, "Phone:", |ui| list_editor(ui, "phones", &mut c.phones, "+1 555 0100", "Add Phone"));
            F.row(ui, "Notes:", |ui| {
                ui.add(egui::TextEdit::multiline(&mut c.notes).desired_width(320.0).desired_rows(4));
            });
        });
        if photo_status.is_some() {
            self.status = photo_status;
        }
        if *c != before {
            // photo / VIP changes are saved right away, typing after a pause
            let immediate = c.photo != before.photo || c.vip != before.vip;
            self.edited_at = Some(if immediate { f64::NEG_INFINITY } else { now });
        }
    }

    fn delete_dialog(&mut self, ui: &mut Ui, cx: &mut UiCx, now: f64) {
        let Some(id) = self.confirm_delete else { return };
        let pal = Palette::of(ui.ctx());
        let name = self.contacts.iter().find(|c| c.id == id).map(|c| c.display_name()).unwrap_or_default();
        let mut decision = None;
        egui::Modal::new(egui::Id::new("confirm_delete_contact")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(format!("Delete “{}”?", if name.is_empty() { "No Name" } else { &name })).family(super::fonts::bold()).size(14.0));
            form::hint(ui, "This contact is removed from your address book.");
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Delete").color(egui::Color32::WHITE)).fill(pal.danger).min_size(vec2(72.0, 26.0))).clicked() {
                    decision = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        if let Some(d) = decision {
            self.confirm_delete = None;
            if d {
                self.edited_at = None;
                if let Err(e) = cx.store.delete_contact(id) {
                    self.status = Some(e.to_string());
                }
                self.selected = None;
                self.edit = None;
                self.reload(cx, now);
            }
        }
    }

    fn import(&mut self, cx: &mut UiCx, now: f64) {
        let Some(path) = rfd::FileDialog::new().set_title("Import Contacts").add_filter("vCard", &["vcf", "vcard"]).pick_file() else { return };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let cards = crate::core::vcard::parse(&text);
                let mut n = 0;
                for mut c in cards {
                    c.id = 0;
                    if cx.store.save_contact(&c).is_ok() {
                        n += 1;
                    }
                }
                self.status = Some(match n {
                    0 => "No contacts found in this file.".into(),
                    1 => "Imported 1 contact.".into(),
                    n => format!("Imported {n} contacts."),
                });
                self.reload(cx, now);
            }
            Err(e) => self.status = Some(format!("Could not read {}: {e}", path.display())),
        }
    }

    fn export(&mut self, cx: &mut UiCx) {
        let Some(path) = rfd::FileDialog::new().set_title("Export Contacts").set_file_name("Contacts.vcf").add_filter("vCard", &["vcf"]).save_file() else { return };
        let text = crate::core::vcard::export_all(&cx.store.contacts());
        self.status = Some(match std::fs::write(&path, text) {
            Ok(()) => format!("Exported {} contacts to {}.", self.contacts.len(), path.display()),
            Err(e) => format!("Could not write {}: {e}", path.display()),
        });
    }

    // ------------------------------------------------------------------ previous recipients

    fn recipients_tab(&mut self, ui: &mut Ui, cx: &mut UiCx) {
        let pal = Palette::of(ui.ctx());
        let all: Vec<PreviousRecipient> = cx.store.previous_recipients();
        let known: HashSet<String> = self.contacts.iter().flat_map(|c| c.emails.iter().map(|e| e.to_lowercase())).collect();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Previous Recipients").family(super::fonts::bold()).size(15.0));
                ui.label(RichText::new("People you have sent messages to. They are suggested when you address a message.").size(12.0).color(pal.text_secondary));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.pr_search).hint_text(format!("{} Search", ic::MAGNIFYING_GLASS)).desired_width(200.0));
            });
        });
        ui.add_space(8.0);
        let q = self.pr_search.trim().to_lowercase();
        let rows: Vec<&PreviousRecipient> = all.iter().filter(|r| q.is_empty() || r.email.contains(&q) || r.name.to_lowercase().contains(&q)).collect();
        let table_h = ui.available_height() - 40.0;
        let mut clicked: Option<String> = None;
        form::list_frame(ui).show(ui, |ui| {
            ui.set_height(table_h);
            ui.set_width(ui.available_width());
            TableBuilder::new(ui)
                .id_salt("prev_recipients")
                .striped(true)
                .sense(Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(200.0).at_least(80.0).clip(true))
                .column(Column::remainder().at_least(120.0).clip(true))
                .column(Column::initial(130.0))
                .column(Column::initial(60.0))
                .min_scrolled_height(0.0)
                .max_scroll_height(table_h)
                .header(22.0, |mut h| {
                    for t in ["Name", "Email", "Last Used", "Messages"] {
                        h.col(|ui| {
                            ui.label(RichText::new(t).size(11.5).family(super::fonts::bold()).color(pal.text_secondary));
                        });
                    }
                })
                .body(|body| {
                    body.rows(24.0, rows.len(), |mut row| {
                        let r = rows[row.index()];
                        row.set_selected(self.pr_selected.as_deref() == Some(r.email.as_str()));
                        row.col(|ui| {
                            if known.contains(&r.email) {
                                ui.label(RichText::new(ic::ADDRESS_BOOK).color(pal.accent)).on_hover_text("In Contacts");
                            }
                            ui.label(if r.name.is_empty() { "—" } else { &r.name });
                        });
                        row.col(|ui| {
                            ui.label(&r.email);
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(widgets::format_list_date(r.last_used)).color(pal.text_secondary));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(r.count.to_string()).color(pal.text_secondary));
                        });
                        if row.response().clicked() {
                            clicked = Some(r.email.clone());
                        }
                    });
                });
            if rows.is_empty() {
                ui.vertical_centered(|ui| ui.label(RichText::new(if all.is_empty() { "No previous recipients" } else { "No matches" }).color(pal.text_tertiary)));
            }
        });
        if clicked.is_some() {
            self.pr_selected = clicked;
        }
        ui.add_space(6.0);
        let sel = self.pr_selected.as_ref().and_then(|e| all.iter().find(|r| &r.email == e));
        ui.horizontal(|ui| {
            let in_contacts = sel.is_some_and(|r| known.contains(&r.email));
            if ui.add_enabled(sel.is_some() && !in_contacts, egui::Button::new(format!("{} Add to Contacts", ic::USER_PLUS))).clicked()
                && let Some(r) = sel {
                    cx.act(Action::AddToContacts(Address::new(r.name.clone(), r.email.clone())));
                    self.loaded = false;
                }
            if ui.add_enabled(sel.is_some(), egui::Button::new("Remove from List")).clicked()
                && let Some(r) = sel {
                    let _ = cx.store.remove_previous_recipient(&r.email);
                    self.pr_selected = None;
                }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{} of {}", rows.len(), all.len())).size(11.5).color(pal.text_tertiary));
            });
        });
    }

    // ------------------------------------------------------------------ blocked

    fn blocked_tab(&mut self, ui: &mut Ui, cx: &mut UiCx) {
        let pal = Palette::of(ui.ctx());
        let blocked = cx.store.blocked_senders();
        ui.label(RichText::new("Blocked Senders").family(super::fonts::bold()).size(15.0));
        ui.label(RichText::new("Messages from these addresses are moved to the Trash automatically.").size(12.0).color(pal.text_secondary));
        ui.add_space(8.0);
        let h = ui.available_height() - 40.0;
        form::list_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h);
            egui::ScrollArea::vertical().id_salt("blocked_list").auto_shrink([false, false]).show(ui, |ui| {
                if blocked.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(ic::PROHIBIT).size(36.0).color(pal.text_tertiary));
                        ui.label(RichText::new("No blocked senders").color(pal.text_secondary));
                    });
                }
                for b in &blocked {
                    let sel = self.blocked_selected.as_deref() == Some(b.as_str());
                    let name = self.contacts.iter().find(|c| c.emails.iter().any(|e| e.eq_ignore_ascii_case(b))).map(|c| c.display_name()).unwrap_or_default();
                    if form::list_row(ui, sel, Some((ic::PROHIBIT, pal.danger)), if name.is_empty() { b } else { &name }, if name.is_empty() { "" } else { b }).clicked() {
                        self.blocked_selected = Some(b.clone());
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let sel_valid = self.blocked_selected.as_ref().is_some_and(|s| blocked.contains(s));
            if ui.add_enabled(sel_valid, egui::Button::new("Unblock")).clicked()
                && let Some(e) = self.blocked_selected.take() {
                    cx.act(Action::BlockSender { email: e, blocked: false });
                }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let valid = Address::parse_one(&self.new_blocked).is_some_and(|a| a.is_valid());
                let clicked = ui.add_enabled(valid, egui::Button::new(format!("{} Block", ic::PROHIBIT))).clicked();
                let r = ui.add(egui::TextEdit::singleline(&mut self.new_blocked).hint_text("name@example.com").desired_width(240.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (clicked || enter) && valid
                    && let Some(a) = Address::parse_one(&self.new_blocked) {
                        cx.act(Action::BlockSender { email: a.email, blocked: true });
                        self.new_blocked.clear();
                    }
            });
        });
    }
}

/// Editor for a list of strings (one text field per entry with remove buttons).
fn list_editor(ui: &mut Ui, id: &str, items: &mut Vec<String>, hint: &str, add_label: &str) {
    let mut remove = None;
    for (i, v) in items.iter_mut().enumerate() {
        ui.push_id((id, i), |ui| {
            ui.horizontal(|ui| {
                form::text_field(ui, v, 260.0, hint);
                if form::icon_button(ui, ic::MINUS, "Remove", true).clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        items.remove(i);
    }
    if ui.small_button(format!("{} {add_label}", ic::PLUS)).clicked() {
        items.push(String::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matching() {
        let c = Contact { first_name: "Ada".into(), last_name: "Lovelace".into(), organization: "Analytical Engines".into(), emails: vec!["ada@engines.example".into()], phones: vec!["+44 20".into()], ..Default::default() };
        assert!(contact_matches(&c, ""));
        assert!(contact_matches(&c, "ada love"));
        assert!(contact_matches(&c, "ENGINES"));
        assert!(contact_matches(&c, "+44"));
        assert!(!contact_matches(&c, "babbage"));
    }

    #[test]
    fn photos_are_square_pngs() {
        let img = image::RgbImage::from_fn(600, 300, |x, _| image::Rgb([(x % 255) as u8, 80, 160]));
        let mut src = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut src, image::ImageFormat::Png).unwrap();
        let png = photo_png(src.get_ref()).unwrap();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (PHOTO_SIZE, PHOTO_SIZE));
        assert!(photo_png(b"not an image").is_err());
    }

    #[test]
    fn sorting() {
        let mk = |f: &str, l: &str, o: &str| Contact { first_name: f.into(), last_name: l.into(), organization: o.into(), ..Default::default() };
        let mut v = vec![mk("", "", "Pizza Palace"), mk("Ada", "Lovelace", ""), mk("Linus", "Berg", "")];
        v.sort_by_cached_key(sort_key);
        let names: Vec<String> = v.iter().map(|c| c.display_name()).collect();
        assert_eq!(names, ["Linus Berg", "Ada Lovelace", "Pizza Palace"]);
    }

    #[test]
    fn photo_uri_changes_with_photo() {
        let mut c = Contact { id: 3, photo: Some(vec![1, 2, 3]), ..Default::default() };
        let a = photo_uri(&c).unwrap();
        c.photo = Some(vec![1, 2, 4]);
        assert_ne!(a, photo_uri(&c).unwrap());
        c.photo = None;
        assert!(photo_uri(&c).is_none());
    }
}
