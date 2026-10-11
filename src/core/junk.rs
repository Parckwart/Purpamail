//! Adaptive junk mail filter (naive Bayes over tokens stored in the `junk_tokens` table).
//!
//! Messages are reduced to a set of tokens (subject and body words, sender
//! address and domain, link domains, a few structural features). Per-token
//! spam / ham counts live in the `junk_tokens` table, and the number of trained
//! messages per class in the `kv` table. Scoring uses Gary Robinson's
//! per-token probability estimate combined with Fisher's chi-square method
//! (as popularized by SpamBayes). Until enough mail has been trained the filter
//! falls back to a deliberately conservative phrase heuristic that only flags
//! blatant junk, so legitimate mail is not caught.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::params;

use super::mime::ParsedMessage;
use super::store::Store;

const KV_SPAM: &str = "junk:spam_messages";
const KV_HAM: &str = "junk:ham_messages";
const KV_TRAINED_PREFIX: &str = "junk:trained:";
/// Messages of each class needed before Bayesian scores are used.
pub const MIN_TRAINED_PER_CLASS: u64 = 10;
/// Body text considered for tokens.
const MAX_BODY_CHARS: usize = 30_000;
/// Number of most significant tokens used for a decision.
const MAX_DISCRIMINATORS: usize = 150;
const UNKNOWN_WORD_STRENGTH: f64 = 0.45;
const UNKNOWN_WORD_PROB: f64 = 0.5;
const MIN_PROB_STRENGTH: f64 = 0.1;

static URL_HOST: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:https?://|www\.)([a-z0-9.\-]+)").unwrap());
static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{L}\p{N}$€£!'’\-]+").unwrap());

/// Phrases typical of junk mail (English and German), used by the heuristic fallback.
const SPAM_PHRASES: &[&str] = &[
    "viagra",
    "cialis",
    "levitra",
    "you have won",
    "you've won",
    "you are a winner",
    "lottery",
    "claim your prize",
    "claim your reward",
    "100% free",
    "risk-free",
    "risk free",
    "act now",
    "limited time only",
    "click here to claim",
    "wire transfer",
    "western union",
    "bitcoin investment",
    "crypto investment",
    "double your money",
    "make money fast",
    "earn money from home",
    "work from home and earn",
    "cheap meds",
    "no prescription",
    "weight loss",
    "lose weight fast",
    "inheritance",
    "next of kin",
    "beneficiary",
    "dear friend",
    "dear beneficiary",
    "urgent reply",
    "strictly confidential",
    "million dollars",
    "million usd",
    "congratulations you",
    "online casino",
    "casino bonus",
    "hot singles",
    "enlargement",
    "unclaimed funds",
    "your account has been suspended",
    "verify your account immediately",
    "nigeria",
    "barrister",
    "100% guaranteed",
    "this is not spam",
    "remove me",
    "sie haben gewonnen",
    "gewinnspiel gewonnen",
    "erbschaft",
    "dringende antwort",
    "schnell geld verdienen",
    "geld verdienen von zu hause",
    "streng vertraulich",
    "millionen euro",
    "kostenlos und unverbindlich",
];

/// Probability (0..1) that a message is junk.
pub fn score(store: &Store, p: &ParsedMessage) -> f32 {
    let heuristic = heuristic_score(p);
    let (nspam, nham) = stats(store);
    if nspam < MIN_TRAINED_PER_CLASS || nham < MIN_TRAINED_PER_CLASS {
        return heuristic;
    }
    let tokens = tokenize(p);
    let counts = token_counts(store, &tokens);
    let bayes = combine(&tokens, &counts, nspam, nham);
    // A blatant heuristic hit wins over an undecided Bayesian score.
    if heuristic >= 0.95 && (0.3..0.7).contains(&bayes) { heuristic } else { bayes }
}

/// Train the filter with a message the user marked as junk (`spam = true`) or not junk.
///
/// Training the same message again with the same class has no effect; training it
/// with the other class moves its tokens from one class to the other.
pub fn train(store: &Store, p: &ParsedMessage, spam: bool) {
    let key = format!("{KV_TRAINED_PREFIX}{}", message_key(p));
    let previous = store.kv_get(&key);
    let class = if spam { "spam" } else { "ham" };
    if previous.as_deref() == Some(class) {
        return;
    }
    let tokens = tokenize(p);
    let (mut nspam, mut nham) = stats(store);
    let (ds, dh): (i64, i64) = match (previous.as_deref(), spam) {
        (Some("ham"), true) => (1, -1),
        (Some("spam"), false) => (-1, 1),
        (_, true) => (1, 0),
        (_, false) => (0, 1),
    };
    if let Err(e) = adjust_tokens(store, &tokens, ds, dh) {
        log::warn!("junk filter training failed: {e}");
        return;
    }
    nspam = (nspam as i64 + ds).max(0) as u64;
    nham = (nham as i64 + dh).max(0) as u64;
    store.kv_set(KV_SPAM, &nspam.to_string()).ok();
    store.kv_set(KV_HAM, &nham.to_string()).ok();
    store.kv_set(&key, class).ok();
}

