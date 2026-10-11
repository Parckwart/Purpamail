//! Accounts preferences: account list, account information, server settings
//! with connection test, mailbox behaviors and advanced options.

use std::collections::HashMap;

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::Edit;
use super::form::{self, Form};
use crate::core::config::{AccountConfig, AccountKind, DownloadAttachments, EraseAfter, Pop3Removal, Secrets, Settings};
use crate::core::model::{AccountId, MailboxId, MailboxRole};
use crate::net::{Command, Event};
use crate::ui::account_setup::{adjust_port, server_fields};
use crate::ui::actions::{Action, UiCx};
use crate::ui::theme::Palette;

const F: Form = Form::new(170.0);
const TEST_TIMEOUT: f64 = 40.0;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum SubTab {
    #[default]
    Info,
    Behaviors,
    Server,
    Advanced,
}

type TestResult = (Result<(), String>, Result<(), String>);

#[derive(Default)]
pub struct AccountsState {
    selected: Option<AccountId>,
    sub: SubTab,
    /// Account whose passwords are loaded into the buffers below.
    pw_for: Option<AccountId>,
    pw_in: String,
    pw_out: String,
    /// Connection test in progress: account and start time.
    testing: Option<(AccountId, f64)>,
    /// Result received from the engine, not yet attributed to an account.
    incoming_result: Option<TestResult>,
    results: HashMap<AccountId, TestResult>,
    status: HashMap<AccountId, (bool, Option<String>)>,
    confirm_remove: Option<AccountId>,
}

impl AccountsState {
    pub fn on_event(&mut self, ev: &Event) {
        match ev {
            Event::TestAccountResult { incoming, outgoing } => self.incoming_result = Some((incoming.clone(), outgoing.clone())),
            Event::AccountStatus { account, online, error } => {
                self.status.insert(account.clone(), (*online, error.clone()));
            }
            _ => {}
        }
    }

    fn status_text(&self, a: &AccountConfig) -> (String, egui::Color32, String) {
        let green = egui::Color32::from_rgb(0x3c, 0xb8, 0x4a);
        let kind = if a.kind == AccountKind::Pop3 { "POP" } else { "IMAP" };
        if !a.enabled {
            return (format!("{kind} · Disabled"), egui::Color32::GRAY, "Disabled".into());
        }
        match self.status.get(&a.id) {
            Some((_, Some(e))) => (format!("{kind} · Error"), egui::Color32::from_rgb(0xe0, 0x3b, 0x3b), if e.is_empty() { "Error".into() } else { e.clone() }),
            Some((true, None)) => (format!("{kind} · Online"), green, "Online".into()),
            Some((false, None)) => (format!("{kind} · Offline"), egui::Color32::GRAY, "Offline".into()),
            None => (kind.to_string(), egui::Color32::GRAY, "Not connected yet".into()),
        }
    }
}

