//! Core data types shared by storage, networking and the user interface.

use serde::{Deserialize, Serialize};

pub type AccountId = String;
pub type MailboxId = i64;
pub type MessageId = i64;

/// Account id used for mailboxes stored only on this computer.
pub const LOCAL_ACCOUNT: &str = "local";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum MailboxRole {
    Inbox,
    Drafts,
    Sent,
    Junk,
    Trash,
    Archive,
    Outbox,
    #[default]
    Custom,
}

impl MailboxRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MailboxRole::Inbox => "inbox",
            MailboxRole::Drafts => "drafts",
            MailboxRole::Sent => "sent",
            MailboxRole::Junk => "junk",
            MailboxRole::Trash => "trash",
            MailboxRole::Archive => "archive",
            MailboxRole::Outbox => "outbox",
            MailboxRole::Custom => "custom",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "inbox" => MailboxRole::Inbox,
            "drafts" => MailboxRole::Drafts,
            "sent" => MailboxRole::Sent,
            "junk" => MailboxRole::Junk,
            "trash" => MailboxRole::Trash,
            "archive" => MailboxRole::Archive,
            "outbox" => MailboxRole::Outbox,
            _ => MailboxRole::Custom,
        }
    }
    pub fn display_name(&self) -> &'static str {
        match self {
            MailboxRole::Inbox => "Inbox",
            MailboxRole::Drafts => "Drafts",
            MailboxRole::Sent => "Sent",
            MailboxRole::Junk => "Junk",
            MailboxRole::Trash => "Trash",
            MailboxRole::Archive => "Archive",
            MailboxRole::Outbox => "Outbox",
            MailboxRole::Custom => "Mailbox",
        }
    }
    /// Standard roles in sidebar order.
    pub const STANDARD: [MailboxRole; 7] = [
        MailboxRole::Inbox,
        MailboxRole::Drafts,
        MailboxRole::Sent,
        MailboxRole::Junk,
        MailboxRole::Trash,
        MailboxRole::Archive,
        MailboxRole::Outbox,
    ];
    /// Guess a role from a server mailbox name and special-use attributes.
    pub fn guess(path: &str, attributes: &[String]) -> Self {
        for a in attributes {
            match a.to_ascii_lowercase().as_str() {
                "\\drafts" => return MailboxRole::Drafts,
                "\\sent" => return MailboxRole::Sent,
                "\\junk" | "\\spam" => return MailboxRole::Junk,
                "\\trash" => return MailboxRole::Trash,
                "\\archive" | "\\all" => return MailboxRole::Archive,
                _ => {}
            }
        }
        let last = path.rsplit(['/', '.']).next().unwrap_or(path).to_ascii_lowercase();
        if path.eq_ignore_ascii_case("inbox") {
            return MailboxRole::Inbox;
        }
        match last.as_str() {
            "drafts" | "draft" | "entwürfe" => MailboxRole::Drafts,
            "sent" | "sent items" | "sent messages" | "sent mail" | "gesendet" => MailboxRole::Sent,
            "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => MailboxRole::Junk,
            "trash" | "deleted" | "deleted items" | "deleted messages" | "bin" | "papierkorb" => MailboxRole::Trash,
            "archive" | "archives" | "all mail" | "archiv" => MailboxRole::Archive,
            "outbox" => MailboxRole::Outbox,
            _ => MailboxRole::Custom,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Mailbox {
    pub id: MailboxId,
    pub account_id: AccountId,
    /// Full server path (for local mailboxes: the hierarchical name).
    pub path: String,
    /// Display name (last path component).
    pub name: String,
    pub parent: Option<MailboxId>,
    pub role: MailboxRole,
    pub delimiter: String,
    pub unread: u32,
    pub total: u32,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub selectable: bool,
    pub subscribed: bool,
}

impl Mailbox {
    pub fn is_local(&self) -> bool {
        self.account_id == LOCAL_ACCOUNT
    }
}

/// Message flag bits.
pub mod flags {
    pub const SEEN: u32 = 1;
    pub const ANSWERED: u32 = 1 << 1;
    pub const FLAGGED: u32 = 1 << 2;
    pub const DELETED: u32 = 1 << 3;
    pub const DRAFT: u32 = 1 << 4;
    pub const FORWARDED: u32 = 1 << 5;
    pub const REDIRECTED: u32 = 1 << 6;
    pub const JUNK: u32 = 1 << 7;
    pub const NOT_JUNK: u32 = 1 << 8;
    /// Body has been downloaded and stored locally.
    pub const HAS_BODY: u32 = 1 << 9;
    /// Message was explicitly marked "Remind me" and resurfaced.
    pub const REMINDED: u32 = 1 << 10;
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum FlagColor {
    #[default]
    Red = 0,
    Orange = 1,
    Yellow = 2,
    Green = 3,
    Blue = 4,
    Purple = 5,
    Gray = 6,
}

impl FlagColor {
    pub const ALL: [FlagColor; 7] = [
        FlagColor::Red,
        FlagColor::Orange,
        FlagColor::Yellow,
        FlagColor::Green,
        FlagColor::Blue,
        FlagColor::Purple,
        FlagColor::Gray,
    ];
    pub fn from_u8(v: u8) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or_default()
    }
    pub fn name(&self) -> &'static str {
        match self {
            FlagColor::Red => "Red",
            FlagColor::Orange => "Orange",
            FlagColor::Yellow => "Yellow",
            FlagColor::Green => "Green",
            FlagColor::Blue => "Blue",
            FlagColor::Purple => "Purple",
            FlagColor::Gray => "Gray",
        }
    }
    pub fn rgb(&self) -> [u8; 3] {
        match self {
            FlagColor::Red => [0xf0, 0x40, 0x3c],
            FlagColor::Orange => [0xf5, 0x8b, 0x1f],
            FlagColor::Yellow => [0xf0, 0xc0, 0x1c],
            FlagColor::Green => [0x3c, 0xb8, 0x4a],
            FlagColor::Blue => [0x2f, 0x7c, 0xf6],
            FlagColor::Purple => [0x9b, 0x4d, 0xd6],
            FlagColor::Gray => [0x8e, 0x8e, 0x93],
        }
    }
    /// The color is encoded on IMAP servers with three widely used keyword bits.
    pub fn to_keyword_bits(&self) -> [bool; 3] {
        let v = *self as u8;
        [v & 1 != 0, v & 2 != 0, v & 4 != 0]
    }
    pub fn from_keyword_bits(bits: [bool; 3]) -> Self {
        Self::from_u8(bits[0] as u8 | (bits[1] as u8) << 1 | (bits[2] as u8) << 2)
    }
}

pub const FLAG_COLOR_KEYWORDS: [&str; 3] = ["$MailFlagBit0", "$MailFlagBit1", "$MailFlagBit2"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Address {
    pub name: String,
    pub email: String,
}

impl Address {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self { name: name.into(), email: email.into() }
    }
    /// Name if present, otherwise the email address.
    pub fn display(&self) -> &str {
        if self.name.trim().is_empty() { &self.email } else { &self.name }
    }
    /// RFC 5322 style formatting: `"Name" <email>`.
    pub fn to_header(&self) -> String {
        if self.name.is_empty() {
            self.email.clone()
        } else if self.name.chars().any(|c| ",;<>@\"()[]:\\.".contains(c)) {
            format!("\"{}\" <{}>", self.name.replace('"', "\\\""), self.email)
        } else {
            format!("{} <{}>", self.name, self.email)
        }
    }
    /// Parse a comma separated list of addresses as typed by the user.
    pub fn parse_list(s: &str) -> Vec<Address> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quote = false;
        let mut in_angle = false;
        for c in s.chars() {
            match c {
                '"' => {
                    in_quote = !in_quote;
                    cur.push(c)
                }
                '<' => {
                    in_angle = true;
                    cur.push(c)
                }
                '>' => {
                    in_angle = false;
                    cur.push(c)
                }
                ',' | ';' if !in_quote && !in_angle => {
                    if let Some(a) = Self::parse_one(&cur) {
                        out.push(a)
                    }
                    cur.clear();
                }
                _ => cur.push(c),
            }
        }
        if let Some(a) = Self::parse_one(&cur) {
            out.push(a)
        }
        out
    }
    pub fn parse_one(s: &str) -> Option<Address> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let (Some(l), Some(r)) = (s.rfind('<'), s.rfind('>')) {
            if l < r {
                let email = s[l + 1..r].trim().to_string();
                let name = s[..l].trim().trim_matches('"').trim().to_string();
                return Some(Address { name, email });
            }
        }
        Some(Address { name: String::new(), email: s.trim_matches('"').to_string() })
    }
    pub fn is_valid(&self) -> bool {
        let e = self.email.trim();
        match e.split_once('@') {
            Some((l, d)) => !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && !e.contains(' '),
            None => false,
        }
    }
    pub fn domain(&self) -> &str {
        self.email.rsplit_once('@').map(|(_, d)| d).unwrap_or("")
    }
}