/// Remove a previously trained message from the statistics.
pub fn untrain(store: &Store, p: &ParsedMessage) {
    let key = format!("{KV_TRAINED_PREFIX}{}", message_key(p));
    let Some(previous) = store.kv_get(&key) else { return };
    let tokens = tokenize(p);
    let (nspam, nham) = stats(store);
    let (ds, dh) = if previous == "spam" { (-1, 0) } else { (0, -1) };
    if adjust_tokens(store, &tokens, ds, dh).is_ok() {
        store.kv_set(KV_SPAM, &(nspam as i64 + ds).max(0).to_string()).ok();
        store.kv_set(KV_HAM, &(nham as i64 + dh).max(0).to_string()).ok();
        store.with_conn(|c| c.execute("DELETE FROM kv WHERE key=?1", params![key]).ok());
    }
}

/// Forget everything the filter has learned.
pub fn reset(store: &Store) {
    store.with_conn(|c| {
        c.execute("DELETE FROM junk_tokens", []).ok();
        c.execute("DELETE FROM kv WHERE key LIKE 'junk:%'", []).ok();
    });
}

/// Number of messages trained as (junk, not junk).
pub fn stats(store: &Store) -> (u64, u64) {
    let get = |k: &str| store.kv_get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    (get(KV_SPAM), get(KV_HAM))
}

/// Whether enough mail has been trained for Bayesian scoring.
pub fn is_trained(store: &Store) -> bool {
    let (s, h) = stats(store);
    s >= MIN_TRAINED_PER_CLASS && h >= MIN_TRAINED_PER_CLASS
}

/// Whether server-side spam headers (X-Spam-Flag, X-Spam-Status, ...) mark the message as junk.
pub fn server_says_junk(p: &ParsedMessage) -> bool {
    static SCL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bSCL:\s*(-?\d+)").unwrap());
    for (k, v) in &p.headers {
        let name = k.to_ascii_lowercase();
        let val = v.trim().to_ascii_lowercase();
        let hit = match name.as_str() {
            "x-spam-flag" | "x-barracuda-spam-flag" => val.starts_with("yes") || val.starts_with("true"),
            "x-spam-status" | "x-barracuda-spam-status" | "x-spam-verdict" => val.starts_with("yes") || val.starts_with("spam"),
            "x-spam" | "x-is-spam" => val.starts_with("yes") || val.starts_with("true") || val == "spam" || val == "1",
            "x-spam-level" => v.matches('*').count() >= 5,
            "x-spam-score" | "x-rspamd-score" => val.split_whitespace().next().and_then(|n| n.parse::<f32>().ok()).is_some_and(|n| n >= 6.0),
            "x-ms-exchange-organization-scl" => val.trim().parse::<i32>().is_ok_and(|n| n >= 5),
            "x-microsoft-antispam" | "x-forefront-antispam-report" | "x-microsoft-antispam-mailbox-delivery" => {
                SCL.captures(v).and_then(|c| c[1].parse::<i32>().ok()).is_some_and(|n| n >= 5)
            }
            _ => false,
        };
        if hit {
            return true;
        }
    }
    false
}

/// Stable identifier of a message for remembering how it was trained.
fn message_key(p: &ParsedMessage) -> String {
    if !p.message_id.is_empty() {
        return p.message_id.clone();
    }
    // FNV-1a over a few identifying fields.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}\u{0}{}\u{0}{}\u{0}{}", p.subject, p.from.email, p.date, p.plain_text().len()).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("hash:{h:016x}")
}

fn word_tokens(text: &str, prefix: &str, out: &mut HashSet<String>) {
    for m in WORD.find_iter(text) {
        let w = m.as_str().trim_matches(|c: char| matches!(c, '\'' | '’' | '-' | '!')).to_lowercase();
        let n = w.chars().count();
        if n < 3 {
            continue;
        }
        if n > 25 {
            out.insert(format!("{prefix}skip:{}", n / 10 * 10));
            continue;
        }
        if w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        out.insert(format!("{prefix}{w}"));
    }
}

