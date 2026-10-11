//! Conditions shared by mail rules and smart mailboxes.

use serde::{Deserialize, Serialize};

use super::model::{Category, FlagColor, MailboxId, MessageSummary, Priority, flags, now_ts};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Field {
    #[default]
    From,
    To,
    Cc,
    AnyRecipient,
    Subject,
    Body,
    /// Subject or body or sender.
    EntireMessage,
    /// A named header, see [`Condition::header`].
    Header,
    Account,
    SenderInContacts,
    SenderNotInContacts,
    SenderIsVip,
    SenderIsBlocked,
    DateReceived,
    DateSent,
    Priority,
    HasAttachment,
    IsUnread,
    IsRead,
    IsFlagged,
    FlagColor,
    IsJunk,
    IsReplied,
    Category,
    InMailbox,
    Every,
}

impl Field {
    pub const ALL: [Field; 25] = [
        Field::From,
        Field::To,
        Field::Cc,
        Field::AnyRecipient,
        Field::Subject,
        Field::Body,
        Field::EntireMessage,
        Field::Header,
        Field::Account,
        Field::SenderInContacts,
        Field::SenderNotInContacts,
        Field::SenderIsVip,
        Field::SenderIsBlocked,
        Field::DateReceived,
        Field::DateSent,
        Field::Priority,
        Field::HasAttachment,
        Field::IsUnread,
        Field::IsRead,
        Field::IsFlagged,
        Field::FlagColor,
        Field::IsJunk,
        Field::IsReplied,
        Field::Category,
        Field::Every,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            Field::From => "From",
            Field::To => "To",
            Field::Cc => "Cc",
            Field::AnyRecipient => "Any Recipient",
            Field::Subject => "Subject",
            Field::Body => "Message content",
            Field::EntireMessage => "Entire message",
            Field::Header => "Header",
            Field::Account => "Account",
            Field::SenderInContacts => "Sender is in my Contacts",
            Field::SenderNotInContacts => "Sender is not in my Contacts",
            Field::SenderIsVip => "Sender is VIP",
            Field::SenderIsBlocked => "Sender is blocked",
            Field::DateReceived => "Date Received",
            Field::DateSent => "Date Sent",
            Field::Priority => "Priority",
            Field::HasAttachment => "Message has attachments",
            Field::IsUnread => "Message is unread",
            Field::IsRead => "Message is read",
            Field::IsFlagged => "Message is flagged",
            Field::FlagColor => "Flag color",
            Field::IsJunk => "Message is junk mail",
            Field::IsReplied => "Message was replied to",
            Field::Category => "Category",
            Field::InMailbox => "Message is in mailbox",
            Field::Every => "Every message",
        }
    }
    /// What kind of operand this field takes.
    pub fn kind(&self) -> FieldKind {
        match self {
            Field::From | Field::To | Field::Cc | Field::AnyRecipient | Field::Subject | Field::Body | Field::EntireMessage | Field::Header => FieldKind::Text,
            Field::Account => FieldKind::Account,
            Field::DateReceived | Field::DateSent => FieldKind::Date,
            Field::Priority => FieldKind::Priority,
            Field::FlagColor => FieldKind::Color,
            Field::Category => FieldKind::Category,
            Field::InMailbox => FieldKind::Mailbox,
            _ => FieldKind::None,
        }
    }
    pub fn needs_body(&self) -> bool {
        matches!(self, Field::Body | Field::EntireMessage | Field::Header)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    Text,
    Date,
    Account,
    Priority,
    Color,
    Category,
    Mailbox,
    None,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Op {
    #[default]
    Contains,
    DoesNotContain,
    BeginsWith,
    EndsWith,
    IsEqualTo,
    MatchesRegex,
    /// For dates: within the last N days.
    IsInTheLast,
    IsNotInTheLast,
    /// For dates: exactly today
    IsToday,
    /// Priority / color / category equality.
    Is,
    IsNot,
}

impl Op {
    pub const TEXT: [Op; 6] = [Op::Contains, Op::DoesNotContain, Op::BeginsWith, Op::EndsWith, Op::IsEqualTo, Op::MatchesRegex];
    pub const DATE: [Op; 3] = [Op::IsInTheLast, Op::IsNotInTheLast, Op::IsToday];
    pub const EQ: [Op; 2] = [Op::Is, Op::IsNot];
    pub fn name(&self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::DoesNotContain => "does not contain",
            Op::BeginsWith => "begins with",
            Op::EndsWith => "ends with",
            Op::IsEqualTo => "is equal to",
            Op::MatchesRegex => "matches pattern",
            Op::IsInTheLast => "is in the last (days)",
            Op::IsNotInTheLast => "is not in the last (days)",
            Op::IsToday => "is today",
            Op::Is => "is",
            Op::IsNot => "is not",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Condition {
    pub field: Field,
    pub op: Op,
    #[serde(default)]
    pub value: String,
    /// Header name for [`Field::Header`].
    #[serde(default)]
    pub header: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Criteria {
    /// true: all conditions must match; false: any condition.
    pub match_all: bool,
    pub conditions: Vec<Condition>,
}

impl Default for Criteria {
    fn default() -> Self {
        Self { match_all: false, conditions: vec![Condition::default()] }
    }
}

/// Information about a message and its environment needed to evaluate conditions.
pub struct EvalContext<'a> {
    pub msg: &'a MessageSummary,
    /// Plain body text, if available.
    pub body: Option<&'a str>,
    /// Raw header list, if available.
    pub headers: Option<&'a [(String, String)]>,
    pub account_name: &'a str,
    pub sender_in_contacts: bool,
    pub sender_is_vip: bool,
    pub sender_is_blocked: bool,
}

fn text_match(op: Op, hay: &str, needle: &str) -> bool {
    let h = hay.to_lowercase();
    let n = needle.to_lowercase();
    match op {
        Op::Contains => h.contains(&n),
        Op::DoesNotContain => !h.contains(&n),
        Op::BeginsWith => h.starts_with(&n),
        Op::EndsWith => h.ends_with(&n),
        Op::IsEqualTo | Op::Is => h.trim() == n.trim(),
        Op::IsNot => h.trim() != n.trim(),
        Op::MatchesRegex => regex::RegexBuilder::new(needle).case_insensitive(true).build().map(|r| r.is_match(hay)).unwrap_or(false),
        _ => false,
    }
}

fn any_text(op: Op, hays: &[String], needle: &str) -> bool {
    if op == Op::DoesNotContain {
        hays.iter().all(|h| text_match(op, h, needle))
    } else {
        hays.iter().any(|h| text_match(op, h, needle))
    }
}

fn date_match(op: Op, ts: i64, value: &str) -> bool {
    let days: i64 = value.trim().parse().unwrap_or(1);
    let now = now_ts();
    match op {
        Op::IsInTheLast => ts >= now - days * 86400,
        Op::IsNotInTheLast => ts < now - days * 86400,
        Op::IsToday => {
            let d = chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&chrono::Local).date_naive());
            d == Some(chrono::Local::now().date_naive())
        }
        _ => false,
    }
}

impl Condition {
    pub fn matches(&self, cx: &EvalContext) -> bool {
        let m = cx.msg;
        let addr_strings = |list: &mut dyn Iterator<Item = &super::model::Address>| -> Vec<String> { list.map(|a| format!("{} <{}>", a.name, a.email)).collect() };
        match self.field {
            Field::From => any_text(self.op, &[format!("{} <{}>", m.from.name, m.from.email)], &self.value),
            Field::To => any_text(self.op, &addr_strings(&mut m.to.iter()), &self.value),
            Field::Cc => any_text(self.op, &addr_strings(&mut m.cc.iter()), &self.value),
            Field::AnyRecipient => any_text(self.op, &addr_strings(&mut m.all_recipients()), &self.value),
            Field::Subject => text_match(self.op, &m.subject, &self.value),
            Field::Body => text_match(self.op, cx.body.unwrap_or(&m.snippet), &self.value),
            Field::EntireMessage => {
                let all = format!("{}\n{} <{}>\n{}", m.subject, m.from.name, m.from.email, cx.body.unwrap_or(&m.snippet));
                text_match(self.op, &all, &self.value)
            }
            Field::Header => {
                let vals: Vec<String> = cx
                    .headers
                    .map(|h| h.iter().filter(|(k, _)| k.eq_ignore_ascii_case(self.header.trim())).map(|(_, v)| v.clone()).collect())
                    .unwrap_or_default();
                if vals.is_empty() {
                    return self.op == Op::DoesNotContain;
                }
                any_text(self.op, &vals, &self.value)
            }
            Field::Account => (m.account_id == self.value || cx.account_name == self.value) == (self.op != Op::IsNot),
            Field::SenderInContacts => cx.sender_in_contacts,
            Field::SenderNotInContacts => !cx.sender_in_contacts,
            Field::SenderIsVip => cx.sender_is_vip,
            Field::SenderIsBlocked => cx.sender_is_blocked,
            Field::DateReceived => date_match(self.op, m.received, &self.value),
            Field::DateSent => date_match(self.op, m.date, &self.value),
            Field::Priority => {
                let want = match self.value.as_str() {
                    "High" => Priority::High,
                    "Low" => Priority::Low,
                    _ => Priority::Normal,
                };
                (m.priority == want) == (self.op != Op::IsNot)
            }
            Field::HasAttachment => m.has_attachments,
            Field::IsUnread => m.is_unread(),
            Field::IsRead => !m.is_unread(),
            Field::IsFlagged => m.is_flagged(),
            Field::FlagColor => {
                let want = FlagColor::ALL.iter().find(|c| c.name() == self.value).copied();
                (m.is_flagged() && m.color == want) == (self.op != Op::IsNot)
            }
            Field::IsJunk => m.has(flags::JUNK),
            Field::IsReplied => m.has(flags::ANSWERED),
            Field::Category => {
                let want = Category::ALL.iter().find(|c| c.name() == self.value).copied().unwrap_or_default();
                (m.category == want) == (self.op != Op::IsNot)
            }
            Field::InMailbox => {
                let want: MailboxId = self.value.parse().unwrap_or(-1);
                (m.mailbox_id == want) == (self.op != Op::IsNot)
            }
            Field::Every => true,
        }
    }

    /// Short human readable description.
    pub fn describe(&self) -> String {
        match self.field.kind() {
            FieldKind::None => self.field.name().to_string(),
            FieldKind::Text if self.field == Field::Header => format!("{} {} “{}”", self.header, self.op.name(), self.value),
            _ => format!("{} {} “{}”", self.field.name(), self.op.name(), self.value),
        }
    }
}

impl Criteria {
    pub fn matches(&self, cx: &EvalContext) -> bool {
        if self.conditions.is_empty() {
            return false;
        }
        if self.match_all { self.conditions.iter().all(|c| c.matches(cx)) } else { self.conditions.iter().any(|c| c.matches(cx)) }
    }
    pub fn needs_body(&self) -> bool {
        self.conditions.iter().any(|c| c.field.needs_body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Address;

    fn cx(m: &MessageSummary) -> EvalContext<'_> {
        EvalContext { msg: m, body: Some("hello world invoice"), headers: None, account_name: "Work", sender_in_contacts: false, sender_is_vip: false, sender_is_blocked: false }
    }

    #[test]
    fn criteria_any_all() {
        let m = MessageSummary { subject: "Your Invoice".into(), from: Address::new("Shop", "billing@shop.com"), ..Default::default() };
        let c = Criteria {
            match_all: true,
            conditions: vec![
                Condition { field: Field::From, op: Op::EndsWith, value: "shop.com>".into(), header: String::new() },
                Condition { field: Field::Subject, op: Op::Contains, value: "invoice".into(), header: String::new() },
            ],
        };
        assert!(c.matches(&cx(&m)));
        let c2 = Criteria { match_all: true, conditions: vec![Condition { field: Field::Body, op: Op::DoesNotContain, value: "hello".into(), header: String::new() }] };
        assert!(!c2.matches(&cx(&m)));
    }
}
