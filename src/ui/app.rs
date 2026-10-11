//! The main application: window layout, menus, toolbar, keyboard shortcuts and
//! the central processing of [`Action`]s.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use egui::{Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText, Ui, Vec2};
use egui_phosphor::regular as ic;
use parking_lot::RwLock;

use super::account_setup::AccountSetup;
use super::actions::*;
use super::compose::ComposeWindow;
use super::contacts::{ContactsTab, ContactsWindow};
use super::prefs::PrefsWindow;
use super::rule_editor::SmartMailboxDialog;
use super::theme::Palette;
use super::viewer::MessageViewer;
use super::widgets;
use crate::core::compose::{OutgoingMessage, ReplyKind};
use crate::core::config::{FavoriteItem, ListLayout, Secrets, Settings, SmartMailbox, ToolbarItem};
use crate::core::criteria::EvalContext;
use crate::core::model::*;
use crate::core::search::{self, SearchField};
use crate::core::store::{MessageQuery, SharedStore};
use crate::core::{ops, rules};
use crate::net::engine::{SharedSecrets, SharedSettings};
use crate::net::{Activity, ActivityState, Command, DoctorResult, Engine, EngineHandle, Event};

/// One row in the message list.
#[derive(Clone, Debug)]
pub struct ListRow {
    pub msg: MessageSummary,
    /// Number of messages in the conversation (1 = single message).
    pub thread_count: usize,
    pub thread_unread: bool,
    pub thread_flag: Option<Option<FlagColor>>,
    /// Row is a member shown under an expanded conversation.
    pub is_child: bool,
    pub muted: bool,
    pub vip: bool,
    pub highlight: Option<FlagColor>,
}

/// Cached counts shown in the sidebar.
#[derive(Default, Clone, Debug)]
pub struct SideCounts {
    pub flagged_unread: u32,
    pub flagged_total: u32,
    pub per_color: Vec<(FlagColor, u32)>,
    pub vip_unread: u32,
    pub vips: Vec<(String, String, u32)>,
    pub smart: HashMap<String, u32>,
    pub reminders: u32,
    pub follow_up: u32,
    pub outbox: u32,
}

/// A simple text prompt dialog (new mailbox, rename, ...).
pub struct Prompt {
    pub title: String,
    pub label: String,
    pub text: String,
    pub kind: PromptKind,
}

pub enum PromptKind {
    NewMailbox { account: AccountId, parent: Option<MailboxId> },
    RenameMailbox(MailboxId),
    RemindCustom(Vec<MessageId>),
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub ok: String,
    pub action: Action,
}

/// Something that can be undone with Edit > Undo.
pub struct UndoEntry {
    pub label: String,
    /// (message, original mailbox)
    pub moves: Vec<(MessageId, MailboxId)>,
    /// (message, original flags, original color)
    pub flags: Vec<(MessageId, u32, Option<FlagColor>)>,
}

pub struct PendingSend {
    pub outbox_id: MessageId,
    pub message: OutgoingMessage,
    pub deadline: f64,
}

pub struct Toast {
    pub text: String,
    pub until: f64,
}

pub struct PurpamailApp {
    pub store: SharedStore,
    pub settings: SharedSettings,
    pub secrets: SharedSecrets,
    pub engine: EngineHandle,
    pub actions: Vec<Action>,

    // selection & list
    pub sel: SidebarSel,
    pub selected: Vec<MessageId>,
    pub anchor: Option<MessageId>,
    pub rows: Vec<ListRow>,
    pub list_dirty: bool,
    pub expanded_threads: HashSet<i64>,
    pub scroll_to_selected: bool,
    pub filter: ListFilter,
    pub search_text: String,
    pub search_field: SearchField,
    pub search_all_mailboxes: bool,
    pub search_active: bool,
    pub focus_search: bool,
    pub list_has_focus: bool,

    // caches refreshed with the list
    pub mailboxes: Vec<Mailbox>,
    pub muted: HashSet<i64>,
    pub vips: HashSet<String>,
    pub contacts: HashSet<String>,
    pub counts_dirty: bool,
    pub side: SideCounts,

    // components
    pub viewer: MessageViewer,
    pub composers: Vec<ComposeWindow>,
    next_window_id: u64,
    pub prefs: PrefsWindow,
    pub contacts_win: ContactsWindow,
    pub account_setup: Option<AccountSetup>,
    pub smart_dialog: Option<SmartMailboxDialog>,
    pub message_windows: Vec<(u64, MessageId, MessageViewer)>,
    pub raw_windows: Vec<MessageId>,
    pub prompt: Option<Prompt>,
    pub confirm: Option<Confirm>,
    pub show_activity: bool,
    pub show_doctor: bool,
    pub show_about: bool,
    pub show_shortcuts: bool,
    pub activities: BTreeMap<u64, (Activity, f64)>,
    pub doctor: Vec<DoctorResult>,
    pub doctor_running: bool,
    pub account_status: HashMap<AccountId, (bool, Option<String>)>,
    pub toasts: Vec<Toast>,
    pub undo: Vec<UndoEntry>,
    pub pending_send: Option<PendingSend>,
    pub log: Vec<String>,
    pub last_updated: Option<i64>,
    ipc_rx: crossbeam_channel::Receiver<String>,

    fonts_applied: crate::core::config::FontsColorsPrefs,
    theme_applied: Option<(crate::core::config::Theme, f32)>,
    last_reminder_check: f64,
    last_save_state: f64,
    pub sidebar_drop_hover: Option<MailboxId>,
}

