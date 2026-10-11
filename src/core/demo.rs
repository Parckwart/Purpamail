//! Sample data for trying Purpamail without a mail account.
//!
//! [`populate`] fills the "On This Computer" mailboxes with about three weeks
//! of believable mail between fictional people: multi-message conversations
//! that span Inbox and Sent, HTML newsletters, receipts, a shipping notice, a
//! promotion, a calendar invitation, messages with small generated
//! attachments, flagged and unread mail, drafts, junk, and a few custom
//! mailboxes. Every message is built as real RFC 5322 source and goes through
//! the normal MIME parser, so the sample data exercises the same code paths as
//! real mail.

use anyhow::Result;
use chrono::{Duration, Local, TimeZone};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as BAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::raw::Raw;
use mail_builder::mime::MimePart;

use super::categorize;
use super::mime;
use super::model::{FlagColor, LOCAL_ACCOUNT, Mailbox, MailboxId, MailboxRole, MessageId, flags, now_ts};
use super::ops;
use super::store::{Contact, Store};

/// A fictional correspondent.
#[derive(Clone, Copy, Debug)]
struct P {
    name: &'static str,
    email: &'static str,
}

const ME: P = P { name: "Alex Morgan", email: "me@example.org" };
const PRIYA: P = P { name: "Priya Natarajan", email: "priya@northwind-studio.example" };
const JONAS: P = P { name: "Jonas Weber", email: "jonas.weber@example.net" };
const LENA: P = P { name: "Lena Morgan", email: "lena@morgan-family.example" };
const MARCUS: P = P { name: "Marcus Chen", email: "marcus@brightleaf.example" };
const HANNAH: P = P { name: "Hannah Okafor", email: "hannah@brightleaf.example" };
const SOFIA: P = P { name: "Sofia Alvarez", email: "sofia.alvarez@example.com" };
const TOM: P = P { name: "Tom Becker", email: "tom@riverside-cycling.example" };
const RUTH: P = P { name: "Ruth Lindqvist", email: "ruth.lindqvist@example.net" };
const CORNER_BOOKS: P = P { name: "Corner Books", email: "orders@cornerbooks.example" };
const PARCEL: P = P { name: "Parcel Express", email: "no-reply@parcel-express.example" };
const FERN: P = P { name: "Fern & Co", email: "news@fernandco.example" };
const TRAILHEAD: P = P { name: "Trailhead Outdoor", email: "hello@trailhead-outdoor.example" };
const OSW: P = P { name: "Open Source Weekly", email: "newsletter@opensourceweekly.example" };
const LIBRARY: P = P { name: "City Library", email: "notifications@citylibrary.example" };
const CLOUD: P = P { name: "Nimbus Cloud", email: "no-reply@nimbus-cloud.example" };
const ENERGY: P = P { name: "Greenfield Energy", email: "billing@greenfield-energy.example" };
const RAIL: P = P { name: "Coastline Rail", email: "tickets@coastline-rail.example" };
const BANK: P = P { name: "Harbor Bank", email: "no-reply@harborbank.example" };
const PIXEL: P = P { name: "Pixel & Paper Print Shop", email: "billing@pixelandpaper.example" };
const PODCAST: P = P { name: "The Long Walk Podcast", email: "episodes@longwalk.example" };
const CONF: P = P { name: "Open Web Days", email: "program@openwebdays.example" };

/// Unix time `days` days ago at the given local wall-clock time (never in the future).
fn at(days: i64, hour: u32, minute: u32) -> i64 {
    let date = Local::now().date_naive() - Duration::days(days);
    let ts = date
        .and_hms_opt(hour, minute, 0)
        .and_then(|d| Local.from_local_datetime(&d).earliest())
        .map(|d| d.timestamp())
        .unwrap_or_else(|| now_ts() - days * 86400);
    let now = now_ts();
    if ts > now { now - 600 - (minute as i64 * 7) } else { ts }
}

fn domain(email: &str) -> &str {
    email.rsplit_once('@').map(|(_, d)| d).unwrap_or("example.org")
}

/// A message under construction.
#[derive(Clone)]
struct M {
    from: P,
    to: Vec<P>,
    cc: Vec<P>,
    subject: String,
    date: i64,
    id: String,
    in_reply_to: Option<String>,
    refs: Vec<String>,
    text: String,
    html: Option<String>,
    attachments: Vec<(String, String, Vec<u8>)>,
    headers: Vec<(String, String)>,
    calendar: Option<String>,
}

impl M {
    fn new(slug: &str, from: P, subject: &str, date: i64) -> M {
        M {
            from,
            to: vec![ME],
            cc: Vec::new(),
            subject: subject.to_string(),
            date,
            id: format!("{slug}.{}@{}", date, domain(from.email)),
            in_reply_to: None,
            refs: Vec::new(),
            text: String::new(),
            html: None,
            attachments: Vec::new(),
            headers: Vec::new(),
            calendar: None,
        }
    }
    fn to(mut self, to: &[P]) -> M {
        self.to = to.to_vec();
        self
    }
    fn cc(mut self, cc: &[P]) -> M {
        self.cc = cc.to_vec();
        self
    }
    fn text(mut self, t: &str) -> M {
        self.text = t.to_string();
        self
    }
    fn html(mut self, h: String) -> M {
        self.html = Some(h);
        self
    }
    fn reply_to(mut self, parent: &M) -> M {
        self.in_reply_to = Some(parent.id.clone());
        self.refs = parent.refs.clone();
        self.refs.push(parent.id.clone());
        self
    }
    fn attach(mut self, name: &str, mime_type: &str, data: Vec<u8>) -> M {
        self.attachments.push((name.to_string(), mime_type.to_string(), data));
        self
    }
    fn header(mut self, k: &str, v: &str) -> M {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    fn calendar(mut self, ics: String) -> M {
        self.calendar = Some(ics);
        self
    }

    /// Serialize to RFC 5322 bytes.
    fn build(&self) -> Vec<u8> {
        let addr = |p: &P| BAddress::new_address(Some(p.name.to_string()), p.email.to_string());
        let list = |v: &[P]| BAddress::new_list(v.iter().map(addr).collect());
        let mut b = MessageBuilder::new().from(addr(&self.from)).subject(self.subject.clone()).date(self.date).message_id(self.id.clone());
        if !self.to.is_empty() {
            b = b.to(list(&self.to));
        }
        if !self.cc.is_empty() {
            b = b.cc(list(&self.cc));
        }
        if let Some(irt) = &self.in_reply_to {
            b = b.in_reply_to(irt.clone());
        }
        if !self.refs.is_empty() {
            b = b.header("References", Raw::new(self.refs.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" ")));
        }
        for (k, v) in &self.headers {
            b = b.header(k.clone(), Raw::new(v.clone()));
        }
        if let Some(ics) = &self.calendar {
            let text = MimePart::new("text/plain", self.text.clone());
            let cal = MimePart::new(ContentType::new("text/calendar").attribute("charset", "utf-8").attribute("method", "REQUEST"), ics.clone());
            let invite = MimePart::new("application/ics", ics.as_bytes().to_vec()).attachment("invite.ics");
            let alt = MimePart::new("multipart/alternative", vec![text, cal]);
            b = b.body(MimePart::new("multipart/mixed", vec![alt, invite]));
        } else {
            b = b.text_body(self.text.clone());
            if let Some(h) = &self.html {
                b = b.html_body(h.clone());
            }
            for (name, mt, data) in &self.attachments {
                b = b.attachment(mt.clone(), name.clone(), data.clone());
            }
        }
        b.write_to_vec().unwrap_or_default()
    }
}