pub fn format_address_list(list: &[Address]) -> String {
    list.iter().map(|a| a.to_header()).collect::<Vec<_>>().join(", ")
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

impl Priority {
    pub fn to_i32(self) -> i32 {
        match self {
            Priority::Low => -1,
            Priority::Normal => 0,
            Priority::High => 1,
        }
    }
    pub fn from_i32(v: i32) -> Self {
        match v.cmp(&0) {
            std::cmp::Ordering::Less => Priority::Low,
            std::cmp::Ordering::Equal => Priority::Normal,
            std::cmp::Ordering::Greater => Priority::High,
        }
    }
}

/// Automatic inbox categories.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Category {
    #[default]
    Primary,
    Transactions,
    Updates,
    Promotions,
}

impl Category {
    pub const ALL: [Category; 4] = [Category::Primary, Category::Transactions, Category::Updates, Category::Promotions];
    pub fn name(&self) -> &'static str {
        match self {
            Category::Primary => "Primary",
            Category::Transactions => "Transactions",
            Category::Updates => "Updates",
            Category::Promotions => "Promotions",
        }
    }
    pub fn to_i32(self) -> i32 {
        self as i32
    }
    pub fn from_i32(v: i32) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or_default()
    }
}

/// Summary row for a message as stored locally.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: MessageId,
    pub mailbox_id: MailboxId,
    pub account_id: AccountId,
    pub uid: u32,
    /// The `Message-ID` header value (without angle brackets).
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub thread_id: i64,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    /// Date header, unix seconds.
    pub date: i64,
    /// Time we received / stored the message, unix seconds.
    pub received: i64,
    pub size: u32,
    pub flags: u32,
    pub color: Option<FlagColor>,
    pub has_attachments: bool,
    pub snippet: String,
    pub priority: Priority,
    pub category: Category,
    pub remind_at: Option<i64>,
    pub list_unsubscribe: String,
    /// For outbox messages: scheduled send time.
    pub send_at: Option<i64>,
}

