//! Background sync engine.
//!
//! Threads:
//! * a **coordinator** receiving [`Command`]s from the UI, routing them, running timers
//!   (periodic checks, Outbox flushing, maintenance) and managing the account workers;
//! * one **worker** per enabled account, owning that account's server connection
//!   (IMAP or POP3). IMAP workers additionally run an **IDLE watcher** thread with a
//!   second connection so new Inbox mail arrives promptly;
//! * an **Outbox sender** delivering messages over SMTP one at a time;
//! * short-lived threads for the connection doctor and account tests.
//!
//! All mail data flows through the shared [`Store`]; the UI is informed with [`Event`]s.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use parking_lot::{Mutex, RwLock};

use super::imap_client::{FetchItem, IdleEvent, ImapClient, NetError, NetResult, color_keywords, flags_from_imap, flags_to_imap};
use super::pop3::{self, Pop3Client};
use super::{Activity, ActivityState, Command, Event, doctor, smtp};
use crate::core::compose::OutgoingMessage;
use crate::core::config::{AccountConfig, AccountKind, DownloadAttachments, EraseAfter, JunkAction, Secrets, Settings};
use crate::core::mime::{self, ParsedMessage};
use crate::core::model::*;
use crate::core::ops::{self, Accounts};
use crate::core::rules::{self, SideEffect};
use crate::core::store::{MessageQuery, RemoteOp, SharedStore, Store};
use crate::core::{categorize, junk};

pub type SharedSettings = Arc<RwLock<Settings>>;
pub type SharedSecrets = Arc<RwLock<Secrets>>;
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// How often the coordinator tries to deliver due Outbox messages.
const OUTBOX_INTERVAL: Duration = Duration::from_secs(15);
/// Safety net for queued remote operations that were not flushed explicitly.
const OPS_INTERVAL: Duration = Duration::from_secs(60);
/// First maintenance run after start, then every [`MAINTENANCE_INTERVAL`].
const MAINTENANCE_FIRST: Duration = Duration::from_secs(60);
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// Periodic full synchronization (flags of every mailbox, deletions).
const FULL_SYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// IDLE is re-issued after this long (RFC 2177 recommends < 29 minutes).
const IDLE_REISSUE: Duration = Duration::from_secs(25 * 60);
/// Delay before restarting a failed IDLE watcher.
const IDLE_RETRY: Duration = Duration::from_secs(60);
/// An idle main connection is verified with NOOP before reuse after this long.
const KEEPALIVE_CHECK: Duration = Duration::from_secs(4 * 60);
/// Queued operations are dropped after this many failed attempts.
const MAX_OP_ATTEMPTS: i64 = 10;
/// Messages per header fetch.
const HEADER_BATCH: usize = 100;
/// Limits per body prefetch batch.
const BODY_BATCH_MESSAGES: usize = 20;
const BODY_BATCH_BYTES: u64 = 4 * 1024 * 1024;
/// Mailboxes with at most this many messages get their flags reconciled on every check.
const RECONCILE_LIMIT: usize = 5000;

/// Flag bits that mirror server state.
const SERVER_BITS: u32 = flags::SEEN | flags::ANSWERED | flags::FLAGGED | flags::DELETED | flags::DRAFT | flags::FORWARDED | flags::JUNK | flags::NOT_JUNK;

/// Handle held by the UI.
#[derive(Clone)]
pub struct EngineHandle {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

pub struct Engine;

impl Engine {
    /// Start the engine threads. Account workers are started for every enabled account
    /// and immediately synchronize (unless working offline).
    pub fn start(store: SharedStore, settings: SharedSettings, secrets: SharedSecrets, repaint: Repaint) -> EngineHandle {
        let (ctx_tx, ctx_rx) = unbounded::<Command>();
        let (etx, erx) = unbounded::<Event>();
        let (job_tx, job_rx) = unbounded::<SendJob>();
        let work_offline = settings.read().work_offline;
        let ctx = Ctx {
            store,
            settings,
            secrets,
            events: etx,
            repaint,
            commands: ctx_tx.clone(),
            send_jobs: job_tx,
            online: Arc::new(RwLock::new(OnlineState { global: !work_offline, overrides: HashMap::new() })),
        };
        let sender = OutboxSender { ctx: ctx.clone(), rx: job_rx, failures: HashMap::new() };
        spawn_named("purpamail-outbox", move || sender.run());
        let coordinator = Coordinator::new(ctx, ctx_rx, work_offline);
        spawn_named("purpamail-engine", move || coordinator.run());
        EngineHandle { tx: ctx_tx, rx: erx }
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.to_string()).spawn(f) {
        log::error!("could not start thread {name}: {e}");
    }
}

// ------------------------------------------------------------------ shared context

/// Online / offline state shared by all threads.
#[derive(Debug)]
struct OnlineState {
    global: bool,
    overrides: HashMap<AccountId, bool>,
}

impl OnlineState {
    fn is_online(&self, account: &str) -> bool {
        self.overrides.get(account).copied().unwrap_or(self.global)
    }
}

/// Everything a thread needs to talk to the store, the settings and the UI.
#[derive(Clone)]
struct Ctx {
    store: SharedStore,
    settings: SharedSettings,
    secrets: SharedSecrets,
    events: Sender<Event>,
    repaint: Repaint,
    /// The coordinator's own command channel (for follow-up work such as FlushOps).
    commands: Sender<Command>,
    send_jobs: Sender<SendJob>,
    online: Arc<RwLock<OnlineState>>,
}

impl Ctx {
    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
        (self.repaint)();
    }

    fn emit_changed(&self, touched: &BTreeSet<MailboxId>) {
        for id in touched {
            let _ = self.events.send(Event::MessagesChanged(*id));
        }
        if !touched.is_empty() {
            (self.repaint)();
        }
    }

    fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    fn is_online(&self, account: &str) -> bool {
        self.online.read().is_online(account)
    }

    /// Ask the coordinator to replay queued operations of accounts.
    fn request_flush(&self, accounts: impl IntoIterator<Item = AccountId>) {
        for a in accounts {
            if a != LOCAL_ACCOUNT {
                let _ = self.commands.send(Command::FlushOps(a));
            }
        }
    }
}

static NEXT_ACTIVITY: AtomicU64 = AtomicU64::new(1);

/// Reports a long running operation to the UI (`Running` → `Done` / `Failed`).
/// Dropping an unfinished tracker reports it as failed.
struct ActivityTracker {
    ctx: Ctx,
    act: Activity,
    finished: bool,
}

impl ActivityTracker {
    fn start(ctx: &Ctx, account: &str, title: impl Into<String>, detail: impl Into<String>) -> Self {
        let act = Activity {
            id: NEXT_ACTIVITY.fetch_add(1, Ordering::Relaxed),
            account: account.to_string(),
            title: title.into(),
            detail: detail.into(),
            progress: None,
            state: ActivityState::Running,
        };
        ctx.emit(Event::Activity(act.clone()));
        Self { ctx: ctx.clone(), act, finished: false }
    }

    fn update(&mut self, progress: Option<f32>, detail: impl Into<String>) {
        self.act.progress = progress.map(|p| p.clamp(0.0, 1.0));
        self.act.detail = detail.into();
        self.ctx.emit(Event::Activity(self.act.clone()));
    }

    fn finish(&mut self, state: ActivityState) {
        self.finished = true;
        if state == ActivityState::Done {
            self.act.progress = Some(1.0);
        }
        self.act.state = state;
        self.ctx.emit(Event::Activity(self.act.clone()));
    }

    fn done(mut self) {
        self.finish(ActivityState::Done);
    }

    fn fail(mut self, error: &str) {
        self.finish(ActivityState::Failed(error.to_string()));
    }
}

impl Drop for ActivityTracker {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(ActivityState::Failed("Interrupted".into()));
        }
    }
}

// ------------------------------------------------------------------ coordinator

/// Messages from the coordinator to an account worker.
#[derive(Debug, Clone, PartialEq)]
enum WorkerMsg {
    /// Check for new mail; `full` reconciles every mailbox, `user` bypasses connection backoff.
    Check { full: bool, user: bool },
    SyncMailbox(MailboxId),
    FetchBody(MessageId),
    FlushOps,
    /// The IDLE watcher saw a change in the Inbox.
    IdleChanged,
    /// The IDLE watcher stopped because of an error.
    IdleDied(String),
    SetOnline(bool),
    Shutdown,
}

impl WorkerMsg {
    fn is_priority(&self) -> bool {
        matches!(self, WorkerMsg::FetchBody(_) | WorkerMsg::SyncMailbox(_) | WorkerMsg::SetOnline(_) | WorkerMsg::Shutdown)
    }
}

/// Add a message to a worker queue, merging duplicates.
fn enqueue(queue: &mut VecDeque<WorkerMsg>, msg: WorkerMsg) {
    match &msg {
        WorkerMsg::Check { full, user } => {
            for m in queue.iter_mut() {
                if let WorkerMsg::Check { full: f, user: u } = m {
                    *f |= *full;
                    *u |= *user;
                    return;
                }
            }
        }
        WorkerMsg::IdleChanged | WorkerMsg::FlushOps | WorkerMsg::FetchBody(_) | WorkerMsg::SyncMailbox(_)
            if queue.contains(&msg) => {
                return;
            }
        _ => {}
    }
    queue.push_back(msg);
}

struct WorkerHandle {
    tx: Sender<WorkerMsg>,
    config: AccountConfig,
    /// Passwords at start, so credential changes restart the worker.
    secrets: (String, String),
}

struct Coordinator {
    ctx: Ctx,
    rx: Receiver<Command>,
    workers: HashMap<AccountId, WorkerHandle>,
    last_work_offline: bool,
    last_check: Instant,
    last_outbox: Instant,
    last_ops: Instant,
    next_maintenance: Instant,
}

impl Coordinator {
    fn new(ctx: Ctx, rx: Receiver<Command>, work_offline: bool) -> Self {
        let now = Instant::now();
        Self {
            ctx,
            rx,
            workers: HashMap::new(),
            last_work_offline: work_offline,
            last_check: now,
            last_outbox: now,
            last_ops: now,
            next_maintenance: now + MAINTENANCE_FIRST,
        }
    }