/// Mailboxes used by the sample data.
struct Boxes {
    inbox: Mailbox,
    sent: Mailbox,
    drafts: Mailbox,
    archive: Mailbox,
    junk: Mailbox,
    trash: Mailbox,
    projects: MailboxId,
    website: MailboxId,
    receipts: MailboxId,
}

fn custom_mailbox(store: &Store, path: &str, parent: Option<MailboxId>) -> Result<MailboxId> {
    let id = store.ensure_mailbox(LOCAL_ACCOUNT, path, MailboxRole::Custom)?;
    if parent.is_some()
        && let Some(mut mb) = store.mailbox(id)
    {
        mb.parent = parent;
        store.upsert_mailbox(&mb)?;
    }
    Ok(id)
}

/// Store a message: parse it, categorize it and insert it with the given flags and color.
fn put(store: &Store, mailbox: MailboxId, m: &M, flag_bits: u32, color: Option<FlagColor>) -> Result<MessageId> {
    let raw = m.build();
    let p = mime::parse(&raw).ok_or_else(|| anyhow::anyhow!("sample message could not be parsed: {}", m.subject))?;
    let mut s = mime::summarize(&p, raw.len());
    s.mailbox_id = mailbox;
    s.account_id = LOCAL_ACCOUNT.to_string();
    s.flags = flag_bits | flags::HAS_BODY | if color.is_some() { flags::FLAGGED } else { 0 };
    s.color = color;
    s.category = categorize::categorize(&p);
    s.received = m.date;
    store.insert_message(&s, Some(&raw), &p.plain_text())
}

// ------------------------------------------------------------------ tiny generated attachments

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Encode an RGB image as a PNG using stored (uncompressed) deflate blocks.
fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for y in 0..height {
        raw.push(0); // filter: none
        for x in 0..width {
            raw.extend_from_slice(&pixel(x, y));
        }
    }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        z.push(u8::from(i + 1 == chunks.len()));
        let len = c.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

fn trail_map_png() -> Vec<u8> {
    png(32, 24, |x, y| {
        let on_trail = (y as i32 - (12.0 + 6.0 * ((x as f32) / 5.0).sin()) as i32).abs() <= 1;
        if on_trail { [214, 72, 52] } else { [(90 + x * 2) as u8, (150 + y * 3) as u8, 90] }
    })
}

fn logo_png(base: [u8; 3]) -> Vec<u8> {
    png(24, 24, move |x, y| {
        let dx = x as i32 - 12;
        let dy = y as i32 - 12;
        if dx * dx + dy * dy <= 81 { base } else { [250, 250, 248] }
    })
}

fn brief_pdf() -> Vec<u8> {
    use super::pdf::PdfLine;
    super::pdf::render(
        "Brightleaf website relaunch — project brief",
        &[
            PdfLine::new("Brightleaf website relaunch").bold().size(18.0),
            PdfLine::new("Project brief, version 2").gray(0.45),
            PdfLine::new(""),
            PdfLine::new("Goals").bold().size(13.0),
            PdfLine::new("• A faster, accessible site that works well on phones.").indent(12.0),
            PdfLine::new("• Clearer product pages and a simple contact flow.").indent(12.0),
            PdfLine::new("• An editorial blog the team can update without help.").indent(12.0),
            PdfLine::new(""),
            PdfLine::new("Timeline").bold().size(13.0),
            PdfLine::new("Discovery: two weeks. Design: three weeks. Build and content: four weeks. Launch: early next quarter."),
        ],
    )
}

fn invitation_ics(start: i64) -> String {
    let fmt = |ts: i64| chrono::DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y%m%dT%H%M%SZ").to_string()).unwrap_or_default();
    let lines = [
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Northwind Studio//Team Calendar//EN".into(),
        "METHOD:REQUEST".into(),
        "BEGIN:VEVENT".into(),
        "UID:design-review-logo@northwind-studio.example".into(),
        format!("DTSTAMP:{}", fmt(now_ts() - 86400)),
        format!("DTSTART:{}", fmt(start)),
        format!("DTEND:{}", fmt(start + 3600)),
        "SUMMARY:Design review: Brightleaf logo".into(),
        "LOCATION:Northwind Studio\\, Room 2 / video call".into(),
        "DESCRIPTION:Let's pick the final logo direction together.\\nPlease bring your notes.".into(),
        format!("ORGANIZER;CN={}:mailto:{}", PRIYA.name, PRIYA.email),
        format!("ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN={}:mailto:{}", ME.name, ME.email),
        format!("ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN={}:mailto:{}", MARCUS.name, MARCUS.email),
        "SEQUENCE:0".into(),
        "STATUS:CONFIRMED".into(),
        "END:VEVENT".into(),
        "END:VCALENDAR".into(),
    ];
    lines.iter().map(|l| super::ics::fold_line(l)).collect()
}

// ------------------------------------------------------------------ HTML templates

fn newsletter_html(title: &str, accent: &str, intro: &str, items: &[(&str, &str, &str)], footer: &str) -> String {
    let mut s = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
<body style=\"margin:0;padding:0;background:#f4f4f2;font-family:Helvetica,Arial,sans-serif;color:#222;\">\
<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"background:#f4f4f2;\"><tr><td align=\"center\" style=\"padding:24px 12px;\">\
<table role=\"presentation\" width=\"600\" cellpadding=\"0\" cellspacing=\"0\" style=\"max-width:600px;background:#ffffff;border-radius:10px;overflow:hidden;\">\
<tr><td style=\"background:{accent};padding:28px 32px;color:#ffffff;\"><h1 style=\"margin:0;font-size:26px;font-weight:700;\">{title}</h1></td></tr>\
<tr><td style=\"padding:24px 32px 8px 32px;font-size:15px;line-height:1.55;\"><p style=\"margin:0 0 12px 0;\">{intro}</p></td></tr>"
    );
    for (heading, body, link) in items {
        s.push_str(&format!(
            "<tr><td style=\"padding:12px 32px;border-top:1px solid #eeeeee;\"><h2 style=\"margin:0 0 6px 0;font-size:18px;color:{accent};\">{heading}</h2>\
<p style=\"margin:0 0 8px 0;font-size:15px;line-height:1.5;\">{body}</p>\
<a href=\"{link}\" style=\"color:{accent};font-weight:600;text-decoration:none;\">Read more &rarr;</a></td></tr>"
        ));
    }
    s.push_str(&format!(
        "<tr><td style=\"padding:20px 32px;background:#fafafa;font-size:12px;color:#888888;line-height:1.5;\">{footer}</td></tr></table></td></tr></table></body></html>"
    ));
    s
}