/// Reduce a message to its set of tokens.
pub fn tokenize(p: &ParsedMessage) -> HashSet<String> {
    let mut t = HashSet::new();
    word_tokens(&p.subject, "s:", &mut t);
    let letters: Vec<char> = p.subject.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 8 && letters.iter().filter(|c| c.is_uppercase()).count() * 10 >= letters.len() * 8 {
        t.insert("subject:allcaps".into());
    }
    if p.subject.contains("!!") {
        t.insert("subject:exclamations".into());
    }
    let email = p.from.email.to_lowercase();
    if !email.is_empty() {
        t.insert(format!("from:{email}"));
        if let Some((_, domain)) = email.rsplit_once('@') {
            t.insert(format!("fromdomain:{domain}"));
        }
    } else {
        t.insert("from:none".into());
    }
    word_tokens(&p.from.name, "fromname:", &mut t);
    if let (Some(r), Some((_, fd))) = (p.reply_to.first(), email.rsplit_once('@'))
        && !r.domain().eq_ignore_ascii_case(fd)
    {
        t.insert("replyto:otherdomain".into());
    }
    if p.to.is_empty() && p.cc.is_empty() {
        t.insert("to:none".into());
    }
    if let Some(xm) = p.header("X-Mailer").or(p.header("User-Agent"))
        && let Some(first) = xm.split_whitespace().next()
    {
        t.insert(format!("mailer:{}", first.to_lowercase()));
    }
    if p.html.is_some() && p.text.as_deref().is_none_or(|s| s.trim().is_empty()) {
        t.insert("body:html-only".into());
    }
    for a in p.real_attachments() {
        let ext = a.filename.rsplit('.').next().unwrap_or("").to_lowercase();
        t.insert(format!("attach:{ext}"));
    }
    let mut body = p.plain_text();
    if body.chars().count() > MAX_BODY_CHARS {
        body = body.chars().take(MAX_BODY_CHARS).collect();
    }
    // Links: count and host names (and their registrable part).
    let hosts: Vec<String> = URL_HOST.captures_iter(&body).map(|c| c[1].trim_end_matches('.').to_lowercase()).collect();
    if let Some(h) = &p.html {
        for c in URL_HOST.captures_iter(h).take(200) {
            t.insert(format!("url:{}", c[1].trim_end_matches('.').to_lowercase()));
        }
    }
    for h in &hosts {
        t.insert(format!("url:{h}"));
        let parts: Vec<&str> = h.split('.').collect();
        if parts.len() > 2 {
            t.insert(format!("url:{}", parts[parts.len() - 2..].join(".")));
        }
        if h.chars().all(|c| c.is_ascii_digit() || c == '.') {
            t.insert("url:ip-address".into());
        }
    }
    let without_urls = URL_HOST.replace_all(&body, " ");
    word_tokens(&without_urls, "", &mut t);
    t
}

fn token_counts(store: &Store, tokens: &HashSet<String>) -> HashMap<String, (u64, u64)> {
    let list: Vec<&String> = tokens.iter().collect();
    let mut out = HashMap::new();
    store.with_conn(|c| {
        for chunk in list.chunks(400) {
            let sql = format!("SELECT token, spam, ham FROM junk_tokens WHERE token IN ({})", vec!["?"; chunk.len()].join(","));
            let Ok(mut st) = c.prepare(&sql) else { continue };
            let rows = st.query_map(rusqlite::params_from_iter(chunk.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)));
            if let Ok(rows) = rows {
                for (tok, s, h) in rows.flatten() {
                    out.insert(tok, (s.max(0) as u64, h.max(0) as u64));
                }
            }
        }
    });
    out
}

fn adjust_tokens(store: &Store, tokens: &HashSet<String>, ds: i64, dh: i64) -> rusqlite::Result<()> {
    store.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO junk_tokens (token, spam, ham) VALUES (?1, max(?2, 0), max(?3, 0))
                 ON CONFLICT(token) DO UPDATE SET spam = max(spam + ?2, 0), ham = max(ham + ?3, 0)",
            )?;
            for t in tokens {
                st.execute(params![t, ds, dh])?;
            }
        }
        tx.execute("DELETE FROM junk_tokens WHERE spam = 0 AND ham = 0", [])?;
        tx.commit()
    })
}