    fn run(mut self) {
        self.reload_workers();
        loop {
            match self.rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.timers();
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        maintenance(&self.ctx, true);
        for (_, w) in self.workers.drain() {
            let _ = w.tx.send(WorkerMsg::Shutdown);
        }
        let _ = self.ctx.send_jobs.send(SendJob::Shutdown);
    }

    fn worker(&self, account: &str) -> Option<&WorkerHandle> {
        self.workers.get(account)
    }

    fn to_worker(&self, account: &str, msg: WorkerMsg) -> bool {
        match self.workers.get(account) {
            Some(w) => w.tx.send(msg).is_ok(),
            None => false,
        }
    }

    fn handle(&mut self, cmd: Command) {
        log::debug!("engine: {cmd:?}");
        match cmd {
            Command::CheckMail(None) => {
                for w in self.workers.values() {
                    let _ = w.tx.send(WorkerMsg::Check { full: false, user: true });
                }
                self.last_check = Instant::now();
            }
            Command::CheckMail(Some(a)) => {
                self.to_worker(&a, WorkerMsg::Check { full: false, user: true });
            }
            Command::Synchronize(a) => {
                self.to_worker(&a, WorkerMsg::Check { full: true, user: true });
            }
            Command::SyncMailbox(id) => match self.ctx.store.mailbox(id) {
                Some(mb) if mb.account_id != LOCAL_ACCOUNT && self.worker(&mb.account_id).is_some() => {
                    self.to_worker(&mb.account_id, WorkerMsg::SyncMailbox(id));
                }
                _ => self.ctx.emit(Event::MessagesChanged(id)),
            },
            Command::FetchBody(id) => self.route_fetch_body(id),
            Command::FlushOps(a) => self.flush_ops(&a),
            Command::Send { outbox_id, message } => {
                let _ = self.ctx.send_jobs.send(SendJob::Send { outbox_id, message });
            }
            Command::FlushOutbox => {
                let _ = self.ctx.send_jobs.send(SendJob::Flush { user: true });
                self.last_outbox = Instant::now();
            }
            Command::Maintenance => {
                for a in maintenance(&self.ctx, false) {
                    self.flush_ops(&a);
                }
            }
            Command::SetOnline { account, online } => self.set_online(account, online),
            Command::ReloadSettings => self.reload_workers(),
            Command::ConnectionDoctor => {
                let ctx = self.ctx.clone();
                spawn_named("purpamail-doctor", move || {
                    let settings = ctx.settings();
                    let secrets = ctx.secrets.read().clone();
                    for acct in &settings.accounts {
                        let act = ActivityTracker::start(&ctx, &acct.id, "Connection Doctor", acct.display_name());
                        let result = doctor::diagnose(acct, &secrets);
                        act.done();
                        ctx.emit(Event::Doctor(result));
                    }
                });
            }
            Command::TestAccount(cfg, password) => {
                let ctx = self.ctx.clone();
                spawn_named("purpamail-test-account", move || {
                    let (incoming, outgoing) = doctor::test_account(&cfg, &password);
                    ctx.emit(Event::TestAccountResult { incoming, outgoing });
                });
            }
            Command::Shutdown => {}
        }
    }

    fn route_fetch_body(&self, id: MessageId) {
        let store = &self.ctx.store;
        let Some(m) = store.message(id) else {
            self.ctx.emit(Event::BodyFetchFailed(id, "The message no longer exists.".into()));
            return;
        };
        let has_raw = m.has(flags::HAS_BODY) && store.raw(id).is_some();
        if has_raw {
            self.ctx.emit(Event::BodyFetched(id));
            return;
        }
        let is_imap = self.worker(&m.account_id).map(|w| w.config.kind == AccountKind::Imap).unwrap_or(false);
        if m.account_id == LOCAL_ACCOUNT || !is_imap {
            if store.raw(id).is_some() {
                self.ctx.emit(Event::BodyFetched(id));
            } else {
                self.ctx.emit(Event::BodyFetchFailed(id, "The message content is not available.".into()));
            }
            return;
        }
        self.to_worker(&m.account_id, WorkerMsg::FetchBody(id));
    }

    fn flush_ops(&self, account: &str) {
        if account == LOCAL_ACCOUNT {
            return;
        }
        match self.worker(account) {
            Some(w) if w.config.kind == AccountKind::Imap => {
                let _ = w.tx.send(WorkerMsg::FlushOps);
            }
            Some(_) => drop_ops(&self.ctx.store, account),
            None => {
                // Account removed: its operations can never be replayed.
                if self.ctx.settings.read().account(account).is_none() {
                    drop_ops(&self.ctx.store, account);
                }
            }
        }
    }

    fn set_online(&mut self, account: Option<AccountId>, online: bool) {
        {
            let mut st = self.ctx.online.write();
            match &account {
                None => {
                    st.global = online;
                    st.overrides.clear();
                }
                Some(a) => {
                    st.overrides.insert(a.clone(), online);
                }
            }
        }
        self.notify_online_state();
        if online {
            let _ = self.ctx.send_jobs.send(SendJob::Flush { user: true });
        }
    }

    fn notify_online_state(&self) {
        let st = self.ctx.online.read();
        for (id, w) in &self.workers {
            let _ = w.tx.send(WorkerMsg::SetOnline(st.is_online(id)));
        }
    }

    /// Start, stop or restart workers to match the configured accounts.
    fn reload_workers(&mut self) {
        let settings = self.ctx.settings();
        if settings.work_offline != self.last_work_offline {
            self.last_work_offline = settings.work_offline;
            {
                let mut st = self.ctx.online.write();
                st.global = !settings.work_offline;
                st.overrides.clear();
            }
            self.notify_online_state();
        }
        let secrets = self.ctx.secrets.read().clone();
        let wanted: HashMap<AccountId, AccountConfig> = settings.accounts.iter().filter(|a| a.enabled).map(|a| (a.id.clone(), a.clone())).collect();
        let current: Vec<AccountId> = self.workers.keys().cloned().collect();
        for id in current {
            let keep = match (wanted.get(&id), self.workers.get(&id)) {
                (Some(cfg), Some(w)) => *cfg == w.config && w.secrets == (secrets.incoming(&id), secrets.outgoing(cfg)),
                _ => false,
            };
            if !keep
                && let Some(w) = self.workers.remove(&id) {
                    log::info!("engine: stopping worker for {id}");
                    let _ = w.tx.send(WorkerMsg::Shutdown);
                }
        }
        for (id, cfg) in wanted {
            if self.workers.contains_key(&id) {
                continue;
            }
            log::info!("engine: starting worker for {} ({})", cfg.display_name(), id);
            let (tx, rx) = unbounded::<WorkerMsg>();
            let online = self.ctx.is_online(&id);
            let worker = Worker::new(self.ctx.clone(), cfg.clone(), rx, tx.clone(), online);
            spawn_named(&format!("purpamail-acct-{}", cfg.display_name()), move || worker.run());
            let pw = (secrets.incoming(&id), secrets.outgoing(&cfg));
            self.workers.insert(id, WorkerHandle { tx, config: cfg, secrets: pw });
        }
    }

    fn timers(&mut self) {
        let now = Instant::now();
        let freq = self.ctx.settings.read().general.check_frequency;
        if let Some(secs) = freq.seconds()
            && now.duration_since(self.last_check) >= Duration::from_secs(secs) {
                self.last_check = now;
                for w in self.workers.values().filter(|w| w.config.include_when_checking) {
                    let _ = w.tx.send(WorkerMsg::Check { full: false, user: false });
                }
            }
        if now.duration_since(self.last_outbox) >= OUTBOX_INTERVAL {
            self.last_outbox = now;
            let _ = self.ctx.send_jobs.send(SendJob::Flush { user: false });
        }
        if now.duration_since(self.last_ops) >= OPS_INTERVAL {
            self.last_ops = now;
            let ids: Vec<AccountId> = self.workers.keys().cloned().collect();
            for id in ids {
                if !self.ctx.store.pending_ops(&id).is_empty() {
                    self.flush_ops(&id);
                }
            }
        }
        if now >= self.next_maintenance {
            self.next_maintenance = now + MAINTENANCE_INTERVAL;
            for a in maintenance(&self.ctx, false) {
                self.flush_ops(&a);
            }
        }
    }
}

/// Remove all queued operations of an account (POP accounts have nothing to replay).
fn drop_ops(store: &Store, account: &str) {
    for (id, _, _) in store.pending_ops(account) {
        let _ = store.remove_op(id);
    }
}

/// Erase old messages from Trash / Junk / Sent according to each account's policy.
/// With `quitting`, only "when quitting" policies apply. Returns accounts with new ops.
fn maintenance(ctx: &Ctx, quitting: bool) -> Accounts {
    let settings = ctx.settings();
    let store = &ctx.store;
    let now = now_ts();
    let mut accts = Accounts::new();
    let mut touched = BTreeSet::new();
    for acct in &settings.accounts {
        for (role, policy) in [(MailboxRole::Trash, acct.erase_deleted), (MailboxRole::Junk, acct.erase_junk), (MailboxRole::Sent, acct.erase_sent)] {
            let Some(mb) = store.mailbox_by_role(&acct.id, role) else { continue };
            let ids = match (policy, quitting) {
                (EraseAfter::WhenQuitting, true) => store.ids_in_mailbox(mb.id),
                (p, false) => p.seconds().map(|s| store.older_than(mb.id, now - s)).unwrap_or_default(),
                _ => Vec::new(),
            };
            if ids.is_empty() {
                continue;
            }
            log::info!("maintenance: erasing {} messages from {} of {}", ids.len(), mb.name, acct.display_name());
            match ops::erase_messages(store, &ids) {
                Ok(a) => accts.extend(a),
                Err(e) => log::warn!("maintenance: {e}"),
            }
            touched.insert(mb.id);
        }
    }
    ctx.emit_changed(&touched);
    accts
}

// ------------------------------------------------------------------ account worker

/// Exponential reconnection backoff.
#[derive(Debug, Default)]
struct Backoff {
    failures: u32,
    next: Option<Instant>,
}

impl Backoff {
    fn fail(&mut self, auth: bool) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let base: u64 = if auth { 300 } else { 5 };
        let secs = base.saturating_mul(1u64 << (self.failures - 1).min(10)).min(900);
        let d = Duration::from_secs(secs);
        self.next = Some(Instant::now() + d);
        d
    }
    fn reset(&mut self) {
        self.failures = 0;
        self.next = None;
    }
    fn waiting(&self) -> bool {
        self.next.is_some_and(|t| Instant::now() < t)
    }
}