fn promo_html() -> String {
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head>\
<body style=\"margin:0;background:#fbf7f0;font-family:Georgia,'Times New Roman',serif;color:#2b2b2b;\">\
<div style=\"max-width:560px;margin:0 auto;padding:28px;\">\
<p style=\"text-align:center;font-size:13px;letter-spacing:2px;color:#7a6a55;margin:0 0 18px 0;\">FERN &amp; CO · HOME &amp; GARDEN</p>\
<div style=\"background:#2f5d50;color:#fff;border-radius:12px;padding:36px 28px;text-align:center;\">\
<p style=\"margin:0;font-size:15px;\">This weekend only</p>\
<h1 style=\"margin:8px 0 12px 0;font-size:44px;\">30% off everything</h1>\
<p style=\"margin:0 0 22px 0;font-size:16px;\">Planters, linen, ceramics and our new autumn candles.</p>\
<a href=\"https://fernandco.example/sale?code=AUTUMN30\" style=\"display:inline-block;background:#f2c14e;color:#2b2b2b;padding:12px 26px;border-radius:24px;text-decoration:none;font-weight:bold;\">Shop the sale</a></div>\
<p style=\"font-size:14px;line-height:1.6;margin:22px 0;\">Use promo code <b>AUTUMN30</b> at checkout. Free shipping on orders over €50. Offer ends Sunday at midnight.</p>\
<p style=\"font-size:11px;color:#999;text-align:center;\">You receive this email because you signed up at fernandco.example. \
<a href=\"https://fernandco.example/unsubscribe\" style=\"color:#999;\">Unsubscribe</a></p></div></body></html>"
        .to_string()
}

fn receipt_html() -> String {
    "<!DOCTYPE html><html><body style=\"font-family:Helvetica,Arial,sans-serif;color:#333;background:#ffffff;\">\
<div style=\"max-width:520px;margin:0 auto;padding:24px;border:1px solid #e5e5e5;border-radius:8px;\">\
<h2 style=\"margin-top:0;color:#8a3b12;\">Corner Books</h2><p>Thanks for your order! Here is your receipt.</p>\
<table width=\"100%\" style=\"border-collapse:collapse;font-size:14px;\">\
<tr><td style=\"padding:6px 0;border-bottom:1px solid #eee;\">The Overstory (paperback)</td><td align=\"right\" style=\"border-bottom:1px solid #eee;\">€14.00</td></tr>\
<tr><td style=\"padding:6px 0;border-bottom:1px solid #eee;\">A Field Guide to Getting Lost</td><td align=\"right\" style=\"border-bottom:1px solid #eee;\">€12.50</td></tr>\
<tr><td style=\"padding:6px 0;\">Shipping</td><td align=\"right\">€0.00</td></tr>\
<tr><td style=\"padding:6px 0;font-weight:bold;\">Total</td><td align=\"right\" style=\"font-weight:bold;\">€26.50</td></tr></table>\
<p style=\"font-size:13px;color:#777;\">Order number CB-48213 · Paid with card ending 4417</p></div></body></html>"
        .to_string()
}

// ------------------------------------------------------------------ populate

/// Populate the "On This Computer" mailboxes with sample conversations.
///
/// Does nothing if the local Inbox already contains messages.
pub fn populate(store: &Store) -> Result<()> {
    if store.mailbox_by_role(LOCAL_ACCOUNT, MailboxRole::Inbox).is_some_and(|mb| mb.total > 0) {
        return Ok(());
    }
    let b = Boxes {
        inbox: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Inbox)?,
        sent: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Sent)?,
        drafts: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Drafts)?,
        archive: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Archive)?,
        junk: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Junk)?,
        trash: ops::role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Trash)?,
        projects: custom_mailbox(store, "Projects", None)?,
        website: 0,
        receipts: custom_mailbox(store, "Receipts", None)?,
    };
    let b = Boxes { website: custom_mailbox(store, "Projects/Website", Some(b.projects))?, ..b };
    add_contacts(store)?;
    conversations(store, &b)?;
    singles(store, &b)?;
    other_mailboxes(store, &b)?;
    Ok(())
}

fn add_contacts(store: &Store) -> Result<()> {
    let contacts = [
        Contact {
            first_name: "Priya".into(),
            last_name: "Natarajan".into(),
            organization: "Northwind Studio".into(),
            emails: vec![PRIYA.email.into()],
            phones: vec!["+44 20 7946 0321".into()],
            notes: "Design lead for the Brightleaf relaunch.".into(),
            vip: true,
            ..Default::default()
        },
        Contact {
            first_name: "Jonas".into(),
            last_name: "Weber".into(),
            emails: vec![JONAS.email.into()],
            phones: vec!["+49 151 23456789".into()],
            notes: "Hiking buddy.".into(),
            ..Default::default()
        },
        Contact {
            first_name: "Lena".into(),
            last_name: "Morgan".into(),
            nickname: "Lenny".into(),
            emails: vec![LENA.email.into()],
            phones: vec!["+1 555 201 7788".into()],
            notes: "Sister.".into(),
            ..Default::default()
        },
        Contact {
            first_name: "Marcus".into(),
            last_name: "Chen".into(),
            organization: "Brightleaf".into(),
            emails: vec![MARCUS.email.into()],
            ..Default::default()
        },
        Contact {
            first_name: "Hannah".into(),
            last_name: "Okafor".into(),
            organization: "Brightleaf".into(),
            emails: vec![HANNAH.email.into()],
            ..Default::default()
        },
        Contact {
            first_name: "Sofia".into(),
            last_name: "Alvarez".into(),
            emails: vec![SOFIA.email.into()],
            notes: "Book club host.".into(),
            ..Default::default()
        },
    ];
    for c in &contacts {
        if store.contact_by_email(&c.emails[0]).is_none() {
            store.save_contact(c)?;
        }
    }
    for p in [JONAS, LENA, MARCUS, HANNAH, SOFIA, TOM, PRIYA] {
        store.record_recipient(&super::model::Address::new(p.name, p.email))?;
    }
    Ok(())
}

const SEEN: u32 = flags::SEEN;
const UNREAD: u32 = 0;

