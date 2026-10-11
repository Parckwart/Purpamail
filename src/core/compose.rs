//! Building outgoing messages (new, reply, forward, redirect) as MIME.

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as BAddress;
use mail_builder::headers::raw::Raw;
use serde::{Deserialize, Serialize};

use super::mime::{self, ParsedMessage, escape_html};
use super::model::{Address, MessageSummary, Priority, format_address_list, now_ts};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct OutgoingAttachment {
    pub filename: String,
    pub mime_type: String,
    #[serde(with = "b64")]
    pub data: Vec<u8>,
    /// Inline content id (for images embedded in rich text).
    pub content_id: Option<String>,
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

/// A message as composed by the user.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct OutgoingMessage {
    pub account_id: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub text: String,
    /// HTML alternative (rich text messages).
    pub html: Option<String>,
    pub attachments: Vec<OutgoingAttachment>,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub priority: Priority,
    /// Request a read receipt.
    pub request_receipt: bool,
    /// Pre-generated Message-ID (without brackets).
    pub message_id: String,
    /// Extra headers (e.g. Resent-* for redirect).
    pub extra_headers: Vec<(String, String)>,
    /// For redirect: the original raw message, sent unchanged with Resent-* headers.
    #[serde(default)]
    pub redirect_raw: Option<Vec<u8>>,
}

pub fn new_message_id(from: &Address) -> String {
    let domain = from.domain();
    let domain = if domain.is_empty() { "purpamail.local" } else { domain };
    format!("{}@{}", uuid::Uuid::new_v4().simple(), domain)
}

fn baddr(a: &Address) -> BAddress<'static> {
    if a.name.is_empty() {
        BAddress::new_address(None::<String>, a.email.clone())
    } else {
        BAddress::new_address(Some(a.name.clone()), a.email.clone())
    }
}

fn blist(v: &[Address]) -> BAddress<'static> {
    BAddress::new_list(v.iter().map(baddr).collect())
}

impl OutgoingMessage {
    pub fn all_recipients(&self) -> Vec<Address> {
        self.to.iter().chain(self.cc.iter()).chain(self.bcc.iter()).cloned().collect()
    }

    /// Serialize to RFC 5322. `include_bcc` should be true for the copy stored in Sent.
    pub fn to_mime(&self, include_bcc: bool) -> Vec<u8> {
        if let Some(raw) = &self.redirect_raw {
            let mut out = String::new();
            for (k, v) in &self.extra_headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            let mut bytes = out.into_bytes();
            bytes.extend_from_slice(raw);
            return bytes;
        }
        let mid = if self.message_id.is_empty() { new_message_id(&self.from) } else { self.message_id.clone() };
        let mut b = MessageBuilder::new().from(baddr(&self.from)).subject(self.subject.clone()).date(now_ts()).message_id(mid);
        if !self.to.is_empty() {
            b = b.to(blist(&self.to));
        }
        if !self.cc.is_empty() {
            b = b.cc(blist(&self.cc));
        }
        if include_bcc && !self.bcc.is_empty() {
            b = b.bcc(blist(&self.bcc));
        }
        if !self.reply_to.is_empty() {
            b = b.reply_to(blist(&self.reply_to));
        }
        if !self.in_reply_to.is_empty() {
            b = b.in_reply_to(self.in_reply_to.clone());
        }
        if !self.references.is_empty() {
            b = b.header("References", Raw::new(self.references.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" ")));
        }
        match self.priority {
            Priority::High => {
                b = b.header("X-Priority", Raw::new("1 (Highest)")).header("Importance", Raw::new("High"));
            }
            Priority::Low => {
                b = b.header("X-Priority", Raw::new("5 (Lowest)")).header("Importance", Raw::new("Low"));
            }
            Priority::Normal => {}
        }
        if self.request_receipt {
            b = b.header("Disposition-Notification-To", Raw::new(self.from.to_header()));
        }
        b = b.header("X-Mailer", Raw::new(format!("Purpamail {}", env!("CARGO_PKG_VERSION"))));
        for (k, v) in &self.extra_headers {
            b = b.header(k.clone(), Raw::new(v.clone()));
        }
        b = b.text_body(self.text.clone());
        if let Some(h) = &self.html {
            b = b.html_body(h.clone());
        }
        for a in &self.attachments {
            let mt = if a.mime_type.is_empty() { guess_mime(&a.filename).to_string() } else { a.mime_type.clone() };
            if let Some(cid) = &a.content_id {
                b = b.inline(mt, cid.clone(), a.data.clone());
            } else {
                b = b.attachment(mt, a.filename.clone(), a.data.clone());
            }
        }
        b.write_to_vec().unwrap_or_default()
    }

    /// Summary row for storing this message locally (Sent / Drafts / Outbox).
    pub fn summary(&self) -> MessageSummary {
        MessageSummary {
            account_id: self.account_id.clone(),
            message_id: self.message_id.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            subject: self.subject.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            reply_to: self.reply_to.clone(),
            date: now_ts(),
            received: now_ts(),
            has_attachments: self.attachments.iter().any(|a| a.content_id.is_none()),
            snippet: mime::make_snippet(&self.text),
            priority: self.priority,
            ..Default::default()
        }
    }
}

