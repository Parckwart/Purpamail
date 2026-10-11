//! Persistent configuration: accounts, preferences, rules, smart mailboxes, signatures.
//!
//! Stored as TOML in `$XDG_CONFIG_HOME/purpamail/settings.toml`. Passwords are kept in
//! a separate file (`secrets.toml`, mode 0600) so that settings can be shared safely.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::criteria::Criteria;
use super::model::{AccountId, Category, FlagColor, MailboxId, MailboxRole, SortKey};

pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PURPAMAIL_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("org", "Purpamail", "purpamail").map(|d| d.config_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from(".purpamail"))
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PURPAMAIL_DATA_DIR") {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("org", "Purpamail", "purpamail").map(|d| d.data_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from(".purpamail"))
}

pub fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from(".")))
}

// ------------------------------------------------------------------ accounts

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AccountKind {
    #[default]
    Imap,
    Pop3,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Security {
    None,
    StartTls,
    #[default]
    Tls,
}

impl Security {
    pub fn name(&self) -> &'static str {
        match self {
            Security::None => "None",
            Security::StartTls => "STARTTLS",
            Security::Tls => "SSL/TLS",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AuthMethod {
    #[default]
    Password,
    /// OAuth 2.0 bearer token supplied in the password field.
    OAuth2,
    None,
}

impl AuthMethod {
    pub fn name(&self) -> &'static str {
        match self {
            AuthMethod::Password => "Password",
            AuthMethod::OAuth2 => "OAuth 2.0 token",
            AuthMethod::None => "None",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub auth: AuthMethod,
    /// Accept invalid / self-signed certificates.
    #[serde(default)]
    pub allow_insecure_certs: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EraseAfter {
    Never,
    OneDay,
    OneWeek,
    #[default]
    OneMonth,
    WhenQuitting,
}

impl EraseAfter {
    pub const ALL: [EraseAfter; 5] = [EraseAfter::Never, EraseAfter::OneDay, EraseAfter::OneWeek, EraseAfter::OneMonth, EraseAfter::WhenQuitting];
    pub fn name(&self) -> &'static str {
        match self {
            EraseAfter::Never => "Never",
            EraseAfter::OneDay => "One day old",
            EraseAfter::OneWeek => "One week old",
            EraseAfter::OneMonth => "One month old",
            EraseAfter::WhenQuitting => "When quitting Purpamail",
        }
    }
    pub fn seconds(&self) -> Option<i64> {
        match self {
            EraseAfter::OneDay => Some(86400),
            EraseAfter::OneWeek => Some(7 * 86400),
            EraseAfter::OneMonth => Some(30 * 86400),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Pop3Removal {
    Never,
    OneDay,
    #[default]
    OneWeek,
    OneMonth,
    WhenMovedFromInbox,
}

impl Pop3Removal {
    pub const ALL: [Pop3Removal; 5] = [Pop3Removal::Never, Pop3Removal::OneDay, Pop3Removal::OneWeek, Pop3Removal::OneMonth, Pop3Removal::WhenMovedFromInbox];
    pub fn name(&self) -> &'static str {
        match self {
            Pop3Removal::Never => "Never",
            Pop3Removal::OneDay => "After one day",
            Pop3Removal::OneWeek => "After one week",
            Pop3Removal::OneMonth => "After one month",
            Pop3Removal::WhenMovedFromInbox => "When moved from Inbox",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DownloadAttachments {
    #[default]
    All,
    Recent,
    None,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct AccountConfig {
    pub id: AccountId,
    pub enabled: bool,
    pub kind: AccountKind,
    /// Description shown in the sidebar.
    pub name: String,
    pub full_name: String,
    /// Email addresses; the first is the primary address, the rest are aliases.
    pub emails: Vec<String>,
    pub incoming: ServerConfig,
    pub outgoing: ServerConfig,
    /// Whether to use the incoming credentials for SMTP.
    pub smtp_same_credentials: bool,
    // Mailbox behaviors
    pub store_drafts_on_server: bool,
    pub store_sent_on_server: bool,
    pub store_junk_on_server: bool,
    pub move_deleted_to_trash: bool,
    pub store_deleted_on_server: bool,
    pub erase_deleted: EraseAfter,
    pub erase_junk: EraseAfter,
    pub erase_sent: EraseAfter,
    pub archive_mailbox_path: String,
    // Advanced
    pub include_when_checking: bool,
    pub download_attachments: DownloadAttachments,
    pub use_idle: bool,
    pub imap_path_prefix: String,
    pub pop3_remove: Pop3Removal,
    pub pop3_size_warning_kb: u32,
    pub signature: Option<String>,
    /// Explicit role assignments by server path, overriding automatic detection.
    pub role_overrides: BTreeMap<String, MailboxRole>,
    pub auto_bcc_myself: bool,
}

impl Default for AccountConfig {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            enabled: true,
            kind: AccountKind::Imap,
            name: String::new(),
            full_name: String::new(),
            emails: vec![],
            incoming: ServerConfig { port: 993, security: Security::Tls, ..Default::default() },
            outgoing: ServerConfig { port: 587, security: Security::StartTls, ..Default::default() },
            smtp_same_credentials: true,
            store_drafts_on_server: true,
            store_sent_on_server: true,
            store_junk_on_server: true,
            move_deleted_to_trash: true,
            store_deleted_on_server: true,
            erase_deleted: EraseAfter::Never,
            erase_junk: EraseAfter::OneMonth,
            erase_sent: EraseAfter::Never,
            archive_mailbox_path: String::new(),
            include_when_checking: true,
            download_attachments: DownloadAttachments::All,
            use_idle: true,
            imap_path_prefix: String::new(),
            pop3_remove: Pop3Removal::OneWeek,
            pop3_size_warning_kb: 0,
            signature: None,
            role_overrides: BTreeMap::new(),
            auto_bcc_myself: false,
        }
    }
}

impl AccountConfig {
    pub fn primary_email(&self) -> &str {
        self.emails.first().map(|s| s.as_str()).unwrap_or("")
    }
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { self.primary_email() } else { &self.name }
    }
    pub fn owns_address(&self, email: &str) -> bool {
        self.emails.iter().any(|e| e.eq_ignore_ascii_case(email))
    }
    /// Fill in server defaults for well-known providers from the email domain.
    pub fn autoconfigure(&mut self, email: &str) {
        let domain = email.rsplit_once('@').map(|(_, d)| d.to_lowercase()).unwrap_or_default();
        let (imap, smtp): (&str, &str) = match domain.as_str() {
            "gmail.com" | "googlemail.com" => ("imap.gmail.com", "smtp.gmail.com"),
            "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => ("outlook.office365.com", "smtp.office365.com"),
            "yahoo.com" | "ymail.com" => ("imap.mail.yahoo.com", "smtp.mail.yahoo.com"),
            "aol.com" => ("imap.aol.com", "smtp.aol.com"),
            "gmx.de" | "gmx.net" | "gmx.com" => ("imap.gmx.net", "mail.gmx.net"),
            "web.de" => ("imap.web.de", "smtp.web.de"),
            "posteo.de" | "posteo.net" => ("posteo.de", "posteo.de"),
            "mailbox.org" => ("imap.mailbox.org", "smtp.mailbox.org"),
            "fastmail.com" | "fastmail.fm" => ("imap.fastmail.com", "smtp.fastmail.com"),
            "zoho.com" => ("imap.zoho.com", "smtp.zoho.com"),
            "yandex.com" | "yandex.ru" => ("imap.yandex.com", "smtp.yandex.com"),
            "proton.me" | "protonmail.com" => ("127.0.0.1", "127.0.0.1"),
            _ => ("", ""),
        };
        if !imap.is_empty() {
            self.incoming.host = imap.into();
            self.outgoing.host = smtp.into();
        } else if !domain.is_empty() {
            self.incoming.host = format!("imap.{domain}");
            self.outgoing.host = format!("smtp.{domain}");
        }
        if domain == "proton.me" || domain == "protonmail.com" {
            // local bridge
            self.incoming.port = 1143;
            self.incoming.security = Security::StartTls;
            self.outgoing.port = 1025;
            self.outgoing.security = Security::StartTls;
            self.incoming.allow_insecure_certs = true;
            self.outgoing.allow_insecure_certs = true;
        } else {
            self.incoming.port = if self.kind == AccountKind::Pop3 { 995 } else { 993 };
            self.incoming.security = Security::Tls;
            self.outgoing.port = 587;
            self.outgoing.security = Security::StartTls;
            if self.kind == AccountKind::Pop3 {
                self.incoming.host = self.incoming.host.replacen("imap.", "pop.", 1);
            }
        }
        self.incoming.username = email.to_string();
        self.outgoing.username = email.to_string();
    }
}

// ------------------------------------------------------------------ rules

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum RuleAction {
    MoveTo { mailbox: MailboxId },
    CopyTo { mailbox: MailboxId },
    SetColor { color: FlagColor },
    /// Highlight the message text / background in the list.
    SetHighlight { color: FlagColor },
    MarkAsRead,
    MarkAsUnread,
    MarkAsFlagged { color: FlagColor },
    MarkAsJunk,
    Delete,
    Archive,
    PlaySound { sound: String },
    Notify { text: String },
    ReplyWith { text: String },
    ForwardTo { addresses: String, text: String },
    RedirectTo { addresses: String },
    RunCommand { command: String },
    SetCategory { category: Category },
    MuteThread,
    StopEvaluating,
}

impl RuleAction {
    pub fn name(&self) -> &'static str {
        match self {
            RuleAction::MoveTo { .. } => "Move Message",
            RuleAction::CopyTo { .. } => "Copy Message",
            RuleAction::SetColor { .. } => "Set Color",
            RuleAction::SetHighlight { .. } => "Highlight Message",
            RuleAction::MarkAsRead => "Mark as Read",
            RuleAction::MarkAsUnread => "Mark as Unread",
            RuleAction::MarkAsFlagged { .. } => "Mark as Flagged",
            RuleAction::MarkAsJunk => "Mark as Junk",
            RuleAction::Delete => "Delete Message",
            RuleAction::Archive => "Archive Message",
            RuleAction::PlaySound { .. } => "Play Sound",
            RuleAction::Notify { .. } => "Show Notification",
            RuleAction::ReplyWith { .. } => "Reply to Message",
            RuleAction::ForwardTo { .. } => "Forward Message",
            RuleAction::RedirectTo { .. } => "Redirect Message",
            RuleAction::RunCommand { .. } => "Run Command",
            RuleAction::SetCategory { .. } => "Set Category",
            RuleAction::MuteThread => "Mute Conversation",
            RuleAction::StopEvaluating => "Stop Evaluating Rules",
        }
    }
    pub fn templates() -> Vec<RuleAction> {
        vec![
            RuleAction::MoveTo { mailbox: 0 },
            RuleAction::CopyTo { mailbox: 0 },
            RuleAction::SetColor { color: FlagColor::Red },
            RuleAction::SetHighlight { color: FlagColor::Yellow },
            RuleAction::MarkAsRead,
            RuleAction::MarkAsUnread,
            RuleAction::MarkAsFlagged { color: FlagColor::Red },
            RuleAction::MarkAsJunk,
            RuleAction::Delete,
            RuleAction::Archive,
            RuleAction::PlaySound { sound: "default".into() },
            RuleAction::Notify { text: String::new() },
            RuleAction::ReplyWith { text: String::new() },
            RuleAction::ForwardTo { addresses: String::new(), text: String::new() },
            RuleAction::RedirectTo { addresses: String::new() },
            RuleAction::RunCommand { command: String::new() },
            RuleAction::SetCategory { category: Category::Primary },
            RuleAction::MuteThread,
            RuleAction::StopEvaluating,
        ]
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub criteria: Criteria,
    pub actions: Vec<RuleAction>,
}

impl Default for Rule {
    fn default() -> Self {
        Self { id: uuid::Uuid::new_v4().to_string(), name: "Rule".into(), enabled: true, criteria: Criteria::default(), actions: vec![RuleAction::MoveTo { mailbox: 0 }] }
    }
}

// ------------------------------------------------------------------ smart mailboxes

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SmartMailbox {
    pub id: String,
    pub name: String,
    pub criteria: Criteria,
    pub include_trash: bool,
    pub include_sent: bool,
    /// Optional folder this smart mailbox is grouped under.
    #[serde(default)]
    pub folder: String,
}

impl Default for SmartMailbox {
    fn default() -> Self {
        Self { id: uuid::Uuid::new_v4().to_string(), name: "Smart Mailbox".into(), criteria: Criteria::default(), include_trash: false, include_sent: false, folder: String::new() }
    }
}

// ------------------------------------------------------------------ signatures & templates

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Signature {
    pub id: String,
    pub name: String,
    /// Body in the Purpamail rich text markup (see `ui::richtext`), or plain text.
    pub body: String,
    /// Account ids this signature is offered for. Empty = all accounts.
    pub accounts: Vec<AccountId>,
    /// Always use my default message font.
    pub use_default_font: bool,
}

impl Default for Signature {
    fn default() -> Self {
        Self { id: uuid::Uuid::new_v4().to_string(), name: "Signature".into(), body: String::new(), accounts: vec![], use_default_font: true }
    }
}

/// Reusable message templates ("stationery").
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub category: String,
    pub subject: String,
    pub body: String,
    /// Background color (hex) used when composing rich text.
    pub background: String,
    pub accent: String,
}

// ------------------------------------------------------------------ preferences

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CheckFrequency {
    Automatically,
    EveryMinute,
    #[default]
    Every5Minutes,
    Every15Minutes,
    Every30Minutes,
    EveryHour,
    Manually,
}

impl CheckFrequency {
    pub const ALL: [CheckFrequency; 7] = [
        CheckFrequency::Automatically,
        CheckFrequency::EveryMinute,
        CheckFrequency::Every5Minutes,
        CheckFrequency::Every15Minutes,
        CheckFrequency::Every30Minutes,
        CheckFrequency::EveryHour,
        CheckFrequency::Manually,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            CheckFrequency::Automatically => "Automatically",
            CheckFrequency::EveryMinute => "Every minute",
            CheckFrequency::Every5Minutes => "Every 5 minutes",
            CheckFrequency::Every15Minutes => "Every 15 minutes",
            CheckFrequency::Every30Minutes => "Every 30 minutes",
            CheckFrequency::EveryHour => "Every hour",
            CheckFrequency::Manually => "Manually",
        }
    }
    pub fn seconds(&self) -> Option<u64> {
        match self {
            CheckFrequency::Automatically => Some(120),
            CheckFrequency::EveryMinute => Some(60),
            CheckFrequency::Every5Minutes => Some(300),
            CheckFrequency::Every15Minutes => Some(900),
            CheckFrequency::Every30Minutes => Some(1800),
            CheckFrequency::EveryHour => Some(3600),
            CheckFrequency::Manually => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NotifyFor {
    #[default]
    InboxOnly,
    VipsOnly,
    Contacts,
    AllMailboxes,
    None,
}

impl NotifyFor {
    pub const ALL: [NotifyFor; 5] = [NotifyFor::InboxOnly, NotifyFor::VipsOnly, NotifyFor::Contacts, NotifyFor::AllMailboxes, NotifyFor::None];
    pub fn name(&self) -> &'static str {
        match self {
            NotifyFor::InboxOnly => "Inbox only",
            NotifyFor::VipsOnly => "VIPs",
            NotifyFor::Contacts => "Contacts",
            NotifyFor::AllMailboxes => "All mailboxes",
            NotifyFor::None => "None",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RemoveDownloads {
    Never,
    #[default]
    WhenMessageDeleted,
    AfterQuitting,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct GeneralPrefs {
    pub check_frequency: CheckFrequency,
    pub new_mail_sound: String,
    pub play_sounds_for_other_actions: bool,
    pub notify_for: NotifyFor,
    pub show_unread_in_title: bool,
    pub downloads_folder: String,
    pub remove_unedited_downloads: RemoveDownloads,
    pub archive_on_delete: bool,
    pub search_include_trash: bool,
    pub search_include_junk: bool,
    pub search_include_encrypted: bool,
    pub prefer_opening_in_split_view: bool,
    pub undo_send_delay_secs: u32,
    pub launch_at_login: bool,
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        Self {
            check_frequency: CheckFrequency::Every5Minutes,
            new_mail_sound: "default".into(),
            play_sounds_for_other_actions: true,
            notify_for: NotifyFor::InboxOnly,
            show_unread_in_title: true,
            downloads_folder: downloads_dir().display().to_string(),
            remove_unedited_downloads: RemoveDownloads::WhenMessageDeleted,
            archive_on_delete: false,
            search_include_trash: false,
            search_include_junk: false,
            search_include_encrypted: true,
            prefer_opening_in_split_view: false,
            undo_send_delay_secs: 10,
            launch_at_login: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum JunkAction {
    /// Mark as junk but leave in Inbox.
    MarkOnly,
    #[default]
    MoveToJunk,
    /// Run custom rules ("Perform custom actions").
    Custom,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct JunkPrefs {
    pub enabled: bool,
    pub action: JunkAction,
    pub exempt_sender_in_contacts: bool,
    pub exempt_previous_recipients: bool,
    pub exempt_full_name_addressed: bool,
    pub trust_server_headers: bool,
    pub filter_before_rules: bool,
    /// Probability threshold (0..1) above which a message is considered junk.
    pub threshold: f32,
}

impl Default for JunkPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            action: JunkAction::MoveToJunk,
            exempt_sender_in_contacts: true,
            exempt_previous_recipients: true,
            exempt_full_name_addressed: false,
            trust_server_headers: true,
            filter_before_rules: true,
            threshold: 0.9,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct FontsColorsPrefs {
    /// Font family name ("" = default proportional font).
    pub list_font: String,
    pub list_font_size: f32,
    pub message_font: String,
    pub message_font_size: f32,
    pub fixed_font: String,
    pub fixed_font_size: f32,
    pub use_fixed_for_plain_text: bool,
    pub color_quoted_text: bool,
    /// Hex colors for quote levels 1..3.
    pub quote_colors: [String; 3],
}

impl Default for FontsColorsPrefs {
    fn default() -> Self {
        Self {
            list_font: String::new(),
            list_font_size: 13.0,
            message_font: String::new(),
            message_font_size: 14.0,
            fixed_font: String::new(),
            fixed_font_size: 13.0,
            use_fixed_for_plain_text: false,
            color_quoted_text: true,
            quote_colors: ["#7b4fd6".into(), "#3c9a55".into(), "#d07a1a".into()],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ListLayout {
    /// Sidebar | message list | message (columns).
    #[default]
    Columns,
    /// Message list on top, message below (classic layout with columns).
    Classic,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ViewingPrefs {
    pub layout: ListLayout,
    pub theme: Theme,
    pub list_preview_lines: u8,
    pub show_to_cc_labels: bool,
    pub show_contact_photos: bool,
    pub show_message_size: bool,
    pub show_header_detail_all: bool,
    pub display_unread_bold: bool,
    pub load_remote_content: bool,
    pub show_categories: bool,
    pub use_smart_addresses: bool,
    pub organize_by_conversation: bool,
    pub include_related_messages: bool,
    pub mark_all_read_when_opening_conversation: bool,
    pub show_most_recent_on_top: bool,
    pub highlight_conversations_color: Option<FlagColor>,
    pub show_favorites_bar: bool,
    pub show_sidebar: bool,
    pub show_deleted_messages: bool,
    pub sort: SortKey,
    pub sort_ascending: bool,
    pub classic_columns: Vec<String>,
    pub show_summary: bool,
    pub ui_scale: f32,
}

impl Default for ViewingPrefs {
    fn default() -> Self {
        Self {
            layout: ListLayout::Columns,
            theme: Theme::System,
            list_preview_lines: 2,
            show_to_cc_labels: false,
            show_contact_photos: true,
            show_message_size: false,
            show_header_detail_all: false,
            display_unread_bold: true,
            load_remote_content: false,
            show_categories: false,
            use_smart_addresses: true,
            organize_by_conversation: true,
            include_related_messages: true,
            mark_all_read_when_opening_conversation: false,
            show_most_recent_on_top: true,
            highlight_conversations_color: None,
            show_favorites_bar: true,
            show_sidebar: true,
            show_deleted_messages: false,
            sort: SortKey::Date,
            sort_ascending: false,
            classic_columns: ["unread", "flag", "attachment", "from", "subject", "date", "mailbox"].iter().map(|s| s.to_string()).collect(),
            show_summary: true,
            ui_scale: 1.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MessageFormat {
    #[default]
    RichText,
    PlainText,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SpellCheck {
    #[default]
    AsITyped,
    BeforeSending,
    Never,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ComposingPrefs {
    pub format: MessageFormat,
    pub spell_check: SpellCheck,
    pub auto_cc_myself: bool,
    pub auto_bcc_myself: bool,
    pub show_addresses_not_names: bool,
    pub mark_addresses_not_ending_with: String,
    pub mark_addresses_enabled: bool,
    /// Account id to send new messages from; "" = best account for selected mailbox.
    pub send_new_from: String,
    pub reply_same_format: bool,
    pub quote_original: bool,
    pub increase_quote_level: bool,
    pub include_selected_text_only: bool,
    pub include_attachments_in_reply: bool,
    pub insert_attachments_at_end: bool,
    pub send_windows_friendly_attachments: bool,
    pub follow_up_suggestions: bool,
    pub default_font: String,
    pub default_font_size: f32,
}

impl Default for ComposingPrefs {
    fn default() -> Self {
        Self {
            format: MessageFormat::RichText,
            spell_check: SpellCheck::AsITyped,
            auto_cc_myself: false,
            auto_bcc_myself: false,
            show_addresses_not_names: false,
            mark_addresses_not_ending_with: String::new(),
            mark_addresses_enabled: false,
            send_new_from: String::new(),
            reply_same_format: true,
            quote_original: true,
            increase_quote_level: true,
            include_selected_text_only: true,
            include_attachments_in_reply: false,
            insert_attachments_at_end: true,
            send_windows_friendly_attachments: true,
            follow_up_suggestions: true,
            default_font: String::new(),
            default_font_size: 14.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PrivacyPrefs {
    /// Fetch remote content through a privacy-preserving path / block tracking pixels.
    pub protect_mail_activity: bool,
    pub block_all_remote_content: bool,
    pub hide_ip_address: bool,
    pub strip_tracking_parameters: bool,
}

impl Default for PrivacyPrefs {
    fn default() -> Self {
        Self { protect_mail_activity: true, block_all_remote_content: false, hide_ip_address: true, strip_tracking_parameters: true }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum FavoriteItem {
    Mailbox(MailboxId),
    AllInboxes,
    AllFlagged,
    AllDrafts,
    AllSent,
    Vips,
    Smart(String),
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ToolbarItem {
    GetMail,
    NewMessage,
    Archive,
    Delete,
    Junk,
    Reply,
    ReplyAll,
    Forward,
    Flag,
    Mute,
    MoveTo,
    Mailboxes,
    Filter,
    Print,
    Unread,
    Search,
    Space,
    FlexibleSpace,
    Summarize,
    Remind,
}

impl ToolbarItem {
    pub const ALL: [ToolbarItem; 20] = [
        ToolbarItem::Mailboxes,
        ToolbarItem::GetMail,
        ToolbarItem::NewMessage,
        ToolbarItem::Archive,
        ToolbarItem::Delete,
        ToolbarItem::Junk,
        ToolbarItem::Reply,
        ToolbarItem::ReplyAll,
        ToolbarItem::Forward,
        ToolbarItem::Flag,
        ToolbarItem::Mute,
        ToolbarItem::MoveTo,
        ToolbarItem::Filter,
        ToolbarItem::Print,
        ToolbarItem::Unread,
        ToolbarItem::Summarize,
        ToolbarItem::Remind,
        ToolbarItem::Search,
        ToolbarItem::Space,
        ToolbarItem::FlexibleSpace,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            ToolbarItem::GetMail => "Get Mail",
            ToolbarItem::NewMessage => "New Message",
            ToolbarItem::Archive => "Archive",
            ToolbarItem::Delete => "Delete",
            ToolbarItem::Junk => "Junk",
            ToolbarItem::Reply => "Reply",
            ToolbarItem::ReplyAll => "Reply All",
            ToolbarItem::Forward => "Forward",
            ToolbarItem::Flag => "Flag",
            ToolbarItem::Mute => "Mute",
            ToolbarItem::MoveTo => "Move",
            ToolbarItem::Mailboxes => "Sidebar",
            ToolbarItem::Filter => "Filter",
            ToolbarItem::Print => "Print",
            ToolbarItem::Unread => "Read/Unread",
            ToolbarItem::Search => "Search",
            ToolbarItem::Space => "Space",
            ToolbarItem::FlexibleSpace => "Flexible Space",
            ToolbarItem::Summarize => "Summarize",
            ToolbarItem::Remind => "Remind Me",
        }
    }
    pub fn default_set() -> Vec<ToolbarItem> {
        vec![
            ToolbarItem::Mailboxes,
            ToolbarItem::GetMail,
            ToolbarItem::NewMessage,
            ToolbarItem::FlexibleSpace,
            ToolbarItem::Archive,
            ToolbarItem::Delete,
            ToolbarItem::Junk,
            ToolbarItem::Space,
            ToolbarItem::Reply,
            ToolbarItem::ReplyAll,
            ToolbarItem::Forward,
            ToolbarItem::Space,
            ToolbarItem::Flag,
            ToolbarItem::Mute,
            ToolbarItem::MoveTo,
            ToolbarItem::FlexibleSpace,
            ToolbarItem::Search,
        ]
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralPrefs,
    pub junk: JunkPrefs,
    pub fonts: FontsColorsPrefs,
    pub viewing: ViewingPrefs,
    pub composing: ComposingPrefs,
    pub privacy: PrivacyPrefs,
    pub accounts: Vec<AccountConfig>,
    pub signatures: Vec<Signature>,
    pub templates: Vec<Template>,
    pub rules: Vec<Rule>,
    pub smart_mailboxes: Vec<SmartMailbox>,
    pub favorites: Vec<FavoriteItem>,
    pub toolbar: Vec<ToolbarItem>,
    /// Flag color display names (the user can rename flags).
    pub flag_names: BTreeMap<String, String>,
    /// Expanded sidebar sections / mailboxes.
    pub collapsed_sidebar: Vec<String>,
    pub work_offline: bool,
    pub logging: bool,
    /// Has the welcome / account setup been completed?
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: Default::default(),
            junk: Default::default(),
            fonts: Default::default(),
            viewing: Default::default(),
            composing: Default::default(),
            privacy: Default::default(),
            accounts: vec![],
            signatures: vec![],
            templates: default_templates(),
            rules: vec![],
            smart_mailboxes: vec![SmartMailbox {
                id: "today".into(),
                name: "Today".into(),
                criteria: Criteria {
                    match_all: true,
                    conditions: vec![super::criteria::Condition { field: super::criteria::Field::DateReceived, op: super::criteria::Op::IsToday, value: String::new(), header: String::new() }],
                },
                include_trash: false,
                include_sent: false,
                folder: String::new(),
            }],
            favorites: vec![FavoriteItem::AllInboxes, FavoriteItem::Vips, FavoriteItem::AllFlagged, FavoriteItem::AllDrafts, FavoriteItem::AllSent],
            toolbar: ToolbarItem::default_set(),
            flag_names: BTreeMap::new(),
            collapsed_sidebar: vec![],
            work_offline: false,
            logging: false,
            onboarded: false,
        }
    }
}

fn default_templates() -> Vec<Template> {
    let t = |id: &str, name: &str, cat: &str, subject: &str, body: &str, bg: &str, accent: &str| Template {
        id: id.into(),
        name: name.into(),
        category: cat.into(),
        subject: subject.into(),
        body: body.into(),
        background: bg.into(),
        accent: accent.into(),
    };
    vec![
        t("blank", "Blank", "Basic", "", "", "", ""),
        t("birthday", "Birthday Wishes", "Greetings", "Happy Birthday!", "# Happy Birthday!\n\nWishing you a wonderful day full of joy.\n\n", "#fff4e6", "#e8590c"),
        t("thanks", "Thank You", "Greetings", "Thank you", "# Thank you!\n\nI really appreciate it.\n\n", "#f3f0ff", "#7048e8"),
        t("announcement", "Announcement", "Announcements", "Announcement", "# Big News\n\n**What:** \n**When:** \n**Where:** \n\n", "#e7f5ff", "#1c7ed6"),
        t("invitation", "Party Invitation", "Invitations", "You're invited!", "# You're invited!\n\nPlease join us for a celebration.\n\n**Date:** \n**Time:** \n**Place:** \n\nRSVP by replying to this message.\n", "#fff0f6", "#c2255c"),
        t("meeting", "Meeting Notes", "Basic", "Meeting notes", "# Meeting notes\n\n**Attendees:** \n\n## Agenda\n- \n\n## Action items\n- \n", "", "#495057"),
    ]
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.toml")
    }

    pub fn load() -> Settings {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml::from_str(&s) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("could not parse {}: {e}", path.display());
                    let backup = path.with_extension("toml.broken");
                    let _ = std::fs::copy(path, backup);
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let s = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn account(&self, id: &str) -> Option<&AccountConfig> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn account_mut(&mut self, id: &str) -> Option<&mut AccountConfig> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// All email addresses belonging to the user.
    pub fn my_addresses(&self) -> Vec<String> {
        self.accounts.iter().flat_map(|a| a.emails.iter().map(|e| e.to_lowercase())).collect()
    }

    pub fn account_for_address(&self, email: &str) -> Option<&AccountConfig> {
        self.accounts.iter().find(|a| a.owns_address(email))
    }

    pub fn flag_name(&self, c: FlagColor) -> String {
        self.flag_names.get(c.name()).cloned().unwrap_or_else(|| c.name().to_string())
    }

    pub fn signature(&self, id: &str) -> Option<&Signature> {
        self.signatures.iter().find(|s| s.id == id)
    }
}

// ------------------------------------------------------------------ secrets

/// Passwords stored outside the main settings file.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Secrets {
    #[serde(default)]
    pub passwords: BTreeMap<String, String>,
}

impl Secrets {
    pub fn path() -> PathBuf {
        config_dir().join("secrets.toml")
    }
    pub fn load() -> Secrets {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, toml::to_string(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
    pub fn incoming_key(account: &str) -> String {
        format!("{account}:incoming")
    }
    pub fn outgoing_key(account: &str) -> String {
        format!("{account}:outgoing")
    }
    pub fn get(&self, key: &str) -> String {
        self.passwords.get(key).cloned().unwrap_or_default()
    }
    pub fn set(&mut self, key: String, value: String) {
        self.passwords.insert(key, value);
    }
    /// Password for the incoming server of an account.
    pub fn incoming(&self, account: &str) -> String {
        self.get(&Self::incoming_key(account))
    }
    /// Password for SMTP; falls back to the incoming password.
    pub fn outgoing(&self, acct: &AccountConfig) -> String {
        if acct.smtp_same_credentials {
            self.incoming(&acct.id)
        } else {
            self.get(&Self::outgoing_key(&acct.id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut s = Settings::default();
        let mut a = AccountConfig { name: "Work".into(), emails: vec!["me@example.com".into()], ..Default::default() };
        a.autoconfigure("me@gmail.com");
        s.accounts.push(a);
        s.rules.push(Rule::default());
        let t = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&t).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.accounts[0].incoming.host, "imap.gmail.com");
    }
}
