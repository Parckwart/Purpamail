//! Computing the initial contents of a compose window from a [`ComposeInit`].

use crate::core::compose::{self, OutgoingAttachment, OutgoingMessage, ReplyKind};
use crate::core::config::{ComposingPrefs, MessageFormat, Settings};
use crate::core::mime::{self, ParsedMessage, escape_html};
use crate::core::model::{Address, Priority};
use crate::ui::richtext::{self, ParaAttr, RichDoc, Rgb, Style};

/// Where a newly chosen signature goes when the body has none yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SigAnchor {
    /// At the end of the message (new messages).
    #[default]
    End,
    /// After the first paragraph, above the quoted text (replies / forwards).
    AfterFirstParagraph,
}

/// Everything a compose window starts with.
#[derive(Clone, Debug, Default)]
pub struct Prefill {
    /// Account to send from (if determined by the message being answered).
    pub account_id: Option<String>,
    /// Preferred sender address (an alias of `account_id`).
    pub from_email: Option<String>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub body: RichDoc,
    /// Start in plain text mode.
    pub plain: bool,
    pub attachments: Vec<OutgoingAttachment>,
    /// Inline images referenced by the body (content id, bytes).
    pub inline_images: Vec<(String, Vec<u8>)>,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub priority: Priority,
    pub request_receipt: bool,
    /// Existing Message-ID to keep (drafts / undo send).
    pub message_id: Option<String>,
    /// Redirect: the original message, sent unchanged.
    pub redirect_raw: Option<Vec<u8>>,
    pub background: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub sig_anchor: SigAnchor,
    /// Insert the account's default signature.
    pub insert_signature: bool,
    /// Put the cursor in the body instead of the To field.
    pub focus_body: bool,
}

/// Plain text with paragraphs → document (no quote detection).
fn literal(text: &str) -> RichDoc {
    RichDoc::plain(text)
}

/// The original message body as a document.
fn original_body(orig: &ParsedMessage, rich: bool) -> RichDoc {
    match (&orig.html, rich) {
        (Some(h), true) => richtext::from_html_lite(h),
        _ => richtext::from_plain(orig.plain_text().trim_end()),
    }
}

/// Increase the quote level of every paragraph by one.
fn quoted(mut d: RichDoc) -> RichDoc {
    let n = d.char_len();
    d.indent(0, n, 1);
    d
}

/// Inline images of the original that the document references.
fn referenced_images(orig: &ParsedMessage, doc: &RichDoc) -> Vec<(String, Vec<u8>)> {
    doc.image_ids()
        .into_iter()
        .filter_map(|cid| orig.find_cid(&cid).map(|a| (cid.clone(), a.data.clone())))
        .collect()
}

fn to_outgoing(a: &mime::Attachment) -> OutgoingAttachment {
    OutgoingAttachment { filename: if a.filename.is_empty() { "attachment".into() } else { a.filename.clone() }, mime_type: a.mime_type.clone(), data: a.data.clone(), content_id: None }
}

/// A file name for a forwarded message.
pub fn eml_name(subject: &str) -> String {
    let s: String = subject.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim();
    format!("{}.eml", if s.is_empty() { "Forwarded Message" } else { s })
}

/// Attach a raw message as `message/rfc822`.
pub fn message_attachment(raw: &[u8]) -> OutgoingAttachment {
    let subject = mime::parse(raw).map(|p| p.subject).unwrap_or_default();
    OutgoingAttachment { filename: eml_name(&subject), mime_type: "message/rfc822".into(), data: raw.to_vec(), content_id: None }
}

/// The account that received `orig`: the first of the user's addresses among
/// its recipients.
pub fn receiving_address(orig: &ParsedMessage, settings: &Settings) -> Option<(String, String)> {
    let delivered: Vec<Address> = orig
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("Delivered-To") || k.eq_ignore_ascii_case("X-Original-To"))
        .map(|(_, v)| Address::new("", v.trim().trim_matches(['<', '>'])))
        .collect();
    for a in orig.to.iter().chain(orig.cc.iter()).chain(orig.bcc.iter()).chain(delivered.iter()).chain(std::iter::once(&orig.from)) {
        if let Some(acct) = settings.account_for_address(&a.email) {
            let email = acct.emails.iter().find(|e| e.eq_ignore_ascii_case(&a.email)).cloned().unwrap_or_else(|| a.email.clone());
            return Some((acct.id.clone(), email));
        }
    }
    None
}