struct Worker {
    ctx: Ctx,
    acct: AccountConfig,
    rx: Receiver<WorkerMsg>,
    self_tx: Sender<WorkerMsg>,
    queue: VecDeque<WorkerMsg>,
    online: bool,
    conn: Option<ImapClient>,
    last_used: Instant,
    backoff: Backoff,
    last_full: Option<Instant>,
    idle: Option<IdleWatcher>,
    idle_retry_at: Option<Instant>,
    last_status: Option<(bool, Option<String>)>,
}

impl Worker {
    fn new(ctx: Ctx, acct: AccountConfig, rx: Receiver<WorkerMsg>, self_tx: Sender<WorkerMsg>, online: bool) -> Self {
        Self {
            ctx,
            acct,
            rx,
            self_tx,
            queue: VecDeque::new(),
            online,
            conn: None,
            last_used: Instant::now(),
            backoff: Backoff::default(),
            last_full: None,
            idle: None,
            idle_retry_at: None,
            last_status: None,
        }
    }

    fn run(mut self) {
        if self.online {
            self.queue.push_back(WorkerMsg::Check { full: true, user: true });
        } else {
            self.report(false, None);
        }
        while let Some(msg) = self.next_msg() {
            match msg {
                WorkerMsg::Shutdown => break,
                WorkerMsg::Check { full, user } => {
                    if self.online && (user || !self.backoff.waiting()) {
                        self.check(full);
                    }
                }
                WorkerMsg::SyncMailbox(id) => self.sync_mailbox(id),
                WorkerMsg::FetchBody(id) => self.fetch_body(id),
                WorkerMsg::FlushOps => self.flush_ops(),
                WorkerMsg::IdleChanged => {
                    if let Some(inbox) = self.ctx.store.mailbox_by_role(&self.acct.id, MailboxRole::Inbox) {
                        self.sync_mailbox(inbox.id);
                    }
                }
                WorkerMsg::IdleDied(e) => {
                    log::info!("{}: IDLE stopped: {e}", self.acct.display_name());
                    self.idle = None;
                    self.idle_retry_at = Some(Instant::now() + IDLE_RETRY);
                }
                WorkerMsg::SetOnline(online) => self.set_online(online),
            }
        }
        self.stop_idle();
        if let Some(mut c) = self.conn.take() {
            c.logout();
        }
    }