impl PurpamailApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: SharedStore, settings: Settings) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings: SharedSettings = Arc::new(RwLock::new(settings));
        let secrets: SharedSecrets = Arc::new(RwLock::new(Secrets::load()));
        let ctx = cc.egui_ctx.clone();
        let engine = Engine::start(store.clone(), settings.clone(), secrets.clone(), Arc::new(move || ctx.request_repaint()));
        let s = settings.read().clone();
        super::fonts::install(&cc.egui_ctx, &s.fonts);
        super::theme::apply(&cc.egui_ctx, s.viewing.theme, s.viewing.ui_scale);
        let (ipc_tx, ipc_rx) = crossbeam_channel::unbounded();
        let wake = cc.egui_ctx.clone();
        crate::ipc::listen(ipc_tx, move || wake.request_repaint());
        let sel = store.kv_get("ui:sidebar_sel").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(SidebarSel::AllRole(MailboxRole::Inbox));
        let mut app = Self {
            store,
            settings,
            secrets,
            engine,
            actions: vec![],
            sel,
            selected: vec![],
            anchor: None,
            rows: vec![],
            list_dirty: true,
            expanded_threads: HashSet::new(),
            scroll_to_selected: false,
            filter: ListFilter::default(),
            search_text: String::new(),
            search_field: SearchField::Entire,
            search_all_mailboxes: true,
            search_active: false,
            focus_search: false,
            list_has_focus: true,
            mailboxes: vec![],
            muted: HashSet::new(),
            vips: HashSet::new(),
            contacts: HashSet::new(),
            counts_dirty: true,
            side: SideCounts::default(),
            viewer: MessageViewer::new(),
            composers: vec![],
            next_window_id: 1,
            prefs: PrefsWindow::default(),
            contacts_win: ContactsWindow::default(),
            account_setup: None,
            smart_dialog: None,
            message_windows: vec![],
            raw_windows: vec![],
            prompt: None,
            confirm: None,
            show_activity: false,
            show_doctor: false,
            show_about: false,
            show_shortcuts: false,
            activities: BTreeMap::new(),
            doctor: vec![],
            doctor_running: false,
            account_status: HashMap::new(),
            toasts: vec![],
            undo: vec![],
            pending_send: None,
            log: vec![],
            last_updated: None,
            ipc_rx,
            fonts_applied: s.fonts.clone(),
            theme_applied: Some((s.viewing.theme, s.viewing.ui_scale)),
            last_reminder_check: 0.0,
            last_save_state: 0.0,
            sidebar_drop_hover: None,
        };
        if s.accounts.is_empty() && !s.onboarded {
            app.account_setup = Some(AccountSetup::new());
        }
        // Make sure the local mailboxes exist.
        let _ = ops::outbox(&app.store);
        let _ = app.store.ensure_mailbox(LOCAL_ACCOUNT, "Inbox", MailboxRole::Inbox);
        app.engine.send(Command::CheckMail(None));
        app
    }

    /// Handle a request from the command line or another Purpamail invocation.
    pub fn handle_request(&mut self, r: &str) {
        if r.starts_with("mailto:") {
            self.actions.push(Action::Compose(ComposeInit::Mailto(r.to_string())));
        } else if let Some(path) = r.strip_prefix("open:") {
            if let Ok(raw) = std::fs::read(path) {
                if let Ok(mb) = self.store.ensure_mailbox(LOCAL_ACCOUNT, "Opened Messages", MailboxRole::Custom) {
                    if let Ok(ids) = ops::import_raw(&self.store, mb, &[raw]) {
                        for id in ids {
                            self.actions.push(Action::OpenMessageWindow(id));
                        }
                    }
                }
                self.list_dirty = true;
            }
        }
    }

    /// Files dropped on the main window: messages are opened, mailboxes imported.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if files.is_empty() {
            return;
        }
        let mut imported = 0;
        for f in files {
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            match ext.as_str() {
                "eml" | "msg" => self.handle_request(&format!("open:{}", f.display())),
                "mbox" | "mbx" | "" => {
                    let raws = if f.is_dir() { crate::core::mbox::import_mailbox_tree(&f).into_iter().flat_map(|(_, r)| r).collect() } else { crate::core::mbox::import(&f).unwrap_or_default() };
                    let name = f.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported").to_string();
                    if let Ok(mb) = self.store.ensure_mailbox(LOCAL_ACCOUNT, &format!("Import/{name}"), MailboxRole::Custom) {
                        imported += ops::import_raw(&self.store, mb, &raws).map(|v| v.len()).unwrap_or(0);
                    }
                    let _ = self.store.ensure_mailbox(LOCAL_ACCOUNT, "Import", MailboxRole::Custom);
                    self.fix_local_parents();
                }
                _ => {}
            }
        }
        if imported > 0 {
            self.toast(ctx, format!("Imported {imported} messages"));
        }
        self.list_dirty = true;
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_window_id += 1;
        self.next_window_id
    }

    fn cx(&mut self) -> (UiCx<'_>, &mut MessageViewer) {
        (UiCx { store: &self.store, settings: &self.settings, secrets: &self.secrets, engine: &self.engine, actions: &mut self.actions }, &mut self.viewer)
    }

    pub fn toast(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        self.toasts.push(Toast { text: text.into(), until: now + 3.5 });
    }

    fn flush(&self, accts: ops::Accounts) {
        for a in accts {
            self.engine.send(Command::FlushOps(a));
        }
    }

    // ------------------------------------------------------------------ list

    /// Mailbox ids for the current sidebar selection (empty vec = all).
    pub fn scope_mailboxes(&self, sel: &SidebarSel) -> Vec<MailboxId> {
        match sel {
            SidebarSel::Mailbox(id) => vec![*id],
            SidebarSel::AllRole(role) => {
                let v: Vec<MailboxId> = self.mailboxes.iter().filter(|m| m.role == *role).map(|m| m.id).collect();
                if v.is_empty() { vec![-1] } else { v }
            }
            _ => vec![],
        }
    }

    fn excluded_for_global(&self, s: &Settings, include_trash: bool, include_sent: bool) -> Vec<MailboxId> {
        self.mailboxes
            .iter()
            .filter(|m| {
                (m.role == MailboxRole::Trash && !include_trash)
                    || (m.role == MailboxRole::Junk && !s.general.search_include_junk)
                    || (m.role == MailboxRole::Sent && !include_sent)
                    || m.role == MailboxRole::Outbox && false
            })
            .map(|m| m.id)
            .collect()
    }

    pub fn refresh_caches(&mut self) {
        self.mailboxes = self.store.mailboxes(None);
        self.muted = self.store.muted_threads();
        self.vips = self.store.vip_emails().into_iter().collect();
        self.contacts = self.store.contact_emails();
        self.side = self.compute_side_counts();
        self.counts_dirty = false;
    }

    fn compute_side_counts(&self) -> SideCounts {
        let mut c = SideCounts::default();
        let s = self.settings.read().clone();
        let excl = self.excluded_for_global(&s, false, true);
        let flagged = self.store.list_messages(&MessageQuery { only_flagged: true, exclude_mailboxes: excl.clone(), ..Default::default() });
        c.flagged_total = flagged.len() as u32;
        c.flagged_unread = flagged.iter().filter(|m| m.is_unread()).count() as u32;
        for col in FlagColor::ALL {
            let n = flagged.iter().filter(|m| m.color.unwrap_or_default() == col).count() as u32;
            if n > 0 {
                c.per_color.push((col, n));
            }
        }
        if !self.vips.is_empty() {
            let excl2 = self.excluded_for_global(&s, false, false);
            let vm = self.store.list_messages(&MessageQuery { from_any: self.vips.iter().cloned().collect(), exclude_mailboxes: excl2, only_unread: true, ..Default::default() });
            c.vip_unread = vm.len() as u32;
            for ct in self.store.contacts().into_iter().filter(|ct| ct.vip) {
                for e in &ct.emails {
                    let n = vm.iter().filter(|m| m.from.email.eq_ignore_ascii_case(e)).count() as u32;
                    c.vips.push((ct.display_name(), e.to_lowercase(), n));
                }
            }
        }
        c.reminders = self.store.with_conn(|c| c.query_row("SELECT COUNT(*) FROM messages WHERE remind_at IS NOT NULL AND (flags & 8)=0", [], |r| r.get::<_, i64>(0)).unwrap_or(0)) as u32;
        c.outbox = self.mailboxes.iter().filter(|m| m.role == MailboxRole::Outbox).map(|m| m.total).sum();
        c
    }

    pub fn refresh_list(&mut self) {
        self.refresh_caches();
        let s = self.settings.read().clone();
        let mut q = MessageQuery {
            sort: s.viewing.sort,
            ascending: s.viewing.sort_ascending,
            include_deleted: s.viewing.show_deleted_messages,
            filter: self.filter.clone(),
            my_addresses: s.my_addresses(),
            vip_addresses: self.vips.iter().cloned().collect(),
            ..Default::default()
        };
        let mut smart: Option<SmartMailbox> = None;
        let searching = self.search_active && !self.search_text.trim().is_empty();
        if searching {
            let p = search::parse(&self.search_text, self.search_field);
            q.fts = Some(p.fts);
            q.only_unread |= p.unread;
            q.only_flagged |= p.flagged;
            if p.has_attachment {
                q.filter.enabled = true;
                q.filter.with_attachments = true;
            }
            if self.search_all_mailboxes || self.sel == SidebarSel::Search {
                q.exclude_mailboxes = self.excluded_for_global(&s, s.general.search_include_trash, true);
            } else {
                q.mailboxes = self.scope_mailboxes(&self.sel);
            }
        } else {
            match &self.sel {
                SidebarSel::Mailbox(_) | SidebarSel::AllRole(_) => q.mailboxes = self.scope_mailboxes(&self.sel),
                SidebarSel::Flagged(c) => {
                    q.only_flagged = true;
                    q.only_color = *c;
                    q.exclude_mailboxes = self.excluded_for_global(&s, false, true);
                }
                SidebarSel::Vip(v) => {
                    q.from_any = match v {
                        Some(e) => vec![e.clone()],
                        None => self.vips.iter().cloned().collect(),
                    };
                    if q.from_any.is_empty() {
                        q.from_any = vec!["\u{0}".into()];
                    }
                    q.exclude_mailboxes = self.excluded_for_global(&s, false, false);
                }
                SidebarSel::Smart(id) => {
                    smart = s.smart_mailboxes.iter().find(|m| &m.id == id).cloned();
                    if let Some(sm) = &smart {
                        q.exclude_mailboxes = self.excluded_for_global(&s, sm.include_trash, sm.include_sent);
                    }
                }
                SidebarSel::Search => {
                    q.mailboxes = vec![-1];
                }
                SidebarSel::Reminders => {
                    let ids: Vec<MessageId> = self.store.with_conn(|c| {
                        let mut st = c.prepare("SELECT id FROM messages WHERE remind_at IS NOT NULL").unwrap();
                        st.query_map([], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
                    });
                    q.mailboxes = self.store.messages_by_ids(&ids).iter().map(|m| m.mailbox_id).collect();
                    if q.mailboxes.is_empty() {
                        q.mailboxes = vec![-1];
                    }
                }
                SidebarSel::FollowUp => {
                    q.mailboxes = self.mailboxes.iter().filter(|m| m.role == MailboxRole::Sent).map(|m| m.id).collect();
                    if q.mailboxes.is_empty() {
                        q.mailboxes = vec![-1];
                    }
                }
            }
        }
        let mut msgs = self.store.list_messages(&q);
        if let SidebarSel::Reminders = self.sel {
            msgs.retain(|m| m.remind_at.is_some());
        }
        if let SidebarSel::FollowUp = self.sel {
            msgs = self.follow_up_candidates(msgs);
        }
        if let Some(sm) = smart {
            let needs_body = sm.criteria.needs_body();
            let acct_names: HashMap<String, String> = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
            msgs.retain(|m| {
                let parsed = if needs_body { self.store.raw(m.id).and_then(|r| crate::core::mime::parse(&r)) } else { None };
                let body = parsed.as_ref().map(|p| p.plain_text());
                let cx = EvalContext {
                    msg: m,
                    body: body.as_deref(),
                    headers: parsed.as_ref().map(|p| p.headers.as_slice()),
                    account_name: acct_names.get(&m.account_id).map(|s| s.as_str()).unwrap_or(""),
                    sender_in_contacts: self.contacts.contains(&m.from.email.to_lowercase()),
                    sender_is_vip: self.vips.contains(&m.from.email.to_lowercase()),
                    sender_is_blocked: false,
                };
                sm.criteria.matches(&cx)
            });
        }
        let threaded = s.viewing.organize_by_conversation && !searching;
        self.rows = self.build_rows(msgs, threaded, &s);
        // keep selection valid
        let visible: HashSet<MessageId> = self.rows.iter().map(|r| r.msg.id).collect();
        self.selected.retain(|id| visible.contains(id) || self.store.message(*id).is_some_and(|m| self.rows.iter().any(|r| r.msg.thread_id == m.thread_id)));
        self.list_dirty = false;
    }

    fn follow_up_candidates(&self, sent: Vec<MessageSummary>) -> Vec<MessageSummary> {
        let now = now_ts();
        sent.into_iter()
            .filter(|m| {
                let age = now - m.date;
                if !(3 * 86400..30 * 86400).contains(&age) {
                    return false;
                }
                // any later message in the thread not from me?
                let thread = self.store.thread_messages(m.thread_id);
                !thread.iter().any(|t| t.date > m.date && t.id != m.id)
            })
            .collect()
    }

    fn build_rows(&self, msgs: Vec<MessageSummary>, threaded: bool, s: &Settings) -> Vec<ListRow> {
        let highlight_of = |id: MessageId| rules::highlight_color(&self.store, id);
        let mk = |m: MessageSummary, count: usize, unread: bool, flag: Option<Option<FlagColor>>, child: bool| ListRow {
            vip: self.vips.contains(&m.from.email.to_lowercase()),
            muted: self.muted.contains(&m.thread_id),
            highlight: highlight_of(m.id),
            msg: m,
            thread_count: count,
            thread_unread: unread,
            thread_flag: flag,
            is_child: child,
        };
        if !threaded {
            return msgs
                .into_iter()
                .map(|m| {
                    let u = m.is_unread();
                    let f = if m.is_flagged() { Some(m.color) } else { None };
                    mk(m, 1, u, f, false)
                })
                .collect();
        }
        // Group by thread, preserving the order of each thread's first (best sorted) message.
        let mut order: Vec<i64> = Vec::new();
        let mut groups: HashMap<i64, Vec<MessageSummary>> = HashMap::new();
        for m in msgs {
            let t = m.thread_id;
            if !groups.contains_key(&t) {
                order.push(t);
            }
            groups.entry(t).or_default().push(m);
        }
        let mut rows = Vec::new();
        for t in order {
            let mut g = groups.remove(&t).unwrap_or_default();
            let count = g.len();
            let unread = g.iter().any(|m| m.is_unread());
            let flag = g.iter().find(|m| m.is_flagged()).map(|m| m.color);
            g.sort_by_key(|m| std::cmp::Reverse(m.date));
            let head = g[0].clone();
            let expanded = self.expanded_threads.contains(&t) && count > 1;
            rows.push(mk(head, count, unread, flag, false));
            if expanded {
                let children: Vec<MessageSummary> = if s.viewing.show_most_recent_on_top { g } else { g.into_iter().rev().collect() };
                for c in children {
                    let u = c.is_unread();
                    let f = if c.is_flagged() { Some(c.color) } else { None };
                    rows.push(mk(c, 1, u, f, true));
                }
            }
        }
        rows
    }

    /// Messages to show in the viewer for the current selection.
    pub fn viewer_messages(&self) -> Vec<MessageSummary> {
        if self.selected.len() != 1 {
            return vec![];
        }
        let id = self.selected[0];
        let Some(row) = self.rows.iter().find(|r| r.msg.id == id) else {
            return self.store.message(id).into_iter().collect();
        };
        let s = self.settings.read();
        if s.viewing.organize_by_conversation && row.thread_count > 1 && !row.is_child {
            let mut msgs = self.store.thread_messages(row.msg.thread_id);
            if !s.viewing.include_related_messages {
                let scope: HashSet<MailboxId> = self.scope_mailboxes(&self.sel).into_iter().collect();
                if !scope.is_empty() {
                    msgs.retain(|m| scope.contains(&m.mailbox_id));
                }
            }
            // hide duplicate copies (same Message-ID in several mailboxes), prefer non-sent copy
            let mut seen = HashSet::new();
            msgs.retain(|m| m.message_id.is_empty() || seen.insert(m.message_id.clone()));
            if s.viewing.show_most_recent_on_top {
                msgs.reverse();
            }
            msgs
        } else {
            vec![row.msg.clone()]
        }
    }

    /// Ids affected by a command on the selection (expands collapsed conversations).
    pub fn selection_ids(&self) -> Vec<MessageId> {
        let threaded = self.settings.read().viewing.organize_by_conversation;
        let mut out = Vec::new();
        for id in &self.selected {
            if let Some(row) = self.rows.iter().find(|r| r.msg.id == *id) {
                if threaded && row.thread_count > 1 && !row.is_child {
                    let scope: HashSet<MailboxId> = self.scope_mailboxes(&self.sel).into_iter().collect();
                    for m in self.store.thread_messages(row.msg.thread_id) {
                        if scope.is_empty() || scope.contains(&m.mailbox_id) {
                            out.push(m.id);
                        }
                    }
                    continue;
                }
            }
            out.push(*id);
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn selected_messages(&self) -> Vec<MessageSummary> {
        self.store.messages_by_ids(&self.selection_ids())
    }

    /// Select the row after the current selection (used after delete / move).
    fn select_next_after(&mut self, removed: &[MessageId]) {
        let Some(first) = self.rows.iter().position(|r| removed.contains(&r.msg.id) || self.selected.contains(&r.msg.id)) else { return };
        let next = self.rows.iter().skip(first).find(|r| !removed.contains(&r.msg.id) && !self.selected.contains(&r.msg.id)).or_else(|| {
            self.rows[..first].iter().rev().find(|r| !removed.contains(&r.msg.id))
        });
        self.selected = next.map(|r| vec![r.msg.id]).unwrap_or_default();
        self.anchor = self.selected.first().copied();
    }

    // ------------------------------------------------------------------ events

    fn handle_events(&mut self, ctx: &egui::Context) {
        for ev in self.engine.poll() {
            if let Some(setup) = &mut self.account_setup {
                setup.on_event(&ev);
            }
            match ev {
                Event::Activity(a) => {
                    let now = ctx.input(|i| i.time);
                    self.activities.insert(a.id, (a, now));
                }
                Event::MailboxesChanged(_) => {
                    self.list_dirty = true;
                }
                Event::MessagesChanged(_) => {
                    self.list_dirty = true;
                    self.last_updated = Some(now_ts());
                }
                Event::NewMail { account, messages } => {
                    self.list_dirty = true;
                    self.notify_new_mail(&account, &messages);
                }
                Event::BodyFetched(id) => {
                    self.viewer.invalidate(id);
                    for (_, _, v) in &mut self.message_windows {
                        v.invalidate(id);
                    }
                    self.list_dirty = true;
                }
                Event::BodyFetchFailed(_, e) => self.toast(ctx, format!("Could not download message: {e}")),
                Event::AccountStatus { account, online, error } => {
                    if online && error.is_none() {
                        self.last_updated = Some(now_ts());
                    }
                    self.account_status.insert(account, (online, error));
                }
                Event::SendSucceeded { .. } => {
                    self.list_dirty = true;
                    play_sound("message-sent-email");
                }
                Event::SendFailed { error, .. } => {
                    self.list_dirty = true;
                    self.toast(ctx, format!("Message could not be sent: {error}. It remains in the Outbox."));
                }
                Event::Doctor(r) => {
                    self.doctor.retain(|d| d.account != r.account);
                    self.doctor.push(r);
                    let n = self.settings.read().accounts.len();
                    if self.doctor.len() >= n {
                        self.doctor_running = false;
                    }
                }
                Event::TestAccountResult { .. } => {}
                Event::Log(l) => {
                    if let Some(rest) = l.strip_prefix("notify:") {
                        let (t, b) = rest.split_once('\n').unwrap_or((rest, ""));
                        desktop_notify(t, b);
                    } else if let Some(s) = l.strip_prefix("sound:") {
                        play_sound(if s == "default" { "message-new-email" } else { s });
                    }
                    self.log.push(l);
                    if self.log.len() > 2000 {
                        self.log.drain(..500);
                    }
                }
            }
        }
    }

    fn notify_new_mail(&mut self, account: &str, ids: &[MessageId]) {
        let s = self.settings.read().clone();
        use crate::core::config::NotifyFor;
        let msgs = self.store.messages_by_ids(ids);
        let inboxes: HashSet<MailboxId> = self.store.mailboxes(Some(account)).into_iter().filter(|m| m.role == MailboxRole::Inbox).map(|m| m.id).collect();
        let vips: HashSet<String> = self.store.vip_emails().into_iter().collect();
        let contacts = self.store.contact_emails();
        let muted = self.store.muted_threads();
        let wanted: Vec<&MessageSummary> = msgs
            .iter()
            .filter(|m| !muted.contains(&m.thread_id))
            .filter(|m| match s.general.notify_for {
                NotifyFor::InboxOnly => inboxes.contains(&m.mailbox_id),
                NotifyFor::VipsOnly => vips.contains(&m.from.email.to_lowercase()),
                NotifyFor::Contacts => contacts.contains(&m.from.email.to_lowercase()),
                NotifyFor::AllMailboxes => true,
                NotifyFor::None => false,
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        if !s.general.new_mail_sound.is_empty() && s.general.new_mail_sound != "none" {
            play_sound(if s.general.new_mail_sound == "default" { "message-new-email" } else { &s.general.new_mail_sound });
        }
        if wanted.len() <= 3 {
            for m in wanted {
                desktop_notify(m.from.display(), &format!("{}\n{}", m.subject, m.snippet.chars().take(120).collect::<String>()));
            }
        } else {
            desktop_notify("Purpamail", &format!("{} new messages", wanted.len()));
        }
    }

    fn check_reminders(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.last_reminder_check < 20.0 {
            return;
        }
        self.last_reminder_check = now;
        let due = self.store.list_messages(&MessageQuery { reminders_due: true, ..Default::default() });
        if due.is_empty() {
            return;
        }
        let ids: Vec<MessageId> = due.iter().map(|m| m.id).collect();
        let _ = self.store.set_remind_at(&ids, None);
        if let Ok(a) = ops::set_flags(&self.store, &ids, flags::REMINDED, flags::SEEN) {
            self.flush(a);
        }
        for m in &due {
            desktop_notify("Reminder", &format!("{} — {}", m.from.display(), m.subject));
        }
        self.list_dirty = true;
    }

    // ------------------------------------------------------------------ keyboard

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let text_focus = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let ca = Modifiers::COMMAND | Modifiers::ALT;
        let fire = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        if fire(sc(cmd, Key::N)) {
            self.actions.push(Action::Compose(ComposeInit::blank()));
        }
        if fire(sc(cs, Key::N)) {
            self.actions.push(Action::CheckMail(None));
        }
        if fire(sc(cmd, Key::Comma)) {
            self.actions.push(Action::ShowPreferences(None));
        }
        if fire(sc(ca, Key::F)) || fire(sc(cmd, Key::F)) && !text_focus {
            self.focus_search = true;
        }
        if fire(sc(ca, Key::Num0)) {
            self.show_activity = !self.show_activity;
        }
        if fire(sc(cs, Key::M)) {
            let mut s = self.settings.write();
            s.viewing.show_sidebar = !s.viewing.show_sidebar;
        }
        if fire(sc(cmd, Key::Q)) {
            self.actions.push(Action::Quit);
        }
        // favorites Ctrl+1..9
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in keys.iter().enumerate() {
            if fire(sc(cmd, *k)) {
                let fav = self.settings.read().favorites.get(i).cloned();
                if let Some(f) = fav {
                    self.actions.push(Action::SelectSidebar(favorite_sel(&f)));
                }
            }
        }
        if fire(sc(cmd, Key::Z)) && !text_focus {
            self.undo_last(ctx);
        }
        let ids = self.selection_ids();
        let has = !ids.is_empty();
        if has {
            let single = self.selected.len() == 1;
            if fire(sc(cmd, Key::R)) && single {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::Reply, selected_text: self.viewer.selected_text() }));
            }
            if fire(sc(cs, Key::R)) && single {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::ReplyAll, selected_text: self.viewer.selected_text() }));
            }
            if fire(sc(cs, Key::F)) {
                if single {
                    self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::Forward, selected_text: None }));
                } else {
                    self.actions.push(Action::Compose(ComposeInit::ForwardMany(ids.clone())));
                }
            }
            if fire(sc(cs, Key::E)) && single {
                self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::Redirect, selected_text: None }));
            }
            if fire(sc(cs, Key::U)) {
                self.actions.push(Action::ToggleRead(ids.clone()));
            }
            if fire(sc(cs, Key::L)) {
                let flagged = self.selected_messages().iter().any(|m| m.is_flagged());
                self.actions.push(Action::SetFlagColor { ids: ids.clone(), color: if flagged { None } else { Some(FlagColor::Red) } });
            }
            if fire(sc(cs, Key::J)) {
                self.actions.push(Action::MarkJunk { ids: ids.clone(), junk: true });
            }
            if fire(sc(Modifiers::COMMAND | Modifiers::CTRL, Key::A)) || fire(sc(cs, Key::A)) {
                self.actions.push(Action::Archive(ids.clone()));
            }
            if fire(sc(ca, Key::U)) && single {
                self.actions.push(Action::ShowRawSource(self.focus_message()));
            }
            if fire(sc(cmd, Key::P)) && single {
                self.actions.push(Action::Print(self.focus_message()));
            }
            if fire(sc(cs, Key::S)) && single {
                self.actions.push(Action::SaveMessageAs(self.focus_message()));
            }
            if fire(sc(cmd, Key::L)) {
                self.actions.push(Action::ApplyRules(ids.clone()));
            }
            if !text_focus && (fire(sc(Modifiers::NONE, Key::Delete)) || fire(sc(Modifiers::NONE, Key::Backspace)) || fire(sc(cmd, Key::Backspace))) {
                self.actions.push(Action::Delete(ids.clone()));
            }
            if !text_focus && fire(sc(Modifiers::NONE, Key::Enter)) && single {
                self.actions.push(Action::OpenMessageWindow(self.focus_message()));
            }
        }
        if !text_focus {
            if fire(sc(cmd, Key::A)) {
                self.selected = self.rows.iter().map(|r| r.msg.id).collect();
            }
            let down = fire(sc(Modifiers::NONE, Key::ArrowDown));
            let up = fire(sc(Modifiers::NONE, Key::ArrowUp));
            let sdown = fire(sc(Modifiers::SHIFT, Key::ArrowDown));
            let sup = fire(sc(Modifiers::SHIFT, Key::ArrowUp));
            let right = fire(sc(Modifiers::NONE, Key::ArrowRight));
            let left = fire(sc(Modifiers::NONE, Key::ArrowLeft));
            if down || up || sdown || sup {
                self.move_selection(if down || sdown { 1 } else { -1 }, sdown || sup);
            }
            if right || left {
                if let Some(id) = self.selected.first() {
                    if let Some(r) = self.rows.iter().find(|r| r.msg.id == *id) {
                        if right {
                            self.expanded_threads.insert(r.msg.thread_id);
                        } else {
                            self.expanded_threads.remove(&r.msg.thread_id);
                        }
                        self.list_dirty = true;
                    }
                }
            }
            if fire(sc(Modifiers::NONE, Key::Escape)) && self.search_active {
                self.search_active = false;
                self.search_text.clear();
                self.list_dirty = true;
            }
        }
    }

    fn move_selection(&mut self, delta: i32, extend: bool) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.selected.last().and_then(|id| self.rows.iter().position(|r| r.msg.id == *id));
        let next = match cur {
            None => 0,
            Some(i) => (i as i32 + delta).clamp(0, self.rows.len() as i32 - 1) as usize,
        };
        let id = self.rows[next].msg.id;
        if extend {
            if !self.selected.contains(&id) {
                self.selected.push(id);
            }
        } else {
            self.selected = vec![id];
            self.anchor = Some(id);
        }
        self.scroll_to_selected = true;
        self.on_selection_changed();
    }

    /// The message an action like "Reply" applies to.
    pub fn focus_message(&self) -> MessageId {
        let id = self.selected.first().copied().unwrap_or(0);
        // For a collapsed conversation, reply to the newest message.
        if let Some(r) = self.rows.iter().find(|r| r.msg.id == id) {
            if r.thread_count > 1 && !r.is_child {
                let thread = self.store.thread_messages(r.msg.thread_id);
                let mine: HashSet<String> = self.settings.read().my_addresses().into_iter().collect();
                if let Some(m) = thread.iter().rev().find(|m| !mine.contains(&m.from.email.to_lowercase())).or(thread.last()) {
                    return m.id;
                }
            }
        }
        id
    }

    pub fn on_selection_changed(&mut self) {
        if self.selected.len() != 1 {
            return;
        }
        let msgs = self.viewer_messages();
        let mark_all = self.settings.read().viewing.mark_all_read_when_opening_conversation;
        let mut to_mark = Vec::new();
        for m in &msgs {
            if !m.has(flags::HAS_BODY) {
                self.engine.send(Command::FetchBody(m.id));
            }
            if m.is_unread() && (mark_all || m.id == self.selected[0] || msgs.len() == 1) {
                to_mark.push(m.id);
            }
        }
        if !to_mark.is_empty() {
            if let Ok(a) = ops::set_flags(&self.store, &to_mark, flags::SEEN, flags::REMINDED) {
                self.flush(a);
            }
            for r in &mut self.rows {
                if to_mark.contains(&r.msg.id) {
                    r.msg.flags |= flags::SEEN;
                    r.thread_unread = false;
                }
            }
            self.counts_dirty = true;
        }
    }

    // ------------------------------------------------------------------ undo

    fn record_undo(&mut self, label: &str, ids: &[MessageId], moves: bool) {
        let msgs = self.store.messages_by_ids(ids);
        let entry = UndoEntry {
            label: label.to_string(),
            moves: if moves { msgs.iter().map(|m| (m.id, m.mailbox_id)).collect() } else { vec![] },
            flags: msgs.iter().map(|m| (m.id, m.flags, m.color)).collect(),
        };
        self.undo.push(entry);
        if self.undo.len() > 50 {
            self.undo.remove(0);
        }
    }

    pub fn undo_last(&mut self, ctx: &egui::Context) {
        let Some(u) = self.undo.pop() else { return };
        let mut by_dest: BTreeMap<MailboxId, Vec<MessageId>> = BTreeMap::new();
        for (id, mb) in &u.moves {
            if self.store.message(*id).is_some_and(|m| m.mailbox_id != *mb) {
                by_dest.entry(*mb).or_default().push(*id);
            }
        }
        for (dest, ids) in by_dest {
            if let Ok(a) = ops::move_to(&self.store, &ids, dest) {
                self.flush(a);
            }
        }
        for (id, f, c) in &u.flags {
            if let Some(m) = self.store.message(*id) {
                let add = f & !m.flags;
                let remove = m.flags & !f & !flags::HAS_BODY;
                if add != 0 || remove != 0 {
                    if let Ok(a) = ops::set_flags(&self.store, &[*id], add & !flags::HAS_BODY, remove) {
                        self.flush(a);
                    }
                }
                if m.color != *c && (f & flags::FLAGGED != 0) {
                    let _ = ops::set_flag_color(&self.store, &[*id], *c);
                }
            }
        }
        self.list_dirty = true;
        self.toast(ctx, format!("Undid {}", u.label));
    }

    // ------------------------------------------------------------------ actions

    fn process_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            if let Err(e) = self.process_action(ctx, a) {
                self.toast(ctx, format!("{e}"));
            }
        }
    }

    fn process_action(&mut self, ctx: &egui::Context, a: Action) -> anyhow::Result<()> {
        match a {
            Action::SelectSidebar(sel) => {
                if let SidebarSel::Mailbox(id) = &sel {
                    self.engine.send(Command::SyncMailbox(*id));
                }
                if sel != SidebarSel::Search {
                    self.search_active = false;
                }
                self.sel = sel;
                self.selected.clear();
                self.filter.category = None;
                self.list_dirty = true;
                let _ = self.store.kv_set("ui:sidebar_sel", &serde_json::to_string(&self.sel).unwrap_or_default());
            }
            Action::SelectMessages(ids) => {
                self.selected = ids;
                self.anchor = self.selected.first().copied();
                self.on_selection_changed();
            }
            Action::OpenMessageWindow(id) => {
                let wid = self.next_id();
                self.message_windows.push((wid, id, MessageViewer::new()));
                if let Some(m) = self.store.message(id) {
                    if !m.has(flags::HAS_BODY) {
                        self.engine.send(Command::FetchBody(id));
                    }
                }
            }
            Action::ShowRawSource(id) => {
                if !self.raw_windows.contains(&id) {
                    self.raw_windows.push(id);
                }
            }
            Action::Search(q) => {
                self.search_text = q;
                self.search_active = !self.search_text.trim().is_empty();
                self.list_dirty = true;
            }
            Action::Compose(init) => {
                let id = self.next_id();
                let (mut cx, _) = self.cx();
                let w = ComposeWindow::new(&mut cx, init, id);
                self.composers.push(w);
            }
            Action::Send { message, send_at, draft } => {
                let delay = self.settings.read().general.undo_send_delay_secs as i64;
                let at = send_at.or(if delay > 0 { Some(now_ts() + delay) } else { None });
                let outbox_id = ops::queue_outgoing(&self.store, &message, at)?;
                if let Some(d) = draft {
                    let a = ops::erase_messages(&self.store, &[d])?;
                    self.flush(a);
                }
                if send_at.is_some() {
                    self.toast(ctx, format!("Message scheduled for {}", widgets::format_full_date(send_at.unwrap_or(0))));
                } else if delay > 0 {
                    let now = ctx.input(|i| i.time);
                    self.pending_send = Some(PendingSend { outbox_id, message: (*message).clone(), deadline: now + delay as f64 });
                } else {
                    self.engine.send(Command::Send { outbox_id, message });
                }
                self.list_dirty = true;
            }
            Action::UndoSend(outbox_id) => {
                if let Some(p) = self.pending_send.take() {
                    if p.outbox_id == outbox_id {
                        self.store.delete_messages(&[outbox_id])?;
                        self.actions.push(Action::Compose(ComposeInit::Outgoing(Box::new(p.message))));
                    }
                } else if let Some(raw) = self.store.raw(outbox_id) {
                    // scheduled message: reopen from its source
                    if let Some(p) = crate::core::mime::parse(&raw) {
                        let msg = OutgoingMessage {
                            account_id: self.settings.read().account_for_address(&p.from.email).map(|a| a.id.clone()).unwrap_or_default(),
                            from: p.from.clone(),
                            to: p.to.clone(),
                            cc: p.cc.clone(),
                            bcc: p.bcc.clone(),
                            subject: p.subject.clone(),
                            text: p.text.clone().unwrap_or_default(),
                            html: p.html.clone(),
                            in_reply_to: p.in_reply_to.clone(),
                            references: p.references.clone(),
                            ..Default::default()
                        };
                        self.store.delete_messages(&[outbox_id])?;
                        self.actions.push(Action::Compose(ComposeInit::Outgoing(Box::new(msg))));
                    }
                }
                self.list_dirty = true;
            }
            Action::SetFlags { ids, add, remove } => {
                self.record_undo("Mark", &ids, false);
                let a = ops::set_flags(&self.store, &ids, add, remove)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::ToggleRead(ids) => {
                self.record_undo("Mark as Read", &ids, false);
                let a = ops::toggle_read(&self.store, &ids)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::SetFlagColor { ids, color } => {
                self.record_undo("Flag", &ids, false);
                let a = ops::set_flag_color(&self.store, &ids, color)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::Move { ids, dest } => {
                self.record_undo("Move", &ids, true);
                self.select_next_after(&ids);
                let a = ops::move_to(&self.store, &ids, dest)?;
                self.flush(a);
                self.list_dirty = true;
                play_sound_if(&self.settings, "dialog-information");
            }
            Action::Copy { ids, dest } => {
                let a = ops::copy_to(&self.store, &ids, dest)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::Delete(ids) => {
                self.record_undo("Delete", &ids, true);
                self.select_next_after(&ids);
                let s = self.settings.read().clone();
                let a = ops::delete(&self.store, &s, &ids)?;
                self.flush(a);
                self.list_dirty = true;
                play_sound_if(&self.settings, "trash-empty");
            }
            Action::Archive(ids) => {
                self.record_undo("Archive", &ids, true);
                self.select_next_after(&ids);
                let s = self.settings.read().clone();
                let a = ops::archive(&self.store, &s, &ids)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::MarkJunk { ids, junk } => {
                self.record_undo(if junk { "Mark as Junk" } else { "Mark as Not Junk" }, &ids, true);
                if junk {
                    self.select_next_after(&ids);
                }
                let s = self.settings.read().clone();
                let a = ops::mark_junk(&self.store, &s, &ids, junk)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::Mute { thread, muted } => {
                self.store.set_muted(thread, muted)?;
                self.list_dirty = true;
            }
            Action::Remind { ids, at } => {
                self.store.set_remind_at(&ids, at)?;
                if let Some(t) = at {
                    self.toast(ctx, format!("You'll be reminded {}", widgets::format_full_date(t)));
                }
                self.list_dirty = true;
            }
            Action::Unsubscribe(id) => self.unsubscribe(ctx, id),
            Action::BlockSender { email, blocked } => {
                self.store.set_blocked(&email, blocked)?;
                self.toast(ctx, if blocked { format!("Blocked {email}. Future messages go to the Trash.") } else { format!("Unblocked {email}") });
            }
            Action::SetVip { address, vip } => {
                self.store.set_vip(&address, vip)?;
                self.list_dirty = true;
            }
            Action::AddToContacts(address) => {
                if self.store.contact_by_email(&address.email).is_none() {
                    let mut parts = address.name.splitn(2, ' ');
                    let c = crate::core::store::Contact {
                        first_name: parts.next().unwrap_or("").to_string(),
                        last_name: parts.next().unwrap_or("").to_string(),
                        emails: vec![address.email.clone()],
                        ..Default::default()
                    };
                    self.store.save_contact(&c)?;
                    self.toast(ctx, format!("Added {} to Contacts", address.display()));
                }
            }
            Action::ApplyRules(ids) => {
                let s = self.settings.read().clone();
                let effects = rules::apply_rules(&self.store, &s, &ids, None);
                self.run_side_effects(effects);
                self.list_dirty = true;
            }
            Action::RemoveAttachments(ids) => {
                let a = ops::remove_attachments(&self.store, &ids)?;
                self.flush(a);
                for id in ids {
                    self.viewer.invalidate(id);
                }
                self.list_dirty = true;
            }
            Action::SetCategory { id, category } => {
                self.store.set_category(id, category)?;
                self.list_dirty = true;
            }
            Action::FetchBody(id) => self.engine.send(Command::FetchBody(id)),
            Action::OpenAttachment { message, index } => {
                let path = self.write_attachment(message, index, true)?;
                widgets::open_external(&path.display().to_string());
            }
            Action::SaveAttachment { message, index } => {
                let path = self.write_attachment(message, index, false)?;
                self.toast(ctx, format!("Saved to {}", path.display()));
            }
            Action::SaveAllAttachments(message) => {
                let raw = self.store.raw(message).ok_or_else(|| anyhow::anyhow!("Message not downloaded yet"))?;
                let p = crate::core::mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("Could not parse message"))?;
                let n = p.real_attachments().count();
                if let Some(dir) = rfd::FileDialog::new().set_title("Save Attachments").pick_folder() {
                    for a in p.real_attachments() {
                        std::fs::write(unique_path(&dir, &a.filename), &a.data)?;
                    }
                    self.toast(ctx, format!("Saved {n} attachments"));
                }
            }
            Action::SaveMessageAs(id) => {
                let raw = self.store.raw(id).ok_or_else(|| anyhow::anyhow!("Message not downloaded yet"))?;
                let m = self.store.message(id);
                let name = sanitize_filename(&m.map(|m| m.subject).unwrap_or_else(|| "Message".into()));
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{name}.eml"))
                    .add_filter("Raw Message Source", &["eml"])
                    .add_filter("Plain Text", &["txt"])
                    .add_filter("PDF", &["pdf"])
                    .save_file()
                {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("eml").to_lowercase();
                    match ext.as_str() {
                        "txt" => {
                            let p = crate::core::mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("parse error"))?;
                            std::fs::write(&path, message_as_text(&p))?
                        }
                        "pdf" => std::fs::write(&path, self.message_pdf(id)?)?,
                        _ => std::fs::write(&path, &raw)?,
                    }
                }
            }
            Action::ExportPdf(id) => {
                let pdf = self.message_pdf(id)?;
                let name = sanitize_filename(&self.store.message(id).map(|m| m.subject).unwrap_or_default());
                if let Some(path) = rfd::FileDialog::new().set_file_name(format!("{name}.pdf")).add_filter("PDF", &["pdf"]).save_file() {
                    std::fs::write(path, pdf)?;
                }
            }
            Action::Print(id) => {
                let pdf = self.message_pdf(id)?;
                let path = std::env::temp_dir().join(format!("purpamail-print-{id}.pdf"));
                std::fs::write(&path, pdf)?;
                // Use the system print spooler when available, otherwise open in the PDF viewer.
                let printed = std::process::Command::new("lp").arg(&path).status().map(|s| s.success()).unwrap_or(false);
                if printed {
                    self.toast(ctx, "Sent to the default printer");
                } else {
                    widgets::open_external(&path.display().to_string());
                }
            }
            Action::OpenUrl(url) => {
                if let Some(rest) = url.strip_prefix("mailto:") {
                    let _ = rest;
                    self.actions.push(Action::Compose(ComposeInit::Mailto(url.clone())));
                } else {
                    widgets::open_external(&url);
                }
            }
            Action::Rsvp { message, response } => self.rsvp(ctx, message, response)?,
            Action::CheckMail(a) => {
                self.engine.send(Command::CheckMail(a));
                self.engine.send(Command::FlushOutbox);
            }
            Action::Synchronize(a) => self.engine.send(Command::Synchronize(a)),
            Action::NewMailbox { parent, account } => {
                self.prompt = Some(Prompt { title: "New Mailbox".into(), label: "Name:".into(), text: String::new(), kind: PromptKind::NewMailbox { account, parent } });
            }
            Action::RenameMailbox(id) => {
                if let Some(mb) = self.store.mailbox(id) {
                    self.prompt = Some(Prompt { title: "Rename Mailbox".into(), label: "New name:".into(), text: mb.name.clone(), kind: PromptKind::RenameMailbox(id) });
                }
            }
            Action::DeleteMailbox(id) => {
                if let Some(mb) = self.store.mailbox(id) {
                    self.confirm = Some(Confirm {
                        title: format!("Delete the mailbox “{}”?", mb.name),
                        message: "All messages in this mailbox will be deleted. This cannot be undone.".into(),
                        ok: "Delete".into(),
                        action: Action::Toast(format!("__delete_mailbox:{id}")),
                    });
                }
            }
            Action::UseMailboxFor { mailbox, role } => {
                self.store.set_mailbox_role(mailbox, role)?;
                if let Some(mb) = self.store.mailbox(mailbox) {
                    let mut s = self.settings.write();
                    if let Some(acct) = s.account_mut(&mb.account_id) {
                        acct.role_overrides.retain(|_, r| *r != role);
                        acct.role_overrides.insert(mb.path.clone(), role);
                    }
                }
                self.actions.push(Action::SettingsChanged);
                self.list_dirty = true;
            }
            Action::EraseDeleted(acct) => {
                let a = ops::erase_deleted(&self.store, acct.as_deref())?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::EraseJunk => {
                let a = ops::erase_junk(&self.store, None)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::MarkAllRead(mbs) => {
                let ids: Vec<MessageId> = self.store.list_messages(&MessageQuery { mailboxes: mbs, only_unread: true, ..Default::default() }).iter().map(|m| m.id).collect();
                let a = ops::set_flags(&self.store, &ids, flags::SEEN, 0)?;
                self.flush(a);
                self.list_dirty = true;
            }
            Action::ImportMailboxes => self.import_mailboxes(ctx)?,
            Action::ExportMailbox(id) => {
                let mb = self.store.mailbox(id).ok_or_else(|| anyhow::anyhow!("no mailbox"))?;
                if let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.mbox", sanitize_filename(&mb.name))).save_file() {
                    let raws: Vec<Vec<u8>> = self.store.ids_in_mailbox(id).into_iter().filter_map(|i| self.store.raw(i)).collect();
                    let n = raws.len();
                    crate::core::mbox::export(&path, &raws)?;
                    self.toast(ctx, format!("Exported {n} messages"));
                }
            }
            Action::RebuildMailbox(id) => {
                self.store.clear_mailbox(id)?;
                self.engine.send(Command::SyncMailbox(id));
                self.list_dirty = true;
            }
            Action::NewSmartMailbox(sm) => {
                let is_new = true;
                self.smart_dialog = Some(SmartMailboxDialog::new(sm.unwrap_or_default(), is_new));
            }
            Action::EditSmartMailbox(id) => {
                if let Some(sm) = self.settings.read().smart_mailboxes.iter().find(|m| m.id == id).cloned() {
                    self.smart_dialog = Some(SmartMailboxDialog::new(sm, false));
                }
            }
            Action::DeleteSmartMailbox(id) => {
                {
                    let mut s = self.settings.write();
                    s.smart_mailboxes.retain(|m| m.id != id);
                    s.favorites.retain(|f| f != &FavoriteItem::Smart(id.clone()));
                }
                if self.sel == SidebarSel::Smart(id) {
                    self.sel = SidebarSel::AllRole(MailboxRole::Inbox);
                }
                self.actions.push(Action::SettingsChanged);
                self.list_dirty = true;
            }
            Action::AddFavorite(f) => {
                let mut s = self.settings.write();
                if !s.favorites.contains(&f) {
                    s.favorites.push(f);
                }
                drop(s);
                self.actions.push(Action::SettingsChanged);
            }
            Action::RemoveFavorite(f) => {
                self.settings.write().favorites.retain(|x| x != &f);
                self.actions.push(Action::SettingsChanged);
            }
            Action::SetOnline { account, online } => {
                if account.is_none() {
                    self.settings.write().work_offline = !online;
                    self.actions.push(Action::SettingsChanged);
                }
                self.engine.send(Command::SetOnline { account, online });
            }
            Action::EraseOlderThan { .. } => self.engine.send(Command::Maintenance),
            Action::ShowPreferences(tab) => {
                self.prefs.open = true;
                if let Some(t) = tab {
                    self.prefs.tab = t;
                }
            }
            Action::ShowActivity => self.show_activity = true,
            Action::ShowConnectionDoctor => {
                self.show_doctor = true;
                self.doctor.clear();
                self.doctor_running = true;
                self.engine.send(Command::ConnectionDoctor);
            }
            Action::ShowContacts => {
                self.contacts_win.open = true;
                self.contacts_win.tab = ContactsTab::Contacts;
            }
            Action::ShowPreviousRecipients => {
                self.contacts_win.open = true;
                self.contacts_win.tab = ContactsTab::PreviousRecipients;
            }
            Action::AddAccount => self.account_setup = Some(AccountSetup::new()),
            Action::ShowAbout => self.show_about = true,
            Action::Toast(t) => {
                if let Some(id) = t.strip_prefix("__delete_mailbox:").and_then(|s| s.parse::<MailboxId>().ok()) {
                    if let Some(mb) = self.store.mailbox(id) {
                        self.store.delete_mailbox(id)?;
                        self.store.push_op(&mb.account_id, &crate::core::store::RemoteOp::DeleteMailbox { path: mb.path.clone() })?;
                        self.engine.send(Command::FlushOps(mb.account_id.clone()));
                        if self.sel == SidebarSel::Mailbox(id) {
                            self.sel = SidebarSel::AllRole(MailboxRole::Inbox);
                        }
                        self.list_dirty = true;
                    }
                } else {
                    self.toast(ctx, t);
                }
            }
            Action::SettingsChanged => {
                let s = self.settings.read().clone();
                if let Err(e) = s.save() {
                    self.toast(ctx, format!("Could not save settings: {e}"));
                }
                let _ = self.secrets.read().save();
                if s.fonts != self.fonts_applied {
                    super::fonts::install(ctx, &s.fonts);
                    self.fonts_applied = s.fonts.clone();
                }
                if self.theme_applied != Some((s.viewing.theme, s.viewing.ui_scale)) {
                    super::theme::apply(ctx, s.viewing.theme, s.viewing.ui_scale);
                    self.theme_applied = Some((s.viewing.theme, s.viewing.ui_scale));
                }
                self.engine.send(Command::ReloadSettings);
                self.list_dirty = true;
            }
            Action::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        Ok(())
    }

    pub fn run_side_effects(&mut self, effects: Vec<rules::SideEffect>) {
        for e in effects {
            match e {
                rules::SideEffect::Notify { title, body } => desktop_notify(&title, &body),
                rules::SideEffect::PlaySound(s) => play_sound(if s == "default" { "message-new-email" } else { &s }),
                rules::SideEffect::Send(m) => {
                    if let Ok(id) = ops::queue_outgoing(&self.store, &m, None) {
                        self.engine.send(Command::Send { outbox_id: id, message: m });
                    }
                }
                rules::SideEffect::RunCommand { command, message } => {
                    if let Some(raw) = self.store.raw(message) {
                        let path = std::env::temp_dir().join(format!("purpamail-rule-{message}.eml"));
                        if std::fs::write(&path, raw).is_ok() {
                            let _ = std::process::Command::new("sh").arg("-c").arg(&command).arg("purpamail-rule").arg(&path).spawn();
                        }
                    }
                }
                rules::SideEffect::Changed => {
                    self.list_dirty = true;
                    for a in self.settings.read().accounts.iter() {
                        self.engine.send(Command::FlushOps(a.id.clone()));
                    }
                }
            }
        }
    }

    fn write_attachment(&mut self, message: MessageId, index: usize, temp: bool) -> anyhow::Result<std::path::PathBuf> {
        let raw = self.store.raw(message).ok_or_else(|| anyhow::anyhow!("Message not downloaded yet"))?;
        let p = crate::core::mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("Could not parse message"))?;
        let a = p.attachments.get(index).ok_or_else(|| anyhow::anyhow!("No such attachment"))?;
        let path = if temp {
            let dir = std::env::temp_dir().join(format!("purpamail-{message}"));
            std::fs::create_dir_all(&dir)?;
            dir.join(sanitize_filename(&a.filename))
        } else {
            let dl = self.settings.read().general.downloads_folder.clone();
            let default_dir = std::path::PathBuf::from(dl);
            match rfd::FileDialog::new().set_directory(&default_dir).set_file_name(&a.filename).save_file() {
                Some(p) => p,
                None => anyhow::bail!("Cancelled"),
            }
        };
        std::fs::write(&path, &a.data)?;
        Ok(path)
    }

    pub fn message_pdf(&self, id: MessageId) -> anyhow::Result<Vec<u8>> {
        use crate::core::pdf::{PdfLine, render};
        let raw = self.store.raw(id).ok_or_else(|| anyhow::anyhow!("Message not downloaded yet"))?;
        let p = crate::core::mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("Could not parse message"))?;
        let line = |t: &str, bold: bool, size: f32, gray: f32| PdfLine { text: t.to_string(), bold, size, gray, indent: 0.0 };
        let mut lines = vec![line(&p.subject, true, 16.0, 0.0), line("", false, 6.0, 0.0)];
        lines.push(line(&format!("From: {}", p.from.to_header()), false, 10.0, 0.3));
        if !p.to.is_empty() {
            lines.push(line(&format!("To: {}", format_address_list(&p.to)), false, 10.0, 0.3));
        }
        if !p.cc.is_empty() {
            lines.push(line(&format!("Cc: {}", format_address_list(&p.cc)), false, 10.0, 0.3));
        }
        lines.push(line(&format!("Date: {}", widgets::format_full_date(p.date)), false, 10.0, 0.3));
        lines.push(line("", false, 10.0, 0.0));
        for l in p.plain_text().lines() {
            let depth = l.chars().take_while(|c| *c == '>' || *c == ' ').filter(|c| *c == '>').count();
            lines.push(PdfLine { text: l.to_string(), bold: false, size: 11.0, gray: if depth > 0 { 0.4 } else { 0.0 }, indent: 0.0 });
        }
        let atts: Vec<String> = p.real_attachments().map(|a| format!("{} ({})", a.filename, crate::core::mime::human_size(a.size() as u64))).collect();
        if !atts.is_empty() {
            lines.push(line("", false, 10.0, 0.0));
            lines.push(line("Attachments:", true, 10.0, 0.2));
            for a in atts {
                lines.push(line(&format!("• {a}"), false, 10.0, 0.2));
            }
        }
        Ok(render(&p.subject, &lines))
    }

    fn unsubscribe(&mut self, ctx: &egui::Context, id: MessageId) {
        let Some(m) = self.store.message(id) else { return };
        let lu = m.list_unsubscribe.clone();
        let targets: Vec<String> = lu.split(',').map(|s| s.trim().trim_start_matches('<').trim_end_matches('>').to_string()).collect();
        if let Some(mailto) = targets.iter().find(|t| t.starts_with("mailto:")) {
            // Send the unsubscribe request directly.
            let s = self.settings.read().clone();
            let acct = s.accounts.iter().find(|a| a.id == m.account_id).or(s.accounts.first());
            if let Some(acct) = acct {
                let (to, subject, body) = parse_mailto(mailto);
                let msg = OutgoingMessage {
                    account_id: acct.id.clone(),
                    from: Address::new(acct.full_name.clone(), acct.primary_email()),
                    to,
                    subject: if subject.is_empty() { "unsubscribe".into() } else { subject },
                    text: if body.is_empty() { "unsubscribe".into() } else { body },
                    message_id: crate::core::compose::new_message_id(&Address::new("", acct.primary_email())),
                    ..Default::default()
                };
                if let Ok(oid) = ops::queue_outgoing(&self.store, &msg, None) {
                    self.engine.send(Command::Send { outbox_id: oid, message: Box::new(msg) });
                    self.toast(ctx, "Unsubscribe request sent");
                }
                return;
            }
        }
        if let Some(url) = targets.iter().find(|t| t.starts_with("http")) {
            widgets::open_external(url);
        } else {
            self.toast(ctx, "This message has no unsubscribe information");
        }
    }

    fn rsvp(&mut self, ctx: &egui::Context, message: MessageId, response: crate::core::ics::Rsvp) -> anyhow::Result<()> {
        let raw = self.store.raw(message).ok_or_else(|| anyhow::anyhow!("Message not downloaded"))?;
        let p = crate::core::mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("parse error"))?;
        let cal = p.calendar.clone().ok_or_else(|| anyhow::anyhow!("No invitation in this message"))?;
        let ev = crate::core::ics::parse(&cal).ok_or_else(|| anyhow::anyhow!("Invalid invitation"))?;
        let s = self.settings.read().clone();
        let msg_sum = self.store.message(message);
        let acct = msg_sum.as_ref().and_then(|m| s.account(&m.account_id)).or(s.accounts.first()).ok_or_else(|| anyhow::anyhow!("No account to reply from"))?;
        let me = Address::new(acct.full_name.clone(), acct.primary_email());
        let reply_ics = crate::core::ics::build_reply(&ev, &me.email, response);
        let organizer = Address::parse_one(ev.organizer.trim_start_matches("mailto:")).unwrap_or_else(|| p.from.clone());
        let verb = match response {
            crate::core::ics::Rsvp::Accept => "Accepted",
            crate::core::ics::Rsvp::Tentative => "Tentative",
            crate::core::ics::Rsvp::Decline => "Declined",
        };
        let msg = OutgoingMessage {
            account_id: acct.id.clone(),
            from: me.clone(),
            to: vec![organizer],
            subject: format!("{verb}: {}", ev.summary),
            text: format!("{} has {} this invitation.", me.display(), verb.to_lowercase()),
            attachments: vec![crate::core::compose::OutgoingAttachment {
                filename: "reply.ics".into(),
                mime_type: "text/calendar; method=REPLY".into(),
                data: reply_ics.into_bytes(),
                content_id: None,
            }],
            in_reply_to: p.message_id.clone(),
            message_id: crate::core::compose::new_message_id(&me),
            ..Default::default()
        };
        let oid = ops::queue_outgoing(&self.store, &msg, None)?;
        self.engine.send(Command::Send { outbox_id: oid, message: Box::new(msg) });
        self.toast(ctx, format!("{verb} “{}”", ev.summary));
        Ok(())
    }

    fn import_mailboxes(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        let Some(path) = rfd::FileDialog::new().set_title("Import Mailboxes — choose an mbox file").pick_file().or_else(|| rfd::FileDialog::new().set_title("…or a folder").pick_folder()) else {
            return Ok(());
        };
        let tree = if path.is_dir() {
            crate::core::mbox::import_mailbox_tree(&path)
        } else {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported").to_string();
            vec![(name, crate::core::mbox::import(&path)?)]
        };
        let mut total = 0;
        for (name, raws) in tree {
            let mb = self.store.ensure_mailbox(LOCAL_ACCOUNT, &format!("Import/{name}"), MailboxRole::Custom)?;
            total += ops::import_raw(&self.store, mb, &raws)?.len();
        }
        let _ = self.store.ensure_mailbox(LOCAL_ACCOUNT, "Import", MailboxRole::Custom);
        self.fix_local_parents();
        self.toast(ctx, format!("Imported {total} messages into “On This Computer › Import”"));
        self.list_dirty = true;
        Ok(())
    }

    /// Set parent links of local mailboxes from their paths.
    pub fn fix_local_parents(&self) {
        let mbs = self.store.mailboxes(Some(LOCAL_ACCOUNT));
        for mb in &mbs {
            if let Some((parent_path, name)) = mb.path.rsplit_once('/') {
                if let Some(parent) = mbs.iter().find(|m| m.path == parent_path) {
                    if mb.parent != Some(parent.id) || mb.name != name {
                        let mut m = mb.clone();
                        m.parent = Some(parent.id);
                        m.name = name.to_string();
                        let _ = self.store.upsert_mailbox(&m);
                    }
                }
            }
        }
    }

    pub(crate) fn finish_prompt_internal(&mut self, ctx: &egui::Context, p: Prompt) -> anyhow::Result<()> {
        let text = p.text.trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        match p.kind {
            PromptKind::NewMailbox { account, parent } => {
                let parent_mb = parent.and_then(|id| self.store.mailbox(id));
                let delim = parent_mb.as_ref().map(|m| m.delimiter.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| "/".into());
                let path = match &parent_mb {
                    Some(pm) => format!("{}{}{}", pm.path, delim, text),
                    None => {
                        let prefix = self.settings.read().account(&account).map(|a| a.imap_path_prefix.clone()).unwrap_or_default();
                        if prefix.is_empty() { text.clone() } else { format!("{prefix}{delim}{text}") }
                    }
                };
                let id = self.store.upsert_mailbox(&Mailbox {
                    account_id: account.clone(),
                    path: path.clone(),
                    name: text.clone(),
                    parent,
                    delimiter: delim,
                    selectable: true,
                    subscribed: true,
                    ..Default::default()
                })?;
                if account != LOCAL_ACCOUNT {
                    self.store.push_op(&account, &crate::core::store::RemoteOp::CreateMailbox { path })?;
                    self.engine.send(Command::FlushOps(account));
                }
                self.actions.push(Action::SelectSidebar(SidebarSel::Mailbox(id)));
            }
            PromptKind::RenameMailbox(id) => {
                if let Some(mb) = self.store.mailbox(id) {
                    let new_path = match mb.path.rsplit_once(mb.delimiter.as_str()) {
                        Some((parent, _)) if !mb.delimiter.is_empty() => format!("{parent}{}{text}", mb.delimiter),
                        _ => text.clone(),
                    };
                    self.store.rename_mailbox(id, &new_path, &text)?;
                    if mb.account_id != LOCAL_ACCOUNT {
                        self.store.push_op(&mb.account_id, &crate::core::store::RemoteOp::RenameMailbox { from: mb.path.clone(), to: new_path })?;
                        self.engine.send(Command::FlushOps(mb.account_id.clone()));
                    }
                }
            }
            PromptKind::RemindCustom(ids) => {
                let at = crate::core::datadetect::detect(&text).into_iter().find_map(|d| match d.kind {
                    crate::core::datadetect::DetectedKind::Date(Some(t)) => Some(t),
                    _ => None,
                });
                match at {
                    Some(t) => self.actions.push(Action::Remind { ids, at: Some(t) }),
                    None => self.toast(ctx, "Could not understand that date. Try e.g. “tomorrow at 9:00” or “2026-12-24 18:00”."),
                }
            }
        }
        self.list_dirty = true;
        Ok(())
    }
}

// ---------------------------------------------------------------------- helpers

pub fn favorite_sel(f: &FavoriteItem) -> SidebarSel {
    match f {
        FavoriteItem::Mailbox(id) => SidebarSel::Mailbox(*id),
        FavoriteItem::AllInboxes => SidebarSel::AllRole(MailboxRole::Inbox),
        FavoriteItem::AllFlagged => SidebarSel::Flagged(None),
        FavoriteItem::AllDrafts => SidebarSel::AllRole(MailboxRole::Drafts),
        FavoriteItem::AllSent => SidebarSel::AllRole(MailboxRole::Sent),
        FavoriteItem::Vips => SidebarSel::Vip(None),
        FavoriteItem::Smart(id) => SidebarSel::Smart(id.clone()),
    }
}

pub fn sanitize_filename(s: &str) -> String {
    let t: String = s.chars().map(|c| if "/\\:*?\"<>|\0".contains(c) || c.is_control() { '_' } else { c }).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() { "Untitled".into() } else { t.chars().take(120).collect() }
}

fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let name = sanitize_filename(name);
    let mut p = dir.join(&name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{stem} {i}{ext}"));
        i += 1;
    }
    p
}

pub fn message_as_text(p: &crate::core::mime::ParsedMessage) -> String {
    let mut s = format!("From: {}\nSubject: {}\nDate: {}\n", p.from.to_header(), p.subject, widgets::format_full_date(p.date));
    if !p.to.is_empty() {
        s.push_str(&format!("To: {}\n", format_address_list(&p.to)));
    }
    if !p.cc.is_empty() {
        s.push_str(&format!("Cc: {}\n", format_address_list(&p.cc)));
    }
    s.push('\n');
    s.push_str(&p.plain_text());
    s
}

/// Parse a mailto: URL into recipients, subject and body.
pub fn parse_mailto(url: &str) -> (Vec<Address>, String, String) {
    let rest = url.trim_start_matches("mailto:");
    let (addr, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut to = Address::parse_list(&percent_decode(addr));
    let mut subject = String::new();
    let mut body = String::new();
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            let v = percent_decode(v);
            match k.to_ascii_lowercase().as_str() {
                "subject" => subject = v,
                "body" => body = v,
                "to" => to.extend(Address::parse_list(&v)),
                _ => {}
            }
        }
    }
    (to, subject, body)
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 0 && i + 2 <= bytes.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Show a desktop notification through the freedesktop notification service.
pub fn desktop_notify(title: &str, body: &str) {
    let t = title.to_string();
    let b = body.to_string();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary(&t).body(&b).appname("Purpamail").icon("mail-unread").show();
    });
}

