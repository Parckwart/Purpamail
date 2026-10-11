//! Automatic categorization of incoming mail (Primary / Transactions / Updates / Promotions).
//!
//! The classifier combines header signals (mailing list headers, bulk precedence,
//! automated sender addresses, marketing platform headers) with weighted keyword
//! scoring over the subject and the beginning of the body. Keywords are provided
//! for English and German. Mail that looks like it was written by a person — no
//! list headers, no automated sender, no bulk markers — always stays in Primary.

use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};

use super::mime::ParsedMessage;
use super::model::{Category, subject_is_reply};

/// How many bytes of body text are inspected for keywords.
const BODY_SCAN_LIMIT: usize = 4000;

fn keyword_regex(words: &[&str]) -> Regex {
    let alternatives: Vec<String> = words.iter().map(|w| regex::escape(w).replace(' ', "\\s+")).collect();
    // Keywords starting/ending with a non-word character (e.g. "% off", "order #") cannot use \b there.
    let pattern = format!(r"(?:^|\b|\s)(?:{})(?:\b|\s|$)", alternatives.join("|"));
    RegexBuilder::new(&pattern).case_insensitive(true).build().expect("valid keyword regex")
}

static TRANSACTION: Lazy<Regex> = Lazy::new(|| {
    keyword_regex(&[
        // English
        "receipt", "your receipt", "order confirmation", "order confirmed", "your order", "order number", "order #", "order no",
        "invoice", "your invoice", "payment received", "payment confirmation", "payment confirmed", "payment successful",
        "has shipped", "have shipped", "shipped", "shipping confirmation", "shipment", "dispatched", "tracking number",
        "track your package", "track your order", "out for delivery", "has been delivered", "delivered", "delivery",
        "bank statement", "statement is available", "account statement", "booking confirmation", "booking reference",
        "reservation", "your reservation", "itinerary", "e-ticket", "eticket", "boarding pass", "your ticket", "tickets",
        "refund", "refunded", "transaction", "purchase", "subscription renewal", "renewal confirmation", "amount due",
        "billing", "paid", "total",
        // German
        "rechnung", "ihre rechnung", "deine rechnung", "bestellung", "ihre bestellung", "deine bestellung", "bestellbestätigung",
        "auftragsbestätigung", "zahlungsbestätigung", "zahlung erhalten", "quittung", "beleg", "versandbestätigung",
        "versandt", "verschickt", "sendungsverfolgung", "sendungsnummer", "lieferung", "zugestellt", "paket",
        "kontoauszug", "buchungsbestätigung", "buchung", "reservierung", "fahrkarte", "rückerstattung", "erstattung",
        "kaufbestätigung", "gesamtbetrag", "betrag",
    ])
});

/// Phrases that strongly indicate a transaction even on their own.
static TRANSACTION_STRONG: Lazy<Regex> = Lazy::new(|| {
    keyword_regex(&[
        "receipt", "order confirmation", "order confirmed", "your order", "invoice", "payment received", "payment confirmation",
        "has shipped", "shipping confirmation", "tracking number", "out for delivery", "has been delivered", "bank statement",
        "statement is available", "booking confirmation", "your reservation", "itinerary", "e-ticket", "boarding pass", "refund",
        "rechnung", "bestellbestätigung", "auftragsbestätigung", "zahlungsbestätigung", "quittung", "versandbestätigung",
        "sendungsverfolgung", "zugestellt", "kontoauszug", "buchungsbestätigung", "kaufbestätigung", "rückerstattung",
    ])
});

static PROMOTION: Lazy<Regex> = Lazy::new(|| {
    keyword_regex(&[
        // English
        "sale", "on sale", "discount", "discounts", "coupon", "coupons", "promo code", "promo", "voucher", "deal", "deals",
        "limited time", "limited-time", "special offer", "offer", "offers", "free shipping", "shop now", "buy now", "order now",
        "save up to", "save", "exclusive", "black friday", "cyber monday", "last chance", "new arrivals", "clearance",
        "bestseller", "best sellers", "don't miss", "hurry", "ends tonight", "ends soon", "flash sale", "members only",
        "% off", "percent off", "off everything", "gift card", "bogo", "lowest price",
        // German
        "rabatt", "rabatte", "gutschein", "gutscheincode", "angebot", "angebote", "sonderangebot", "aktion", "sparen",
        "jetzt sparen", "reduziert", "jetzt kaufen", "jetzt shoppen", "versandkostenfrei", "nur heute", "schnäppchen",
        "ausverkauf", "neuheiten", "exklusiv", "letzte chance", "nicht verpassen", "bis zu",
    ])
});

static PERCENT_OFF: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\d{1,2}\s?%\s?(?:off|rabatt|reduziert|sparen)|(?:-|bis zu |up to )\d{1,2}\s?%").unwrap());