/// Build the prefill for a reply / forward / redirect / re-edit.
pub fn respond(orig: &ParsedMessage, raw: &[u8], kind: ReplyKind, selected_text: Option<&str>, settings: &Settings, message_account: Option<&str>) -> Prefill {
    let prefs: &ComposingPrefs = &settings.composing;
    let my = settings.my_addresses();
    let mut p = Prefill { priority: Priority::Normal, ..Default::default() };
    if let Some((acct, email)) = receiving_address(orig, settings) {
        p.account_id = Some(acct);
        p.from_email = Some(email);
    } else if let Some(a) = message_account.and_then(|id| settings.account(id)) {
        p.account_id = Some(a.id.clone());
    }
    let rich_default = prefs.format == MessageFormat::RichText;
    let rich = if prefs.reply_same_format { orig.html.is_some() } else { rich_default };
    match kind {
        ReplyKind::Reply | ReplyKind::ReplyAll => {
            let (to, cc) = compose::reply_recipients(orig, kind == ReplyKind::ReplyAll, &my);
            p.to = to;
            p.cc = cc;
            p.subject = compose::prefixed_subject("Re:", &orig.subject);
            p.in_reply_to = orig.message_id.clone();
            p.references = orig.references.clone();
            if !orig.message_id.is_empty() && !p.references.contains(&orig.message_id) {
                p.references.push(orig.message_id.clone());
            }
            p.plain = !rich;
            let mut body = RichDoc::plain(if prefs.quote_original { "\n\n" } else { "" });
            if prefs.quote_original {
                let quoted_part = match selected_text.filter(|s| prefs.include_selected_text_only && !s.trim().is_empty()) {
                    Some(sel) => richtext::from_plain(sel.trim_end()),
                    None => original_body(orig, rich),
                };
                let quoted_part = if prefs.increase_quote_level { quoted(quoted_part) } else { quoted_part };
                body.append(&literal(&compose::attribution(orig)), false);
                body.append(&quoted_part, true);
                p.inline_images = referenced_images(orig, &body);
            }
            p.body = body;
            if prefs.include_attachments_in_reply {
                p.attachments = orig.real_attachments().map(to_outgoing).collect();
            }
            p.sig_anchor = SigAnchor::AfterFirstParagraph;
            p.insert_signature = true;
            p.focus_body = true;
        }
        ReplyKind::Forward => {
            p.subject = compose::prefixed_subject("Fwd:", &orig.subject);
            p.in_reply_to = orig.message_id.clone();
            p.references = orig.references.clone();
            if !orig.message_id.is_empty() && !p.references.contains(&orig.message_id) {
                p.references.push(orig.message_id.clone());
            }
            p.plain = !rich;
            let mut body = RichDoc::plain("\n");
            if rich {
                let inner = match &orig.html {
                    Some(h) => h.clone(),
                    None => escape_html(&orig.plain_text()).replace('\n', "<br>"),
                };
                let fwd = richtext::from_html_lite(&format!("{}{}</blockquote>", compose::forward_header_html(orig), inner));
                body.append(&fwd, true);
            } else {
                body.append(&literal(&format!("\n{}\n{}", compose::forward_header(orig), orig.plain_text().trim_end())), false);
            }
            p.inline_images = referenced_images(orig, &body);
            p.body = body;
            p.attachments = orig.real_attachments().map(to_outgoing).collect();
            p.sig_anchor = SigAnchor::AfterFirstParagraph;
            p.insert_signature = true;
            return p;
        }
        ReplyKind::ForwardAsAttachment => {
            p.subject = compose::prefixed_subject("Fwd:", &orig.subject);
            p.in_reply_to = orig.message_id.clone();
            p.attachments = vec![message_attachment(raw)];
            p.plain = !rich_default;
            p.body = RichDoc::default();
            p.insert_signature = true;
            return p;
        }
        ReplyKind::Redirect => {
            p.subject = orig.subject.clone();
            p.body = original_body(orig, orig.html.is_some());
            p.inline_images = referenced_images(orig, &p.body);
            p.plain = orig.html.is_none();
            p.redirect_raw = Some(raw.to_vec());
            p.from_email = p.from_email.or_else(|| p.account_id.as_deref().and_then(|a| settings.account(a)).map(|a| a.primary_email().to_string()));
            return p;
        }
        ReplyKind::SendAgain | ReplyKind::EditDraft => {
            p.to = orig.to.clone();
            p.cc = orig.cc.clone();
            p.bcc = orig.bcc.clone();
            p.reply_to = orig.reply_to.clone();
            p.subject = orig.subject.clone();
            p.priority = orig.priority;
            p.request_receipt = orig.header("Disposition-Notification-To").is_some();
            p.plain = orig.html.is_none();
            p.body = original_body(orig, orig.html.is_some());
            p.inline_images = referenced_images(orig, &p.body);
            p.attachments = orig.real_attachments().map(to_outgoing).collect();
            if let Some(acct) = settings.account_for_address(&orig.from.email) {
                p.account_id = Some(acct.id.clone());
                p.from_email = Some(orig.from.email.clone());
            }
            if kind == ReplyKind::EditDraft {
                p.in_reply_to = orig.in_reply_to.clone();
                p.references = orig.references.clone();
                if !orig.message_id.is_empty() {
                    p.message_id = Some(orig.message_id.clone());
                }
            }
            p.focus_body = !p.to.is_empty();
            return p;
        }
    }
    p
}