/// Robinson's smoothed probability that a message containing the token is junk.
fn token_probability(spam: u64, ham: u64, nspam: u64, nham: u64) -> f64 {
    let b = (spam as f64 / nspam.max(1) as f64).min(1.0);
    let g = (ham as f64 / nham.max(1) as f64).min(1.0);
    let n = (spam + ham) as f64;
    let p = if b + g > 0.0 { b / (b + g) } else { UNKNOWN_WORD_PROB };
    (UNKNOWN_WORD_STRENGTH * UNKNOWN_WORD_PROB + n * p) / (UNKNOWN_WORD_STRENGTH + n)
}

/// Inverse chi-square survival function for even degrees of freedom.
fn chi2q(x2: f64, v: usize) -> f64 {
    let m = x2 / 2.0;
    let mut term = (-m).exp();
    let mut sum = term;
    for i in 1..v / 2 {
        term *= m / i as f64;
        sum += term;
    }
    sum.min(1.0)
}

fn combine(tokens: &HashSet<String>, counts: &HashMap<String, (u64, u64)>, nspam: u64, nham: u64) -> f32 {
    let mut probs: Vec<f64> = tokens
        .iter()
        .map(|t| {
            let (s, h) = counts.get(t).copied().unwrap_or((0, 0));
            token_probability(s, h, nspam, nham)
        })
        .filter(|p| (p - 0.5).abs() >= MIN_PROB_STRENGTH)
        .collect();
    if probs.is_empty() {
        return 0.5;
    }
    probs.sort_by(|a, b| (b - 0.5).abs().partial_cmp(&(a - 0.5).abs()).unwrap_or(std::cmp::Ordering::Equal));
    probs.truncate(MAX_DISCRIMINATORS);
    let n = probs.len();
    let (mut ln_ham, mut ln_spam) = (0.0f64, 0.0f64);
    for p in &probs {
        let p = p.clamp(1e-6, 1.0 - 1e-6);
        ln_ham += p.ln();
        ln_spam += (1.0 - p).ln();
    }
    let s = 1.0 - chi2q(-2.0 * ln_spam, 2 * n);
    let h = 1.0 - chi2q(-2.0 * ln_ham, 2 * n);
    (((s - h) + 1.0) / 2.0).clamp(0.0, 1.0) as f32
}