    fn next_msg(&mut self) -> Option<WorkerMsg> {
        loop {
            while let Ok(m) = self.rx.try_recv() {
                enqueue(&mut self.queue, m);
            }
            if let Some(pos) = self.queue.iter().position(|m| m.is_priority()) {
                return self.queue.remove(pos);
            }
            if let Some(m) = self.queue.pop_front() {
                return Some(m);
            }
            let wake = [self.backoff.next, self.idle_retry_at].into_iter().flatten().min();
            let r = match wake {
                Some(t) => self.rx.recv_timeout(t.saturating_duration_since(Instant::now())),
                None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match r {
                Ok(m) => enqueue(&mut self.queue, m),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => self.on_timer(),
            }
        }
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        if self.backoff.next.is_some_and(|t| now >= t) {
            self.backoff.next = None;
            if self.online {
                enqueue(&mut self.queue, WorkerMsg::Check { full: false, user: true });
            }
        }
        if self.idle_retry_at.is_some_and(|t| now >= t) {
            self.idle_retry_at = None;
            self.ensure_idle();
        }
    }

    /// Emit an [`Event::AccountStatus`] if the state changed.
    fn report(&mut self, online: bool, error: Option<String>) {
        let st = (online, error);
        if self.last_status.as_ref() != Some(&st) {
            self.ctx.emit(Event::AccountStatus { account: self.acct.id.clone(), online: st.0, error: st.1.clone() });
            self.last_status = Some(st);
        }
    }

    fn on_success(&mut self) {
        self.backoff.reset();
        self.report(true, None);
    }

    fn on_error(&mut self, e: &NetError) {
        let wait = self.backoff.fail(matches!(e, NetError::Auth(_)));
        log::warn!("{}: {e} (retrying in {}s)", self.acct.display_name(), wait.as_secs());
        self.report(true, Some(e.user_message()));
        self.stop_idle();
    }

    fn set_online(&mut self, online: bool) {
        if online == self.online {
            return;
        }
        self.online = online;
        if online {
            self.backoff.reset();
            self.last_status = None;
            enqueue(&mut self.queue, WorkerMsg::Check { full: false, user: true });
        } else {
            self.stop_idle();
            if let Some(mut c) = self.conn.take() {
                c.logout();
            }
            self.report(false, None);
        }
    }

    fn password(&self) -> String {
        self.ctx.secrets.read().incoming(&self.acct.id)
    }

    // -------------------------------------------------------------- dispatch

    fn check(&mut self, full: bool) {
        match self.acct.kind {
            AccountKind::Imap => {
                let full = full || self.last_full.is_none_or(|t| t.elapsed() >= FULL_SYNC_INTERVAL);
                if self.with_imap(|s| s.full_cycle(full)).is_ok() {
                    if full {
                        self.last_full = Some(Instant::now());
                    }
                    self.ensure_idle();
                }
            }
            AccountKind::Pop3 => self.pop_check(),
        }
    }

    fn sync_mailbox(&mut self, id: MailboxId) {
        let Some(mb) = self.ctx.store.mailbox(id) else { return };
        if mb.account_id != self.acct.id {
            return;
        }
        if !self.online {
            self.ctx.emit(Event::MessagesChanged(id));
            return;
        }
        match self.acct.kind {
            AccountKind::Imap => {
                let _ = self.with_imap(|s| {
                    s.flush_ops()?;
                    if let Some(mb) = s.ctx.store.mailbox(id).filter(|m| m.selectable) {
                        s.sync_mailbox(&mb, false, false)?;
                    }
                    s.finish_new_mail()
                });
            }
            AccountKind::Pop3 if mb.role == MailboxRole::Inbox => self.pop_check(),
            AccountKind::Pop3 => self.ctx.emit(Event::MessagesChanged(id)),
        }
    }

    fn fetch_body(&mut self, id: MessageId) {
        if !self.online {
            self.ctx.emit(Event::BodyFetchFailed(id, "You are offline.".into()));
            return;
        }
        if self.acct.kind != AccountKind::Imap {
            self.ctx.emit(Event::BodyFetchFailed(id, "The message content is not available.".into()));
            return;
        }
        if let Err(e) = self.with_imap(|s| s.fetch_body(id)) {
            self.ctx.emit(Event::BodyFetchFailed(id, e.user_message()));
        }
    }

    fn flush_ops(&mut self) {
        if self.acct.kind != AccountKind::Imap {
            drop_ops(&self.ctx.store, &self.acct.id);
            return;
        }
        if !self.online || self.ctx.store.pending_ops(&self.acct.id).is_empty() {
            return;
        }
        let _ = self.with_imap(|s| s.flush_ops());
    }

    // -------------------------------------------------------------- IMAP session handling

    /// The current connection, or a new one.
    fn take_client(&mut self) -> NetResult<ImapClient> {
        if let Some(mut c) = self.conn.take() {
            if self.last_used.elapsed() < KEEPALIVE_CHECK || c.noop().is_ok() {
                return Ok(c);
            }
            log::debug!("{}: connection went stale, reconnecting", self.acct.display_name());
        }
        let mut c = ImapClient::connect(&self.acct.incoming, false)?;
        c.login(&self.acct.incoming, &self.password())?;
        Ok(c)
    }

    /// Run `f` with a connected session, then publish the changes it made.
    fn with_imap<T>(&mut self, f: impl FnOnce(&mut ImapSync) -> NetResult<T>) -> NetResult<T> {
        let mut client = match self.take_client() {
            Ok(c) => c,
            Err(e) => {
                self.on_error(&e);
                return Err(e);
            }
        };
        let mut sync = ImapSync::new(&self.ctx, &self.acct, &mut client, &self.rx, &mut self.queue);
        let res = f(&mut sync);
        let touched = std::mem::take(&mut sync.touched);
        let mailboxes_changed = sync.mailboxes_changed;
        let notify = std::mem::take(&mut sync.notify);
        drop(sync);
        if mailboxes_changed {
            self.ctx.emit(Event::MailboxesChanged(self.acct.id.clone()));
        }
        self.ctx.emit_changed(&touched);
        if !notify.is_empty() {
            self.ctx.emit(Event::NewMail { account: self.acct.id.clone(), messages: notify });
        }
        match &res {
            Ok(_) => {
                self.conn = Some(client);
                self.last_used = Instant::now();
                self.on_success();
            }
            Err(e) if e.is_connection() || matches!(e, NetError::Auth(_)) => self.on_error(e),
            Err(e) => {
                log::warn!("{}: {e}", self.acct.display_name());
                self.conn = Some(client);
                self.last_used = Instant::now();
            }
        }
        res
    }

    fn ensure_idle(&mut self) {
        if !self.acct.use_idle || !self.online || self.idle.is_some() || self.idle_retry_at.is_some() {
            return;
        }
        if !self.conn.as_ref().is_some_and(|c| c.has_cap("IDLE")) {
            return;
        }
        self.idle = Some(IdleWatcher::start(self.acct.clone(), self.password(), self.self_tx.clone()));
    }

    fn stop_idle(&mut self) {
        if let Some(w) = self.idle.take() {
            w.stop();
        }
    }

    // -------------------------------------------------------------- POP3

    fn pop_check(&mut self) {
        let store = self.ctx.store.clone();
        let inbox = match store.mailbox_by_role(&self.acct.id, MailboxRole::Inbox) {
            Some(m) => m.id,
            None => match store.ensure_mailbox(&self.acct.id, "INBOX", MailboxRole::Inbox).and_then(|id| store.set_mailbox_role(id, MailboxRole::Inbox).map(|_| id)) {
                Ok(id) => {
                    self.ctx.emit(Event::MailboxesChanged(self.acct.id.clone()));
                    id
                }
                Err(e) => {
                    log::error!("pop3: cannot create inbox: {e}");
                    return;
                }
            },
        };
        drop_ops(&store, &self.acct.id);
        let password = self.password();
        let ctx = self.ctx.clone();
        let acct = self.acct.clone();
        let result = (|| -> NetResult<pop3::DownloadResult> {
            let mut c = Pop3Client::connect(&acct.incoming, false)?;
            c.login(&acct.incoming, &password)?;
            let mut activity: Option<ActivityTracker> = None;
            let r = pop3::download(&mut c, &store, &acct, inbox, &mut |done, total| {
                if total >= 5 {
                    let act = activity.get_or_insert_with(|| ActivityTracker::start(&ctx, &acct.id, format!("Fetching {total} messages"), acct.display_name()));
                    act.update(Some(done as f32 / total as f32), format!("{done} of {total}"));
                }
            });
            match &r {
                Ok(_) => {
                    if let Some(a) = activity.take() {
                        a.done();
                    }
                    c.quit()?;
                }
                Err(e) => {
                    if let Some(a) = activity.take() {
                        a.fail(&e.user_message());
                    }
                }
            }
            r
        })();
        match result {
            Ok(r) => {
                self.on_success();
                let mut touched = BTreeSet::new();
                let mut notify = Vec::new();
                if !r.new_ids.is_empty() {
                    touched.insert(inbox);
                    if !r.initial {
                        notify = process_new_mail(&self.ctx, &self.acct, inbox, &r.new_ids, &HashMap::new(), &mut touched);
                        drop_ops(&store, &self.acct.id);
                    }
                }
                self.ctx.emit_changed(&touched);
                if !notify.is_empty() {
                    self.ctx.emit(Event::NewMail { account: self.acct.id.clone(), messages: notify });
                }
            }
            Err(e) => self.on_error(&e),
        }
    }
}

// ------------------------------------------------------------------ IDLE watcher

/// Watches the Inbox on a dedicated connection with IMAP IDLE.
struct IdleWatcher {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
}

impl IdleWatcher {
    fn start(acct: AccountConfig, password: String, tx: Sender<WorkerMsg>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let socket: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
        let (s2, sock2) = (stop.clone(), socket.clone());
        spawn_named(&format!("purpamail-idle-{}", acct.display_name()), move || {
            let res = idle_loop(&acct, &password, &tx, &s2, &sock2);
            if !s2.load(Ordering::SeqCst) {
                let reason = match res {
                    Ok(()) => "connection closed".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(WorkerMsg::IdleDied(reason));
            }
        });
        Self { stop, socket }
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = self.socket.lock().take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for IdleWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn idle_loop(acct: &AccountConfig, password: &str, tx: &Sender<WorkerMsg>, stop: &AtomicBool, socket: &Mutex<Option<TcpStream>>) -> NetResult<()> {
    let mut c = ImapClient::connect(&acct.incoming, false)?;
    *socket.lock() = c.shutdown_handle();
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    c.login(&acct.incoming, password)?;
    if !c.has_cap("IDLE") {
        return Err(NetError::No("the server does not support IDLE".into()));
    }
    c.examine("INBOX")?;
    loop {
        c.idle_start()?;
        let ev = c.idle_wait(IDLE_REISSUE)?;
        c.idle_done()?;
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if ev == IdleEvent::Changed && tx.send(WorkerMsg::IdleChanged).is_err() {
            return Ok(());
        }
    }
}

// ------------------------------------------------------------------ IMAP synchronization

/// Why a queued operation could not be replayed.
enum OpError {
    Net(NetError),
    /// The operation can never succeed (e.g. its mailbox is gone) and is discarded.
    Drop(String),
}

impl From<NetError> for OpError {
    fn from(e: NetError) -> Self {
        OpError::Net(e)
    }
}

fn is_trycreate(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("trycreate") || t.contains("nonexistent") || t.contains("not exist") || t.contains("doesn't exist") || t.contains("no such")
}

fn is_already_exists(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("alreadyexists") || t.contains("already exists") || t.contains("exists")
}

fn role_order(r: MailboxRole) -> u8 {
    match r {
        MailboxRole::Inbox => 0,
        MailboxRole::Drafts => 1,
        MailboxRole::Sent => 2,
        MailboxRole::Archive => 3,
        MailboxRole::Custom => 4,
        MailboxRole::Junk => 5,
        MailboxRole::Trash => 6,
        MailboxRole::Outbox => 7,
    }
}

/// Whether the body of a new message should be downloaded right away.
fn should_prefetch(policy: DownloadAttachments, role: MailboxRole, size: u32, date: i64) -> bool {
    const KB: u32 = 1024;
    let size = if size == 0 { u32::MAX } else { size };
    match role {
        MailboxRole::Junk | MailboxRole::Trash => size <= 32 * KB,
        MailboxRole::Inbox => match policy {
            DownloadAttachments::All => size <= 10 * 1024 * KB,
            DownloadAttachments::Recent => {
                if now_ts() - date <= 30 * 86400 {
                    size <= 10 * 1024 * KB
                } else {
                    size <= 64 * KB
                }
            }
            DownloadAttachments::None => size <= 64 * KB,
        },
        _ => match policy {
            DownloadAttachments::All => size <= 1024 * KB,
            DownloadAttachments::Recent => size <= if now_ts() - date <= 30 * 86400 { 256 * KB } else { 32 * KB },
            DownloadAttachments::None => size <= 32 * KB,
        },
    }
}

/// Store a downloaded message source and refresh the fields derived from it.
fn apply_body(store: &Store, id: MessageId, raw: &[u8]) {
    let res = match mime::parse(raw) {
        Some(p) => {
            let body = p.plain_text();
            let snippet = mime::make_snippet(&body);
            let r = store.set_raw(id, raw, &body, &snippet, p.real_attachments().next().is_some());
            let _ = store.set_category(id, categorize::categorize(&p));
            r
        }
        None => store.set_raw(id, raw, "", "", false),
    };
    if let Err(e) = res {
        log::warn!("could not store message body: {e}");
    }
}

/// One synchronization pass over an IMAP connection.
struct ImapSync<'a> {
    ctx: &'a Ctx,
    acct: &'a AccountConfig,
    c: &'a mut ImapClient,
    rx: &'a Receiver<WorkerMsg>,
    queue: &'a mut VecDeque<WorkerMsg>,
    touched: BTreeSet<MailboxId>,
    mailboxes_changed: bool,
    delimiter: Option<Option<String>>,
    /// Messages that arrived in the Inbox during this pass, with their header block.
    new_inbox: Vec<MessageId>,
    new_headers: HashMap<MessageId, Vec<u8>>,
    /// Messages to announce with [`Event::NewMail`].
    notify: Vec<MessageId>,
}

impl<'a> ImapSync<'a> {
    fn new(ctx: &'a Ctx, acct: &'a AccountConfig, c: &'a mut ImapClient, rx: &'a Receiver<WorkerMsg>, queue: &'a mut VecDeque<WorkerMsg>) -> Self {
        Self {
            ctx,
            acct,
            c,
            rx,
            queue,
            touched: BTreeSet::new(),
            mailboxes_changed: false,
            delimiter: None,
            new_inbox: Vec::new(),
            new_headers: HashMap::new(),
            notify: Vec::new(),
        }
    }

    fn store(&self) -> &'a Store {
        &self.ctx.store
    }

    /// Replay queued operations, sync the mailbox list and every mailbox.
    fn full_cycle(&mut self, full: bool) -> NetResult<()> {
        self.flush_ops()?;
        self.sync_mailbox_list()?;
        self.sync_all(full)?;
        self.finish_new_mail()
    }

    /// Serve urgent requests (body downloads) between batches of a long sync.
    fn service_priority(&mut self) -> NetResult<()> {
        while let Ok(m) = self.rx.try_recv() {
            enqueue(self.queue, m);
        }
        let mut bodies = Vec::new();
        self.queue.retain(|m| match m {
            WorkerMsg::FetchBody(id) => {
                bodies.push(*id);
                false
            }
            _ => true,
        });
        for id in bodies {
            self.fetch_body(id)?;
        }
        Ok(())
    }

    /// Take a pending "sync this mailbox now" request.
    fn take_sync_request(&mut self) -> Option<MailboxId> {
        let pos = self.queue.iter().position(|m| matches!(m, WorkerMsg::SyncMailbox(_)))?;
        match self.queue.remove(pos) {
            Some(WorkerMsg::SyncMailbox(id)) => Some(id),
            _ => None,
        }
    }

    // -------------------------------------------------------------- bodies

    /// Download one message body; failures other than lost connections are reported as events.
    fn fetch_body(&mut self, id: MessageId) -> NetResult<()> {
        let store = self.store();
        let fail = |msg: &str| self.ctx.emit(Event::BodyFetchFailed(id, msg.to_string()));
        let Some(m) = store.message(id) else {
            fail("The message no longer exists.");
            return Ok(());
        };
        if store.raw(id).is_some() {
            self.ctx.emit(Event::BodyFetched(id));
            return Ok(());
        }
        let Some(mb) = store.mailbox(m.mailbox_id).filter(|mb| mb.account_id == self.acct.id) else {
            fail("The message content is not available.");
            return Ok(());
        };
        if m.uid == 0 {
            fail("The message has not been synchronized with the server yet.");
            return Ok(());
        }
        let res = (|| -> NetResult<Option<Vec<u8>>> {
            if self.c.selected() != Some(mb.path.as_str()) {
                self.c.select(&mb.path)?;
            }
            Ok(self.c.fetch_bodies(&[m.uid])?.into_iter().find(|f| f.uid == m.uid).and_then(|f| f.body))
        })();
        match res {
            Ok(Some(raw)) => {
                apply_body(store, id, &raw);
                self.touched.insert(mb.id);
                self.ctx.emit(Event::BodyFetched(id));
            }
            Ok(None) => self.ctx.emit(Event::BodyFetchFailed(id, "The message is no longer on the server.".into())),
            Err(e) if e.is_connection() => return Err(e),
            Err(e) => self.ctx.emit(Event::BodyFetchFailed(id, e.user_message())),
        }
        Ok(())
    }

    // -------------------------------------------------------------- queued operations

    /// Replay queued [`RemoteOp`]s in order.
    fn flush_ops(&mut self) -> NetResult<()> {
        let store = self.store();
        for (op_id, op, attempts) in store.pending_ops(&self.acct.id) {
            match self.replay(&op) {
                Ok(()) => {
                    let _ = store.remove_op(op_id);
                }
                Err(OpError::Drop(reason)) => {
                    log::info!("{}: discarding {op:?}: {reason}", self.acct.display_name());
                    let _ = store.remove_op(op_id);
                }
                Err(OpError::Net(e)) if e.is_connection() => return Err(e),
                Err(OpError::Net(e)) => {
                    log::warn!("{}: {op:?} failed: {e}", self.acct.display_name());
                    if attempts + 1 >= MAX_OP_ATTEMPTS {
                        let _ = store.remove_op(op_id);
                        self.ctx.emit(Event::Log(format!("{}: gave up on a change that could not be applied on the server ({}): {e}", self.acct.display_name(), describe_op(&op))));
                    } else {
                        let _ = store.bump_op_attempts(op_id);
                    }
                }
            }
        }
        Ok(())
    }

    fn op_mailbox(&self, id: MailboxId) -> Result<Mailbox, OpError> {
        match self.store().mailbox(id) {
            Some(mb) if mb.account_id == self.acct.id => Ok(mb),
            _ => Err(OpError::Drop(format!("mailbox {id} no longer exists"))),
        }
    }

    fn op_select(&mut self, path: &str) -> Result<(), OpError> {
        if self.c.selected() == Some(path) {
            return Ok(());
        }
        match self.c.select(path) {
            Ok(_) => Ok(()),
            Err(NetError::No(t)) | Err(NetError::Bad(t)) => Err(OpError::Drop(format!("cannot open “{path}”: {t}"))),
            Err(e) => Err(e.into()),
        }
    }

    /// Run `f`; if the server says the destination does not exist, create it and retry.
    fn with_trycreate(&mut self, dest: &str, f: impl Fn(&mut ImapClient) -> NetResult<()>) -> Result<(), OpError> {
        match f(self.c) {
            Err(NetError::No(t)) if is_trycreate(&t) => {
                self.create_exact(dest)?;
                f(self.c).map_err(OpError::from)
            }
            r => r.map_err(OpError::from),
        }
    }

    fn replay(&mut self, op: &RemoteOp) -> Result<(), OpError> {
        match op {
            RemoteOp::SetFlags { mailbox, uids, add, remove } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let mb = self.op_mailbox(*mailbox)?;
                self.op_select(&mb.path)?;
                self.c.uid_store(uids, true, &flags_to_imap(*add))?;
                self.c.uid_store(uids, false, &flags_to_imap(*remove))?;
            }
            RemoteOp::SetColor { mailbox, uids, color } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let mb = self.op_mailbox(*mailbox)?;
                self.op_select(&mb.path)?;
                let (add, remove) = color_keywords(*color);
                self.c.uid_store(uids, true, &add)?;
                self.c.uid_store(uids, false, &remove)?;
            }
            RemoteOp::Move { from, to, uids } | RemoteOp::Copy { from, to, uids } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let src = self.op_mailbox(*from)?;
                let dst = self.op_mailbox(*to)?;
                self.op_select(&src.path)?;
                let is_move = matches!(op, RemoteOp::Move { .. });
                self.with_trycreate(&dst.path, |c| if is_move { c.uid_move(uids, &dst.path) } else { c.uid_copy(uids, &dst.path) })?;
                self.touched.insert(*to);
            }
            RemoteOp::Delete { mailbox, uids } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let mb = self.op_mailbox(*mailbox)?;
                self.op_select(&mb.path)?;
                self.c.uid_delete(uids)?;
            }
            RemoteOp::Append { mailbox, message } => self.replay_append(*mailbox, *message)?,
            RemoteOp::Expunge { mailbox } => {
                let mb = self.op_mailbox(*mailbox)?;
                self.op_select(&mb.path)?;
                self.c.expunge()?;
            }
            RemoteOp::CreateMailbox { path } => self.create_mailbox(path)?,
            RemoteOp::RenameMailbox { from, to } => {
                match self.c.rename(from, to) {
                    Ok(()) => {}
                    Err(NetError::No(t)) if is_trycreate(&t) => return Err(OpError::Drop(format!("“{from}” does not exist on the server"))),
                    Err(e) => return Err(e.into()),
                }
                self.mailboxes_changed = true;
            }
            RemoteOp::DeleteMailbox { path } => {
                match self.c.delete(path) {
                    Ok(()) => {}
                    Err(NetError::No(t)) if is_trycreate(&t) => {}
                    Err(e) => return Err(e.into()),
                }
                self.mailboxes_changed = true;
            }
        }
        Ok(())
    }

    fn replay_append(&mut self, mailbox: MailboxId, message: MessageId) -> Result<(), OpError> {
        let store = self.store();
        let Some(m) = store.message(message) else {
            return Err(OpError::Drop("the message no longer exists".into()));
        };
        if m.uid != 0 {
            return Ok(());
        }
        // The message may have been moved locally before it reached the server.
        let target = if m.mailbox_id == mailbox {
            mailbox
        } else if store.mailbox(m.mailbox_id).is_some_and(|b| b.account_id == self.acct.id) {
            m.mailbox_id
        } else {
            return Err(OpError::Drop("the message was moved to another account".into()));
        };
        let mb = self.op_mailbox(target)?;
        let Some(raw) = store.raw(message) else {
            return Err(OpError::Drop("no local copy of the message source".into()));
        };
        let mut fl = flags_to_imap(m.flags & !flags::DELETED);
        if m.has(flags::FLAGGED) {
            fl.extend(color_keywords(m.color).0);
        }
        let date = (m.received > 0).then_some(m.received);
        let mut result = None;
        let path = mb.path.clone();
        {
            let r = std::cell::RefCell::new(&mut result);
            self.with_trycreate(&path, |c| {
                **r.borrow_mut() = c.append(&path, &fl, date, &raw)?;
                Ok(())
            })?;
        }
        if let Some((validity, uid)) = result
            && validity != 0 && validity == mb.uidvalidity && store.message_by_uid(target, uid).is_none() {
                let _ = store.set_uid(message, uid);
            }
        self.touched.insert(target);
        Ok(())
    }

    fn delimiter(&mut self) -> NetResult<Option<String>> {
        if let Some(d) = &self.delimiter {
            return Ok(d.clone());
        }
        let d = match self.c.hierarchy_delimiter() {
            Ok(d) => d,
            Err(e) if e.is_connection() => return Err(e),
            Err(_) => None,
        };
        self.delimiter = Some(d.clone());
        Ok(d)
    }

    /// Map a locally created mailbox path to the server namespace (prefix, delimiter).
    fn server_path(&mut self, local: &str) -> NetResult<String> {
        if local.eq_ignore_ascii_case("INBOX") {
            return Ok(local.to_string());
        }
        let delim = self.delimiter()?;
        let mut p = local.to_string();
        if let Some(d) = delim.as_deref().filter(|d| *d != "/") {
            p = p.replace('/', d);
        }
        let prefix = self.acct.imap_path_prefix.trim();
        if !prefix.is_empty() {
            let mut pre = prefix.to_string();
            if let Some(d) = delim.as_deref()
                && !pre.ends_with(d) {
                    pre.push_str(d);
                }
            if !p.starts_with(&pre) && !p.eq_ignore_ascii_case(prefix) {
                p = format!("{pre}{p}");
            }
        }
        Ok(p)
    }

    /// Create a mailbox exactly as named.
    fn create_exact(&mut self, path: &str) -> Result<(), OpError> {
        match self.c.create(path) {
            Ok(()) => {}
            Err(NetError::No(t)) if is_already_exists(&t) => {}
            Err(e) => return Err(e.into()),
        }
        if let Err(e) = self.c.subscribe(path)
            && e.is_connection() {
                return Err(e.into());
            }
        self.mailboxes_changed = true;
        Ok(())
    }

    /// Replay `CreateMailbox`: create on the server and adopt the server path locally.
    fn create_mailbox(&mut self, local: &str) -> Result<(), OpError> {
        let server = self.server_path(local)?;
        self.create_exact(&server)?;
        if server != local {
            let store = self.store();
            if let (Some(mb), None) = (store.mailbox_by_path(&self.acct.id, local), store.mailbox_by_path(&self.acct.id, &server)) {
                let delim = self.delimiter.clone().flatten().unwrap_or_else(|| "/".into());
                let name = server.rsplit(delim.as_str()).next().unwrap_or(&server).to_string();
                let _ = store.rename_mailbox(mb.id, &server, &name);
            }
        }
        Ok(())
    }

    /// UIDs of a mailbox that have queued operations (their local state must win).
    fn protected_uids(&self, mailbox: MailboxId) -> HashSet<u32> {
        let mut out = HashSet::new();
        for (_, op, _) in self.store().pending_ops(&self.acct.id) {
            match op {
                RemoteOp::SetFlags { mailbox: m, uids, .. } | RemoteOp::SetColor { mailbox: m, uids, .. } | RemoteOp::Delete { mailbox: m, uids } if m == mailbox => out.extend(uids),
                RemoteOp::Move { from, uids, .. } | RemoteOp::Copy { from, uids, .. } if from == mailbox => out.extend(uids),
                _ => {}
            }
        }
        out
    }

    // -------------------------------------------------------------- mailbox list

    fn sync_mailbox_list(&mut self) -> NetResult<()> {
        let store = self.store();
        let entries = self.c.list()?;
        let before = store.mailboxes(Some(&self.acct.id));
        // Mailboxes referenced by queued operations must survive until they are replayed.
        let mut protected_ids = HashSet::new();
        let mut protected_paths = HashSet::new();
        for (_, op, _) in store.pending_ops(&self.acct.id) {
            match op {
                RemoteOp::SetFlags { mailbox, .. } | RemoteOp::SetColor { mailbox, .. } | RemoteOp::Delete { mailbox, .. } | RemoteOp::Append { mailbox, .. } | RemoteOp::Expunge { mailbox } => {
                    protected_ids.insert(mailbox);
                }
                RemoteOp::Move { from, to, .. } | RemoteOp::Copy { from, to, .. } => {
                    protected_ids.insert(from);
                    protected_ids.insert(to);
                }
                RemoteOp::CreateMailbox { path } => {
                    protected_paths.insert(path);
                }
                RemoteOp::RenameMailbox { to, .. } => {
                    protected_paths.insert(to);
                }
                RemoteOp::DeleteMailbox { .. } => {}
            }
        }
        let mut seen: HashMap<String, MailboxId> = HashMap::new();
        let mut rows: Vec<(Mailbox, Option<String>)> = Vec::new();
        for e in &entries {
            let path = if e.path.eq_ignore_ascii_case("INBOX") { "INBOX".to_string() } else { e.path.clone() };
            if path.is_empty() || seen.contains_key(&path) {
                continue;
            }
            let existing = before.iter().find(|m| m.path == path);
            let role = match self.acct.role_overrides.get(&path) {
                Some(r) => *r,
                None => match MailboxRole::guess(&path, &e.attributes) {
                    // The Outbox is always local.
                    MailboxRole::Outbox => MailboxRole::Custom,
                    MailboxRole::Custom => existing.map(|m| m.role).filter(|r| *r != MailboxRole::Outbox).unwrap_or(MailboxRole::Custom),
                    r => r,
                },
            };
            let mb = Mailbox {
                account_id: self.acct.id.clone(),
                name: if path == "INBOX" { "INBOX".into() } else { e.name() },
                path: path.clone(),
                parent: None,
                role,
                delimiter: e.delimiter.clone().unwrap_or_else(|| "/".into()),
                selectable: e.selectable(),
                subscribed: true,
                ..Default::default()
            };
            match store.upsert_mailbox(&mb) {
                Ok(id) => {
                    seen.insert(path.clone(), id);
                    let parent = e.delimiter.as_deref().filter(|d| !d.is_empty()).and_then(|d| path.rsplit_once(d)).map(|(p, _)| p.to_string());
                    rows.push((Mailbox { id, ..mb }, parent));
                }
                Err(err) => log::warn!("cannot store mailbox {path}: {err}"),
            }
        }
        // Parents (second pass, now that every id is known).
        for (mut mb, parent) in rows {
            let pid = parent.and_then(|p| seen.get(&p).copied());
            if pid.is_some() {
                mb.parent = pid;
                let _ = store.upsert_mailbox(&mb);
            }
        }
        if !seen.contains_key("INBOX")
            && let Ok(id) = store.ensure_mailbox(&self.acct.id, "INBOX", MailboxRole::Inbox) {
                seen.insert("INBOX".into(), id);
            }
        // Mailboxes that disappeared from the server.
        for mb in &before {
            if seen.contains_key(&mb.path) || protected_ids.contains(&mb.id) || protected_paths.contains(&mb.path) {
                continue;
            }
            log::info!("{}: mailbox “{}” vanished from the server", self.acct.display_name(), mb.path);
            let _ = store.delete_mailbox(mb.id);
        }
        let after = store.mailboxes(Some(&self.acct.id));
        let key = |v: &[Mailbox]| -> Vec<(String, String, MailboxRole, Option<MailboxId>, bool)> {
            let mut k: Vec<_> = v.iter().map(|m| (m.path.clone(), m.name.clone(), m.role, m.parent, m.selectable)).collect();
            k.sort_by(|a, b| a.0.cmp(&b.0));
            k
        };
        if key(&before) != key(&after) {
            self.mailboxes_changed = true;
        }
        Ok(())
    }

    // -------------------------------------------------------------- mailboxes

    fn sync_all(&mut self, full: bool) -> NetResult<()> {
        let mut mbs: Vec<Mailbox> = self.store().mailboxes(Some(&self.acct.id)).into_iter().filter(|m| m.selectable).collect();
        mbs.sort_by_key(|m| role_order(m.role));
        let mut done = HashSet::new();
        for mb in mbs {
            self.service_priority()?;
            while let Some(id) = self.take_sync_request() {
                if done.insert(id)
                    && let Some(m) = self.store().mailbox(id).filter(|m| m.selectable && m.account_id == self.acct.id) {
                        self.sync_mailbox_logged(&m, false, false)?;
                    }
            }
            if !done.insert(mb.id) {
                continue;
            }
            let quick = !full && mb.role != MailboxRole::Inbox;
            self.sync_mailbox_logged(&mb, quick, full)?;
        }
        Ok(())
    }

    /// Like [`sync_mailbox`](Self::sync_mailbox), but errors limited to one mailbox are logged and skipped.
    fn sync_mailbox_logged(&mut self, mb: &Mailbox, quick: bool, full: bool) -> NetResult<()> {
        match self.sync_mailbox(mb, quick, full) {
            Err(e) if !e.is_connection() => {
                log::warn!("{}: cannot synchronize “{}”: {e}", self.acct.display_name(), mb.path);
                Ok(())
            }
            r => r,
        }
    }

    /// Synchronize one mailbox: new messages, flags and deletions.
    ///
    /// `quick` skips the mailbox when STATUS reports no change; `full` forces flag
    /// reconciliation even for very large mailboxes.
    fn sync_mailbox(&mut self, mb: &Mailbox, quick: bool, full: bool) -> NetResult<()> {
        let store = self.store();
        let mut known: HashMap<u32, (MessageId, u32, Option<FlagColor>)> = store.uid_map(mb.id).into_iter().map(|(u, id, f, c)| (u, (id, f, c))).collect();
        if quick && mb.uidvalidity != 0 {
            let st = self.c.status(&mb.path)?;
            if st.uidvalidity == mb.uidvalidity && st.uidnext != 0 && st.uidnext == mb.uidnext && st.messages as usize == known.len() {
                return Ok(());
            }
        }
        let info = self.c.select(&mb.path)?;
        let mut uidnext = mb.uidnext;
        if mb.uidvalidity != 0 && info.uidvalidity != 0 && info.uidvalidity != mb.uidvalidity {
            log::info!("{}: UIDVALIDITY of “{}” changed, reloading", self.acct.display_name(), mb.path);
            if let Err(e) = store.clear_mailbox(mb.id) {
                log::warn!("clear mailbox: {e}");
            }
            known.clear();
            uidnext = 0;
            self.touched.insert(mb.id);
        }
        let initial = uidnext == 0;
        if info.exists == 0 {
            if !known.is_empty() {
                let ids: Vec<MessageId> = known.values().map(|v| v.0).collect();
                let _ = store.delete_messages(&ids);
                self.touched.insert(mb.id);
            }
            let _ = store.set_mailbox_sync_state(mb.id, info.uidvalidity, info.uidnext.max(uidnext));
            return Ok(());
        }
        let start = uidnext.max(1);
        let protected = self.protected_uids(mb.id);

        // Fetch UID / FLAGS / size: of everything when reconciling, else of the new range.
        let mut items: Vec<FetchItem> = Vec::new();
        let mut reconciled = false;
        if !known.is_empty() && (full || known.len() <= RECONCILE_LIMIT) {
            items = self.c.fetch_sizes("1:*")?;
            reconciled = true;
        } else if info.uidnext == 0 || info.uidnext > start || info.exists as usize != known.len() {
            items = self.c.fetch_sizes(&format!("{start}:*"))?;
            let new_count = items.iter().filter(|f| f.uid >= start && !known.contains_key(&f.uid)).count();
            if !known.is_empty() && info.exists as usize != known.len() + new_count {
                items = self.c.fetch_sizes("1:*")?;
                reconciled = true;
            }
        }

        let mut server_uids = HashSet::new();
        let mut new_items: Vec<FetchItem> = Vec::new();
        let mut max_uid = 0;
        for it in items {
            max_uid = max_uid.max(it.uid);
            server_uids.insert(it.uid);
            if let Some((id, lflags, lcolor)) = known.get(&it.uid) {
                if protected.contains(&it.uid) {
                    continue;
                }
                let (bits, color) = flags_from_imap(it.flags.as_deref().unwrap_or_default());
                if (lflags & SERVER_BITS) != bits || *lcolor != color {
                    let _ = store.set_server_flags(*id, bits, color);
                    self.touched.insert(mb.id);
                }
            } else if (reconciled || it.uid >= start) && !protected.contains(&it.uid) {
                new_items.push(it);
            }
        }
        if reconciled {
            let vanished: Vec<MessageId> = known.iter().filter(|(u, _)| !server_uids.contains(u)).map(|(_, v)| v.0).collect();
            if !vanished.is_empty() {
                let _ = store.delete_messages(&vanished);
                self.touched.insert(mb.id);
            }
        }

        if !new_items.is_empty() {
            self.download_new(mb, &info, new_items, initial)?;
        }
        let next = info.uidnext.max(max_uid.saturating_add(1)).max(start);
        let _ = store.set_mailbox_sync_state(mb.id, info.uidvalidity, next);
        Ok(())
    }

    /// Download headers (and possibly bodies) of new messages, newest first.
    fn download_new(&mut self, mb: &Mailbox, info: &super::imap_client::SelectInfo, mut new_items: Vec<FetchItem>, initial: bool) -> NetResult<()> {
        let store = self.store();
        new_items.sort_by_key(|f| std::cmp::Reverse(f.uid));
        let total = new_items.len();
        let sizes: HashMap<u32, u32> = new_items.iter().map(|f| (f.uid, f.size.unwrap_or(0))).collect();
        let mut activity = (total > 10).then(|| ActivityTracker::start(self.ctx, &self.acct.id, format!("Fetching {total} messages"), mb.name.clone()));
        let mut prefetch: Vec<(u32, MessageId)> = Vec::new();
        let mut done = 0usize;
        for chunk in new_items.chunks(HEADER_BATCH) {
            if !self.reselect_after_service(mb, info)? {
                return Ok(());
            }
            let uids: Vec<u32> = chunk.iter().map(|f| f.uid).collect();
            let mut fetched = self.c.fetch_headers(&uids)?;
            fetched.sort_by_key(|f| std::cmp::Reverse(f.uid));
            for it in fetched {
                if !uids.contains(&it.uid) {
                    continue;
                }
                let header = it.header.clone().unwrap_or_default();
                let p = mime::parse(&header).unwrap_or_default();
                let (bits, color) = flags_from_imap(it.flags.as_deref().unwrap_or_default());
                // A message moved / appended locally: adopt the server UID instead of duplicating it.
                if let Some(local) = store.pending_message_by_msgid(mb.id, &p.message_id) {
                    let _ = store.set_uid(local, it.uid);
                    let _ = store.set_server_flags(local, bits, color);
                    self.touched.insert(mb.id);
                    continue;
                }
                if store.message_by_uid(mb.id, it.uid).is_some() {
                    continue;
                }
                let size = it.size.or_else(|| sizes.get(&it.uid).copied()).unwrap_or(0);
                let mut s = mime::summarize(&p, size as usize);
                s.mailbox_id = mb.id;
                s.account_id = self.acct.id.clone();
                s.uid = it.uid;
                s.size = size;
                s.flags = bits;
                s.color = color;
                s.received = it.internal_date.unwrap_or_else(now_ts);
                s.snippet = String::new();
                s.has_attachments = p.header("Content-Type").is_some_and(|ct| ct.to_ascii_lowercase().contains("multipart/mixed"));
                s.category = categorize::categorize(&p);
                match store.insert_message(&s, None, "") {
                    Ok(id) => {
                        self.touched.insert(mb.id);
                        if should_prefetch(self.acct.download_attachments, mb.role, size, s.date) {
                            prefetch.push((it.uid, id));
                        }
                        if mb.role == MailboxRole::Inbox && !initial {
                            self.new_inbox.push(id);
                            self.new_headers.insert(id, header);
                        }
                    }
                    Err(e) => log::warn!("cannot store message: {e}"),
                }
            }
            done += chunk.len();
            if let Some(a) = activity.as_mut() {
                a.update(Some(done as f32 / total as f32 * if prefetch.is_empty() { 1.0 } else { 0.5 }), format!("{} — {done} of {total}", mb.name));
            }
            // Let the UI show messages as they arrive.
            self.ctx.emit(Event::MessagesChanged(mb.id));
        }

        // Bodies, in batches limited by count and size.
        let total_bodies = prefetch.len();
        let mut fetched_bodies = 0usize;
        let mut i = 0;
        while i < prefetch.len() {
            let mut batch = Vec::new();
            let mut bytes = 0u64;
            while i < prefetch.len() && batch.len() < BODY_BATCH_MESSAGES && (batch.is_empty() || bytes < BODY_BATCH_BYTES) {
                bytes += sizes.get(&prefetch[i].0).copied().unwrap_or(0) as u64;
                batch.push(prefetch[i]);
                i += 1;
            }
            if !self.reselect_after_service(mb, info)? {
                return Ok(());
            }
            let uids: Vec<u32> = batch.iter().map(|(u, _)| *u).collect();
            for it in self.c.fetch_bodies(&uids)? {
                if let (Some(raw), Some((_, id))) = (it.body.as_deref(), batch.iter().find(|(u, _)| *u == it.uid))
                    && store.raw(*id).is_none() {
                        apply_body(store, *id, raw);
                    }
            }
            fetched_bodies += batch.len();
            if let Some(a) = activity.as_mut() {
                a.update(Some(0.5 + 0.5 * fetched_bodies as f32 / total_bodies as f32), format!("{} — downloading message contents", mb.name));
            }
        }
        if let Some(a) = activity.take() {
            a.done();
        }
        Ok(())
    }

    /// Serve priority requests, then make sure `mb` is selected again.
    /// Returns false when the mailbox changed identity and the pass should stop.
    fn reselect_after_service(&mut self, mb: &Mailbox, info: &super::imap_client::SelectInfo) -> NetResult<bool> {
        self.service_priority()?;
        if self.c.selected() != Some(mb.path.as_str()) {
            let again = self.c.select(&mb.path)?;
            if again.uidvalidity != info.uidvalidity {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Run junk filtering, blocked senders and rules on new Inbox messages.
    fn finish_new_mail(&mut self) -> NetResult<()> {
        if self.new_inbox.is_empty() {
            return Ok(());
        }
        let ids = std::mem::take(&mut self.new_inbox);
        let headers = std::mem::take(&mut self.new_headers);
        let Some(inbox) = self.store().mailbox_by_role(&self.acct.id, MailboxRole::Inbox) else { return Ok(()) };
        let notify = process_new_mail(self.ctx, self.acct, inbox.id, &ids, &headers, &mut self.touched);
        self.notify.extend(notify);
        // Push the changes made by the junk filter and rules.
        self.flush_ops()
    }
}

fn describe_op(op: &RemoteOp) -> String {
    match op {
        RemoteOp::SetFlags { uids, .. } => format!("change flags of {} messages", uids.len()),
        RemoteOp::SetColor { uids, .. } => format!("change flag color of {} messages", uids.len()),
        RemoteOp::Move { uids, .. } => format!("move {} messages", uids.len()),
        RemoteOp::Copy { uids, .. } => format!("copy {} messages", uids.len()),
        RemoteOp::Delete { uids, .. } => format!("delete {} messages", uids.len()),
        RemoteOp::Append { .. } => "upload a message".into(),
        RemoteOp::Expunge { .. } => "erase deleted messages".into(),
        RemoteOp::CreateMailbox { path } => format!("create mailbox “{path}”"),
        RemoteOp::RenameMailbox { from, to } => format!("rename mailbox “{from}” to “{to}”"),
        RemoteOp::DeleteMailbox { path } => format!("delete mailbox “{path}”"),
    }
}

// ------------------------------------------------------------------ new mail pipeline

/// Junk filter, blocked senders and rules for messages that just arrived in `inbox`.
/// Returns the messages to announce (still in the Inbox, unread and not junk).
fn process_new_mail(ctx: &Ctx, acct: &AccountConfig, inbox: MailboxId, ids: &[MessageId], headers: &HashMap<MessageId, Vec<u8>>, touched: &mut BTreeSet<MailboxId>) -> Vec<MessageId> {
    let store = &*ctx.store;
    let settings = ctx.settings();
    touched.insert(inbox);
    let parsed = |id: MessageId| -> Option<ParsedMessage> { store.raw(id).or_else(|| headers.get(&id).cloned()).and_then(|r| mime::parse(&r)) };
    let mut remaining: Vec<MessageId> = ids.to_vec();
    if settings.junk.enabled && settings.junk.filter_before_rules {
        remaining = filter_junk(store, &settings, acct, &remaining, &parsed, touched);
    }
    remaining = filter_blocked(store, acct, &remaining, touched);
    if !remaining.is_empty() && settings.rules.iter().any(|r| r.enabled) {
        let effects = rules::apply_rules(store, &settings, &remaining, None);
        execute_side_effects(ctx, acct, effects);
        remaining.retain(|id| store.message(*id).is_some_and(|m| m.mailbox_id == inbox));
    }
    if settings.junk.enabled && !settings.junk.filter_before_rules {
        remaining = filter_junk(store, &settings, acct, &remaining, &parsed, touched);
    }
    for m in store.messages_by_ids(ids) {
        touched.insert(m.mailbox_id);
    }
    // Rules may have queued operations for other accounts.
    ctx.request_flush(settings.accounts.iter().filter(|a| a.id != acct.id && !store.pending_ops(&a.id).is_empty()).map(|a| a.id.clone()));
    store.messages_by_ids(&remaining).into_iter().filter(|m| m.mailbox_id == inbox && m.is_unread() && !m.has(flags::JUNK)).map(|m| m.id).collect()
}

/// Whether the message is addressed to one of the account's addresses using the user's full name.
fn addressed_with_full_name(m: &MessageSummary, acct: &AccountConfig) -> bool {
    let full = acct.full_name.trim();
    !full.is_empty() && m.to.iter().chain(m.cc.iter()).any(|a| acct.owns_address(&a.email) && a.name.trim().eq_ignore_ascii_case(full))
}

/// Apply the junk filter; returns the messages that stay in the Inbox.
fn filter_junk(store: &Store, settings: &Settings, acct: &AccountConfig, ids: &[MessageId], parsed: &dyn Fn(MessageId) -> Option<ParsedMessage>, touched: &mut BTreeSet<MailboxId>) -> Vec<MessageId> {
    let jp = &settings.junk;
    let contacts = if jp.exempt_sender_in_contacts { store.contact_emails() } else { HashSet::new() };
    let previous: HashSet<String> = if jp.exempt_previous_recipients { store.previous_recipients().into_iter().map(|r| r.email.to_lowercase()).collect() } else { HashSet::new() };
    let mine: HashSet<String> = settings.my_addresses().into_iter().collect();
    let mut keep = Vec::new();
    for m in store.messages_by_ids(ids) {
        let from = m.from.email.to_lowercase();
        let exempt = m.has(flags::NOT_JUNK) || mine.contains(&from) || contacts.contains(&from) || previous.contains(&from) || (jp.exempt_full_name_addressed && addressed_with_full_name(&m, acct));
        let is_junk = !exempt
            && (m.has(flags::JUNK)
                || parsed(m.id).is_some_and(|p| (jp.trust_server_headers && junk::server_says_junk(&p)) || junk::score(store, &p) >= jp.threshold));
        if !is_junk {
            keep.push(m.id);
            continue;
        }
        log::info!("junk filter: “{}” from {}", m.subject, m.from.email);
        if let Err(e) = ops::set_flags(store, &[m.id], flags::JUNK, flags::NOT_JUNK) {
            log::warn!("junk filter: {e}");
        }
        match jp.action {
            JunkAction::MoveToJunk => match ops::role_mailbox(store, &m.account_id, MailboxRole::Junk).and_then(|jm| ops::move_to(store, &[m.id], jm.id).map(|_| jm.id)) {
                Ok(jm) => {
                    touched.insert(jm);
                }
                Err(e) => {
                    log::warn!("junk filter: cannot move message: {e}");
                    keep.push(m.id);
                }
            },
            // Marked only (or left to custom rules): stays in the Inbox, flagged as junk.
            JunkAction::MarkOnly | JunkAction::Custom => keep.push(m.id),
        }
    }
    keep
}

/// Move mail from blocked senders to the Trash; returns the other messages.
fn filter_blocked(store: &Store, acct: &AccountConfig, ids: &[MessageId], touched: &mut BTreeSet<MailboxId>) -> Vec<MessageId> {
    let (blocked, keep): (Vec<MessageSummary>, Vec<MessageSummary>) = store.messages_by_ids(ids).into_iter().partition(|m| store.is_blocked(&m.from.email));
    if !blocked.is_empty() {
        let bids: Vec<MessageId> = blocked.iter().map(|m| m.id).collect();
        match ops::role_mailbox(store, &acct.id, MailboxRole::Trash).and_then(|t| ops::move_to(store, &bids, t.id).map(|_| t.id)) {
            Ok(t) => {
                touched.insert(t);
            }
            Err(e) => log::warn!("blocked senders: {e}"),
        }
    }
    let keep_ids: HashSet<MessageId> = keep.iter().map(|m| m.id).collect();
    ids.iter().copied().filter(|id| keep_ids.contains(id)).collect()
}

/// Perform rule side effects that the store cannot handle itself.
fn execute_side_effects(ctx: &Ctx, acct: &AccountConfig, effects: Vec<SideEffect>) {
    let mut changed = false;
    for e in effects {
        match e {
            SideEffect::Notify { title, body } => ctx.emit(Event::Log(format!("notify:{title}\n{body}"))),
            SideEffect::PlaySound(name) => ctx.emit(Event::Log(format!("sound:{name}"))),
            SideEffect::Send(msg) => match ops::queue_outgoing(&ctx.store, &msg, None) {
                Ok(id) => {
                    if let Ok(ob) = ops::outbox(&ctx.store) {
                        ctx.emit(Event::MessagesChanged(ob.id));
                    }
                    let _ = ctx.send_jobs.send(SendJob::Send { outbox_id: id, message: msg });
                }
                Err(e) => log::warn!("rules: cannot queue message: {e}"),
            },
            SideEffect::RunCommand { command, message } => run_rule_command(&ctx.store, &command, message),
            SideEffect::Changed => changed = true,
        }
    }
    if changed {
        ctx.emit(Event::MailboxesChanged(acct.id.clone()));
    }
}

/// Run a rule's shell command with the message source saved to a temporary file (`$1`).
fn run_rule_command(store: &Store, command: &str, message: MessageId) {
    let raw = store.raw(message).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("purpamail-rule-{message}-{nanos}.eml"));
    if let Err(e) = std::fs::write(&path, &raw) {
        log::warn!("rules: cannot write {}: {e}", path.display());
        return;
    }
    let command = command.to_string();
    spawn_named("purpamail-rule-command", move || {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .arg("purpamail-rule")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if !s.success() => log::warn!("rules: command “{command}” exited with {s}"),
            Err(e) => log::warn!("rules: cannot run “{command}”: {e}"),
            _ => {}
        }
        let _ = std::fs::remove_file(&path);
    });
}