/// Prefill for a new message.
pub fn new_message(to: Vec<Address>, cc: Vec<Address>, subject: String, body: &str, attachments: Vec<OutgoingAttachment>, settings: &Settings) -> Prefill {
    Prefill {
        to,
        cc,
        subject,
        body: if body.is_empty() { RichDoc::default() } else { richtext::from_plain(body) },
        plain: settings.composing.format == MessageFormat::PlainText,
        attachments,
        insert_signature: true,
        ..Default::default()
    }
}

/// Prefill from a `mailto:` URL.
pub fn mailto(url: &str, settings: &Settings) -> Prefill {
    let (to, subject, body) = crate::ui::app::parse_mailto(url);
    let mut p = new_message(to, vec![], subject, &body, vec![], settings);
    p.focus_body = !p.to.is_empty();
    p
}

/// Prefill from a template / stationery.
pub fn template(id: &str, settings: &Settings) -> Prefill {
    let mut p = new_message(vec![], vec![], String::new(), "", vec![], settings);
    if let Some(t) = settings.templates.iter().find(|t| t.id == id) {
        p.subject = t.subject.clone();
        p.body = richtext::from_markup(&t.body);
        p.background = crate::ui::theme::hex(&t.background).map(|c| [c.r(), c.g(), c.b()]);
        p.accent = crate::ui::theme::hex(&t.accent).map(|c| [c.r(), c.g(), c.b()]);
        p.plain = false;
    }
    p
}

/// Prefill re-opening an outgoing message exactly (undo send).
pub fn outgoing(m: &OutgoingMessage) -> Prefill {
    let body = match &m.html {
        Some(h) => richtext::from_html_lite(h),
        None => richtext::from_plain(&m.text),
    };
    let (inline, files): (Vec<_>, Vec<_>) = m.attachments.iter().cloned().partition(|a| a.content_id.is_some());
    Prefill {
        account_id: (!m.account_id.is_empty()).then(|| m.account_id.clone()),
        from_email: (!m.from.email.is_empty()).then(|| m.from.email.clone()),
        to: m.to.clone(),
        cc: m.cc.clone(),
        bcc: m.bcc.clone(),
        reply_to: m.reply_to.clone(),
        subject: m.subject.clone(),
        body,
        plain: m.html.is_none(),
        attachments: files,
        inline_images: inline.into_iter().map(|a| (a.content_id.unwrap_or_default().trim_matches(['<', '>']).to_string(), a.data)).collect(),
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        priority: m.priority,
        request_receipt: m.request_receipt,
        message_id: (!m.message_id.is_empty()).then(|| m.message_id.clone()),
        redirect_raw: m.redirect_raw.clone(),
        focus_body: true,
        ..Default::default()
    }
}

/// Words that suggest the sender meant to attach something.
pub const ATTACHMENT_WORDS: [&str; 7] = ["attached", "attachment", "attaching", "enclosed", "anbei", "anhang", "angehängt"];

/// Does the user's own text (not quotes, not the signature) mention an attachment?
pub fn mentions_attachment(doc: &RichDoc) -> bool {
    let ranges = doc.para_ranges();
    for (i, r) in ranges.iter().enumerate() {
        if doc.paras()[i].quote > 0 {
            continue;
        }
        let text: String = doc.slice(r.clone()).chars().enumerate().filter(|(ci, _)| !doc.style_at(r.start + ci).signature).map(|(_, c)| c).collect();
        let lower = text.to_lowercase();
        if lower.split(|c: char| !c.is_alphanumeric()).any(|w| ATTACHMENT_WORDS.contains(&w)) {
            return true;
        }
    }
    false
}