/// Play a sound from the freedesktop sound theme (by event name) or a file path.
pub fn play_sound(name: &str) {
    let n = name.to_string();
    std::thread::spawn(move || {
        let path = if n.contains('/') {
            std::path::PathBuf::from(&n)
        } else {
            let mut found = None;
            for dir in ["/usr/share/sounds/freedesktop/stereo", "/usr/share/sounds/gnome/default/alerts", "/usr/share/sounds"] {
                for ext in ["oga", "ogg", "wav"] {
                    let p = std::path::Path::new(dir).join(format!("{n}.{ext}"));
                    if p.exists() {
                        found = Some(p);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            match found {
                Some(p) => p,
                None => {
                    let _ = std::process::Command::new("canberra-gtk-play").arg("-i").arg(&n).stderr(std::process::Stdio::null()).status();
                    return;
                }
            }
        };
        for player in ["pw-play", "paplay", "aplay"] {
            if std::process::Command::new(player).arg(&path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false) {
                return;
            }
        }
    });
}

fn play_sound_if(settings: &SharedSettings, name: &str) {
    if settings.read().general.play_sounds_for_other_actions {
        play_sound(name);
    }
}

// ---------------------------------------------------------------------- eframe

impl eframe::App for PurpamailApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        while let Ok(r) = self.ipc_rx.try_recv() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if r != "focus" {
                self.handle_request(&r);
            }
        }
        self.check_reminders(&ctx);
        self.handle_dropped_files(&ctx);
        self.shortcuts(&ctx);
        if self.list_dirty {
            self.refresh_list();
        } else if self.counts_dirty {
            self.refresh_caches();
        }

        // pending undo-send
        if let Some(p) = &self.pending_send {
            let now = ctx.input(|i| i.time);
            if now >= p.deadline {
                let p = self.pending_send.take().unwrap();
                let _ = self.store.set_send_at(p.outbox_id, None);
                self.engine.send(Command::Send { outbox_id: p.outbox_id, message: Box::new(p.message) });
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }

        // window title with unread count
        let unread = self.store.unread_count(&self.mailboxes.iter().filter(|m| m.role == MailboxRole::Inbox).map(|m| m.id).collect::<Vec<_>>());
        let title = if self.settings.read().general.show_unread_in_title && unread > 0 { format!("Purpamail — Inbox ({unread} unread)") } else { "Purpamail".to_string() };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let pal = Palette::of(&ctx);
        self.menu_bar(ui);
        self.toolbar(ui);
        let show_fav = self.settings.read().viewing.show_favorites_bar;
        if show_fav {
            self.favorites_bar(ui);
        }
        self.status_bar(ui);
        let show_sidebar = self.settings.read().viewing.show_sidebar;
        if show_sidebar {
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(230.0)
                .min_size(160.0)
                .max_size(420.0)
                .frame(egui::Frame::new().fill(pal.sidebar_bg).inner_margin(egui::Margin { left: 6, right: 6, top: 6, bottom: 6 }))
                .show(ui, |ui| self.sidebar(ui));
        }
        let layout = self.settings.read().viewing.layout;
        match layout {
            ListLayout::Columns => {
                egui::Panel::left("message_list")
                    .resizable(true)
                    .default_size(360.0)
                    .min_size(240.0)
                    .max_size(800.0)
                    .frame(egui::Frame::new().fill(pal.list_bg))
                    .show(ui, |ui| self.message_list(ui));
            }
            ListLayout::Classic => {
                egui::Panel::top("message_table")
                    .resizable(true)
                    .default_size(300.0)
                    .min_size(120.0)
                    .frame(egui::Frame::new().fill(pal.list_bg))
                    .show(ui, |ui| self.message_table(ui));
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.viewer_bg)).show(ui, |ui| self.viewer_pane(ui));

        self.windows(&ctx);
        self.toasts_ui(&ctx);
        self.process_actions(&ctx);

        let now = ctx.input(|i| i.time);
        if now - self.last_save_state > 30.0 {
            self.last_save_state = now;
        }
        // Keep counters/timers ticking.
        ctx.request_repaint_after(std::time::Duration::from_secs(5));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let s = self.settings.read().clone();
        let _ = s.save();
        for a in &s.accounts {
            use crate::core::config::EraseAfter;
            if a.erase_deleted == EraseAfter::WhenQuitting {
                let _ = ops::erase_deleted(&self.store, Some(&a.id));
            }
            if a.erase_junk == EraseAfter::WhenQuitting {
                let _ = ops::erase_junk(&self.store, Some(&a.id));
            }
        }
        self.engine.send(Command::Shutdown);
    }
}

// ---------------------------------------------------------------------- chrome

impl PurpamailApp {
    fn viewer_pane(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let msgs = self.viewer_messages();
        if self.selected.len() > 1 {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(format!("{} messages selected", self.selected.len())).size(18.0).color(pal.text_tertiary));
            });
            return;
        }
        if msgs.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No Message Selected").size(20.0).color(pal.text_tertiary));
            });
            return;
        }
        let focus = self.selected.first().copied();
        let (mut cx, viewer) = self.cx();
        viewer.show(ui, &mut cx, &msgs, focus);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let running: Vec<Activity> = self.activities.values().filter(|(a, _)| a.state == ActivityState::Running).map(|(a, _)| a.clone()).collect();
        let offline = self.settings.read().work_offline;
        let errors: Vec<(String, String)> = {
            let s = self.settings.read();
            self.account_status
                .iter()
                .filter_map(|(id, (_, e))| e.as_ref().map(|e| (s.account(id).map(|a| a.display_name().to_string()).unwrap_or_default(), e.clone())))
                .collect()
        };
        let pending = self.pending_send.as_ref().map(|p| (p.outbox_id, p.deadline));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(10, 3)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some((oid, deadline)) = pending {
                        let left = (deadline - ui.input(|i| i.time)).max(0.0);
                        ui.label(RichText::new(format!("{} Sending in {:.0} s", ic::PAPER_PLANE_TILT, left.ceil())).color(pal.text_secondary));
                        if ui.link("Undo Send").clicked() {
                            self.actions.push(Action::UndoSend(oid));
                        }
                        ui.separator();
                    }
                    if offline {
                        ui.label(RichText::new(format!("{} Offline", ic::WIFI_SLASH)).color(pal.text_secondary));
                        if ui.link("Go Online").clicked() {
                            self.actions.push(Action::SetOnline { account: None, online: true });
                        }
                    } else if let Some(a) = running.first() {
                        ui.add(egui::Spinner::new().size(12.0));
                        let mut t = a.title.clone();
                        if !a.detail.is_empty() {
                            t = format!("{t} — {}", a.detail);
                        }
                        if running.len() > 1 {
                            t = format!("{t} (+{} more)", running.len() - 1);
                        }
                        if ui.add(egui::Label::new(RichText::new(t).color(pal.text_secondary).size(12.0)).sense(egui::Sense::click())).clicked() {
                            self.show_activity = true;
                        }
                        if let Some(p) = a.progress {
                            ui.add(egui::ProgressBar::new(p).desired_width(80.0).desired_height(6.0));
                        }
                    } else if let Some((acct, e)) = errors.first() {
                        if ui.add(egui::Label::new(RichText::new(format!("{} {acct}: {e}", ic::WARNING)).color(pal.danger).size(12.0)).sense(egui::Sense::click())).clicked() {
                            self.actions.push(Action::ShowConnectionDoctor);
                        }
                    } else if let Some(t) = self.last_updated {
                        let when = if now_ts() - t < 60 { "Updated just now".to_string() } else { format!("Updated {}", widgets::format_list_date(t)) };
                        ui.label(RichText::new(when).color(pal.text_tertiary).size(12.0));
                    } else if self.settings.read().accounts.is_empty() {
                        if ui.link(RichText::new(format!("{} Add an account to get started", ic::PLUS_CIRCLE)).size(12.0)).clicked() {
                            self.actions.push(Action::AddAccount);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ops = self.store.pending_op_count();
                        if ops > 0 {
                            ui.label(RichText::new(format!("{} {ops} pending", ic::CLOUD_ARROW_UP)).color(pal.text_tertiary).size(12.0)).on_hover_text("Changes waiting to be sent to the server");
                        }
                    });
                });
            });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let items = self.settings.read().toolbar.clone();
        let has_sel = !self.selected.is_empty();
        let single = self.selected.len() == 1;
        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(8, 6)).stroke(egui::Stroke::NONE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // Title area: mailbox name + count
                    let total_flex = items.iter().filter(|i| **i == ToolbarItem::FlexibleSpace).count().max(1);
                    let fixed_width: f32 = items
                        .iter()
                        .map(|i| match i {
                            ToolbarItem::FlexibleSpace => 0.0,
                            ToolbarItem::Space => 12.0,
                            ToolbarItem::Search => 240.0,
                            ToolbarItem::Flag | ToolbarItem::MoveTo | ToolbarItem::Remind => 46.0,
                            _ => 36.0,
                        })
                        .sum::<f32>()
                        + 180.0;
                    let flex = ((ui.available_width() - fixed_width) / total_flex as f32).max(4.0);
                    let mut title_done = false;
                    for item in items {
                        match item {
                            ToolbarItem::Mailboxes => {
                                if widgets::tool_button(ui, ic::SIDEBAR_SIMPLE, "Show / Hide Sidebar", true).clicked() {
                                    let mut s = self.settings.write();
                                    s.viewing.show_sidebar = !s.viewing.show_sidebar;
                                }
                                if !title_done {
                                    title_done = true;
                                    self.toolbar_title(ui);
                                }
                            }
                            ToolbarItem::GetMail => {
                                if widgets::tool_button(ui, ic::ENVELOPE_OPEN, "Get Mail (Ctrl+Shift+N)", true).clicked() {
                                    self.actions.push(Action::CheckMail(None));
                                }
                            }
                            ToolbarItem::NewMessage => {
                                if widgets::tool_button(ui, ic::NOTE_PENCIL, "New Message (Ctrl+N)", true).clicked() {
                                    self.actions.push(Action::Compose(ComposeInit::blank()));
                                }
                            }
                            ToolbarItem::Archive => {
                                if widgets::tool_button(ui, ic::ARCHIVE, "Archive", has_sel).clicked() {
                                    self.actions.push(Action::Archive(self.selection_ids()));
                                }
                            }
                            ToolbarItem::Delete => {
                                if widgets::tool_button(ui, ic::TRASH, "Delete", has_sel).clicked() {
                                    self.actions.push(Action::Delete(self.selection_ids()));
                                }
                            }
                            ToolbarItem::Junk => {
                                let in_junk = self.selected_messages().iter().all(|m| m.has(flags::JUNK)) && has_sel;
                                if widgets::tool_button(ui, if in_junk { ic::TRAY } else { ic::PROHIBIT }, if in_junk { "Not Junk" } else { "Move to Junk" }, has_sel).clicked() {
                                    self.actions.push(Action::MarkJunk { ids: self.selection_ids(), junk: !in_junk });
                                }
                            }
                            ToolbarItem::Reply => {
                                if widgets::tool_button(ui, ic::ARROW_BEND_UP_LEFT, "Reply (Ctrl+R)", single).clicked() {
                                    self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::Reply, selected_text: self.viewer.selected_text() }));
                                }
                            }
                            ToolbarItem::ReplyAll => {
                                if widgets::tool_button(ui, ic::ARROW_BEND_DOUBLE_UP_LEFT, "Reply All (Ctrl+Shift+R)", single).clicked() {
                                    self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::ReplyAll, selected_text: self.viewer.selected_text() }));
                                }
                            }
                            ToolbarItem::Forward => {
                                if widgets::tool_button(ui, ic::ARROW_BEND_UP_RIGHT, "Forward (Ctrl+Shift+F)", has_sel).clicked() {
                                    if single {
                                        self.actions.push(Action::Compose(ComposeInit::Respond { message: self.focus_message(), kind: ReplyKind::Forward, selected_text: None }));
                                    } else {
                                        self.actions.push(Action::Compose(ComposeInit::ForwardMany(self.selection_ids())));
                                    }
                                }
                            }
                            ToolbarItem::Flag => {
                                ui.add_enabled_ui(has_sel, |ui| {
                                    let r = widgets::tool_button(ui, ic::FLAG, "Flag", has_sel);
                                    if r.clicked() {
                                        let flagged = self.selected_messages().iter().any(|m| m.is_flagged());
                                        self.actions.push(Action::SetFlagColor { ids: self.selection_ids(), color: if flagged { None } else { Some(FlagColor::Red) } });
                                    }
                                    egui::Popup::menu(&r).show(|ui| self.flag_menu(ui));
                                    let caret = ui.add_enabled(has_sel, egui::Button::new(RichText::new(ic::CARET_DOWN).size(10.0)).frame(false));
                                    egui::Popup::menu(&caret).show(|ui| self.flag_menu(ui));
                                });
                            }
                            ToolbarItem::Mute => {
                                let muted = self.selected_messages().first().map(|m| self.muted.contains(&m.thread_id)).unwrap_or(false);
                                if widgets::tool_button(ui, if muted { ic::BELL } else { ic::BELL_SLASH }, if muted { "Unmute" } else { "Mute conversation" }, has_sel).clicked() {
                                    let threads: HashSet<i64> = self.selected_messages().iter().map(|m| m.thread_id).collect();
                                    for t in threads {
                                        self.actions.push(Action::Mute { thread: t, muted: !muted });
                                    }
                                }
                            }
                            ToolbarItem::MoveTo => {
                                ui.add_enabled_ui(has_sel, |ui| {
                                    let r = widgets::tool_button(ui, ic::FOLDER_SIMPLE, "Move to…", has_sel);
                                    egui::Popup::menu(&r).show(|ui| {
                                        let ids = self.selection_ids();
                                        self.mailbox_picker(ui, &mut |a, dest| a.push(Action::Move { ids: ids.clone(), dest }));
                                    });
                                });
                            }
                            ToolbarItem::Filter => {
                                let on = self.filter.enabled;
                                if widgets::tool_button(ui, if on { ic::FUNNEL } else { ic::FUNNEL_SIMPLE }, "Filter", true).clicked() {
                                    self.filter.enabled = !on;
                                    if !self.filter.any() {
                                        self.filter.unread = true;
                                    }
                                    self.list_dirty = true;
                                }
                            }
                            ToolbarItem::Print => {
                                if widgets::tool_button(ui, ic::PRINTER, "Print", single).clicked() {
                                    self.actions.push(Action::Print(self.focus_message()));
                                }
                            }
                            ToolbarItem::Unread => {
                                if widgets::tool_button(ui, ic::ENVELOPE_SIMPLE, "Mark as Read / Unread", has_sel).clicked() {
                                    self.actions.push(Action::ToggleRead(self.selection_ids()));
                                }
                            }
                            ToolbarItem::Summarize => {
                                if widgets::tool_button(ui, ic::SPARKLE, "Summarize", single).clicked() {
                                    let mut s = self.settings.write();
                                    s.viewing.show_summary = !s.viewing.show_summary;
                                }
                            }
                            ToolbarItem::Remind => {
                                ui.add_enabled_ui(has_sel, |ui| {
                                    let r = widgets::tool_button(ui, ic::ALARM, "Remind Me", has_sel);
                                    egui::Popup::menu(&r).show(|ui| self.remind_menu(ui));
                                });
                            }
                            ToolbarItem::Search => self.search_field_ui(ui),
                            ToolbarItem::Space => {
                                ui.add_space(12.0);
                            }
                            ToolbarItem::FlexibleSpace => {
                                ui.add_space(flex);
                            }
                        }
                    }
                    if !title_done {
                        self.toolbar_title(ui);
                    }
                });
            });
    }

    fn toolbar_title(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(self.selection_title()).family(super::fonts::bold()).size(14.0).color(pal.text));
            let n = self.rows.iter().map(|r| if r.is_child { 0 } else { r.thread_count }).sum::<usize>();
            let unread = self.rows.iter().filter(|r| !r.is_child && r.thread_unread).count();
            let sub = if unread > 0 { format!("{n} messages, {unread} unread") } else { format!("{n} messages") };
            ui.label(RichText::new(sub).size(11.0).color(pal.text_tertiary));
        });
        ui.add_space(6.0);
    }

    pub fn selection_title(&self) -> String {
        if self.search_active && !self.search_text.is_empty() {
            return "Search Results".into();
        }
        match &self.sel {
            SidebarSel::Mailbox(id) => self.mailboxes.iter().find(|m| m.id == *id).map(|m| {
                if m.role != MailboxRole::Custom && m.path.eq_ignore_ascii_case(m.role.display_name()) || m.path.eq_ignore_ascii_case("inbox") {
                    let acct = self.account_name(&m.account_id);
                    format!("{} — {}", m.role.display_name(), acct)
                } else {
                    m.name.clone()
                }
            }),
            SidebarSel::AllRole(r) => Some(match r {
                MailboxRole::Inbox => "All Inboxes".to_string(),
                MailboxRole::Drafts => "Drafts".into(),
                MailboxRole::Sent => "Sent".into(),
                other => other.display_name().to_string(),
            }),
            SidebarSel::Flagged(None) => Some("Flagged".into()),
            SidebarSel::Flagged(Some(c)) => Some(self.settings.read().flag_name(*c)),
            SidebarSel::Vip(None) => Some("VIPs".into()),
            SidebarSel::Vip(Some(e)) => Some(e.clone()),
            SidebarSel::Smart(id) => self.settings.read().smart_mailboxes.iter().find(|m| &m.id == id).map(|m| m.name.clone()),
            SidebarSel::Search => Some("Search".into()),
            SidebarSel::Reminders => Some("Remind Me".into()),
            SidebarSel::FollowUp => Some("Follow Up".into()),
        }
        .unwrap_or_else(|| "Purpamail".into())
    }

    pub fn account_name(&self, id: &str) -> String {
        if id == LOCAL_ACCOUNT {
            return "On This Computer".into();
        }
        self.settings.read().account(id).map(|a| a.display_name().to_string()).unwrap_or_else(|| id.to_string())
    }

    fn search_field_ui(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        egui::Frame::new().fill(pal.field_bg).corner_radius(7.0).stroke(egui::Stroke::new(1.0, pal.separator)).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::MAGNIFYING_GLASS).color(pal.text_tertiary));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.search_text).hint_text("Search").frame(egui::Frame::NONE).desired_width(180.0));
                if self.focus_search {
                    resp.request_focus();
                    self.focus_search = false;
                }
                if resp.changed() {
                    self.search_active = !self.search_text.trim().is_empty();
                    self.list_dirty = true;
                }
                if !self.search_text.is_empty() && ui.add(egui::Button::new(RichText::new(ic::X_CIRCLE).color(pal.text_tertiary)).frame(false)).clicked() {
                    self.search_text.clear();
                    self.search_active = false;
                    self.list_dirty = true;
                }
                let r = ui.add(egui::Button::new(RichText::new(ic::CARET_DOWN).size(10.0).color(pal.text_tertiary)).frame(false)).on_hover_text("Search options");
                egui::Popup::menu(&r).show(|ui| {
                    ui.label(RichText::new("Search in").small().color(pal.text_tertiary));
                    if ui.radio(self.search_all_mailboxes, "All Mailboxes").clicked() {
                        self.search_all_mailboxes = true;
                        self.list_dirty = true;
                    }
                    if ui.radio(!self.search_all_mailboxes, "Current Mailbox").clicked() {
                        self.search_all_mailboxes = false;
                        self.list_dirty = true;
                    }
                    ui.separator();
                    ui.label(RichText::new("Match words in").small().color(pal.text_tertiary));
                    for f in SearchField::ALL {
                        if ui.radio(self.search_field == f, f.name()).clicked() {
                            self.search_field = f;
                            self.list_dirty = true;
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("Tip: from:, to:, subject:, body:, is:unread, is:flagged, has:attachment, -word").small().color(pal.text_tertiary));
                    ui.separator();
                    if ui.button("Save as Smart Mailbox…").clicked() {
                        let sm = smart_from_search(&self.search_text, self.search_field);
                        self.actions.push(Action::NewSmartMailbox(Some(sm)));
                    }
                });
            });
        });
    }

    pub fn flag_menu(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let ids = self.selection_ids();
        let names: Vec<(FlagColor, String)> = FlagColor::ALL.iter().map(|c| (*c, self.settings.read().flag_name(*c))).collect();
        for (c, name) in names {
            if ui.button(RichText::new(format!("{}  {}", ic::FLAG, name)).color(pal.flag(c))).clicked() {
                self.actions.push(Action::SetFlagColor { ids: ids.clone(), color: Some(c) });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Clear Flag").clicked() {
            self.actions.push(Action::SetFlagColor { ids, color: None });
            ui.close();
        }
    }

    pub fn remind_menu(&mut self, ui: &mut Ui) {
        use chrono::{Datelike, Duration, Local, TimeZone, Weekday};
        let ids = self.selection_ids();
        let now = Local::now();
        let at = |h: u32, days: i64| -> i64 {
            let d = now.date_naive() + Duration::days(days);
            Local.from_local_datetime(&d.and_hms_opt(h, 0, 0).unwrap()).single().map(|t| t.timestamp()).unwrap_or(now_ts())
        };
        let mut options: Vec<(String, i64)> = vec![("Remind me in 1 hour".into(), now_ts() + 3600)];
        if now.time() < chrono::NaiveTime::from_hms_opt(20, 0, 0).unwrap() {
            options.push(("Remind me tonight (21:00)".into(), at(21, 0)));
        }
        options.push(("Remind me tomorrow (8:00)".into(), at(8, 1)));
        let days_to_sat = (Weekday::Sat.num_days_from_monday() as i64 - now.weekday().num_days_from_monday() as i64 + 7) % 7;
        options.push(("Remind me this weekend".into(), at(9, if days_to_sat == 0 { 7 } else { days_to_sat })));
        let days_to_mon = (7 - now.weekday().num_days_from_monday() as i64) % 7;
        options.push(("Remind me next week".into(), at(8, if days_to_mon == 0 { 7 } else { days_to_mon })));
        for (label, t) in options {
            if ui.button(label).clicked() {
                self.actions.push(Action::Remind { ids: ids.clone(), at: Some(t) });
                ui.close();
            }
        }
        if ui.button("Remind me later…").clicked() {
            self.prompt = Some(Prompt { title: "Remind Me".into(), label: "When? (e.g. “Friday at 14:00”)".into(), text: String::new(), kind: PromptKind::RemindCustom(ids.clone()) });
            ui.close();
        }
        if self.selected_messages().iter().any(|m| m.remind_at.is_some()) {
            ui.separator();
            if ui.button("Cancel Reminder").clicked() {
                self.actions.push(Action::Remind { ids, at: None });
                ui.close();
            }
        }
    }

    /// A hierarchical list of mailboxes to choose from (for Move / Copy menus).
    pub fn mailbox_picker(&mut self, ui: &mut Ui, on_pick: &mut dyn FnMut(&mut Vec<Action>, MailboxId)) {
        let s = self.settings.read().clone();
        let mut accounts: Vec<(String, String)> = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
        accounts.push((LOCAL_ACCOUNT.to_string(), "On This Computer".to_string()));
        let mbs = self.mailboxes.clone();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (aid, aname) in accounts {
                let list: Vec<&Mailbox> = mbs.iter().filter(|m| m.account_id == aid && m.selectable && m.role != MailboxRole::Outbox).collect();
                if list.is_empty() {
                    continue;
                }
                ui.label(RichText::new(aname).small().strong());
                fn walk(ui: &mut Ui, all: &[&Mailbox], parent: Option<MailboxId>, depth: usize, on_pick: &mut dyn FnMut(&mut Vec<Action>, MailboxId), acts: &mut Vec<Action>) {
                    let mut kids: Vec<&&Mailbox> = all.iter().filter(|m| m.parent == parent).collect();
                    kids.sort_by_key(|m| (role_order(m.role), m.name.to_lowercase()));
                    for m in kids {
                        let label = format!("{}{} {}", "    ".repeat(depth), role_icon(m.role), if m.role != MailboxRole::Custom && m.parent.is_none() { m.role.display_name().to_string() } else { m.name.clone() });
                        if ui.button(label).clicked() {
                            on_pick(acts, m.id);
                            ui.close();
                        }
                        walk(ui, all, Some(m.id), depth + 1, on_pick, acts);
                    }
                }
                let ids: HashSet<MailboxId> = list.iter().map(|m| m.id).collect();
                let roots_fixed: Vec<Mailbox> = list.iter().map(|m| if m.parent.is_some_and(|p| !ids.contains(&p)) { Mailbox { parent: None, ..(*m).clone() } } else { (*m).clone() }).collect();
                let refs: Vec<&Mailbox> = roots_fixed.iter().collect();
                walk(ui, &refs, None, 0, on_pick, &mut self.actions);
                ui.separator();
            }
        });
    }

    fn favorites_bar(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let favs = self.settings.read().favorites.clone();
        egui::Panel::top("favorites")
            .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 5 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, f) in favs.iter().enumerate() {
                        let sel = favorite_sel(f);
                        let (icon, name) = self.favorite_label(f);
                        let active = self.sel == sel && !self.search_active;
                        let text = RichText::new(format!("{icon} {name}")).size(12.5).color(if active { pal.accent } else { pal.text_secondary });
                        let r = ui.add(egui::Button::new(text).frame(active).fill(pal.accent_soft)).on_hover_text(format!("Ctrl+{}", i + 1));
                        if r.clicked() {
                            self.actions.push(Action::SelectSidebar(sel.clone()));
                        }
                        // drop target
                        if let FavoriteItem::Mailbox(mb) = f {
                            if let Some(ids) = r.dnd_release_payload::<Vec<MessageId>>() {
                                self.actions.push(Action::Move { ids: (*ids).clone(), dest: *mb });
                            }
                        }
                        r.context_menu(|ui| {
                            if ui.button("Remove from Favorites").clicked() {
                                self.actions.push(Action::RemoveFavorite(f.clone()));
                                ui.close();
                            }
                        });
                    }
                });
            });
    }

    pub fn favorite_label(&self, f: &FavoriteItem) -> (&'static str, String) {
        match f {
            FavoriteItem::Mailbox(id) => {
                let m = self.mailboxes.iter().find(|m| m.id == *id);
                (m.map(|m| role_icon(m.role)).unwrap_or(ic::FOLDER_SIMPLE), m.map(|m| m.name.clone()).unwrap_or_else(|| "Mailbox".into()))
            }
            FavoriteItem::AllInboxes => (ic::TRAY, "Inbox".into()),
            FavoriteItem::AllFlagged => (ic::FLAG, "Flagged".into()),
            FavoriteItem::AllDrafts => (ic::FILE_TEXT, "Drafts".into()),
            FavoriteItem::AllSent => (ic::PAPER_PLANE_TILT, "Sent".into()),
            FavoriteItem::Vips => (ic::STAR, "VIPs".into()),
            FavoriteItem::Smart(id) => (ic::GEAR_SIX, self.settings.read().smart_mailboxes.iter().find(|m| &m.id == id).map(|m| m.name.clone()).unwrap_or_default()),
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        let pal = Palette::of(ctx);
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -36.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            for t in &self.toasts {
                egui::Frame::popup(ui.style()).fill(if pal.dark { Color32::from_rgb(0x3a, 0x35, 0x45) } else { Color32::from_rgb(0x2a, 0x26, 0x33) }).show(ui, |ui| {
                    ui.label(RichText::new(&t.text).color(Color32::WHITE));
                });
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

pub fn role_icon(role: MailboxRole) -> &'static str {
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

pub fn role_order(role: MailboxRole) -> u8 {
    match role {
        MailboxRole::Inbox => 0,
        MailboxRole::Drafts => 1,
        MailboxRole::Sent => 2,
        MailboxRole::Junk => 3,
        MailboxRole::Trash => 4,
        MailboxRole::Archive => 5,
        MailboxRole::Outbox => 6,
        MailboxRole::Custom => 10,
    }
}

fn smart_from_search(text: &str, field: SearchField) -> SmartMailbox {
    use crate::core::criteria::{Condition, Criteria, Field, Op};
    let mut conds = Vec::new();
    for tok in text.split_whitespace() {
        let (f, v) = match tok.split_once(':') {
            Some(("from", v)) => (Field::From, v),
            Some(("to", v)) => (Field::AnyRecipient, v),
            Some(("subject", v)) => (Field::Subject, v),
            Some(("body", v)) => (Field::Body, v),
            Some(("is", "unread")) => (Field::IsUnread, ""),
            Some(("is", "flagged")) => (Field::IsFlagged, ""),
            Some(("has", _)) => (Field::HasAttachment, ""),
            _ => (
                match field {
                    SearchField::Entire => Field::EntireMessage,
                    SearchField::From => Field::From,
                    SearchField::To => Field::AnyRecipient,
                    SearchField::Subject => Field::Subject,
                    SearchField::Body => Field::Body,
                },
                tok,
            ),
        };
        conds.push(Condition { field: f, op: Op::Contains, value: v.trim_matches('"').to_string(), header: String::new() });
    }
    SmartMailbox { name: if text.is_empty() { "Smart Mailbox".into() } else { text.to_string() }, criteria: Criteria { match_all: true, conditions: conds }, ..Default::default() }
}