pub fn show(ui: &mut Ui, st: &mut AccountsState, s: &mut Settings, cx: &mut UiCx, edit: &mut Edit) {
    let now = ui.input(|i| i.time);
    // attribute test results / timeouts
    if let Some((id, t0)) = st.testing.clone() {
        if let Some(r) = st.incoming_result.take() {
            st.results.insert(id, r);
            st.testing = None;
        } else if now - t0 > TEST_TIMEOUT {
            st.results.insert(id, (Err("Timed out".into()), Err("Timed out".into())));
            st.testing = None;
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
    if st.selected.as_ref().is_none_or(|id| s.account(id).is_none()) {
        st.selected = s.accounts.first().map(|a| a.id.clone());
    }
    let pal = Palette::of(ui.ctx());
    let avail_h = ui.available_height();
    ui.horizontal_top(|ui| {
        // ---------------------------------------------------------- account list
        ui.vertical(|ui| {
            ui.set_width(200.0);
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(avail_h - 44.0);
                egui::ScrollArea::vertical().id_salt("acct_list").auto_shrink([false, false]).show(ui, |ui| {
                    if s.accounts.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("No accounts").color(pal.text_tertiary));
                    }
                    for a in &s.accounts {
                        let (sub, _, _) = st.status_text(a);
                        let sel = st.selected.as_deref() == Some(a.id.as_str());
                        let icon_col = if a.enabled { pal.accent } else { pal.text_tertiary };
                        let r = form::list_row(ui, sel, Some((ic::ENVELOPE_SIMPLE, icon_col)), a.display_name(), &sub);
                        if r.clicked() {
                            st.selected = Some(a.id.clone());
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if form::icon_button(ui, ic::PLUS, "Add an account", true).clicked() {
                    cx.act(Action::AddAccount);
                }
                if form::icon_button(ui, ic::MINUS, "Remove the selected account", st.selected.is_some()).clicked() {
                    st.confirm_remove = st.selected.clone();
                }
            });
        });
        ui.add_space(10.0);
        // ---------------------------------------------------------- details
        ui.vertical(|ui| {
            let Some(id) = st.selected.clone() else {
                empty_state(ui, cx);
                return;
            };
            if st.pw_for.as_ref() != Some(&id) {
                let sec = cx.secrets.read();
                st.pw_in = sec.get(&Secrets::incoming_key(&id));
                st.pw_out = sec.get(&Secrets::outgoing_key(&id));
                st.pw_for = Some(id.clone());
            }
            let Some(idx) = s.accounts.iter().position(|a| a.id == id) else { return };
            ui.vertical_centered(|ui| {
                form::segmented(ui, &mut st.sub, &[(SubTab::Info, "Account Information"), (SubTab::Behaviors, "Mailbox Behaviors"), (SubTab::Server, "Server Settings"), (SubTab::Advanced, "Advanced")]);
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical().id_salt(("acct_detail", st.sub as u8)).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                match st.sub {
                    SubTab::Info => info_tab(ui, st, &mut s.accounts[idx]),
                    SubTab::Behaviors => behaviors_tab(ui, &mut s.accounts[idx], cx),
                    SubTab::Server => server_tab(ui, st, &mut s.accounts[idx], cx, edit, now),
                    SubTab::Advanced => {
                        let sigs: Vec<(String, String)> = s.signatures.iter().map(|g| (g.id.clone(), g.name.clone())).collect();
                        advanced_tab(ui, &mut s.accounts[idx], &sigs)
                    }
                }
            });
        });
    });

    // ---------------------------------------------------------- remove confirmation
    if let Some(id) = st.confirm_remove.clone() {
        let name = s.account(&id).map(|a| a.display_name().to_string()).unwrap_or_default();
        let mut decision: Option<bool> = None;
        egui::Modal::new(egui::Id::new("confirm_remove_account")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::WARNING_CIRCLE).size(32.0).color(pal.danger));
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("Remove the account “{name}”?")).family(crate::ui::fonts::bold()).size(14.0));
                    form::hint(ui, "All of its messages and mailboxes stored on this computer are deleted. Messages on the server are not affected.");
                });
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Remove").color(egui::Color32::WHITE)).fill(pal.danger).min_size(egui::vec2(72.0, 26.0))).clicked() {
                    decision = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                remove_account(s, cx, &id);
                st.confirm_remove = None;
                st.selected = None;
                st.pw_for = None;
                edit.now = true;
            }
            Some(false) => st.confirm_remove = None,
            None => {}
        }
    }
}

/// Remove an account from the settings together with its secrets and local data.
fn remove_account(s: &mut Settings, cx: &mut UiCx, id: &str) {
    s.accounts.retain(|a| a.id != id);
    for sig in &mut s.signatures {
        sig.accounts.retain(|a| a != id);
    }
    if s.composing.send_new_from == id {
        s.composing.send_new_from.clear();
    }
    {
        let mut sec = cx.secrets.write();
        sec.passwords.remove(&Secrets::incoming_key(id));
        sec.passwords.remove(&Secrets::outgoing_key(id));
    }
    if let Err(e) = cx.store.delete_account_data(id) {
        cx.act(Action::Toast(format!("Could not remove local data: {e}")));
    }
}