static UPDATES: Lazy<Regex> = Lazy::new(|| {
    keyword_regex(&[
        // English
        "newsletter", "digest", "weekly digest", "daily digest", "notification", "notifications", "security alert",
        "new sign-in", "new sign in", "sign-in attempt", "new login", "login attempt", "password", "reset your password",
        "verify your", "verification code", "confirm your email", "account", "your account", "update", "updates",
        "what's new", "weekly", "monthly", "mentioned you", "commented", "replied to", "liked your", "followed you",
        "new follower", "wants to connect", "invitation to connect", "reminder", "your subscription", "terms of service",
        "privacy policy", "release notes", "changelog", "status update", "two-factor", "2fa", "one-time code",
        // German
        "benachrichtigung", "sicherheitshinweis", "sicherheitswarnung", "neue anmeldung", "anmeldung", "passwort",
        "kennwort", "bestätigen sie", "bestätige deine", "konto", "ihr konto", "dein konto", "neuigkeiten",
        "wöchentlich", "monatlich", "erinnerung", "hat dich erwähnt", "hat kommentiert", "datenschutz",
        "nutzungsbedingungen", "bestätigungscode",
    ])
});

/// Local parts that are typical of automated senders.
static NOREPLY_LOCAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:no[-_.]?reply|do[-_.]?not[-_.]?reply|dont[-_.]?reply|keine[-_.]?antwort|noreply[-_.].*|.*[-_.]noreply|mailer[-_.]daemon|postmaster|notifications?|notify|alerts?|automated|auto[-_.]?confirm|bounces?(?:[-+].*)?)$").unwrap()
});

/// Local parts typical of role / business mailboxes (orders@, billing@, news@...).
static ROLE_LOCAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(?:orders?|order[-_.]?confirmation|bestellung(?:en)?|billing|invoices?|rechnung(?:en)?|receipts?|payments?|accounts?|shop|store|sales|support|service|kundenservice|customer[-_.]?(?:service|care)|info|news|newsletters?|marketing|offers?|deals|promo(?:tions)?|hello|team|updates?|booking|bookings|reservations?|tickets?|shipping|versand|delivery|tracking|statements?|security|digest|community|members?|rewards)$").unwrap()
});

/// Headers that marketing / bulk sending platforms add.
const BULK_HEADERS: [&str; 8] = ["X-Campaign", "X-CampaignID", "X-Mailchimp-Campaign", "X-Mailgun-Tag", "X-SG-EID", "X-Newsletter", "X-Marketing", "X-Mailer-Campaign"];

/// Numeric signal strengths computed for a message, exposed for debugging and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signals {
    pub has_list: bool,
    pub bulk: bool,
    pub automated_sender: bool,
    pub role_sender: bool,
    pub auto_submitted: bool,
    pub marketing_headers: bool,
    pub is_reply: bool,
    pub transaction: u32,
    pub transaction_strong: bool,
    pub promotion: u32,
    pub updates: u32,
}

impl Signals {
    /// The message was very likely not written by a person for the recipient.
    pub fn automated(&self) -> bool {
        self.has_list || self.bulk || self.automated_sender || self.role_sender || self.auto_submitted || self.marketing_headers
    }
}

