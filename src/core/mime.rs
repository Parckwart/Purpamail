//! Parsing of raw RFC 5322 / MIME messages.

use mail_parser::{HeaderValue, MessageParser, MimeHeaders, PartType};

use super::model::{Address, Category, MessageSummary, Priority, flags, now_ts};

#[derive(Clone, Debug, Default)]
pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    pub content_id: Option<String>,
    pub inline: bool,
    pub data: Vec<u8>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ParsedMessage {
    pub headers: Vec<(String, String)>,
    pub subject: String,
    pub from: Address,
    pub sender: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub date: i64,
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub priority: Priority,
    pub list_unsubscribe: String,
    pub list_id: String,
    /// text/calendar payload (meeting invitations).
    pub calendar: Option<String>,
    /// S/MIME or PGP signed.
    pub signed: bool,
    /// S/MIME or PGP encrypted.
    pub encrypted: bool,
}

impl ParsedMessage {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Best plain-text representation of the body.
    pub fn plain_text(&self) -> String {
        if let Some(t) = &self.text {
            if !t.trim().is_empty() {
                return t.clone();
            }
        }
        if let Some(h) = &self.html {
            return html_to_text(h);
        }
        String::new()
    }

    pub fn real_attachments(&self) -> impl Iterator<Item = &Attachment> {
        self.attachments.iter().filter(|a| !(a.inline && a.content_id.is_some() && a.is_image()))
    }

    pub fn find_cid(&self, cid: &str) -> Option<&Attachment> {
        let cid = cid.trim_start_matches("cid:").trim_matches(['<', '>']);
        self.attachments.iter().find(|a| a.content_id.as_deref().map(|c| c.trim_matches(['<', '>'])) == Some(cid))
    }
}

fn conv_addrs(v: Option<&mail_parser::Address>) -> Vec<Address> {
    match v {
        Some(a) => a
            .iter()
            .map(|x| Address { name: x.name().unwrap_or("").to_string(), email: x.address().unwrap_or("").to_string() })
            .filter(|a| !a.email.is_empty() || !a.name.is_empty())
            .collect(),
        None => Vec::new(),
    }
}

fn text_list(v: &HeaderValue) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

pub fn parse(raw: &[u8]) -> Option<ParsedMessage> {
    let msg = MessageParser::default().parse(raw)?;
    let mut p = ParsedMessage {
        headers: msg.headers_raw().map(|(k, v)| (k.to_string(), unfold(v))).collect(),
        subject: msg.subject().unwrap_or("").to_string(),
        from: conv_addrs(msg.from()).into_iter().next().unwrap_or_default(),
        sender: conv_addrs(msg.sender()).into_iter().next(),
        to: conv_addrs(msg.to()),
        cc: conv_addrs(msg.cc()),
        bcc: conv_addrs(msg.bcc()),
        reply_to: conv_addrs(msg.reply_to()),
        date: msg.date().map(|d| d.to_timestamp()).unwrap_or(0),
        message_id: msg.message_id().unwrap_or("").to_string(),
        in_reply_to: text_list(msg.in_reply_to()).into_iter().next().unwrap_or_default(),
        references: text_list(msg.references()),
        ..Default::default()
    };
    // Decode the raw header values for display (RFC 2047)
    for (k, v) in p.headers.iter_mut() {
        if let Some(h) = msg.headers().iter().find(|h| h.name().eq_ignore_ascii_case(k)) {
            if let HeaderValue::Text(t) = h.value() {
                *v = t.to_string();
            }
        }
    }
    p.priority = priority_from_headers(&p);
    p.list_unsubscribe = p.header("List-Unsubscribe").unwrap_or("").to_string();
    p.list_id = p.header("List-Id").unwrap_or("").to_string();

    if let Some(ct) = msg.root_part().content_type() {
        let sub = ct.subtype().unwrap_or("").to_ascii_lowercase();
        if ct.ctype().eq_ignore_ascii_case("multipart") && sub == "signed" {
            p.signed = true;
        }
        if ct.ctype().eq_ignore_ascii_case("multipart") && sub == "encrypted" {
            p.encrypted = true;
        }
        if ct.ctype().eq_ignore_ascii_case("application") && (sub == "pkcs7-mime" || sub == "x-pkcs7-mime") {
            p.encrypted = !matches!(ct.attribute("smime-type"), Some("signed-data"));
            p.signed = !p.encrypted;
        }
    }

    let texts: Vec<String> = msg.text_bodies().filter(|part| !part.is_text_html()).filter_map(|part| part.text_contents().map(|t| t.to_string())).collect();
    let htmls: Vec<String> = msg
        .html_bodies()
        .filter(|part| part.is_text_html())
        .filter_map(|part| part.text_contents().map(|t| t.to_string()))
        .collect();
    if !texts.is_empty() {
        p.text = Some(texts.join("\n"));
    }
    if !htmls.is_empty() {
        p.html = Some(htmls.join("\n"));
    }

    for part in msg.parts.iter() {
        let ct = part.content_type();
        let ctype = ct.map(|c| c.ctype().to_ascii_lowercase()).unwrap_or_else(|| "text".into());
        let sub = ct.and_then(|c| c.subtype()).map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "plain".into());
        if ctype == "text" && sub == "calendar" {
            if let Some(t) = part.text_contents() {
                p.calendar = Some(t.to_string());
            }
        }
        if ctype == "application" && (sub == "pgp-signature" || sub == "pkcs7-signature" || sub == "x-pkcs7-signature") {
            p.signed = true;
        }
        let disposition_attachment = part.content_disposition().map(|d| d.is_attachment()).unwrap_or(false);
        let is_attachment_like = match &part.body {
            PartType::Binary(_) | PartType::InlineBinary(_) => true,
            PartType::Message(_) => true,
            PartType::Text(_) | PartType::Html(_) => disposition_attachment || part.attachment_name().is_some(),
            PartType::Multipart(_) => false,
        };
        if !is_attachment_like {
            continue;
        }
        let data: Vec<u8> = match &part.body {
            PartType::Message(m) => m.raw_message().to_vec(),
            _ => part.contents().to_vec(),
        };
        let filename = part.attachment_name().map(|s| s.to_string()).unwrap_or_else(|| {
            let ext = match (ctype.as_str(), sub.as_str()) {
                ("message", "rfc822") => "eml",
                ("text", "calendar") => "ics",
                ("text", "html") => "html",
                ("text", _) => "txt",
                ("image", s) => s,
                _ => "bin",
            };
            format!("Attachment.{ext}")
        });
        p.attachments.push(Attachment {
            filename,
            mime_type: format!("{ctype}/{sub}"),
            content_id: part.content_id().map(|s| s.to_string()),
            inline: matches!(part.body, PartType::InlineBinary(_)) || part.content_disposition().map(|d| d.is_inline()).unwrap_or(false),
            data,
        });
    }
    Some(p)
}