fn empty_state(ui: &mut Ui, cx: &mut UiCx) {
    let pal = Palette::of(ui.ctx());
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(ic::AT).size(44.0).color(pal.text_tertiary));
        ui.label(RichText::new("No mail accounts").family(crate::ui::fonts::bold()).size(15.0));
        ui.label(RichText::new("Add an account to send and receive email.").color(pal.text_secondary));
        ui.add_space(8.0);
        if crate::ui::widgets::primary_button(ui, "Add Account…").clicked() {
            cx.act(Action::AddAccount);
        }
    });
}

fn info_tab(ui: &mut Ui, st: &AccountsState, a: &mut AccountConfig) {
    let pal = Palette::of(ui.ctx());
    F.indented(ui, |ui| ui.checkbox(&mut a.enabled, "Enable this account"));
    F.row(ui, "Status:", |ui| {
        let (_, col, text) = st.status_text(a);
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(col));
            ui.add(egui::Label::new(text).wrap());
        });
    });
    form::section(ui);
    F.row(ui, "Description:", |ui| {
        let placeholder = a.primary_email().to_string();
        form::text_field(ui, &mut a.name, 260.0, &placeholder);
        form::hint(ui, "Shown in the sidebar and in the From menu.");
    });
    F.row(ui, "Full Name:", |ui| form::text_field(ui, &mut a.full_name, 260.0, "Your name"));
    F.row(ui, "Email Addresses:", |ui| {
        let mut remove = None;
        let n = a.emails.len();
        for (i, e) in a.emails.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                form::text_field(ui, e, 230.0, "name@example.com");
                if i == 0 {
                    ui.label(RichText::new("primary").size(11.0).color(pal.text_tertiary));
                } else if form::icon_button(ui, ic::MINUS, "Remove this alias", true).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            a.emails.remove(i);
        }
        if n == 0 {
            a.emails.push(String::new());
        }
        if ui.button(format!("{} Add Alias", ic::PLUS)).clicked() {
            a.emails.push(String::new());
        }
        form::hint(ui, "Aliases are offered in the From menu and recognised as your own addresses.");
    });
}

fn server_tab(ui: &mut Ui, st: &mut AccountsState, a: &mut AccountConfig, cx: &mut UiCx, edit: &mut Edit, now: f64) {
    let pal = Palette::of(ui.ctx());
    let id = a.id.clone();
    form::title(ui, "Incoming Mail Server");
    ui.add_space(4.0);
    F.row(ui, "Account Type:", |ui| {
        let mut kind = a.kind;
        if form::segmented(ui, &mut kind, &[(AccountKind::Imap, "IMAP"), (AccountKind::Pop3, "POP")]) {
            a.kind = kind;
            a.incoming.port = adjust_port(a.incoming.port, true, kind, a.incoming.security);
        }
    });
    let kind = a.kind;
    server_fields(ui, F, "in", &mut a.incoming, true, kind, Some(&mut st.pw_in), true);
    // password edits are written straight to the secrets store
    if cx.secrets.read().get(&Secrets::incoming_key(&id)) != st.pw_in {
        cx.secrets.write().set(Secrets::incoming_key(&id), st.pw_in.clone());
        edit.dirty = true;
    }
    form::section(ui);
    form::title(ui, "Outgoing Mail Server (SMTP)");
    ui.add_space(4.0);
    F.indented(ui, |ui| ui.checkbox(&mut a.smtp_same_credentials, "Use the same user name and password as for incoming mail"));
    let same = a.smtp_same_credentials;
    server_fields(ui, F, "out", &mut a.outgoing, false, kind, Some(&mut st.pw_out), !same);
    if !same && cx.secrets.read().get(&Secrets::outgoing_key(&id)) != st.pw_out {
        cx.secrets.write().set(Secrets::outgoing_key(&id), st.pw_out.clone());
        edit.dirty = true;
    }
    form::section(ui);
    F.indented(ui, |ui| {
        let testing = st.testing.as_ref().is_some_and(|(t, _)| *t == id);
        ui.horizontal(|ui| {
            if ui.add_enabled(st.testing.is_none(), egui::Button::new(format!("{} Test Connection", ic::PLUG))).clicked() {
                st.results.remove(&id);
                st.incoming_result = None;
                st.testing = Some((id.clone(), now));
                cx.engine.send(Command::TestAccount(Box::new(a.clone()), st.pw_in.clone()));
            }
            if testing {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new("Connecting…").color(pal.text_secondary));
            }
        });
        if let Some((inc, out)) = st.results.get(&id) {
            result_line(ui, "Incoming server", inc);
            result_line(ui, "Outgoing server", out);
        } else if !testing {
            form::hint(ui, "Checks that Purpamail can log in to both servers with these settings.");
        }
    });
}