// ------------------------------------------------------------------ Outbox sender

enum SendJob {
    Send { outbox_id: MessageId, message: Box<OutgoingMessage> },
    /// Deliver every due Outbox message; `user` ignores the retry backoff.
    Flush { user: bool },
    Shutdown,
}

/// Delivers Outbox messages over SMTP, one at a time.
struct OutboxSender {
    ctx: Ctx,
    rx: Receiver<SendJob>,
    /// Failed deliveries: attempts and earliest automatic retry.
    failures: HashMap<MessageId, (u32, Instant)>,
}

impl OutboxSender {
    fn run(mut self) {
        while let Ok(first) = self.rx.recv() {
            // Collapse queued flush requests.
            let mut jobs = vec![first];
            jobs.extend(self.rx.try_iter());
            let mut flush: Option<bool> = None;
            for job in jobs {
                match job {
                    SendJob::Shutdown => return,
                    SendJob::Send { outbox_id, message } => self.send(outbox_id, &message, None, true),
                    SendJob::Flush { user } => flush = Some(flush.unwrap_or(false) || user),
                }
            }
            if let Some(user) = flush {
                self.flush(user);
            }
        }
    }

    /// Deliver every due message in the Outbox.
    fn flush(&mut self, user: bool) {
        let store = self.ctx.store.clone();
        let settings = self.ctx.settings();
        let Ok(ob) = ops::outbox(&store) else { return };
        let rows = store.list_messages(&MessageQuery { mailboxes: vec![ob.id], include_deleted: true, ascending: true, ..Default::default() });
        let now = now_ts();
        // Freshly queued messages without a schedule are left alone during the undo-send delay.
        let delay = settings.general.undo_send_delay_secs as i64;
        for row in rows {
            let due = match row.send_at {
                Some(t) => t <= now,
                None => row.received <= now - delay,
            };
            if !due {
                continue;
            }
            let Some(raw) = store.raw(row.id) else { continue };
            match smtp::queued_from_raw(&raw, &settings) {
                Some(q) => self.send(row.id, &q.message, Some(q.data), user),
                None => log::warn!("outbox: cannot parse queued message {}", row.id),
            }
        }
        self.failures.retain(|id, _| store.message(*id).is_some());
    }