fn unfold(v: &str) -> String {
    v.replace("\r\n", "").replace('\n', "").trim().to_string()
}

fn priority_from_headers(p: &ParsedMessage) -> Priority {
    if let Some(x) = p.header("X-Priority") {
        let n: i32 = x.trim().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(3);
        return match n {
            1 | 2 => Priority::High,
            4 | 5 => Priority::Low,
            _ => Priority::Normal,
        };
    }
    if let Some(x) = p.header("Importance").or(p.header("Priority")) {
        let x = x.to_ascii_lowercase();
        if x.contains("high") || x.contains("urgent") {
            return Priority::High;
        }
        if x.contains("low") || x.contains("non-urgent") {
            return Priority::Low;
        }
    }
    Priority::Normal
}

/// Build a message summary from a parsed message. Ids / mailbox are left unset.
pub fn summarize(p: &ParsedMessage, raw_len: usize) -> MessageSummary {
    let body = p.plain_text();
    MessageSummary {
        message_id: p.message_id.clone(),
        in_reply_to: p.in_reply_to.clone(),
        references: p.references.clone(),
        subject: p.subject.clone(),
        from: p.from.clone(),
        to: p.to.clone(),
        cc: p.cc.clone(),
        bcc: p.bcc.clone(),
        reply_to: p.reply_to.clone(),
        date: if p.date == 0 { now_ts() } else { p.date },
        received: now_ts(),
        size: raw_len as u32,
        flags: flags::HAS_BODY,
        has_attachments: p.real_attachments().next().is_some(),
        snippet: make_snippet(&body),
        priority: p.priority,
        category: Category::Primary,
        list_unsubscribe: p.list_unsubscribe.clone(),
        ..Default::default()
    }
}

