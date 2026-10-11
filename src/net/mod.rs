//! Networking: IMAP / POP3 retrieval, SMTP delivery and the background sync engine.
//!
//! The UI talks to the engine exclusively through [`Command`]s and receives
//! [`Event`]s back. All mail data flows through the shared [`Store`](crate::core::store::Store).

pub mod engine;
pub mod imap_client;
pub mod pop3;
pub mod smtp;
pub mod doctor;

use crate::core::compose::OutgoingMessage;
use crate::core::model::{AccountId, MailboxId, MessageId};

pub use engine::{Engine, EngineHandle};

#[derive(Debug, Clone)]
pub enum Command {
    /// Check for new mail in one account or all accounts (None).
    CheckMail(Option<AccountId>),
    /// Fully synchronize an account (mailbox list, flags, deletions).
    Synchronize(AccountId),
    /// Synchronize one mailbox now (e.g. when the user selects it).
    SyncMailbox(MailboxId),
    /// Download the full body of a message.
    FetchBody(MessageId),
    /// Replay queued [`RemoteOp`](crate::core::store::RemoteOp)s for an account.
    FlushOps(AccountId),
    /// Queue a message for delivery. It has already been stored in the Outbox
    /// with local id `outbox_id`.
    Send { outbox_id: MessageId, message: Box<OutgoingMessage> },
    /// Try to deliver everything in the Outbox that is due.
    FlushOutbox,
    /// Erase deleted items / junk according to account settings.
    Maintenance,
    /// Take an account (or all) online / offline.
    SetOnline { account: Option<AccountId>, online: bool },
    /// Settings changed (accounts added / removed / edited).
    ReloadSettings,
    /// Run connection diagnostics for all accounts.
    ConnectionDoctor,
    /// Test credentials for an account configuration before saving it.
    TestAccount(Box<crate::core::config::AccountConfig>, String),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActivityState {
    Running,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub id: u64,
    pub account: AccountId,
    pub title: String,
    pub detail: String,
    /// 0.0 ..= 1.0, or None for indeterminate.
    pub progress: Option<f32>,
    pub state: ActivityState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceStatus {
    Unknown,
    Ok,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DoctorResult {
    pub account: AccountId,
    pub account_name: String,
    pub incoming: ServiceStatus,
    pub outgoing: ServiceStatus,
    pub log: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Activity(Activity),
    /// Mailbox list of an account changed.
    MailboxesChanged(AccountId),
    /// Messages in a mailbox were added / removed / changed.
    MessagesChanged(MailboxId),
    /// New messages arrived (after rules and junk filtering).
    NewMail { account: AccountId, messages: Vec<MessageId> },
    BodyFetched(MessageId),
    BodyFetchFailed(MessageId, String),
    /// Account went online / offline or hit an error.
    AccountStatus { account: AccountId, online: bool, error: Option<String> },
    SendSucceeded { outbox_id: MessageId, sent_id: Option<MessageId> },
    SendFailed { outbox_id: MessageId, error: String },
    Doctor(DoctorResult),
    TestAccountResult { incoming: Result<(), String>, outgoing: Result<(), String> },
    Log(String),
}