impl MessageSummary {
    pub fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }
    pub fn is_unread(&self) -> bool {
        !self.has(flags::SEEN)
    }
    pub fn is_flagged(&self) -> bool {
        self.has(flags::FLAGGED)
    }
    pub fn all_recipients(&self) -> impl Iterator<Item = &Address> {
        self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter())
    }
}

/// Strip reply / forward prefixes for conversation grouping and sorting.
pub fn normalize_subject(s: &str) -> String {
    let mut s = s.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let mut stripped = false;
        for p in ["re:", "fw:", "fwd:", "aw:", "wg:", "sv:", "vs:", "antw:", "rif:", "r:", "tr:", "ref:"] {
            if lower.starts_with(p) {
                s = s[p.len()..].trim_start();
                stripped = true;
                break;
            }
        }
        // "Re[2]:" style
        if !stripped && lower.starts_with("re[") {
            if let Some(end) = s.find("]:") {
                s = s[end + 2..].trim_start();
                stripped = true;
            }
        }
        if !stripped {
            break;
        }
    }
    s.to_string()
}

pub fn subject_is_reply(s: &str) -> bool {
    normalize_subject(s).len() != s.trim().len()
}

/// Composite sort keys for message lists.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SortKey {
    Attachments,
    #[default]
    Date,
    Flags,
    From,
    Size,
    Subject,
    To,
    Unread,
    Mailbox,
    Category,
}

impl SortKey {
    pub const ALL: [SortKey; 10] = [
        SortKey::Attachments,
        SortKey::Date,
        SortKey::Flags,
        SortKey::From,
        SortKey::Size,
        SortKey::Subject,
        SortKey::To,
        SortKey::Unread,
        SortKey::Mailbox,
        SortKey::Category,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            SortKey::Attachments => "Attachments",
            SortKey::Date => "Date",
            SortKey::Flags => "Flags",
            SortKey::From => "From",
            SortKey::Size => "Size",
            SortKey::Subject => "Subject",
            SortKey::To => "To",
            SortKey::Unread => "Unread",
            SortKey::Mailbox => "Mailbox",
            SortKey::Category => "Category",
        }
    }
}

/// Quick filters applied on top of a mailbox list ("Filter" button).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ListFilter {
    pub enabled: bool,
    pub unread: bool,
    pub flagged: bool,
    pub to_me: bool,
    pub cc_me: bool,
    pub with_attachments: bool,
    pub from_vip: bool,
    pub today: bool,
    pub category: Option<Category>,
}

impl ListFilter {
    pub fn any(&self) -> bool {
        self.unread || self.flagged || self.to_me || self.cc_me || self.with_attachments || self.from_vip || self.today
    }
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}