fn conversations(store: &Store, b: &Boxes) -> Result<()> {
    let inbox = b.inbox.id;
    let sent = b.sent.id;

    // --- Website relaunch (client project): older part filed in Projects/Website.
    let kickoff = M::new("relaunch-kickoff", MARCUS, "Brightleaf website relaunch — kickoff", at(19, 9, 12)).to(&[ME]).cc(&[HANNAH]).text(
        "Hi Alex,\n\nGreat talking to you last week! As promised, here is a short summary of what we would like to achieve with the relaunch:\n\n\
- a faster site that works well on phones\n- clearer product pages\n- a blog our team can update without help\n\n\
Could we do a kickoff call next Tuesday or Wednesday? Hannah will join from our side.\n\nBest,\nMarcus\n-- \nMarcus Chen\nHead of Marketing, Brightleaf",
    );
    put(store, b.website, &kickoff, SEEN | flags::ANSWERED, None)?;
    let kickoff_re =
        M::new("relaunch-kickoff-re", ME, "Re: Brightleaf website relaunch — kickoff", at(19, 11, 40)).to(&[MARCUS]).cc(&[HANNAH]).reply_to(&kickoff).text(
            "Hi Marcus,\n\nThanks for the summary, that sounds like a great project. Wednesday at 10:00 works for me — I'll send an invite.\n\nBest,\nAlex\n\n\
On Monday, Marcus Chen wrote:\n> Could we do a kickoff call next Tuesday or Wednesday?",
        );
    put(store, sent, &kickoff_re, SEEN, None)?;
    let hannah1 = M::new("relaunch-hannah", HANNAH, "Re: Brightleaf website relaunch — kickoff", at(18, 8, 55))
        .to(&[ME, MARCUS])
        .reply_to(&kickoff_re)
        .text("Hello both,\n\nWednesday at 10 is perfect. I'll bring our analytics numbers and the list of pages we can retire.\n\nHannah");
    put(store, b.website, &hannah1, SEEN, None)?;
    let brief = M::new("relaunch-brief", MARCUS, "Re: Brightleaf website relaunch — kickoff", at(15, 16, 20))
        .to(&[ME])
        .cc(&[HANNAH])
        .reply_to(&hannah1)
        .text("Hi Alex,\n\nThanks for a productive kickoff! Attached is the updated project brief with the goals and the rough timeline we discussed.\n\nLet me know if anything is missing.\n\nMarcus")
        .attach("project-brief.pdf", "application/pdf", brief_pdf());
    put(store, inbox, &brief, SEEN, None)?;
    let brief_re = M::new("relaunch-brief-re", ME, "Re: Brightleaf website relaunch — kickoff", at(14, 9, 5))
        .to(&[MARCUS])
        .cc(&[HANNAH])
        .reply_to(&brief)
        .text("Thanks Marcus, the brief looks complete. I'll share first wireframes by the end of next week.\n\nAlex");
    put(store, sent, &brief_re, SEEN, None)?;
    let timeline = M::new("relaunch-timeline", MARCUS, "Re: Brightleaf website relaunch — kickoff", at(2, 10, 30)).to(&[ME]).cc(&[HANNAH]).reply_to(&brief_re).text(
        "Hi Alex,\n\nQuick update: our board meeting moved, so we would love to see the wireframes on Thursday instead of Friday. Would that still work for you?\n\nThanks,\nMarcus",
    );
    put(store, inbox, &timeline, UNREAD, None)?;
    let hannah2 = M::new("relaunch-hannah-2", HANNAH, "Re: Brightleaf website relaunch — kickoff", at(1, 16, 45)).to(&[ME, MARCUS]).reply_to(&timeline).text(
        "Thursday works on our side too. I booked the big meeting room from 14:00 to 15:30 and will set up a video link for anyone working from home.\n\nHannah",
    );
    put(store, inbox, &hannah2, UNREAD, None)?;
    let contract = M::new("relaunch-contract", MARCUS, "Signed contract", at(17, 15, 10))
        .text("Hi Alex,\n\nGood news: the contract is signed and the purchase order is on its way to you. Looking forward to working together!\n\nMarcus");
    put(store, inbox, &contract, SEEN, None)?;

    // --- Weekend hike with Jonas and Tom.
    let hike = M::new("hike", JONAS, "Weekend hike?", at(9, 18, 2)).text(
        "Hey Alex,\n\nThe weather looks fantastic for Saturday. Fancy the ridge trail again? Start at 8:30 at the station, back by late afternoon.\n\nJonas",
    );
    put(store, inbox, &hike, SEEN | flags::ANSWERED, None)?;
    let hike_re =
        M::new("hike-re", ME, "Re: Weekend hike?", at(9, 20, 15)).to(&[JONAS]).cc(&[TOM]).reply_to(&hike).text(
            "Count me in! I'm adding Tom, he wanted to join next time.\n\nAlex\n\n> The weather looks fantastic for Saturday. Fancy the ridge trail again?",
        );
    put(store, sent, &hike_re, SEEN, None)?;
    let hike_map = M::new("hike-map", JONAS, "Re: Weekend hike?", at(8, 7, 48))
        .to(&[ME, TOM])
        .reply_to(&hike_re)
        .text(
            "Great! Here's the route I have in mind — about 16 km with one steep bit after the hut. Bring water, there's no refill until the summit.\n\nJonas",
        )
        .attach("trail-map.png", "image/png", trail_map_png());
    put(store, inbox, &hike_map, SEEN, Some(FlagColor::Green))?;
    let hike_tom = M::new("hike-tom", TOM, "Re: Weekend hike?", at(8, 12, 3))
        .to(&[JONAS, ME])
        .reply_to(&hike_map)
        .text("Sounds great, thanks for including me! I'll bring my camera and some snacks. See you at the station.\n\nTom");
    put(store, inbox, &hike_tom, SEEN, None)?;
    let hike_photos = M::new("hike-photos", TOM, "Re: Weekend hike?", at(5, 20, 12))
        .to(&[JONAS, ME])
        .reply_to(&hike_tom)
        .text("What a day! Here's my favourite shot from the summit — the light over the valley was unreal. Same time next month?\n\nTom")
        .attach("summit.png", "image/png", png(32, 20, |x, y| if y < 9 { [(120 + x * 3) as u8, (170 + y * 4) as u8, 230] } else { [70, (110 + x) as u8, 60] }));
    put(store, inbox, &hike_photos, SEEN, None)?;

    // --- Family: birthday dinner.
    let bday = M::new("bday", LENA, "Mom's birthday dinner", at(6, 19, 30)).text(
        "Hi Alex,\n\nMom turns 65 on the 24th! I reserved a table at Olive & Thyme for Saturday at 7pm. Could you organize the cake? She loves lemon.\n\nLove,\nLena",
    );
    put(store, inbox, &bday, SEEN | flags::ANSWERED, Some(FlagColor::Orange))?;
    let bday_re = M::new("bday-re", ME, "Re: Mom's birthday dinner", at(6, 21, 10))
        .to(&[LENA])
        .reply_to(&bday)
        .text("Lovely idea! I'll order a lemon cake from the bakery on Elm Street. Should we also get her the photo book we talked about?\n\nAlex");
    put(store, sent, &bday_re, SEEN, None)?;
    let bday2 = M::new("bday-2", LENA, "Re: Mom's birthday dinner", at(1, 8, 15))
        .to(&[ME])
        .reply_to(&bday_re)
        .text("Yes, the photo book! I uploaded the pictures to our shared album. Can you pick it up on Friday? I'll pay you back.\n\nLena");
    put(store, inbox, &bday2, UNREAD, None)?;

    // --- Design review with Priya (VIP), high priority, attachments.
    let logos = M::new("logos", PRIYA, "Logo concepts for Brightleaf", at(4, 14, 5))
        .text("Hi Alex,\n\nAttached are the two logo directions we discussed: a warm leaf mark and a bolder geometric version. I lean towards the first one, but I'd love your opinion before we show them to Marcus.\n\nPriya")
        .attach("concept-leaf.png", "image/png", logo_png([60, 140, 90]))
        .attach("concept-geometric.png", "image/png", logo_png([40, 90, 170]));
    put(store, inbox, &logos, SEEN | flags::ANSWERED, Some(FlagColor::Red))?;
    let logos_re = M::new("logos-re", ME, "Re: Logo concepts for Brightleaf", at(4, 16, 40))
        .to(&[PRIYA])
        .reply_to(&logos)
        .text("Hi Priya,\n\nBoth are strong! I agree the leaf mark feels more like Brightleaf. Maybe try it with a slightly heavier wordmark?\n\nAlex");
    put(store, sent, &logos_re, SEEN, None)?;
    let logos_final = M::new("logos-final", PRIYA, "Re: Logo concepts for Brightleaf", at(0, 9, 20))
        .reply_to(&logos_re)
        .header("X-Priority", "1 (Highest)")
        .header("Importance", "High")
        .text("Alex — I need your final pick by noon today, the printer needs the files for the business cards this afternoon. Heavier wordmark is in the shared folder.\n\nThanks!\nPriya");
    put(store, inbox, &logos_final, UNREAD, None)?;

    // --- Book club.
    let book = M::new("bookclub", SOFIA, "Book club: next pick", at(12, 20, 0)).to(&[ME, LENA, JONAS]).text(
        "Hi everyone,\n\nThanks for a great evening! For next month I suggest either \"The Overstory\" or \"Piranesi\". Reply with your vote by Sunday.\n\nSofia",
    );
    put(store, inbox, &book, SEEN | flags::ANSWERED, None)?;
    let book_re = M::new("bookclub-re", ME, "Re: Book club: next pick", at(11, 8, 30))
        .to(&[SOFIA])
        .cc(&[LENA, JONAS])
        .reply_to(&book)
        .text("My vote goes to The Overstory — I just ordered it anyway.\n\nAlex");
    put(store, sent, &book_re, SEEN, None)?;
    let book_lena = M::new("bookclub-lena", LENA, "Re: Book club: next pick", at(11, 12, 45))
        .to(&[SOFIA, ME, JONAS])
        .reply_to(&book_re)
        .text("Piranesi for me, it's shorter and I'm busy this month! :)");
    put(store, inbox, &book_lena, SEEN, None)?;
    let book_final = M::new("bookclub-final", SOFIA, "Re: Book club: next pick", at(7, 9, 10))
        .to(&[ME, LENA, JONAS])
        .reply_to(&book_lena)
        .text("The votes are in: The Overstory wins 3 to 1. We meet on the 28th at my place, 7:30pm. I'll make soup!\n\nSofia");
    put(store, inbox, &book_final, SEEN, None)?;

    // --- Quarterly report with Hannah.
    let report = M::new("report", HANNAH, "Quarterly report draft", at(5, 11, 0))
        .text("Hi Alex,\n\nCould you review the traffic section of our quarterly report? My notes are attached. The numbers for mobile visits look surprisingly good.\n\nHannah")
        .attach("report-notes.txt", "text/plain", b"Quarterly report notes\n\n- Mobile visits up 18% compared to last quarter\n- Bounce rate down on product pages\n- Newsletter sign-ups: 412\n- TODO: chart for returning visitors\n".to_vec());
    put(store, inbox, &report, SEEN | flags::ANSWERED, Some(FlagColor::Yellow))?;
    let report_re = M::new("report-re", ME, "Re: Quarterly report draft", at(5, 15, 30)).to(&[HANNAH]).reply_to(&report).text(
        "Looks good! One suggestion: compare the mobile numbers with the same quarter last year, the seasonal effect is strong. I'll send the chart tomorrow.\n\nAlex",
    );
    put(store, sent, &report_re, SEEN, None)?;
    let report2 = M::new("report-2", HANNAH, "Re: Quarterly report draft", at(3, 9, 40))
        .to(&[ME])
        .reply_to(&report_re)
        .text("Great point, I added the year-over-year comparison. Thanks for the chart!\n\nHannah");
    put(store, inbox, &report2, SEEN, None)?;

    // --- Neighbor: community garden.
    let garden = M::new("garden", RUTH, "Community garden: plot 14", at(10, 17, 25)).text(
        "Hello Alex,\n\nThe garden committee asked me to let you know that plot 14 is available from next spring if you are still interested. The yearly fee is 40 euros.\n\nBest wishes,\nRuth Lindqvist",
    );
    put(store, inbox, &garden, SEEN | flags::ANSWERED, None)?;
    let garden_re = M::new("garden-re", ME, "Re: Community garden: plot 14", at(10, 19, 2))
        .to(&[RUTH])
        .reply_to(&garden)
        .text("Hi Ruth,\n\nYes, I'd love to take it! How do I pay the fee?\n\nAlex");
    put(store, sent, &garden_re, SEEN, None)?;
    let garden2 = M::new("garden-2", RUTH, "Re: Community garden: plot 14", at(0, 7, 55))
        .to(&[ME])
        .reply_to(&garden_re)
        .text("Wonderful! You can pay at the next members' meeting on Thursday at 6pm in the shed, or by bank transfer. I'll bring the key.\n\nRuth");
    put(store, inbox, &garden2, UNREAD, None)?;
    Ok(())
}

