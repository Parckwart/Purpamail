//! The menu bar: File, Edit, View, Mailbox, Message, Window, Help.

use egui::{Button, RichText, Ui};
use egui_phosphor::regular as ic;

use super::actions::*;
use super::app::PurpamailApp;
use super::theme::Palette;
use crate::core::compose::ReplyKind;
use crate::core::config::{FavoriteItem, ListLayout, Theme};
use crate::core::model::*;

fn item(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let b = Button::new(label).shortcut_text(shortcut);
    let r = ui.add_enabled(enabled, b);
    if r.clicked() {
        ui.close();
        true
    } else {
        false
    }
}

impl PurpamailApp {
    pub fn menu_bar(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        egui::Panel::top("menubar").frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(6, 2))).show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| self.file_menu(ui));
                ui.menu_button("Edit", |ui| self.edit_menu(ui));
                ui.menu_button("View", |ui| self.view_menu(ui));
                ui.menu_button("Mailbox", |ui| self.mailbox_menu(ui));
                ui.menu_button("Message", |ui| self.message_menu(ui));
                ui.menu_button("Window", |ui| self.window_menu(ui));
                ui.menu_button("Help", |ui| self.help_menu(ui));
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let single = self.selected.len() == 1;
        if item(ui, "New Message", "Ctrl+N", true) {
            self.actions.push(Action::Compose(ComposeInit::blank()));
        }
        let templates = self.settings.read().templates.clone();
        ui.menu_button("New Message from Template", |ui| {
            for t in templates {
                if ui.button(format!("{} — {}", t.category, t.name)).clicked() {
                    self.actions.push(Action::Compose(ComposeInit::Template(t.id.clone())));
                    ui.close();
                }
            }
        });
        if item(ui, "New Viewer Window", "", true) {
            if let Some(id) = self.selected.first() {
                self.actions.push(Action::OpenMessageWindow(*id));
            }
        }
        ui.separator();
        if item(ui, "Open Message…", "Ctrl+O", true) {
            if let Some(paths) = rfd::FileDialog::new().add_filter("Email message", &["eml", "msg", "txt"]).pick_files() {
                let raws: Vec<Vec<u8>> = paths.iter().filter_map(|p| std::fs::read(p).ok()).collect();
                if let Ok(mb) = self.store.ensure_mailbox(LOCAL_ACCOUNT, "Opened Messages", MailboxRole::Custom) {
                    if let Ok(ids) = crate::core::ops::import_raw(&self.store, mb, &raws) {
                        for id in ids {
                            self.actions.push(Action::OpenMessageWindow(id));
                        }
                    }
                }
                self.list_dirty = true;
            }
        }
        if item(ui, "Save As…", "Ctrl+Shift+S", single) {
            self.actions.push(Action::SaveMessageAs(self.focus_message()));
        }
        if item(ui, "Save Attachments…", "", single) {
            self.actions.push(Action::SaveAllAttachments(self.focus_message()));
        }
        ui.separator();
        if item(ui, "Import Mailboxes…", "", true) {
            self.actions.push(Action::ImportMailboxes);
        }
        if item(ui, "Import Contacts (vCard)…", "", true) {
            if let Some(path) = rfd::FileDialog::new().add_filter("vCard", &["vcf", "vcard"]).pick_file() {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    let cs = crate::core::vcard::parse(&s);
                    let n = cs.len();
                    for c in cs {
                        let _ = self.store.save_contact(&c);
                    }
                    self.actions.push(Action::Toast(format!("Imported {n} contacts")));
                }
            }
        }
        if item(ui, "Export Contacts (vCard)…", "", true) {
            if let Some(path) = rfd::FileDialog::new().set_file_name("Contacts.vcf").save_file() {
                let all = crate::core::vcard::export_all(&self.store.contacts());
                let _ = std::fs::write(path, all);
            }
        }
        ui.separator();
        if item(ui, "Export as PDF…", "", single) {
            self.actions.push(Action::ExportPdf(self.focus_message()));
        }
        if item(ui, "Print…", "Ctrl+P", single) {
            self.actions.push(Action::Print(self.focus_message()));
        }
        ui.separator();
        if item(ui, "Quit Purpamail", "Ctrl+Q", true) {
            self.actions.push(Action::Quit);
        }
    }

    fn edit_menu(&mut self, ui: &mut Ui) {
        let label = self.undo.last().map(|u| format!("Undo {}", u.label)).unwrap_or_else(|| "Undo".into());
        if item(ui, &label, "Ctrl+Z", !self.undo.is_empty()) {
            let ctx = ui.ctx().clone();
            self.undo_last(&ctx);
        }
        ui.separator();
        if item(ui, "Select All", "Ctrl+A", true) {
            self.selected = self.rows.iter().map(|r| r.msg.id).collect();
        }
        if item(ui, "Copy Address", "", self.selected.len() == 1) {
            if let Some(m) = self.store.message(self.focus_message()) {
                ui.ctx().copy_text(m.from.to_header());
            }
        }
        ui.separator();
        if item(ui, "Find…", "Ctrl+F", true) {
            self.focus_search = true;
        }
        if item(ui, "Mailbox Search", "Ctrl+Alt+F", true) {
            self.focus_search = true;
        }
        ui.separator();
        if item(ui, "Preferences…", "Ctrl+,", true) {
            self.actions.push(Action::ShowPreferences(None));
        }
    }

    fn view_menu(&mut self, ui: &mut Ui) {
        let mut s = self.settings.read().clone();
        let before = s.clone();
        ui.menu_button("Sort By", |ui| {
            for k in SortKey::ALL {
                if ui.radio(s.viewing.sort == k, k.name()).clicked() {
                    s.viewing.sort = k;
                    ui.close();
                }
            }
            ui.separator();
            if ui.radio(s.viewing.sort_ascending, "Ascending").clicked() {
                s.viewing.sort_ascending = true;
            }
            if ui.radio(!s.viewing.sort_ascending, "Descending").clicked() {
                s.viewing.sort_ascending = false;
            }
        });
        ui.menu_button("Filter", |ui| {
            let f = &mut self.filter;
            let mut changed = ui.checkbox(&mut f.enabled, "Enable Message Filter").changed();
            ui.separator();
            changed |= ui.checkbox(&mut f.unread, "Unread").changed();
            changed |= ui.checkbox(&mut f.flagged, "Flagged").changed();
            changed |= ui.checkbox(&mut f.to_me, "Addressed to Me").changed();
            changed |= ui.checkbox(&mut f.cc_me, "Cc: Me").changed();
            changed |= ui.checkbox(&mut f.with_attachments, "Only with Attachments").changed();
            changed |= ui.checkbox(&mut f.from_vip, "Only from VIPs").changed();
            changed |= ui.checkbox(&mut f.today, "Only from Today").changed();
            if changed {
                if f.any() && !f.enabled {
                    f.enabled = true;
                }
                self.list_dirty = true;
            }
        });
        ui.separator();
        ui.checkbox(&mut s.viewing.organize_by_conversation, "Organize by Conversation");
        if item(ui, "Expand All Conversations", "", s.viewing.organize_by_conversation) {
            self.expanded_threads = self.rows.iter().filter(|r| r.thread_count > 1).map(|r| r.msg.thread_id).collect();
            self.list_dirty = true;
        }
        if item(ui, "Collapse All Conversations", "", s.viewing.organize_by_conversation) {
            self.expanded_threads.clear();
            self.list_dirty = true;
        }
        ui.checkbox(&mut s.viewing.show_deleted_messages, "Show Deleted Messages");
        ui.checkbox(&mut s.viewing.show_categories, "Show Mail Categories");
        ui.checkbox(&mut s.viewing.show_summary, "Show Message Summaries");
        ui.separator();
        ui.menu_button("Layout", |ui| {
            if ui.radio(s.viewing.layout == ListLayout::Columns, "Columns (list beside message)").clicked() {
                s.viewing.layout = ListLayout::Columns;
                ui.close();
            }
            if ui.radio(s.viewing.layout == ListLayout::Classic, "Classic (list above message)").clicked() {
                s.viewing.layout = ListLayout::Classic;
                ui.close();
            }
        });
        ui.menu_button("Appearance", |ui| {
            for (t, n) in [(Theme::System, "Follow System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")] {
                if ui.radio(s.viewing.theme == t, n).clicked() {
                    s.viewing.theme = t;
                    ui.close();
                }
            }
        });
        ui.menu_button("Zoom", |ui| {
            for z in [0.85f32, 1.0, 1.15, 1.3, 1.5] {
                if ui.radio((s.viewing.ui_scale - z).abs() < 0.01, format!("{:.0}%", z * 100.0)).clicked() {
                    s.viewing.ui_scale = z;
                    ui.close();
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut s.viewing.show_sidebar, "Show Sidebar (Ctrl+Shift+M)");
        ui.checkbox(&mut s.viewing.show_favorites_bar, "Show Favorites Bar");
        ui.separator();
        let mut fs = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
        if ui.checkbox(&mut fs, "Full Screen").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(fs));
        }
        if s != before {
            *self.settings.write() = s;
            self.actions.push(Action::SettingsChanged);
        }
    }

    fn mailbox_menu(&mut self, ui: &mut Ui) {
        let offline = self.settings.read().work_offline;
        if item(ui, "Get All New Mail", "Ctrl+Shift+N", !offline) {
            self.actions.push(Action::CheckMail(None));
        }
        let accounts: Vec<(String, String)> = self.settings.read().accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
        ui.menu_button("Get New Mail", |ui| {
            for (id, name) in &accounts {
                if ui.button(name).clicked() {
                    self.actions.push(Action::CheckMail(Some(id.clone())));
                    ui.close();
                }
            }
        });
        ui.menu_button("Synchronize", |ui| {
            for (id, name) in &accounts {
                if ui.button(name).clicked() {
                    self.actions.push(Action::Synchronize(id.clone()));
                    ui.close();
                }
            }
        });
        if offline {
            if item(ui, "Take All Accounts Online", "", true) {
                self.actions.push(Action::SetOnline { account: None, online: true });
            }
        } else if item(ui, "Take All Accounts Offline", "", true) {
            self.actions.push(Action::SetOnline { account: None, online: false });
        }
        ui.separator();
        if item(ui, "Erase Deleted Items", "", true) {
            self.actions.push(Action::EraseDeleted(None));
        }
        if item(ui, "Erase Junk Mail", "", true) {
            self.actions.push(Action::EraseJunk);
        }
        ui.separator();
        let cur_mb = if let SidebarSel::Mailbox(id) = self.sel { self.mailboxes.iter().find(|m| m.id == id).cloned() } else { None };
        ui.menu_button("New Mailbox…", |ui| {
            for (id, name) in accounts.iter().cloned().chain(std::iter::once((LOCAL_ACCOUNT.to_string(), "On This Computer".to_string()))) {
                if ui.button(name).clicked() {
                    self.actions.push(Action::NewMailbox { parent: None, account: id });
                    ui.close();
                }
            }
        });
        if item(ui, "New Smart Mailbox…", "", true) {
            self.actions.push(Action::NewSmartMailbox(None));
        }
        if let SidebarSel::Smart(id) = &self.sel {
            let id = id.clone();
            if item(ui, "Edit Smart Mailbox…", "", true) {
                self.actions.push(Action::EditSmartMailbox(id));
            }
        }
        let custom = cur_mb.as_ref().is_some_and(|m| m.role == MailboxRole::Custom);
        if item(ui, "Rename Mailbox…", "", custom) {
            self.actions.push(Action::RenameMailbox(cur_mb.as_ref().unwrap().id));
        }
        if item(ui, "Delete Mailbox…", "", custom) {
            self.actions.push(Action::DeleteMailbox(cur_mb.as_ref().unwrap().id));
        }
        if item(ui, "Export Mailbox…", "", cur_mb.is_some()) {
            self.actions.push(Action::ExportMailbox(cur_mb.as_ref().unwrap().id));
        }
        if item(ui, "Rebuild", "", cur_mb.as_ref().is_some_and(|m| !m.is_local())) {
            self.actions.push(Action::RebuildMailbox(cur_mb.as_ref().unwrap().id));
        }
        if let Some(mb) = &cur_mb {
            let mbid = mb.id;
            ui.menu_button("Use This Mailbox For", |ui| {
                for role in [MailboxRole::Drafts, MailboxRole::Sent, MailboxRole::Junk, MailboxRole::Trash, MailboxRole::Archive] {
                    if ui.button(role.display_name()).clicked() {
                        self.actions.push(Action::UseMailboxFor { mailbox: mbid, role });
                        ui.close();
                    }
                }
            });
            if item(ui, "Add to Favorites", "", true) {
                self.actions.push(Action::AddFavorite(FavoriteItem::Mailbox(mbid)));
            }
        }
        if item(ui, "Mark All Messages as Read", "", !self.rows.is_empty()) {
            let mbs = self.scope_mailboxes(&self.sel.clone());
            let ids: Vec<MessageId> = self.rows.iter().map(|r| r.msg.mailbox_id).collect();
            self.actions.push(Action::MarkAllRead(if mbs.is_empty() { ids } else { mbs }));
        }
        ui.separator();
        ui.menu_button("Go to Favorite Mailbox", |ui| {
            let favs = self.settings.read().favorites.clone();
            for (i, f) in favs.iter().enumerate() {
                let (icon, name) = self.favorite_label(f);
                if ui.add(Button::new(format!("{icon} {name}")).shortcut_text(format!("Ctrl+{}", i + 1))).clicked() {
                    self.actions.push(Action::SelectSidebar(super::app::favorite_sel(f)));
                    ui.close();
                }
            }
        });
    }

    fn message_menu(&mut self, ui: &mut Ui) {
        let has = !self.selected.is_empty();
        let single = self.selected.len() == 1;
        let ids = self.selection_ids();
        let msgs = self.store.messages_by_ids(&ids);
        let focus = self.focus_message();
        let sel_text = self.viewer.selected_text();
        let in_outbox = msgs.first().map(|m| self.mailboxes.iter().any(|b| b.id == m.mailbox_id && b.role == MailboxRole::Outbox)).unwrap_or(false);
        let is_sent_or_draft = msgs.first().map(|m| self.mailboxes.iter().any(|b| b.id == m.mailbox_id && matches!(b.role, MailboxRole::Sent | MailboxRole::Drafts))).unwrap_or(false);
        if in_outbox && single {
            if item(ui, "Cancel Sending and Edit", "", true) {
                self.actions.push(Action::UndoSend(focus));
            }
            ui.separator();
        }
        if item(ui, "Send Again", "Ctrl+Shift+D", single && is_sent_or_draft) {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::SendAgain, selected_text: None }));
        }
        ui.separator();
        if item(ui, "Reply", "Ctrl+R", single) {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::Reply, selected_text: sel_text.clone() }));
        }
        if item(ui, "Reply All", "Ctrl+Shift+R", single) {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::ReplyAll, selected_text: sel_text }));
        }
        if item(ui, "Forward", "Ctrl+Shift+F", has) {
            if single {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::Forward, selected_text: None }));
            } else {
                self.actions.push(Action::Compose(ComposeInit::ForwardMany(ids.clone())));
            }
        }
        if item(ui, "Forward as Attachment", "", has) {
            if single {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::ForwardAsAttachment, selected_text: None }));
            } else {
                self.actions.push(Action::Compose(ComposeInit::ForwardMany(ids.clone())));
            }
        }
        if item(ui, "Redirect", "Ctrl+Shift+E", single) {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::Redirect, selected_text: None }));
        }
        ui.separator();
        ui.add_enabled_ui(has, |ui| {
            ui.menu_button("Mark", |ui| {
                if item(ui, "As Read", "", true) {
                    self.actions.push(Action::SetFlags { ids: ids.clone(), add: flags::SEEN, remove: 0 });
                }
                if item(ui, "As Unread", "Ctrl+Shift+U", true) {
                    self.actions.push(Action::SetFlags { ids: ids.clone(), add: 0, remove: flags::SEEN });
                }
                if item(ui, "As Junk Mail", "Ctrl+Shift+J", true) {
                    self.actions.push(Action::MarkJunk { ids: ids.clone(), junk: true });
                }
                if item(ui, "As Not Junk Mail", "", true) {
                    self.actions.push(Action::MarkJunk { ids: ids.clone(), junk: false });
                }
                ui.separator();
                if item(ui, "As Low Priority", "", false) {}
                if item(ui, "As Replied", "", true) {
                    self.actions.push(Action::SetFlags { ids: ids.clone(), add: flags::ANSWERED, remove: 0 });
                }
            });
            ui.menu_button("Flag", |ui| self.flag_menu(ui));
            ui.menu_button("Remind Me", |ui| self.remind_menu(ui));
            ui.menu_button("Move to", |ui| {
                let ids2 = ids.clone();
                self.mailbox_picker(ui, &mut |a, dest| a.push(Action::Move { ids: ids2.clone(), dest }));
            });
            ui.menu_button("Copy to", |ui| {
                let ids2 = ids.clone();
                self.mailbox_picker(ui, &mut |a, dest| a.push(Action::Copy { ids: ids2.clone(), dest }));
            });
            if item(ui, "Archive", "Ctrl+Shift+A", true) {
                self.actions.push(Action::Archive(ids.clone()));
            }
            if item(ui, "Move to Trash", "Delete", true) {
                self.actions.push(Action::Delete(ids.clone()));
            }
        });
        ui.separator();
        if item(ui, "Apply Rules", "Ctrl+L", has) {
            self.actions.push(Action::ApplyRules(ids.clone()));
        }
        let thread = msgs.first().map(|m| m.thread_id);
        let muted = thread.map(|t| self.muted.contains(&t)).unwrap_or(false);
        if item(ui, if muted { "Unmute" } else { "Mute" }, "", has) {
            if let Some(t) = thread {
                self.actions.push(Action::Mute { thread: t, muted: !muted });
            }
        }
        if let Some(m) = msgs.first().cloned() {
            ui.separator();
            let vip = self.vips.contains(&m.from.email.to_lowercase());
            if item(ui, if vip { "Remove from VIPs" } else { "Add Sender to VIPs" }, "", single) {
                self.actions.push(Action::SetVip { address: m.from.clone(), vip: !vip });
            }
            if item(ui, "Add Sender to Contacts", "", single && !self.contacts.contains(&m.from.email.to_lowercase())) {
                self.actions.push(Action::AddToContacts(m.from.clone()));
            }
            let blocked = self.store.is_blocked(&m.from.email);
            if item(ui, if blocked { "Unblock Sender" } else { "Block Sender" }, "", single) {
                self.actions.push(Action::BlockSender { email: m.from.email.clone(), blocked: !blocked });
            }
            if item(ui, "Unsubscribe", "", single && !m.list_unsubscribe.is_empty()) {
                self.actions.push(Action::Unsubscribe(m.id));
            }
            ui.menu_button("Category", |ui| {
                for c in Category::ALL {
                    if ui.radio(m.category == c, c.name()).clicked() {
                        self.actions.push(Action::SetCategory { id: m.id, category: c });
                        ui.close();
                    }
                }
            });
            if item(ui, "Remove Attachments", "", m.has_attachments) {
                self.actions.push(Action::RemoveAttachments(ids.clone()));
            }
            ui.separator();
            if item(ui, "Open in New Window", "Enter", single) {
                self.actions.push(Action::OpenMessageWindow(focus));
            }
            if item(ui, "Show Raw Source", "Ctrl+Alt+U", single) {
                self.actions.push(Action::ShowRawSource(focus));
            }
        }
    }

    fn window_menu(&mut self, ui: &mut Ui) {
        if item(ui, "Message Viewer", "", true) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if item(ui, "Activity", "Ctrl+Alt+0", true) {
            self.show_activity = true;
        }
        if item(ui, "Connection Doctor", "", true) {
            self.actions.push(Action::ShowConnectionDoctor);
        }
        if item(ui, "Contacts", "", true) {
            self.actions.push(Action::ShowContacts);
        }
        if item(ui, "Previous Recipients", "", true) {
            self.actions.push(Action::ShowPreviousRecipients);
        }
        ui.separator();
        if item(ui, "Add Account…", "", true) {
            self.actions.push(Action::AddAccount);
        }
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        if item(ui, "Keyboard Shortcuts", "", true) {
            self.show_shortcuts = true;
        }
        if item(ui, "Mail Log", "", true) {
            self.show_activity = true;
        }
        ui.separator();
        if item(ui, &format!("About Purpamail {}", env!("CARGO_PKG_VERSION")), "", true) {
            self.show_about = true;
        }
    }

    /// Context menu for a message row.
    pub fn message_context_menu(&mut self, ui: &mut Ui) {
        let single = self.selected.len() == 1;
        let ids = self.selection_ids();
        let focus = self.focus_message();
        if ui.button(format!("{}  Open in New Window", ic::ARROW_SQUARE_OUT)).clicked() {
            self.actions.push(Action::OpenMessageWindow(focus));
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(single, Button::new(format!("{}  Reply", ic::ARROW_BEND_UP_LEFT))).clicked() {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::Reply, selected_text: None }));
            ui.close();
        }
        if ui.add_enabled(single, Button::new(format!("{}  Reply All", ic::ARROW_BEND_DOUBLE_UP_LEFT))).clicked() {
            self.actions.push(Action::Compose(ComposeInit::Respond { message: focus, kind: ReplyKind::ReplyAll, selected_text: None }));
            ui.close();
        }
        if ui.button(format!("{}  Forward", ic::ARROW_BEND_UP_RIGHT)).clicked() {
            self.actions.push(Action::Compose(if single { ComposeInit::Respond { message: focus, kind: ReplyKind::Forward, selected_text: None } } else { ComposeInit::ForwardMany(ids.clone()) }));
            ui.close();
        }
        ui.separator();
        let msgs = self.store.messages_by_ids(&ids);
        let unread = msgs.iter().any(|m| m.is_unread());
        if ui.button(format!("{}  {}", ic::ENVELOPE_SIMPLE, if unread { "Mark as Read" } else { "Mark as Unread" })).clicked() {
            self.actions.push(Action::ToggleRead(ids.clone()));
            ui.close();
        }
        ui.menu_button(format!("{}  Flag", ic::FLAG), |ui| self.flag_menu(ui));
        ui.menu_button(format!("{}  Remind Me", ic::ALARM), |ui| self.remind_menu(ui));
        ui.menu_button(format!("{}  Move to", ic::FOLDER_SIMPLE), |ui| {
            let ids2 = ids.clone();
            self.mailbox_picker(ui, &mut |a, dest| a.push(Action::Move { ids: ids2.clone(), dest }));
        });
        ui.menu_button(format!("{}  Copy to", ic::COPY), |ui| {
            let ids2 = ids.clone();
            self.mailbox_picker(ui, &mut |a, dest| a.push(Action::Copy { ids: ids2.clone(), dest }));
        });
        if let Some(m) = msgs.first() {
            let t = m.thread_id;
            let muted = self.muted.contains(&t);
            if ui.button(format!("{}  {}", if muted { ic::BELL } else { ic::BELL_SLASH }, if muted { "Unmute" } else { "Mute" })).clicked() {
                self.actions.push(Action::Mute { thread: t, muted: !muted });
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}  Archive", ic::ARCHIVE)).clicked() {
            self.actions.push(Action::Archive(ids.clone()));
            ui.close();
        }
        if ui.button(format!("{}  Move to Junk", ic::PROHIBIT)).clicked() {
            self.actions.push(Action::MarkJunk { ids: ids.clone(), junk: true });
            ui.close();
        }
        if ui.button(RichText::new(format!("{}  Delete", ic::TRASH)).color(Palette::of(ui.ctx()).danger)).clicked() {
            self.actions.push(Action::Delete(ids.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}  Apply Rules", ic::LIST_CHECKS)).clicked() {
            self.actions.push(Action::ApplyRules(ids));
            ui.close();
        }
    }
}