/// One line of a connection test result.
pub fn result_line(ui: &mut Ui, what: &str, r: &Result<(), String>) {
    let pal = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| match r {
        Ok(()) => {
            ui.label(RichText::new(ic::CHECK_CIRCLE).color(egui::Color32::from_rgb(0x3c, 0xb8, 0x4a)));
            ui.label(format!("{what}: connected"));
        }
        Err(e) => {
            ui.label(RichText::new(ic::X_CIRCLE).color(pal.danger));
            ui.add(egui::Label::new(format!("{what}: {e}")).wrap());
        }
    });
}

fn behaviors_tab(ui: &mut Ui, a: &mut AccountConfig, cx: &mut UiCx) {
    let mailboxes = cx.store.mailboxes(Some(&a.id));
    form::title(ui, "Special Mailboxes");
    ui.add_space(4.0);
    if mailboxes.is_empty() {
        F.indented(ui, |ui| form::hint(ui, "The account's mailboxes appear here after it has been synchronized for the first time."));
    } else {
        for role in [MailboxRole::Drafts, MailboxRole::Sent, MailboxRole::Junk, MailboxRole::Trash, MailboxRole::Archive] {
            F.row(ui, &format!("{} Mailbox:", role.display_name()), |ui| {
                let current = mailboxes.iter().find(|m| m.role == role).map(|m| m.id).unwrap_or(0);
                let mut sel: MailboxId = current;
                let mut opts: Vec<(MailboxId, String)> = vec![(0, "None".into())];
                opts.extend(mailboxes.iter().filter(|m| m.selectable && m.role != MailboxRole::Inbox && m.role != MailboxRole::Outbox).map(|m| (m.id, m.path.clone())));
                let refs: Vec<(MailboxId, &str)> = opts.iter().map(|(i, n)| (*i, n.as_str())).collect();
                if form::combo(ui, ("role", role.as_str()), &mut sel, &refs, 240.0) {
                    if current != 0 {
                        let _ = cx.store.set_mailbox_role(current, MailboxRole::Custom);
                    }
                    a.role_overrides.retain(|_, r| *r != role);
                    if sel != 0 {
                        if role == MailboxRole::Archive
                            && let Some(m) = mailboxes.iter().find(|m| m.id == sel) {
                                a.archive_mailbox_path = m.path.clone();
                            }
                        cx.act(Action::UseMailboxFor { mailbox: sel, role });
                    } else if role == MailboxRole::Archive {
                        a.archive_mailbox_path.clear();
                    }
                }
            });
        }
    }
    form::section(ui);
    let erase_opts: Vec<(EraseAfter, &str)> = EraseAfter::ALL.iter().map(|e| (*e, e.name())).collect();
    form::title(ui, "Drafts & Sent");
    ui.add_space(4.0);
    F.indented(ui, |ui| {
        ui.checkbox(&mut a.store_drafts_on_server, "Store draft messages on the server");
        ui.checkbox(&mut a.store_sent_on_server, "Store sent messages on the server");
    });
    F.row(ui, "Delete sent messages:", |ui| form::combo(ui, "erase_sent", &mut a.erase_sent, &erase_opts, 200.0));
    form::section(ui);
    form::title(ui, "Junk");
    ui.add_space(4.0);
    F.indented(ui, |ui| ui.checkbox(&mut a.store_junk_on_server, "Store junk messages on the server"));
    F.row(ui, "Delete junk messages:", |ui| {
        form::combo(ui, "erase_junk", &mut a.erase_junk, &erase_opts, 200.0);
        if ui.button("Erase Junk Mail Now").clicked() {
            cx.act(Action::EraseJunk);
        }
    });
    form::section(ui);
    form::title(ui, "Trash");
    ui.add_space(4.0);
    F.indented(ui, |ui| {
        ui.checkbox(&mut a.move_deleted_to_trash, "Move deleted messages to the Trash mailbox");
        ui.checkbox(&mut a.store_deleted_on_server, "Store deleted messages on the server");
    });
    F.row(ui, "Permanently erase:", |ui| {
        form::combo(ui, "erase_deleted", &mut a.erase_deleted, &erase_opts, 200.0);
        if ui.button("Erase Deleted Items Now").clicked() {
            cx.act(Action::EraseDeleted(Some(a.id.clone())));
        }
    });
}