fn count_matches(re: &Regex, text: &str) -> u32 {
    let mut seen: Vec<String> = Vec::new();
    for m in re.find_iter(text) {
        let k = m.as_str().trim().to_lowercase();
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    seen.len() as u32
}

fn body_prefix(p: &ParsedMessage) -> String {
    let mut body = p.plain_text();
    if body.len() > BODY_SCAN_LIMIT {
        let mut cut = BODY_SCAN_LIMIT;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    body
}

/// Compute the classification signals for a message.
pub fn signals(p: &ParsedMessage) -> Signals {
    let header = |n: &str| p.header(n).unwrap_or("").trim().to_ascii_lowercase();
    let local = p.from.email.split('@').next().unwrap_or("").trim().to_string();
    let precedence = header("Precedence");
    let auto = header("Auto-Submitted");
    let subject = p.subject.as_str();
    let body = body_prefix(p);

    let mut s = Signals {
        has_list: !p.list_id.trim().is_empty() || !p.list_unsubscribe.trim().is_empty() || p.header("List-Unsubscribe").is_some() || p.header("List-Id").is_some(),
        bulk: matches!(precedence.as_str(), "bulk" | "list" | "junk"),
        automated_sender: !local.is_empty() && NOREPLY_LOCAL.is_match(&local),
        role_sender: !local.is_empty() && ROLE_LOCAL.is_match(&local),
        auto_submitted: !auto.is_empty() && auto != "no",
        marketing_headers: BULK_HEADERS.iter().any(|h| p.header(h).is_some()) || p.header("Feedback-ID").is_some() || p.header("X-Feedback-ID").is_some(),
        is_reply: !p.in_reply_to.is_empty() || subject_is_reply(subject),
        ..Default::default()
    };

    s.transaction = count_matches(&TRANSACTION, subject) * 3 + count_matches(&TRANSACTION, &body).min(6);
    s.transaction_strong = TRANSACTION_STRONG.is_match(subject);
    s.promotion = count_matches(&PROMOTION, subject) * 3 + count_matches(&PROMOTION, &body).min(6);
    if PERCENT_OFF.is_match(subject) {
        s.promotion += 4;
    } else if PERCENT_OFF.is_match(&body) {
        s.promotion += 2;
    }
    s.updates = count_matches(&UPDATES, subject) * 3 + count_matches(&UPDATES, &body).min(6);
    s
}

/// Decide the category of a message from its headers and content.
pub fn categorize(p: &ParsedMessage) -> Category {
    classify(&signals(p))
}

/// Decide a category from precomputed [`Signals`].
pub fn classify(s: &Signals) -> Category {
    if !s.automated() {
        // Written by a person (or at least indistinguishable from it): never demote.
        return Category::Primary;
    }
    // A reply in a conversation with a business mailbox stays in Primary unless it is list traffic.
    if s.is_reply && !s.has_list && !s.bulk && !s.marketing_headers && !s.automated_sender {
        return Category::Primary;
    }
    let mass = s.has_list || s.bulk || s.marketing_headers;

    // Transactions win when the subject clearly says so and it is not dominated by marketing.
    if (s.transaction_strong && s.transaction >= 3 && s.transaction + 2 >= s.promotion) || (s.transaction >= 7 && s.transaction > s.promotion) {
        return Category::Transactions;
    }
    if s.promotion >= 6 || (mass && s.promotion >= 4 && s.promotion > s.updates) {
        return Category::Promotions;
    }
    if s.transaction >= 4 && s.transaction > s.promotion && s.transaction > s.updates {
        return Category::Transactions;
    }
    if mass || s.automated_sender || s.auto_submitted || s.updates >= 3 {
        if s.promotion >= 3 && s.promotion > s.updates + 2 {
            return Category::Promotions;
        }
        return Category::Updates;
    }
    // Role sender (info@, hello@ ...) without any clear signal: treat as personal.
    Category::Primary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mime;

    fn parse(headers: &str, body: &str) -> ParsedMessage {
        let raw = format!("{headers}\r\nMessage-ID: <x@example.com>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\n\r\n{body}\r\n");
        mime::parse(raw.as_bytes()).unwrap()
    }

    #[test]
    fn personal_mail_is_primary() {
        let p = parse("From: Anna Berger <anna.berger@example.net>\r\nTo: me@example.org\r\nSubject: Dinner on Saturday?", "Hi! Do you want to come over for dinner? I found a great deal on fresh fish, and we could split the invoice for the wine.");
        assert_eq!(categorize(&p), Category::Primary);
        let p = parse("From: Tom <tom@example.net>\r\nTo: me@example.org\r\nSubject: Re: your order of the photos", "The prints have shipped, receipt attached.");
        assert_eq!(categorize(&p), Category::Primary);
    }

    #[test]
    fn receipts_and_shipping_are_transactions() {
        let p = parse("From: Corner Books <orders@cornerbooks.example>\r\nTo: me@example.org\r\nSubject: Your order #4411 has shipped", "Good news! Your package is on the way. Tracking number: 1Z999.");
        assert_eq!(categorize(&p), Category::Transactions);
        let p = parse("From: Stadtwerke <noreply@stadtwerke.example>\r\nTo: me@example.org\r\nSubject: Ihre Rechnung für September", "Sehr geehrte Kundin, Ihre Rechnung steht bereit. Gesamtbetrag: 42,10 EUR.");
        assert_eq!(categorize(&p), Category::Transactions);
        let p = parse("From: Railways <tickets@rail.example>\r\nTo: me@example.org\r\nSubject: Booking confirmation and e-ticket", "Your itinerary: Berlin → Hamburg.");
        assert_eq!(categorize(&p), Category::Transactions);
    }

    #[test]
    fn marketing_is_promotions() {
        let p = parse(
            "From: Fern & Co <news@fernco.example>\r\nTo: me@example.org\r\nList-Unsubscribe: <https://fernco.example/u>\r\nSubject: 30% off everything — this weekend only",
            "Shop now and save on our new arrivals. Use promo code FERN30. Free shipping on all orders.",
        );
        assert_eq!(categorize(&p), Category::Promotions);
        let p = parse("From: Shop <info@laden.example>\r\nTo: me@example.org\r\nPrecedence: bulk\r\nSubject: Nur heute: 20% Rabatt mit Gutschein", "Jetzt shoppen und sparen!");
        assert_eq!(categorize(&p), Category::Promotions);
    }

    #[test]
    fn newsletters_and_notifications_are_updates() {
        let p = parse(
            "From: Rust Weekly <newsletter@weekly.example>\r\nTo: me@example.org\r\nList-Id: <weekly.example>\r\nList-Unsubscribe: <mailto:u@weekly.example>\r\nSubject: This week in systems programming #212",
            "Articles, release notes and community updates from the past week.",
        );
        assert_eq!(categorize(&p), Category::Updates);
        let p = parse("From: Example Cloud <no-reply@cloud.example>\r\nTo: me@example.org\r\nSubject: New sign-in to your account", "We noticed a new sign-in from Linux.");
        assert_eq!(categorize(&p), Category::Updates);
    }

    #[test]
    fn role_sender_without_signals_stays_primary() {
        let p = parse("From: Hello <hello@studio.example>\r\nTo: me@example.org\r\nSubject: Quick question about the logo", "Could you send the files again?");
        assert_eq!(categorize(&p), Category::Primary);
    }
}