    /// Deliver one Outbox message. `data` overrides the bytes generated from `msg`.
    fn send(&mut self, outbox_id: MessageId, msg: &OutgoingMessage, data: Option<Vec<u8>>, explicit: bool) {
        let ctx = self.ctx.clone();
        let store = &ctx.store;
        let settings = ctx.settings();
        let Some(row) = store.message(outbox_id) else {
            log::debug!("outbox: message {outbox_id} is gone (already sent?)");
            return;
        };
        if row.send_at.is_some_and(|t| t > now_ts()) {
            return;
        }
        if !explicit && self.failures.get(&outbox_id).is_some_and(|(_, next)| Instant::now() < *next) {
            return;
        }
        let acct = settings.account(&msg.account_id).or_else(|| settings.account_for_address(&msg.from.email)).cloned();
        let Some(acct) = acct else {
            let error = format!("No account is set up to send from “{}”.", msg.from.email);
            self.record_failure(outbox_id, true);
            ctx.emit(Event::SendFailed { outbox_id, error });
            return;
        };
        if !ctx.is_online(&acct.id) {
            if explicit {
                ctx.emit(Event::SendFailed { outbox_id, error: "You are offline. The message will be sent when you go back online.".into() });
            }
            return;
        }
        let (from, rcpts) = smtp::envelope(msg, &acct, &settings);
        let data = data.unwrap_or_else(|| {
            let mut m = msg.clone();
            if m.message_id.is_empty() {
                m.message_id = row.message_id.clone();
            }
            m.to_mime(false)
        });
        let password = ctx.secrets.read().outgoing(&acct);
        let subject = if msg.subject.trim().is_empty() { "(No Subject)" } else { msg.subject.trim() };
        let act = ActivityTracker::start(&ctx, &acct.id, format!("Sending “{subject}”"), acct.display_name());
        match smtp::deliver(&acct, &password, &from, &rcpts, &data) {
            Ok(()) => {
                act.done();
                self.failures.remove(&outbox_id);
                let mut msg = msg.clone();
                if msg.account_id.is_empty() {
                    msg.account_id = acct.id.clone();
                }
                let sent_id = match ops::after_sent(store, &settings, outbox_id, &msg) {
                    Ok((sent, accts)) => {
                        ctx.request_flush(accts);
                        sent
                    }
                    Err(e) => {
                        log::warn!("outbox: bookkeeping after sending failed: {e}");
                        None
                    }
                };
                ctx.emit(Event::MessagesChanged(row.mailbox_id));
                if let Some(s) = sent_id.and_then(|s| store.message(s)) {
                    ctx.emit(Event::MessagesChanged(s.mailbox_id));
                }
                ctx.emit(Event::SendSucceeded { outbox_id, sent_id });
            }
            Err(e) => {
                act.fail(&e.message);
                log::warn!("outbox: sending {outbox_id} failed: {e}");
                self.record_failure(outbox_id, e.permanent);
                ctx.emit(Event::SendFailed { outbox_id, error: e.message });
            }
        }
    }

