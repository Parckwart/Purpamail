//! The action bus: UI components request work by pushing [`Action`]s, which the
//! application processes centrally once per frame.

use crate::core::compose::{OutgoingMessage, ReplyKind};
use crate::core::config::{EraseAfter, SmartMailbox};
use crate::core::model::*;
use crate::net::EngineHandle;
use crate::net::engine::{SharedSecrets, SharedSettings};
use crate::core::store::SharedStore;

/// How a compose window is initialised.
#[derive(Clone, Debug)]
pub enum ComposeInit {
    New { to: Vec<Address>, cc: Vec<Address>, subject: String, body: String, attachments: Vec<std::path::PathBuf> },
    /// Reply / forward / redirect to an existing message.
    Respond { message: MessageId, kind: ReplyKind, selected_text: Option<String> },
    /// Forward several messages as attachments.
    ForwardMany(Vec<MessageId>),
    /// A `mailto:` URL.
    Mailto(String),
    /// Start from a template / stationery.
    Template(String),
    /// Re-open an outgoing message (e.g. after "Undo Send").
    Outgoing(Box<OutgoingMessage>),
}

impl ComposeInit {
    pub fn blank() -> Self {
        ComposeInit::New { to: vec![], cc: vec![], subject: String::new(), body: String::new(), attachments: vec![] }
    }
}

/// What the sidebar has selected.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SidebarSel {
    Mailbox(MailboxId),
    /// All mailboxes with this role across accounts ("All Inboxes", "All Sent", ...).
    AllRole(MailboxRole),
    /// Flagged messages, optionally only one color.
    Flagged(Option<FlagColor>),
    /// All VIPs, or one VIP by email.
    Vip(Option<String>),
    Smart(String),
    /// Search results (all mailboxes).
    Search,
    /// Messages with reminders.
    Reminders,
    /// Sent messages without a reply after a few days.
    FollowUp,
}

#[derive(Clone, Debug)]
pub enum Action {
    // ---- selection / navigation
    SelectSidebar(SidebarSel),
    SelectMessages(Vec<MessageId>),
    /// Open the message in its own window.
    OpenMessageWindow(MessageId),
    ShowRawSource(MessageId),
    Search(String),
    // ---- compose
    Compose(ComposeInit),
    /// Deliver a composed message; `send_at` schedules it (Send Later).
    Send { message: Box<OutgoingMessage>, send_at: Option<i64>, draft: Option<MessageId> },
    /// Cancel a scheduled / undo-able send and reopen the message.
    UndoSend(MessageId),
    // ---- message operations
    SetFlags { ids: Vec<MessageId>, add: u32, remove: u32 },
    ToggleRead(Vec<MessageId>),
    SetFlagColor { ids: Vec<MessageId>, color: Option<FlagColor> },
    Move { ids: Vec<MessageId>, dest: MailboxId },
    Copy { ids: Vec<MessageId>, dest: MailboxId },
    Delete(Vec<MessageId>),
    Archive(Vec<MessageId>),
    MarkJunk { ids: Vec<MessageId>, junk: bool },
    Mute { thread: i64, muted: bool },
    Remind { ids: Vec<MessageId>, at: Option<i64> },
    Unsubscribe(MessageId),
    BlockSender { email: String, blocked: bool },
    SetVip { address: Address, vip: bool },
    AddToContacts(Address),
    ApplyRules(Vec<MessageId>),
    RemoveAttachments(Vec<MessageId>),
    SetCategory { id: MessageId, category: Category },
    FetchBody(MessageId),
    // ---- attachments & files
    OpenAttachment { message: MessageId, index: usize },
    SaveAttachment { message: MessageId, index: usize },
    SaveAllAttachments(MessageId),
    SaveMessageAs(MessageId),
    ExportPdf(MessageId),
    Print(MessageId),
    OpenUrl(String),
    /// Reply to a calendar invitation.
    Rsvp { message: MessageId, response: crate::core::ics::Rsvp },
    // ---- mailboxes
    CheckMail(Option<AccountId>),
    Synchronize(AccountId),
    NewMailbox { parent: Option<MailboxId>, account: AccountId },
    RenameMailbox(MailboxId),
    DeleteMailbox(MailboxId),
    UseMailboxFor { mailbox: MailboxId, role: MailboxRole },
    EraseDeleted(Option<AccountId>),
    EraseJunk,
    MarkAllRead(Vec<MailboxId>),
    ImportMailboxes,
    ExportMailbox(MailboxId),
    RebuildMailbox(MailboxId),
    NewSmartMailbox(Option<SmartMailbox>),
    EditSmartMailbox(String),
    DeleteSmartMailbox(String),
    AddFavorite(crate::core::config::FavoriteItem),
    RemoveFavorite(crate::core::config::FavoriteItem),
    SetOnline { account: Option<AccountId>, online: bool },
    EraseOlderThan { account: AccountId, role: MailboxRole, after: EraseAfter },
    // ---- windows
    ShowPreferences(Option<PrefsTab>),
    ShowActivity,
    ShowConnectionDoctor,
    ShowContacts,
    ShowPreviousRecipients,
    AddAccount,
    ShowAbout,
    /// Show a transient message at the bottom of the window.
    Toast(String),
    SettingsChanged,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrefsTab {
    #[default]
    General,
    Accounts,
    Junk,
    Fonts,
    Viewing,
    Composing,
    Signatures,
    Rules,
    Privacy,
}

/// Everything a UI component may need, passed down each frame.
pub struct UiCx<'a> {
    pub store: &'a SharedStore,
    pub settings: &'a SharedSettings,
    pub secrets: &'a SharedSecrets,
    pub engine: &'a EngineHandle,
    pub actions: &'a mut Vec<Action>,
}

impl UiCx<'_> {
    pub fn act(&mut self, a: Action) {
        self.actions.push(a);
    }
}