/// Conservative phrase based estimate used before the filter has been trained.
pub fn heuristic_score(p: &ParsedMessage) -> f32 {
    let subject = p.subject.to_lowercase();
    let mut body = p.plain_text().to_lowercase();
    if body.len() > MAX_BODY_CHARS {
        let mut cut = MAX_BODY_CHARS;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    let mut points = 0u32;
    for phrase in SPAM_PHRASES {
        if subject.contains(phrase) {
            points += 2;
        } else if body.contains(phrase) {
            points += 1;
        }
    }
    let letters: Vec<char> = p.subject.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 10 && letters.iter().all(|c| c.is_uppercase()) {
        points += 1;
    }
    if p.subject.contains("!!!") || subject.contains("$$$") || body.contains("$$$") {
        points += 1;
    }
    if URL_HOST.captures_iter(&body).any(|c| c[1].chars().all(|ch| ch.is_ascii_digit() || ch == '.')) {
        points += 1;
    }
    match points {
        0 => 0.05,
        1 => 0.2,
        2 => 0.35,
        3 => 0.55,
        4 => 0.75,
        5 => 0.9,
        _ => 0.97,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mime;

    fn msg(id: usize, from: &str, subject: &str, body: &str) -> ParsedMessage {
        let raw = format!(
            "From: {from}\r\nTo: me@example.org\r\nSubject: {subject}\r\nMessage-ID: <m{id}@test>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\n\r\n{body}\r\n"
        );
        mime::parse(raw.as_bytes()).unwrap()
    }

    fn spam(i: usize) -> ParsedMessage {
        let subjects = ["Cheap pills online", "Exclusive casino bonus", "Lose weight now", "Claim your free gift card", "Hot crypto profits"];
        msg(
            i,
            &format!("Deals <promo{i}@cheap-offers.example>"),
            subjects[i % subjects.len()],
            &format!("Buy cheap pills, casino jackpot and crypto profits! Visit http://cheap-offers.example/x{i} now. Unbelievable bargain pills {i}."),
        )
    }

    fn ham(i: usize) -> ParsedMessage {
        let subjects = ["Project meeting notes", "Lunch tomorrow?", "Website draft review", "Holiday photos", "Quarterly report"];
        msg(
            1000 + i,
            &format!("Colleague {i} <person{i}@company.example>"),
            subjects[i % subjects.len()],
            &format!("Hi, attached are the notes from our meeting about the website project. Let me know what you think about the draft. Cheers {i}"),
        )
    }

    #[test]
    fn heuristics_are_conservative_before_training() {
        let store = Store::open_in_memory().unwrap();
        let legit = msg(
            1,
            "Anna <anna@example.net>",
            "Free tickets for the concert?",
            "My friend has two free tickets. Do you want to come along? It's risk free, she said.",
        );
        assert!(score(&store, &legit) < 0.5);
        let blatant = msg(
            2,
            "Barrister John <x@scam.example>",
            "URGENT REPLY NEEDED: YOU HAVE WON!!!",
            "Dear beneficiary, you have won the lottery. Claim your prize of 5 million dollars via wire transfer. Strictly confidential.",
        );
        assert!(score(&store, &blatant) >= 0.9, "{}", score(&store, &blatant));
    }

    #[test]
    fn bayes_learns_and_retrains() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..12 {
            train(&store, &spam(i), true);
            train(&store, &ham(i), false);
        }
        assert_eq!(stats(&store), (12, 12));
        assert!(is_trained(&store));
        // Training the same message twice has no effect.
        train(&store, &spam(0), true);
        assert_eq!(stats(&store), (12, 12));
        let new_spam = spam(50);
        let new_ham = ham(50);
        let ss = score(&store, &new_spam);
        let hs = score(&store, &new_ham);
        assert!(ss > 0.9, "spam score {ss}");
        assert!(hs < 0.1, "ham score {hs}");
        // Re-classify a message: counts move between classes.
        train(&store, &spam(1), false);
        assert_eq!(stats(&store), (11, 13));
        untrain(&store, &spam(1));
        assert_eq!(stats(&store), (11, 12));
        reset(&store);
        assert_eq!(stats(&store), (0, 0));
        let n: i64 = store.with_conn(|c| c.query_row("SELECT COUNT(*) FROM junk_tokens", [], |r| r.get(0)).unwrap());
        assert_eq!(n, 0);
    }

    #[test]
    fn server_headers() {
        let with = |h: &str| mime::parse(format!("From: a@b.example\r\n{h}\r\nSubject: x\r\n\r\nbody\r\n").as_bytes()).unwrap();
        assert!(server_says_junk(&with("X-Spam-Flag: YES")));
        assert!(!server_says_junk(&with("X-Spam-Flag: NO")));
        assert!(server_says_junk(&with("X-Spam-Status: Yes, score=7.1 required=5.0")));
        assert!(!server_says_junk(&with("X-Spam-Status: No, score=0.1 required=5.0")));
        assert!(server_says_junk(&with("X-Spam: yes")));
        assert!(server_says_junk(&with("X-Spam-Level: *******")));
        assert!(!server_says_junk(&with("X-Spam-Level: **")));
        assert!(server_says_junk(&with("X-Microsoft-Antispam: BCL:0; SCL:6;")));
        assert!(server_says_junk(&with("X-Forefront-Antispam-Report: CIP:1.2.3.4;SCL:5;SRV:BULK")));
        assert!(!server_says_junk(&with("X-Forefront-Antispam-Report: SCL:-1;")));
        assert!(server_says_junk(&with("X-MS-Exchange-Organization-SCL: 9")));
        assert!(!server_says_junk(&with("X-Mailer: Test")));
    }

    #[test]
    fn tokens_and_math() {
        let p = msg(
            1,
            "Anna <anna@mail.example.net>",
            "HELLO THERE FRIEND!!",
            "See https://www.shop.example.com/a and http://192.168.0.1/x for déjà-vu items 12345",
        );
        let t = tokenize(&p);
        for k in [
            "s:hello",
            "subject:allcaps",
            "subject:exclamations",
            "from:anna@mail.example.net",
            "fromdomain:mail.example.net",
            "url:www.shop.example.com",
            "url:example.com",
            "url:ip-address",
            "déjà-vu",
            "items",
        ] {
            assert!(t.contains(k), "missing {k}: {t:?}");
        }
        assert!(!t.contains("12345"));
        assert!((chi2q(0.0, 4) - 1.0).abs() < 1e-9);
        assert!(token_probability(0, 0, 10, 10) == 0.5);
        assert!(token_probability(10, 0, 10, 10) > 0.9);
        assert_eq!(combine(&HashSet::new(), &HashMap::new(), 10, 10), 0.5);
    }
}