fn singles(store: &Store, b: &Boxes) -> Result<()> {
    let inbox = b.inbox.id;
    // Calendar invitation (meeting in three days at 14:00 local time).
    let start = at(-3, 14, 0).max(now_ts() + 86400);
    let invite = M::new("invite-design-review", PRIYA, "Invitation: Design review: Brightleaf logo", at(1, 15, 30))
        .to(&[ME, MARCUS])
        .text("Priya Natarajan has invited you to: Design review: Brightleaf logo\n\nLocation: Northwind Studio, Room 2 / video call\n\nLet's pick the final logo direction together. Please bring your notes.")
        .calendar(invitation_ics(start));
    put(store, inbox, &invite, UNREAD, Some(FlagColor::Purple))?;

    let receipt = M::new("receipt", CORNER_BOOKS, "Your Corner Books receipt for order CB-48213", at(11, 9, 2))
        .text("Thanks for your order!\n\nThe Overstory (paperback) — €14.00\nA Field Guide to Getting Lost — €12.50\nShipping — €0.00\nTotal — €26.50\n\nOrder number CB-48213. Paid with card ending 4417.")
        .html(receipt_html());
    put(store, inbox, &receipt, SEEN, Some(FlagColor::Blue))?;

    let shipping = M::new("shipping", PARCEL, "Your package is on its way", at(9, 13, 37)).text(
        "Good news! Your package from Corner Books has shipped.\n\nTracking number: PX 4471 2290 18\nEstimated delivery: Thursday, between 10:00 and 14:00.\n\nTrack your package: https://parcel-express.example/track/PX4471229018\n\nThis is an automated message, please do not reply.",
    );
    put(store, inbox, &shipping, SEEN, None)?;

    let promo = M::new("promo", FERN, "30% off everything — this weekend only", at(3, 7, 0))
        .header("List-Unsubscribe", "<https://fernandco.example/unsubscribe>, <mailto:unsubscribe@fernandco.example>")
        .header("List-Unsubscribe-Post", "List-Unsubscribe=One-Click")
        .header("Precedence", "bulk")
        .text("This weekend only: 30% off everything at Fern & Co. Use promo code AUTUMN30 at checkout. Free shipping on orders over €50. Shop the sale: https://fernandco.example/sale")
        .html(promo_html());
    put(store, inbox, &promo, UNREAD, None)?;

    let trail_sale = M::new("trailhead-sale", TRAILHEAD, "Last chance: autumn gear sale ends tonight", at(1, 18, 0))
        .header("List-Unsubscribe", "<https://trailhead-outdoor.example/u/8823>")
        .header("Precedence", "bulk")
        .text("Save up to 40% on jackets, boots and backpacks. Shop now: https://trailhead-outdoor.example/sale")
        .html(newsletter_html(
            "Autumn gear sale",
            "#c0562b",
            "Our autumn sale ends tonight — save up to 40% on jackets, boots and backpacks. Free shipping on all orders.",
            &[
                ("Waterproof shell jacket", "Light, packable and now 35% off. Perfect for windy ridge walks.", "https://trailhead-outdoor.example/p/shell"),
                ("Trail runners", "Grippy soles for muddy paths — limited sizes left.", "https://trailhead-outdoor.example/p/runners"),
            ],
            "Trailhead Outdoor · 12 Harbour Lane · You can <a href=\"https://trailhead-outdoor.example/u/8823\" style=\"color:#888;\">unsubscribe</a> at any time.",
        ));
    put(store, inbox, &trail_sale, UNREAD, None)?;

    let osw = M::new("osw-212", OSW, "Open Source Weekly #212: tiny tools, big wins", at(2, 6, 30))
        .header("List-Id", "Open Source Weekly <weekly.opensourceweekly.example>")
        .header("List-Unsubscribe", "<https://opensourceweekly.example/unsubscribe/abc123>")
        .text("Open Source Weekly #212\n\n1. Writing a PDF file by hand\n2. Why your build is slow\n3. A gentle introduction to CRDTs\n\nRead online: https://opensourceweekly.example/212")
        .html(newsletter_html(
            "Open Source Weekly #212",
            "#3b5bdb",
            "Hello friends! This week: small tools that save hours, a deep dive into build caches and a very readable CRDT explainer.",
            &[
                ("Writing a PDF file by hand", "It's just text, objects and a cross-reference table. A fun weekend read.", "https://opensourceweekly.example/212/pdf"),
                ("Why your build is slow", "Measuring before optimizing: a practical guide to build profiling.", "https://opensourceweekly.example/212/builds"),
                ("A gentle introduction to CRDTs", "How collaborative editors keep everyone in sync without a central lock.", "https://opensourceweekly.example/212/crdt"),
            ],
            "You are receiving Open Source Weekly because you subscribed. <a href=\"https://opensourceweekly.example/unsubscribe/abc123\" style=\"color:#888;\">Unsubscribe</a>",
        ));
    put(store, inbox, &osw, SEEN, None)?;

    let podcast = M::new("podcast", PODCAST, "New episode: Walking the coast path", at(4, 6, 0))
        .header("List-Id", "<episodes.longwalk.example>")
        .header("List-Unsubscribe", "<mailto:leave@longwalk.example>")
        .text("A new episode is out! This week we walk 40 km of coast path and talk about blisters, sea birds and the best sandwiches. Listen: https://longwalk.example/ep/57");
    put(store, inbox, &podcast, SEEN, None)?;

    let library = M::new("library", LIBRARY, "Reminder: 2 items due on Friday", at(1, 9, 0)).text(
        "Dear reader,\n\nThe following items are due on Friday:\n\n- Piranesi\n- The Wild Places\n\nYou can renew them online at https://citylibrary.example/account.\n\nYour City Library",
    );
    put(store, inbox, &library, UNREAD, None)?;

    let security = M::new("security", CLOUD, "New sign-in to your account", at(6, 22, 41)).text(
        "We noticed a new sign-in to your Nimbus Cloud account from a Linux computer in Hamburg.\n\nIf this was you, you can ignore this message. If not, please reset your password at https://nimbus-cloud.example/security.\n\nThe Nimbus Cloud team",
    );
    put(store, inbox, &security, SEEN, None)?;

    let energy = M::new("energy", ENERGY, "Your invoice for September", at(13, 8, 0)).text(
        "Dear customer,\n\nYour invoice for September is ready.\n\nInvoice number: GE-2026-0917\nAmount due: €58.40\nDue date: in 14 days\n\nThe amount will be debited from your account.\n\nGreenfield Energy",
    );
    put(store, inbox, &energy, SEEN, None)?;

    let train = M::new("rail", RAIL, "Booking confirmation and e-ticket", at(7, 19, 12))
        .text("Your booking is confirmed.\n\nBooking reference: CR7Q2K\nOutbound: Saturday 09:12 Central Station → Seaview, seat 42 (coach 5)\nReturn: Sunday 17:48\n\nYour e-ticket is attached. Have a good trip!")
        .attach("e-ticket-CR7Q2K.pdf", "application/pdf", super::pdf::render("E-ticket CR7Q2K", &[super::pdf::PdfLine::new("Coastline Rail — e-ticket").bold().size(16.0), super::pdf::PdfLine::new("Booking reference CR7Q2K · Coach 5, seat 42")]));
    put(store, inbox, &train, SEEN, None)?;

    let bank = M::new("bank", BANK, "Your monthly statement is available", at(8, 6, 15)).text(
        "Your account statement for last month is now available in online banking.\n\nFor your security we never ask for your password by email.\n\nHarbor Bank",
    );
    put(store, inbox, &bank, SEEN, None)?;

    let conf = M::new("conf", CONF, "Your talk has been accepted!", at(5, 17, 50)).text(
        "Dear Alex,\n\nCongratulations — your talk \"Designing calm interfaces\" has been accepted for Open Web Days! You will speak on the second day at 11:00.\n\nPlease confirm by replying to this message within two weeks.\n\nThe program committee",
    );
    put(store, inbox, &conf, UNREAD, None)?;

    let recipe = M::new("recipe", LENA, "The lemon cake recipe", at(2, 20, 5))
        .text("Here's Grandma's lemon cake recipe you asked for, in case the bakery is fully booked. Don't skip the syrup!\n\nLena")
        .attach("lemon-cake.txt", "text/plain", b"Lemon cake\n\n200 g butter, 200 g sugar, 4 eggs, 200 g flour, 2 tsp baking powder, zest of 2 lemons\nSyrup: juice of 2 lemons + 100 g sugar\n\nBake 45 min at 175 C, pour syrup over the warm cake.\n".to_vec());
    put(store, inbox, &recipe, SEEN, None)?;

    let tom_bike = M::new("bike", TOM, "Club ride on Sunday", at(0, 8, 40)).to(&[ME, JONAS]).text(
        "Morning! The cycling club does an easy 50 km ride on Sunday, leaving 9:00 from the bridge café. Want to come along? Coffee stop guaranteed.\n\nTom",
    );
    put(store, inbox, &tom_bike, UNREAD, None)?;

    let offsite = M::new("offsite", HANNAH, "Team offsite: pick a date", at(0, 10, 5)).to(&[ME, MARCUS]).text(
        "Hi both,\n\nWe're planning a small offsite to celebrate the launch. Could you tell me which works better for you: Friday the 14th or Monday the 17th?\n\nHannah",
    );
    put(store, inbox, &offsite, UNREAD, None)?;

    let print = M::new("print-invoice", PIXEL, "Invoice #2026-114", at(16, 10, 0))
        .text("Hello,\n\nThank you for your order of 250 business cards. Please find invoice #2026-114 attached.\n\nTotal: €89.00, payable within 30 days.\n\nPixel & Paper Print Shop")
        .attach("invoice-2026-114.pdf", "application/pdf", super::pdf::render("Invoice 2026-114", &[super::pdf::PdfLine::new("Invoice #2026-114").bold().size(16.0), super::pdf::PdfLine::new("250 business cards — €89.00")]));
    put(store, inbox, &print, SEEN, None)?;

    let storage = M::new("storage", CLOUD, "Your storage is almost full", at(10, 11, 20)).text(
        "You are using 4.6 GB of your 5 GB storage. When your storage is full, new files will no longer sync.\n\nManage your storage: https://nimbus-cloud.example/storage\n\nThe Nimbus Cloud team",
    );
    put(store, inbox, &storage, SEEN, None)?;

    let stories = M::new("trail-stories", TRAILHEAD, "Trail stories: three quiet valleys for October", at(15, 7, 30))
        .header("List-Id", "Trailhead Journal <journal.trailhead-outdoor.example>")
        .header("List-Unsubscribe", "<https://trailhead-outdoor.example/u/8823>")
        .text("Trail stories for October: three quiet valleys, a guide to layering and readers' photos. Read the journal: https://trailhead-outdoor.example/journal")
        .html(newsletter_html(
            "Trail stories",
            "#2f6f4f",
            "The days are getting shorter, the paths quieter. Here are three valleys our readers love in autumn.",
            &[
                ("The Larch Valley loop", "Golden needles, a lake and an easy 9 km loop for a slow Sunday.", "https://trailhead-outdoor.example/journal/larch"),
                ("How to layer for cold mornings", "Base, mid and shell: a simple system that keeps you warm without overheating.", "https://trailhead-outdoor.example/journal/layers"),
            ],
            "Trailhead Journal · <a href=\"https://trailhead-outdoor.example/u/8823\" style=\"color:#888;\">Unsubscribe</a>",
        ));
    put(store, inbox, &stories, SEEN, None)?;

    let climbing = M::new("climbing", JONAS, "Climbing gym on Thursday?", at(14, 12, 30))
        .text("Hi Alex,\n\nThe new bouldering wall opened at the climbing gym. Want to try it on Thursday after work, around 18:30?\n\nJonas");
    put(store, inbox, &climbing, SEEN, None)?;

    let carpool = M::new("carpool", SOFIA, "Carpool to book club", at(3, 18, 20))
        .to(&[ME, LENA])
        .text("Hi you two,\n\nI can pick you up on the way to book club if you like — I'll pass the station at about 7:10pm. Just let me know!\n\nSofia");
    put(store, inbox, &carpool, SEEN, None)?;

    let library_news = M::new("library-news", LIBRARY, "This month at the City Library", at(18, 10, 0))
        .header("List-Id", "<news.citylibrary.example>")
        .header("List-Unsubscribe", "<https://citylibrary.example/newsletter/leave>")
        .text("This month at the City Library: author reading on the 21st, repair café every Saturday, and new opening hours on Sundays (11:00 to 17:00).");
    put(store, inbox, &library_news, SEEN, None)?;
    Ok(())
}