    fn record_failure(&mut self, outbox_id: MessageId, permanent: bool) {
        let entry = self.failures.entry(outbox_id).or_insert((0, Instant::now()));
        entry.0 += 1;
        let secs = if permanent { 3600 } else { (30u64 << entry.0.min(6)).min(1800) };
        entry.1 = Instant::now() + Duration::from_secs(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_merges_duplicates() {
        let mut q = VecDeque::new();
        enqueue(&mut q, WorkerMsg::Check { full: false, user: false });
        enqueue(&mut q, WorkerMsg::FetchBody(1));
        enqueue(&mut q, WorkerMsg::Check { full: true, user: false });
        enqueue(&mut q, WorkerMsg::FetchBody(1));
        enqueue(&mut q, WorkerMsg::IdleChanged);
        enqueue(&mut q, WorkerMsg::IdleChanged);
        assert_eq!(q.len(), 3);
        assert_eq!(q[0], WorkerMsg::Check { full: true, user: false });
    }

    #[test]
    fn backoff_grows() {
        let mut b = Backoff::default();
        assert_eq!(b.fail(false).as_secs(), 5);
        assert_eq!(b.fail(false).as_secs(), 10);
        assert_eq!(b.fail(false).as_secs(), 20);
        assert!(b.waiting());
        for _ in 0..20 {
            b.fail(false);
        }
        assert_eq!(b.fail(false).as_secs(), 900);
        b.reset();
        assert!(!b.waiting());
        assert_eq!(b.fail(true).as_secs(), 300);
    }

    #[test]
    fn prefetch_policy() {
        let now = now_ts();
        assert!(should_prefetch(DownloadAttachments::All, MailboxRole::Inbox, 5_000_000, now));
        assert!(!should_prefetch(DownloadAttachments::None, MailboxRole::Inbox, 5_000_000, now));
        assert!(should_prefetch(DownloadAttachments::None, MailboxRole::Inbox, 10_000, now));
        assert!(!should_prefetch(DownloadAttachments::All, MailboxRole::Trash, 100_000, now));
        assert!(!should_prefetch(DownloadAttachments::Recent, MailboxRole::Inbox, 500_000, now - 90 * 86400));
    }
}