/// The signature block to insert: two blank lines then the signature, all
/// marked with [`Style::signature`].
pub fn signature_block(markup: &str) -> RichDoc {
    let sig = richtext::from_markup(markup);
    let mut d = RichDoc::styled("\n\n", Style::default(), ParaAttr::default());
    d.append(&sig, false);
    let n = d.char_len();
    d.update_styles(0, n, |s| s.signature = true);
    d
}

/// Replace (or insert, or remove with `markup == None`) the signature block in `doc`.
pub fn replace_signature(doc: &mut RichDoc, markup: Option<&str>, anchor: SigAnchor) {
    let existing = doc.ranges_where(|s| s.signature);
    let mut pos = existing.first().map(|r| r.start);
    for r in existing.iter().rev() {
        doc.delete(r.start, r.end);
    }
    let Some(m) = markup.filter(|m| !m.trim().is_empty()) else { return };
    let pos = pos.take().unwrap_or_else(|| match anchor {
        SigAnchor::End => doc.char_len(),
        SigAnchor::AfterFirstParagraph => doc.para_ranges()[0].end,
    });
    let block = signature_block(m);
    doc.splice(pos, pos, &block);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AccountConfig;
    use crate::ui::richtext::ListKind;

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.accounts.push(AccountConfig { id: "work".into(), full_name: "Me".into(), emails: vec!["me@work.com".into(), "alias@work.com".into()], ..Default::default() });
        s.accounts.push(AccountConfig { id: "home".into(), full_name: "Me".into(), emails: vec!["me@home.org".into()], ..Default::default() });
        s
    }

    fn orig(html: bool) -> ParsedMessage {
        ParsedMessage {
            subject: "Lunch".into(),
            from: Address::new("Ann", "ann@x.com"),
            to: vec![Address::new("Me", "alias@work.com"), Address::new("Bo", "bo@x.com")],
            cc: vec![Address::new("Cy", "cy@x.com")],
            message_id: "orig@x.com".into(),
            references: vec!["root@x.com".into()],
            date: 1_700_000_000,
            text: Some("Hello there\n> older quote\nBye".into()),
            html: html.then(|| "<p>Hello <b>there</b></p><ul><li>pizza</li></ul>".to_string()),
            attachments: vec![mime::Attachment { filename: "menu.pdf".into(), mime_type: "application/pdf".into(), content_id: None, inline: false, data: vec![1, 2, 3] }],
            ..Default::default()
        }
    }

    #[test]
    fn reply_all_recipients_threading_and_from() {
        let s = settings();
        let p = respond(&orig(false), b"raw", ReplyKind::ReplyAll, None, &s, None);
        assert_eq!(p.to, vec![Address::new("Ann", "ann@x.com")]);
        assert_eq!(p.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), vec!["bo@x.com", "cy@x.com"]);
        assert_eq!(p.subject, "Re: Lunch");
        assert_eq!(p.in_reply_to, "orig@x.com");
        assert_eq!(p.references, vec!["root@x.com".to_string(), "orig@x.com".to_string()]);
        assert_eq!(p.account_id.as_deref(), Some("work"));
        assert_eq!(p.from_email.as_deref(), Some("alias@work.com"));
        assert!(p.attachments.is_empty());
    }

    #[test]
    fn plain_reply_quotes_with_levels() {
        let s = settings();
        let p = respond(&orig(false), b"raw", ReplyKind::Reply, None, &s, None);
        assert!(p.plain);
        let plain = richtext::to_plain(&p.body);
        let lines: Vec<&str> = plain.lines().collect();
        assert_eq!(&lines[..2], &["", ""]);
        assert!(lines[2].starts_with("On ") && lines[2].ends_with("Ann <ann@x.com> wrote:"), "{plain}");
        assert_eq!(&lines[3..], &["> Hello there", ">> older quote", "> Bye"]);
    }

    #[test]
    fn rich_reply_keeps_formatting_inside_quote() {
        let s = settings();
        let p = respond(&orig(true), b"raw", ReplyKind::Reply, None, &s, None);
        assert!(!p.plain);
        let d = &p.body;
        let t = d.text();
        let pos = t[..t.find("there").unwrap()].chars().count();
        assert!(d.style_at(pos).bold);
        let pi = d.para_at(pos);
        assert_eq!(d.paras()[pi].quote, 1);
        let last = d.paras().last().unwrap();
        assert_eq!(last.list, ListKind::Bullet);
        assert_eq!(last.quote, 1);
        assert!(richtext::to_html_fragment(d).contains("<blockquote"));
    }

    #[test]
    fn selected_text_only_and_no_quote() {
        let mut s = settings();
        let p = respond(&orig(false), b"raw", ReplyKind::Reply, Some("just this"), &s, None);
        assert!(richtext::to_plain(&p.body).ends_with("> just this"));
        s.composing.quote_original = false;
        let p = respond(&orig(false), b"raw", ReplyKind::Reply, None, &s, None);
        assert_eq!(p.body.text(), "");
        s.composing.quote_original = true;
        s.composing.include_attachments_in_reply = true;
        let p = respond(&orig(false), b"raw", ReplyKind::Reply, None, &s, None);
        assert_eq!(p.attachments.len(), 1);
    }

    #[test]
    fn forward_variants() {
        let s = settings();
        let p = respond(&orig(false), b"raw", ReplyKind::Forward, None, &s, None);
        assert_eq!(p.subject, "Fwd: Lunch");
        assert!(p.to.is_empty());
        assert_eq!(p.attachments.len(), 1);
        assert!(richtext::to_plain(&p.body).contains("Begin forwarded message:"));
        let p = respond(&orig(true), b"raw", ReplyKind::Forward, None, &s, None);
        assert!(p.body.text().contains("Begin forwarded message:"));
        assert!(p.body.paras().iter().any(|a| a.quote == 1));
        let raw = OutgoingMessage { subject: "A/B".into(), from: Address::new("", "a@b.c"), text: "x".into(), ..Default::default() }.to_mime(false);
        let p = respond(&mime::parse(&raw).unwrap(), &raw, ReplyKind::ForwardAsAttachment, None, &s, None);
        assert_eq!(p.attachments[0].mime_type, "message/rfc822");
        assert_eq!(p.attachments[0].filename, "A_B.eml");
        let p = respond(&orig(false), b"raw", ReplyKind::Redirect, None, &s, None);
        assert_eq!(p.redirect_raw.as_deref(), Some(&b"raw"[..]));
        assert!(p.to.is_empty());
    }

    #[test]
    fn signature_insert_replace_remove() {
        let mut d = RichDoc::plain("\n\nOn x wrote:");
        replace_signature(&mut d, Some("**Jane**"), SigAnchor::AfterFirstParagraph);
        assert_eq!(d.text(), "\n\nJane\n\nOn x wrote:");
        assert!(d.style_at(2).bold && d.style_at(2).signature);
        replace_signature(&mut d, Some("Bob"), SigAnchor::AfterFirstParagraph);
        assert_eq!(d.text(), "\n\nBob\n\nOn x wrote:");
        replace_signature(&mut d, None, SigAnchor::AfterFirstParagraph);
        assert_eq!(d.text(), "\n\nOn x wrote:");
        let mut d = RichDoc::plain("Hi");
        replace_signature(&mut d, Some("Sig"), SigAnchor::End);
        assert_eq!(d.text(), "Hi\n\nSig");
    }

    #[test]
    fn attachment_keyword_detection() {
        assert!(mentions_attachment(&RichDoc::plain("See the attached file.")));
        assert!(mentions_attachment(&RichDoc::plain("Anbei die Unterlagen")));
        assert!(!mentions_attachment(&RichDoc::plain("Nothing here")));
        let mut d = richtext::from_plain("Hi\n> see attachment");
        assert!(!mentions_attachment(&d));
        replace_signature(&mut d, Some("Attachments welcome"), SigAnchor::End);
        assert!(!mentions_attachment(&d));
    }

    #[test]
    fn outgoing_round_trip_and_templates() {
        let m = OutgoingMessage {
            account_id: "work".into(),
            from: Address::new("Me", "me@work.com"),
            to: vec![Address::new("", "a@b.cd")],
            subject: "S".into(),
            text: "body".into(),
            html: Some("<div><b>body</b></div>".into()),
            attachments: vec![OutgoingAttachment { filename: "i.png".into(), mime_type: "image/png".into(), data: vec![9], content_id: Some("img1".into()) }],
            message_id: "m1@work.com".into(),
            ..Default::default()
        };
        let p = outgoing(&m);
        assert!(!p.plain && p.body.style_at(0).bold);
        assert_eq!(p.inline_images[0].0, "img1");
        assert!(p.attachments.is_empty());
        assert_eq!(p.message_id.as_deref(), Some("m1@work.com"));
        let s = settings();
        let t = template("birthday", &s);
        assert_eq!(t.subject, "Happy Birthday!");
        assert_eq!(t.background, Some([0xff, 0xf4, 0xe6]));
        assert!(t.body.style_at(0).bold);
        assert_eq!(eml_name(""), "Forwarded Message.eml");
    }
}