fn other_mailboxes(store: &Store, b: &Boxes) -> Result<()> {
    // Drafts
    let draft = M::new("draft-offsite", ME, "Re: Team offsite: pick a date", at(0, 10, 30))
        .to(&[HANNAH])
        .text("Hi Hannah,\n\nFriday the 14th works best for me, Monday I'm at a workshop until 3pm.\n\n");
    put(store, b.drafts.id, &draft, SEEN | flags::DRAFT, None)?;
    let packing = M::new("draft-packing", ME, "Packing list for the coast trip", at(1, 21, 0))
        .to(&[JONAS])
        .text("- rain jacket\n- headlamp\n- sandwiches\n- train tickets (printed!)\n- ");
    put(store, b.drafts.id, &packing, SEEN | flags::DRAFT, None)?;

    // Archive (older mail)
    let welcome = M::new("welcome", SOFIA, "Welcome to the book club!", at(45, 18, 0))
        .text("Hi Alex,\n\nSo glad you're joining us. We meet once a month, usually at my place. First book: \"Station Eleven\".\n\nSofia");
    put(store, b.archive.id, &welcome, SEEN, None)?;
    let tax = M::new("tax", LENA, "Tax documents from Dad", at(38, 12, 10))
        .text("Dad asked me to forward these to you. He says thanks for helping with the forms!\n\nLena");
    put(store, b.archive.id, &tax, SEEN, None)?;
    let old_osw = M::new("osw-208", OSW, "Open Source Weekly #208", at(30, 6, 30))
        .header("List-Id", "Open Source Weekly <weekly.opensourceweekly.example>")
        .header("List-Unsubscribe", "<https://opensourceweekly.example/unsubscribe/abc123>")
        .text("Open Source Weekly #208: terminals, fonts and the joy of plain text.");
    put(store, b.archive.id, &old_osw, SEEN, None)?;

    // Custom mailboxes
    let proj = M::new("projects-overview", HANNAH, "Projects overview for Q4", at(17, 9, 0))
        .text("Hi Alex,\n\nHere's the list of projects for Q4: website relaunch, newsletter redesign and the photo shoot in November.\n\nHannah");
    put(store, b.projects, &proj, SEEN, None)?;
    let photo = M::new("photo-shoot", MARCUS, "Photo shoot in November", at(16, 14, 30))
        .text("Hi Alex,\n\nWe booked the photographer for the 12th of November. Could you prepare a shot list for the product pages?\n\nMarcus");
    put(store, b.projects, &photo, SEEN, None)?;
    let old_energy = M::new("energy-aug", ENERGY, "Your invoice for August", at(43, 8, 0)).text("Your invoice for August is ready. Amount due: €61.20.");
    put(store, b.receipts, &old_energy, SEEN, None)?;
    let old_books = M::new("receipt-old", CORNER_BOOKS, "Your Corner Books receipt for order CB-47120", at(40, 15, 0))
        .text("Thanks for your order! Station Eleven (paperback) — €11.00. Total: €11.00.");
    put(store, b.receipts, &old_books, SEEN, None)?;

    // Junk
    let spam1 = M::new("spam-lottery", P { name: "International Lottery Board", email: "claims@win-big-prizes.example" }, "CONGRATULATIONS!!! YOU HAVE WON €1,500,000", at(2, 3, 14)).text(
        "Dear beneficiary,\n\nYou have won the international lottery! To claim your prize, send your bank details and a processing fee by wire transfer. Strictly confidential!!!",
    );
    put(store, b.junk.id, &spam1, UNREAD | flags::JUNK, None)?;
    let spam2 = M::new("spam-pills", P { name: "Best Deals", email: "offers@cheap-meds-online.example" }, "Cheap meds, no prescription needed", at(5, 4, 2))
        .text("Lose weight fast with our 100% guaranteed miracle pills! No prescription. Act now: http://203.0.113.7/buy");
    put(store, b.junk.id, &spam2, UNREAD | flags::JUNK, None)?;

    // Trash
    let old_promo = M::new("trash-promo", TRAILHEAD, "Summer clearance: up to 60% off", at(20, 7, 0))
        .header("List-Unsubscribe", "<https://trailhead-outdoor.example/u/8823>")
        .text("Summer clearance sale: up to 60% off tents and sleeping bags. Shop now!");
    put(store, b.trash.id, &old_promo, SEEN, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Category;
    use crate::core::store::MessageQuery;

    #[test]
    fn populates_once_with_realistic_data() {
        let store = Store::open_in_memory().unwrap();
        populate(&store).unwrap();
        let inbox = store.mailbox_by_role(LOCAL_ACCOUNT, MailboxRole::Inbox).unwrap();
        assert!((35..=45).contains(&inbox.total), "inbox has {} messages", inbox.total);
        assert!(inbox.unread >= 8 && inbox.unread < inbox.total);
        for role in [MailboxRole::Sent, MailboxRole::Drafts, MailboxRole::Archive, MailboxRole::Junk, MailboxRole::Trash] {
            assert!(store.mailbox_by_role(LOCAL_ACCOUNT, role).unwrap().total > 0, "{role:?} empty");
        }
        let website = store.mailbox_by_path(LOCAL_ACCOUNT, "Projects/Website").unwrap();
        let projects = store.mailbox_by_path(LOCAL_ACCOUNT, "Projects").unwrap();
        assert_eq!(website.parent, Some(projects.id));
        assert!(website.total >= 2);
        assert!(store.mailbox_by_path(LOCAL_ACCOUNT, "Receipts").unwrap().total >= 2);

        // Idempotent
        populate(&store).unwrap();
        assert_eq!(store.mailbox_by_role(LOCAL_ACCOUNT, MailboxRole::Inbox).unwrap().total, inbox.total);

        let all = store.list_messages(&MessageQuery::default());
        // Conversations span mailboxes (Inbox + Sent + Projects/Website).
        let kickoff_thread = all.iter().find(|m| m.subject.contains("kickoff") && m.mailbox_id == website.id).unwrap().thread_id;
        let in_thread: Vec<_> = all.iter().filter(|m| m.thread_id == kickoff_thread).collect();
        assert!(in_thread.len() >= 5, "thread has {}", in_thread.len());
        let sent = store.mailbox_by_role(LOCAL_ACCOUNT, MailboxRole::Sent).unwrap();
        assert!(in_thread.iter().any(|m| m.mailbox_id == sent.id));
        assert!(in_thread.iter().any(|m| m.mailbox_id == inbox.id));

        // Categories, flags, priority, attachments, calendar, VIP
        let by_subject = |s: &str| all.iter().find(|m| m.subject.contains(s)).unwrap_or_else(|| panic!("missing {s}"));
        assert_eq!(by_subject("Your Corner Books receipt for order CB-48213").category, Category::Transactions);
        assert_eq!(by_subject("Your package is on its way").category, Category::Transactions);
        assert_eq!(by_subject("30% off everything").category, Category::Promotions);
        assert_eq!(by_subject("Open Source Weekly #212").category, Category::Updates);
        assert_eq!(by_subject("Mom's birthday dinner").category, Category::Primary);
        assert_eq!(by_subject("Weekend hike?").category, Category::Primary);
        assert!(all.iter().any(|m| m.subject == "Logo concepts for Brightleaf" && m.has_attachments));
        assert_eq!(all.iter().filter(|m| m.priority == crate::core::model::Priority::High).count(), 1);
        let colors: std::collections::HashSet<_> = all.iter().filter(|m| m.is_flagged()).filter_map(|m| m.color).collect();
        assert!(colors.len() >= 5);
        let invite = by_subject("Invitation: Design review");
        let p = mime::parse(&store.raw(invite.id).unwrap()).unwrap();
        let ev = crate::core::ics::parse(p.calendar.as_deref().unwrap()).unwrap();
        assert_eq!(ev.method, "REQUEST");
        assert!(ev.attendees.contains(&ME.email.to_string()));
        assert!(!p.plain_text().contains("BEGIN:VCALENDAR"));
        assert_eq!(store.vip_emails(), vec![PRIYA.email.to_string()]);
        assert!(store.contacts().len() >= 6);

        // Attachments decode to valid files
        let logo = by_subject("Logo concepts for Brightleaf");
        let p = mime::parse(&store.raw(logo.id).unwrap()).unwrap();
        assert!(p.attachments.iter().all(|a| a.data.starts_with(&[0x89, b'P', b'N', b'G'])));
        let brief = all.iter().find(|m| m.has_attachments && m.from.email == MARCUS.email).unwrap();
        let p = mime::parse(&store.raw(brief.id).unwrap()).unwrap();
        assert!(p.attachments[0].data.starts_with(b"%PDF-1.4"));
        // All dates within the last ~7 weeks and not in the future.
        assert!(all.iter().all(|m| m.date <= now_ts() && m.date > now_ts() - 50 * 86400));
    }

    #[test]
    fn png_encoder_produces_valid_crc() {
        let data = png(2, 2, |x, y| [x as u8 * 255, y as u8 * 255, 0]);
        assert!(data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]));
        assert!(data.ends_with(&[0xae, 0x42, 0x60, 0x82])); // IEND CRC
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }
}