fn advanced_tab(ui: &mut Ui, a: &mut AccountConfig, signatures: &[(String, String)]) {
    F.indented(ui, |ui| ui.checkbox(&mut a.include_when_checking, "Include when automatically checking for new messages"));
    F.row(ui, "Download attachments:", |ui| {
        let opts = [(DownloadAttachments::All, "All"), (DownloadAttachments::Recent, "Recent"), (DownloadAttachments::None, "None")];
        form::combo(ui, "dl_att", &mut a.download_attachments, &opts, 160.0);
        form::hint(ui, "“Recent” downloads attachments of messages from the last month.");
    });
    form::section(ui);
    match a.kind {
        AccountKind::Imap => {
            form::title(ui, "IMAP");
            ui.add_space(4.0);
            F.row(ui, "IMAP Path Prefix:", |ui| {
                form::text_field(ui, &mut a.imap_path_prefix, 200.0, "e.g. INBOX");
                form::hint(ui, "Only needed when the server keeps all mailboxes below one folder.");
            });
            F.indented(ui, |ui| {
                ui.checkbox(&mut a.use_idle, "Use IDLE command if the server supports it");
                form::hint(ui, "Receives new messages instantly instead of when checking periodically.");
            });
        }
        AccountKind::Pop3 => {
            form::title(ui, "POP");
            ui.add_space(4.0);
            F.row(ui, "Remove copy from server:", |ui| {
                let opts: Vec<(Pop3Removal, &str)> = Pop3Removal::ALL.iter().map(|p| (*p, p.name())).collect();
                form::combo(ui, "pop3_remove", &mut a.pop3_remove, &opts, 200.0);
            });
            F.row(ui, "Size warning:", |ui| {
                let mut on = a.pop3_size_warning_kb > 0;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, "Prompt me to skip messages over").changed() {
                        a.pop3_size_warning_kb = if on { 1024 } else { 0 };
                    }
                    // edit a copy: the drag value clamps to its range even when disabled
                    let mut kb = a.pop3_size_warning_kb.max(1);
                    if ui.add_enabled(on, egui::DragValue::new(&mut kb).range(1..=1_000_000).suffix(" KB")).changed() && on {
                        a.pop3_size_warning_kb = kb;
                    }
                });
            });
        }
    }
    form::section(ui);
    form::title(ui, "Composing");
    ui.add_space(4.0);
    F.row(ui, "Default signature:", |ui| {
        let mut opts: Vec<(Option<String>, &str)> = vec![(None, "None")];
        opts.extend(signatures.iter().map(|(id, n)| (Some(id.clone()), n.as_str())));
        form::combo(ui, "acct_sig", &mut a.signature, &opts, 200.0);
    });
    F.indented(ui, |ui| ui.checkbox(&mut a.auto_bcc_myself, "Automatically Bcc myself on messages from this account"));
}