/// First ~300 characters of visible text, skipping quoted lines and signatures.
pub fn make_snippet(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let l = line.trim();
        if l.starts_with('>') {
            continue;
        }
        if l == "--" || l == "-- " {
            break;
        }
        if l.starts_with("On ") && l.ends_with("wrote:") {
            break;
        }
        if l.is_empty() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        out.push_str(l);
        out.push(' ');
        if out.len() > 300 {
            break;
        }
    }
    let mut s: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() > 300 {
        let mut cut = 300;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Very small HTML to text conversion used for snippets, search indexing and plain-text replies.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.char_indices().peekable();
    let lower = html.to_ascii_lowercase();
    let mut skip_until: Option<&str> = None;
    while let Some((i, c)) = chars.next() {
        if let Some(end) = skip_until {
            if lower[i..].starts_with(end) {
                skip_until = None;
            }
            continue;
        }
        if c == '<' {
            let rest = &lower[i..];
            if rest.starts_with("<style") {
                skip_until = Some("</style");
                continue;
            }
            if rest.starts_with("<script") {
                skip_until = Some("</script");
                continue;
            }
            if rest.starts_with("<head") {
                skip_until = Some("</head");
                continue;
            }
            if rest.starts_with("<!--") {
                skip_until = Some("-->");
                continue;
            }
            let tag_end = rest.find('>').unwrap_or(rest.len());
            let tag = &rest[1..tag_end];
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            match name.as_str() {
                "br" => out.push('\n'),
                "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "table" | "ul" | "ol" => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if name == "li" && !tag.starts_with('/') {
                        out.push_str("• ");
                    }
                }
                "td" | "th" => out.push(' '),
                _ => {}
            }
            // advance iterator to tag end
            let target = i + tag_end;
            while let Some(&(j, _)) = chars.peek() {
                if j > target {
                    break;
                }
                chars.next();
            }
            continue;
        }
        if c == '&' {
            let rest = &html[i..];
            if let Some(semi) = rest[..rest.len().min(12)].find(';') {
                let ent = &rest[1..semi];
                let decoded = decode_entity(ent);
                if let Some(d) = decoded {
                    out.push_str(&d);
                    let target = i + semi;
                    while let Some(&(j, _)) = chars.peek() {
                        if j > target {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
            }
        }
        if c == '\n' || c == '\r' || c == '\t' {
            if !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
            continue;
        }
        out.push(c);
    }
    // collapse runs of blank lines and spaces
    let mut res = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let l = line.split(' ').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        let l = l.trim_matches('\u{a0}').to_string();
        if l.trim().is_empty() {
            blank += 1;
            if blank <= 1 && !res.is_empty() {
                res.push('\n');
            }
        } else {
            blank = 0;
            res.push_str(&l);
            res.push('\n');
        }
    }
    res.trim().to_string()
}

pub fn decode_entity(ent: &str) -> Option<String> {
    Some(match ent {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" | "#39" => "'".into(),
        "nbsp" => "\u{a0}".into(),
        "copy" => "©".into(),
        "reg" => "®".into(),
        "trade" => "™".into(),
        "hellip" => "…".into(),
        "mdash" => "—".into(),
        "ndash" => "–".into(),
        "laquo" => "«".into(),
        "raquo" => "»".into(),
        "lsquo" => "‘".into(),
        "rsquo" => "’".into(),
        "ldquo" => "“".into(),
        "rdquo" => "”".into(),
        "bull" => "•".into(),
        "middot" => "·".into(),
        "euro" => "€".into(),
        "pound" => "£".into(),
        "yen" => "¥".into(),
        "cent" => "¢".into(),
        "deg" => "°".into(),
        "times" => "×".into(),
        "zwnj" | "zwj" | "shy" => "".into(),
        e if e.starts_with("#x") || e.starts_with("#X") => char::from_u32(u32::from_str_radix(&e[2..], 16).ok()?)?.to_string(),
        e if e.starts_with('#') => char::from_u32(e[1..].parse().ok()?)?.to_string(),
        _ => return None,
    })
}

/// Escape text for inclusion in HTML.
pub fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

/// Strip angle brackets from a message id.
pub fn clean_msgid(s: &str) -> String {
    s.trim().trim_start_matches('<').trim_end_matches('>').to_string()
}

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} bytes")
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", n as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let raw = b"From: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: =?UTF-8?Q?Hi_there?=\r\nMessage-ID: <abc@x>\r\nX-Priority: 1\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\n\r\nHello Bob\r\n> quoted\r\nBye\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(p.subject, "Hi there");
        assert_eq!(p.from.email, "alice@example.com");
        assert_eq!(p.from.name, "Alice");
        assert_eq!(p.message_id, "abc@x");
        assert_eq!(p.priority, Priority::High);
        let s = summarize(&p, raw.len());
        assert_eq!(s.snippet, "Hello Bob Bye");
    }

    #[test]
    fn html_only_message_has_no_plain_part() {
        let raw = b"From: a@b.c\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n<!DOCTYPE html><html><body><p>Hello <b>there</b></p></body></html>\r\n";
        let p = parse(raw).unwrap();
        assert!(p.text.is_none());
        assert_eq!(p.plain_text(), "Hello there");
        assert_eq!(summarize(&p, raw.len()).snippet, "Hello there");
    }

    #[test]
    fn html_text() {
        let t = html_to_text("<html><head><style>p{}</style></head><body><p>Hello&nbsp;<b>World</b></p><ul><li>One</li><li>Two</li></ul></body></html>");
        assert!(t.contains("Hello"));
        assert!(t.contains("• One"));
    }
}