pub fn guess_mime(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "txt" | "log" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "eml" => "message/rfc822",
        "json" => "application/json",
        "xml" => "application/xml",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

// ------------------------------------------------------------------ reply / forward helpers

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplyKind {
    Reply,
    ReplyAll,
    Forward,
    ForwardAsAttachment,
    Redirect,
    /// Open a sent message again as a new draft ("Send Again").
    SendAgain,
    /// Re-open a draft for editing.
    EditDraft,
}

/// Format a timestamp the way quoted headers expect.
pub fn format_date_long(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%A, %-d %B %Y at %H:%M").to_string())
        .unwrap_or_default()
}

/// Prefix a subject with Re: / Fwd: unless already present.
pub fn prefixed_subject(prefix: &str, subject: &str) -> String {
    let lower = subject.trim().to_lowercase();
    let p = prefix.to_lowercase();
    if lower.starts_with(&p) || (p == "fwd:" && lower.starts_with("fw:")) {
        subject.trim().to_string()
    } else {
        format!("{prefix} {}", subject.trim())
    }
}

/// Quote plain text with "> " prefixes.
pub fn quote_text(text: &str) -> String {
    text.lines().map(|l| if l.starts_with('>') { format!(">{l}") } else { format!("> {l}") }).collect::<Vec<_>>().join("\n")
}

/// Compute recipients for a reply.
pub fn reply_recipients(orig: &ParsedMessage, all: bool, my_addresses: &[String]) -> (Vec<Address>, Vec<Address>) {
    let is_me = |a: &Address| my_addresses.iter().any(|m| m.eq_ignore_ascii_case(&a.email));
    let from_me = is_me(&orig.from);
    let mut to: Vec<Address> = if from_me {
        orig.to.clone()
    } else if !orig.reply_to.is_empty() {
        orig.reply_to.clone()
    } else {
        vec![orig.from.clone()]
    };
    let mut cc = Vec::new();
    if all {
        for a in orig.to.iter().chain(orig.cc.iter()) {
            if !is_me(a) && !to.iter().any(|t| t.email.eq_ignore_ascii_case(&a.email)) && !cc.iter().any(|t: &Address| t.email.eq_ignore_ascii_case(&a.email)) {
                cc.push(a.clone());
            }
        }
    }
    to.dedup_by(|a, b| a.email.eq_ignore_ascii_case(&b.email));
    (to, cc)
}

/// Attribution line placed above quoted text.
pub fn attribution(orig: &ParsedMessage) -> String {
    format!("On {}, {} wrote:", format_date_long(orig.date), orig.from.to_header())
}

/// Header block placed above forwarded content.
pub fn forward_header(orig: &ParsedMessage) -> String {
    let mut s = String::from("Begin forwarded message:\n\n");
    s.push_str(&format!("From: {}\n", orig.from.to_header()));
    s.push_str(&format!("Subject: {}\n", orig.subject));
    s.push_str(&format!("Date: {}\n", format_date_long(orig.date)));
    if !orig.to.is_empty() {
        s.push_str(&format!("To: {}\n", format_address_list(&orig.to)));
    }
    if !orig.cc.is_empty() {
        s.push_str(&format!("Cc: {}\n", format_address_list(&orig.cc)));
    }
    s
}

pub fn forward_header_html(orig: &ParsedMessage) -> String {
    let row = |k: &str, v: &str| format!("<div><b>{k}:</b> {}</div>", escape_html(v));
    let mut s = String::from("<div><br>Begin forwarded message:<br><br></div><blockquote type=\"cite\">");
    s.push_str(&row("From", &orig.from.to_header()));
    s.push_str(&row("Subject", &orig.subject));
    s.push_str(&row("Date", &format_date_long(orig.date)));
    if !orig.to.is_empty() {
        s.push_str(&row("To", &format_address_list(&orig.to)));
    }
    if !orig.cc.is_empty() {
        s.push_str(&row("Cc", &format_address_list(&orig.cc)));
    }
    s.push_str("<br>");
    s
}

/// Build Resent-* headers for redirecting a message.
pub fn redirect_headers(from: &Address, to: &[Address]) -> Vec<(String, String)> {
    vec![
        ("Resent-From".into(), from.to_header()),
        ("Resent-To".into(), format_address_list(to)),
        ("Resent-Date".into(), chrono::Utc::now().to_rfc2822()),
        ("Resent-Message-ID".into(), format!("<{}>", new_message_id(from))),
    ]
}

/// Remove attachments from a raw message, replacing each with a short text note.
pub fn strip_attachments(raw: &[u8]) -> Option<Vec<u8>> {
    let p = mime::parse(raw)?;
    let mut b = MessageBuilder::new().from(baddr(&p.from)).subject(p.subject.clone()).date(p.date);
    if !p.message_id.is_empty() {
        b = b.message_id(p.message_id.clone());
    }
    if !p.to.is_empty() {
        b = b.to(blist(&p.to));
    }
    if !p.cc.is_empty() {
        b = b.cc(blist(&p.cc));
    }
    if !p.in_reply_to.is_empty() {
        b = b.in_reply_to(p.in_reply_to.clone());
    }
    if !p.references.is_empty() {
        b = b.header("References", Raw::new(p.references.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" ")));
    }
    let notes: String = p.real_attachments().map(|a| format!("\n[Attachment removed: {} ({})]", a.filename, mime::human_size(a.size() as u64))).collect();
    b = b.text_body(format!("{}{}", p.text.clone().unwrap_or_else(|| p.plain_text()), notes));
    if let Some(h) = &p.html {
        b = b.html_body(format!("{h}<pre>{}</pre>", escape_html(&notes)));
    }
    b.write_to_vec().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_parse() {
        let m = OutgoingMessage {
            from: Address::new("Me", "me@example.com"),
            to: vec![Address::new("You", "you@example.com")],
            bcc: vec![Address::new("", "secret@example.com")],
            subject: "Grüße".into(),
            text: "Hello\nWorld".into(),
            html: Some("<p>Hello</p>".into()),
            attachments: vec![OutgoingAttachment { filename: "a.txt".into(), mime_type: String::new(), data: b"abc".to_vec(), content_id: None }],
            priority: Priority::High,
            message_id: "x1@example.com".into(),
            ..Default::default()
        };
        let raw = m.to_mime(false);
        let p = mime::parse(&raw).unwrap();
        assert_eq!(p.subject, "Grüße");
        assert_eq!(p.message_id, "x1@example.com");
        assert!(p.bcc.is_empty());
        assert_eq!(p.priority, Priority::High);
        assert_eq!(p.real_attachments().count(), 1);
        assert!(p.html.is_some());
        let raw2 = m.to_mime(true);
        assert_eq!(mime::parse(&raw2).unwrap().bcc.len(), 1);
    }

    #[test]
    fn reply_all_excludes_me() {
        let orig = ParsedMessage {
            from: Address::new("A", "a@x.com"),
            to: vec![Address::new("Me", "me@x.com"), Address::new("B", "b@x.com")],
            cc: vec![Address::new("C", "c@x.com")],
            ..Default::default()
        };
        let (to, cc) = reply_recipients(&orig, true, &["me@x.com".into()]);
        assert_eq!(to.len(), 1);
        assert_eq!(cc.len(), 2);
        assert_eq!(prefixed_subject("Re:", "re: hi"), "re: hi");
        assert_eq!(prefixed_subject("Fwd:", "hi"), "Fwd: hi");
    }
}
