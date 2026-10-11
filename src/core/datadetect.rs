//! Data detectors: find links, email addresses, phone numbers and dates in text.
//!
//! Detection runs a set of independent recognizers and then resolves overlaps:
//! links beat email addresses, which beat phone numbers, which beat dates; within
//! the same kind the longer match wins. Dates and times are resolved relative to
//! the current local time ("tomorrow at 3pm", "next Monday", "12 March 2026",
//! "2026-03-12 14:00", "3/12", "12.03.2026", "morgen um 10 Uhr" ...).

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Clone, Debug, PartialEq)]
pub enum DetectedKind {
    Url(String),
    Email(String),
    Phone(String),
    /// A date/time mention with the parsed unix timestamp when known.
    Date(Option<i64>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    /// Byte range in the input text.
    pub start: usize,
    pub end: usize,
    pub kind: DetectedKind,
}

impl Detected {
    /// The matched slice of `text` (the text passed to [`detect`]).
    pub fn text<'a>(&self, text: &'a str) -> &'a str {
        text.get(self.start..self.end).unwrap_or("")
    }
}

const MONTHS: &str = r"january|february|march|april|may|june|july|august|september|october|november|december|januar|jänner|februar|märz|maerz|mai|juni|juli|oktober|dezember|jan|feb|mar|mär|apr|jun|jul|aug|sept|sep|oct|okt|nov|dec|dez";
const WEEKDAYS: &str = r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|montag|dienstag|mittwoch|donnerstag|freitag|samstag|sonnabend|sonntag";
const TIME: &str = r"(?:(?P<h1>\d{1,2}):(?P<m1>\d{2})(?:\s*(?P<ap1>a\.m\.|p\.m\.|am|pm|uhr|h)\b)?|(?P<h2>\d{1,2})(?:\.(?P<m2>\d{2}))?\s*(?P<ap2>a\.m\.|p\.m\.|am|pm|uhr)(?:\b|$)|(?P<word>noon|midnight|mittag|mitternacht)\b)";

static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)\b(?:(?:https?|ftp)://|www\.)[^\s<>"'\[\]{}|\\^`]+|\bmailto:[^\s<>"'()\[\]]+"#).unwrap());
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b[a-z0-9][a-z0-9._%+\-]*@[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?)*\.[a-z]{2,}\b").unwrap());
static PHONE_INTL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\+|\b00)[1-9]\d{0,2}(?:[ .\-/]?\(0\))?(?:[ .\-/]?\(?\d{1,5}\)?){1,6}").unwrap());
static PHONE_NATIONAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\(0\d{1,5}\)|\b0\d{1,5})(?:[ .\-/]?\d{2,8}){1,4}").unwrap());
static PHONE_US: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\([2-9]\d{2}\)\s?|\b[2-9]\d{2}[ .\-])\d{3}[ .\-]\d{4}\b").unwrap());

static ISO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{1,2}):(\d{2})(?::\d{2})?)?\b").unwrap());
static DOTTED_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{1,2})\.(\d{1,2})\.(\d{4}|\d{2})?").unwrap());
static SLASH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{1,2})/(\d{1,2})(?:/(\d{4}|\d{2}))?\b").unwrap());
static DAY_MONTH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"(?i)\b(\d{{1,2}})(?:st|nd|rd|th|\.)?(?:\s+of)?\s+({MONTHS})\b\.?(?:,?\s+(\d{{4}}))?")).unwrap());
static MONTH_DAY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"(?i)\b({MONTHS})\.?\s+(\d{{1,2}})(?:st|nd|rd|th)?\b(?:,?\s+(\d{{4}}))?")).unwrap());
static RELATIVE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(the day after tomorrow|day after tomorrow|übermorgen|tomorrow|today|tonight|this evening|this morning|this afternoon|yesterday|heute abend|heute|morgen früh|morgen|gestern|next week|nächste woche)\b").unwrap()
});
static WEEKDAY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)\b(?:(next|this|coming|on|nächsten|nächster|kommenden|diesen|am)\s+)?({WEEKDAYS})\b")).unwrap());
static TIME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)\b{TIME}")).unwrap());
static TIME_SUFFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)^,?\s*(?:at|um|@|,|from|von|ab|-|–)?\s*{TIME}")).unwrap());

/// Find all detectable items in `text`, sorted by position and non-overlapping.
pub fn detect(text: &str) -> Vec<Detected> {
    detect_at(text, Local::now())
}

/// Like [`detect`], resolving relative dates against `now`.
pub fn detect_at(text: &str, now: DateTime<Local>) -> Vec<Detected> {
    // (priority, candidate); lower priority value wins.
    let mut cands: Vec<(u8, Detected)> = Vec::new();
    for m in URL_RE.find_iter(text) {
        let (s, e) = trim_url(text, m.start(), m.end());
        if e > s + 4 {
            let raw = &text[s..e];
            let url = if raw.to_ascii_lowercase().starts_with("www.") { format!("https://{raw}") } else { raw.to_string() };
            cands.push((0, Detected { start: s, end: e, kind: DetectedKind::Url(url) }));
        }
    }
    for m in EMAIL_RE.find_iter(text) {
        cands.push((1, Detected { start: m.start(), end: m.end(), kind: DetectedKind::Email(m.as_str().to_string()) }));
    }
    for re in [&*PHONE_INTL, &*PHONE_NATIONAL, &*PHONE_US] {
        for m in re.find_iter(text) {
            if let Some((s, e)) = validate_phone(text, m.start(), m.end()) {
                cands.push((2, Detected { start: s, end: e, kind: DetectedKind::Phone(text[s..e].to_string()) }));
            }
        }
    }
    detect_dates(text, now, &mut cands);

    cands.sort_by(|a, b| a.0.cmp(&b.0).then((b.1.end - b.1.start).cmp(&(a.1.end - a.1.start))).then(a.1.start.cmp(&b.1.start)));
    let mut chosen: Vec<Detected> = Vec::new();
    for (_, c) in cands {
        if !chosen.iter().any(|x| c.start < x.end && x.start < c.end) {
            chosen.push(c);
        }
    }
    chosen.sort_by_key(|d| d.start);
    chosen
}

fn trim_url(text: &str, start: usize, mut end: usize) -> (usize, usize) {
    loop {
        let s = &text[start..end];
        let Some(last) = s.chars().last() else { break };
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '>' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            _ => false,
        };
        if !trim {
            break;
        }
        end -= last.len_utf8();
    }
    (start, end)
}

fn validate_phone(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let s = &text[start..end];
    let prev = text[..start].chars().last();
    let mut next_chars = text[end..].chars();
    let next = next_chars.next();
    let after_next = next_chars.next();
    if prev.is_some_and(|c| c.is_alphanumeric() || matches!(c, '-' | '/' | '_' | '.' | ',' | '€' | '$' | '#')) {
        return None;
    }
    if next.is_some_and(|c| c.is_alphanumeric() || matches!(c, '-' | '/' | '_' | '%' | '€' | '$' | ':'))
        || (matches!(next, Some('.') | Some(',')) && after_next.is_some_and(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count() - if s.contains("(0)") { 1 } else { 0 };
    let international = s.starts_with('+') || s.starts_with("00");
    if !(7..=15).contains(&digits) || (international && digits < 8) {
        return None;
    }
    let has_sep = s.chars().any(|c| matches!(c, ' ' | '-' | '.' | '/' | '(' | ')'));
    if !international && !has_sep {
        return None;
    }
    // Things that look like dates or times are not phone numbers.
    if DOTTED_RE.is_match(s) && s.split('.').count() == 3 || ISO_RE.is_match(s) {
        let only_date = s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-');
        if only_date && s.len() <= 10 {
            return None;
        }
    }
    let trimmed_end = start + s.trim_end_matches([' ', '-', '.', '/']).len();
    Some((start, trimmed_end))
}

/// Parse a time match from the TIME sub-pattern captures.
fn time_from_caps(c: &regex::Captures) -> Option<NaiveTime> {
    if let Some(w) = c.name("word") {
        return match w.as_str().to_lowercase().as_str() {
            "noon" | "mittag" => NaiveTime::from_hms_opt(12, 0, 0),
            _ => NaiveTime::from_hms_opt(0, 0, 0),
        };
    }
    let (h, m, ap) = if let Some(h) = c.name("h1") {
        (h.as_str(), c.name("m1").map(|m| m.as_str()), c.name("ap1").map(|m| m.as_str()))
    } else {
        (c.name("h2")?.as_str(), c.name("m2").map(|m| m.as_str()), c.name("ap2").map(|m| m.as_str()))
    };
    let mut hour: u32 = h.parse().ok()?;
    let minute: u32 = m.map(|m| m.parse().ok()).unwrap_or(Some(0))?;
    match ap.map(|a| a.to_lowercase()) {
        Some(a) if a.starts_with('a') || a.starts_with('p') => {
            if hour == 0 || hour > 12 {
                return None;
            }
            let pm = a.starts_with('p');
            if pm && hour != 12 {
                hour += 12;
            } else if !pm && hour == 12 {
                hour = 0;
            }
        }
        _ => {}
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

fn month_from_name(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    let n = n.trim_end_matches('.');
    let m = match n {
        "january" | "januar" | "jänner" | "jan" => 1,
        "february" | "februar" | "feb" => 2,
        "march" | "märz" | "maerz" | "mar" | "mär" => 3,
        "april" | "apr" => 4,
        "may" | "mai" => 5,
        "june" | "juni" | "jun" => 6,
        "july" | "juli" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sept" | "sep" => 9,
        "october" | "oktober" | "oct" | "okt" => 10,
        "november" | "nov" => 11,
        "december" | "dezember" | "dec" | "dez" => 12,
        _ => return None,
    };
    Some(m)
}

fn weekday_from_name(name: &str) -> Option<Weekday> {
    Some(match name.to_lowercase().as_str() {
        "monday" | "montag" => Weekday::Mon,
        "tuesday" | "dienstag" => Weekday::Tue,
        "wednesday" | "mittwoch" => Weekday::Wed,
        "thursday" | "donnerstag" => Weekday::Thu,
        "friday" | "freitag" => Weekday::Fri,
        "saturday" | "samstag" | "sonnabend" => Weekday::Sat,
        "sunday" | "sonntag" => Weekday::Sun,
        _ => return None,
    })
}

fn expand_year(y: &str) -> Option<i32> {
    let v: i32 = y.parse().ok()?;
    Some(if y.len() == 2 { 2000 + v } else { v })
}

/// A date without a year: pick this year, or next year if the date is well in the past.
fn infer_year(month: u32, day: u32, today: NaiveDate) -> Option<NaiveDate> {
    let d = NaiveDate::from_ymd_opt(today.year(), month, day)?;
    if d < today - Duration::days(60) { NaiveDate::from_ymd_opt(today.year() + 1, month, day) } else { Some(d) }
}

fn to_ts(date: NaiveDate, time: Option<NaiveTime>) -> Option<i64> {
    let dt = date.and_time(time.unwrap_or(NaiveTime::MIN));
    Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp())
}

/// Extend a date match with a following time ("… at 3pm", "…, 14:00", "… um 10 Uhr").
fn time_suffix(text: &str, end: usize) -> Option<(usize, NaiveTime)> {
    let rest = &text[end..];
    let c = TIME_SUFFIX_RE.captures(rest)?;
    let t = time_from_caps(&c)?;
    Some((end + c.get(0)?.end(), t))
}

fn push_date(cands: &mut Vec<(u8, Detected)>, text: &str, start: usize, end: usize, date: Option<NaiveDate>, time: Option<NaiveTime>) {
    let Some(date) = date else { return };
    let (end, time) = match time {
        Some(t) => (end, Some(t)),
        None => match time_suffix(text, end) {
            Some((e, t)) => (e, Some(t)),
            None => (end, None),
        },
    };
    cands.push((3, Detected { start, end, kind: DetectedKind::Date(to_ts(date, time)) }));
}

fn boundary_ok(text: &str, start: usize, end: usize, forbidden: &[char]) -> bool {
    let prev = text[..start].chars().last();
    let mut after = text[end..].chars();
    let next = after.next();
    // Sentence punctuation right after a match is fine unless a number continues ("3/12.5").
    let next_bad = match next {
        Some('.' | ',') => after.next().is_some_and(|c| c.is_ascii_digit()),
        Some(c) => c.is_ascii_digit() || forbidden.contains(&c),
        None => false,
    };
    !prev.is_some_and(|c| c.is_ascii_digit() || forbidden.contains(&c)) && !next_bad
}

fn detect_dates(text: &str, now: DateTime<Local>, cands: &mut Vec<(u8, Detected)>) {
    let today = now.date_naive();

    for c in ISO_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let date = (|| NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?))();
        let time = match (c.get(4), c.get(5)) {
            (Some(h), Some(mi)) => NaiveTime::from_hms_opt(h.as_str().parse().unwrap_or(99), mi.as_str().parse().unwrap_or(99), 0),
            _ => None,
        };
        push_date(cands, text, m.start(), m.end(), date, time);
    }

    for c in DOTTED_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        // Without a year the match ends in "." (e.g. "12.03."), which the regex guarantees;
        // "1.2.3" or IP addresses are rejected by the boundary check.
        if !boundary_ok(text, m.start(), m.end(), &['.', ',']) {
            continue;
        }
        let (Ok(day), Ok(month)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) else { continue };
        let date = match c.get(3) {
            Some(y) => expand_year(y.as_str()).and_then(|y| NaiveDate::from_ymd_opt(y, month, day)),
            None => infer_year(month, day, today),
        };
        push_date(cands, text, m.start(), m.end(), date, None);
    }

    for c in SLASH_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        if !boundary_ok(text, m.start(), m.end(), &['/', '.']) {
            continue;
        }
        let (Ok(month), Ok(day)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) else { continue };
        let date = match c.get(3) {
            Some(y) => expand_year(y.as_str()).and_then(|y| NaiveDate::from_ymd_opt(y, month, day)),
            None => infer_year(month, day, today),
        };
        push_date(cands, text, m.start(), m.end(), date, None);
    }

    for c in DAY_MONTH_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let (Ok(day), Some(month)) = (c[1].parse::<u32>(), month_from_name(&c[2])) else { continue };
        let date = match c.get(3) {
            Some(y) => y.as_str().parse().ok().and_then(|y| NaiveDate::from_ymd_opt(y, month, day)),
            None => infer_year(month, day, today),
        };
        push_date(cands, text, m.start(), m.end(), date, None);
    }

    for c in MONTH_DAY_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let (Some(month), Ok(day)) = (month_from_name(&c[1]), c[2].parse::<u32>()) else { continue };
        // "may 5" in lowercase prose is more likely the verb; require a capital or a year.
        if c[1].eq_ignore_ascii_case("may") && !c[1].starts_with('M') && c.get(3).is_none() {
            continue;
        }
        let date = match c.get(3) {
            Some(y) => y.as_str().parse().ok().and_then(|y| NaiveDate::from_ymd_opt(y, month, day)),
            None => infer_year(month, day, today),
        };
        push_date(cands, text, m.start(), m.end(), date, None);
    }

    for c in RELATIVE_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let word = m.as_str();
        let lower = word.to_lowercase();
        let suffix = time_suffix(text, m.end());
        // German "Morgen" (capitalized) usually means "morning"; only accept it with a time.
        if lower.starts_with("morgen") && word.starts_with('M') && suffix.is_none() {
            continue;
        }
        let (date, default_time) = match lower.as_str() {
            "today" | "heute" => (today, None),
            "tonight" | "heute abend" | "this evening" => (today, NaiveTime::from_hms_opt(20, 0, 0)),
            "this morning" => (today, NaiveTime::from_hms_opt(9, 0, 0)),
            "this afternoon" => (today, NaiveTime::from_hms_opt(15, 0, 0)),
            "tomorrow" | "morgen" => (today + Duration::days(1), None),
            "morgen früh" => (today + Duration::days(1), NaiveTime::from_hms_opt(9, 0, 0)),
            "yesterday" | "gestern" => (today - Duration::days(1), None),
            "next week" | "nächste woche" => {
                let to_monday = 7 - today.weekday().num_days_from_monday() as i64;
                (today + Duration::days(to_monday), None)
            }
            _ => (today + Duration::days(2), None),
        };
        let (end, time) = match suffix {
            Some((e, t)) => (e, Some(t)),
            None => (m.end(), default_time),
        };
        cands.push((3, Detected { start: m.start(), end, kind: DetectedKind::Date(to_ts(date, time)) }));
    }

    for c in WEEKDAY_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        let Some(wd) = weekday_from_name(&c[2]) else { continue };
        let modifier = c.get(1).map(|x| x.as_str().to_lowercase()).unwrap_or_default();
        let cur = today.weekday().num_days_from_monday() as i64;
        let target = wd.num_days_from_monday() as i64;
        let mut delta = (target - cur).rem_euclid(7);
        if delta == 0 && (modifier.starts_with("next") || modifier.starts_with("nächst") || modifier == "coming" || modifier == "kommenden") {
            delta = 7;
        }
        let date = today + Duration::days(delta);
        // Keep the modifier word inside the match only when it carries meaning.
        let start = if matches!(modifier.as_str(), "on" | "am") { c.get(2).unwrap().start() } else { m.start() };
        push_date(cands, text, start, m.end(), Some(date), None);
    }

    for c in TIME_RE.captures_iter(text) {
        let m = c.get(0).unwrap();
        if !boundary_ok(text, m.start(), m.end(), &[':', '.']) {
            continue;
        }
        if let Some(t) = time_from_caps(&c) {
            cands.push((3, Detected { start: m.start(), end: m.end(), kind: DetectedKind::Date(to_ts(today, Some(t))) }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        // Wednesday, 7 October 2026, 11:00 local time.
        Local.with_ymd_and_hms(2026, 10, 7, 11, 0, 0).unwrap()
    }

    fn ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> Option<i64> {
        Some(Local.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap().timestamp())
    }

    fn kinds(text: &str) -> Vec<(String, DetectedKind)> {
        detect_at(text, now()).into_iter().map(|d| (d.text(text).to_string(), d.kind)).collect()
    }

    #[test]
    fn urls_and_emails() {
        let k = kinds("See https://example.com/path?a=1. Or (www.example.org/x) and mail anna@example.net, or mailto:bob@example.com!");
        assert_eq!(k[0], ("https://example.com/path?a=1".into(), DetectedKind::Url("https://example.com/path?a=1".into())));
        assert_eq!(k[1], ("www.example.org/x".into(), DetectedKind::Url("https://www.example.org/x".into())));
        assert_eq!(k[2], ("anna@example.net".into(), DetectedKind::Email("anna@example.net".into())));
        assert_eq!(k[3].0, "mailto:bob@example.com");
        assert_eq!(k.len(), 4);
        // URL containing an @ is not split into an email
        let k = kinds("https://user@host.example.com/a");
        assert_eq!(k.len(), 1);
    }

    #[test]
    fn phone_numbers() {
        let k = kinds("Call +49 30 1234567 or (030) 123 456 78 or 555-123-4567 or 0151-23456789.");
        let phones: Vec<String> = k.iter().filter(|(_, k)| matches!(k, DetectedKind::Phone(_))).map(|(t, _)| t.clone()).collect();
        assert_eq!(phones, vec!["+49 30 1234567", "(030) 123 456 78", "555-123-4567", "0151-23456789"]);
        // Plain numbers, prices and dates are not phone numbers
        let k = kinds("Order 12345678 costs 1.234,56 EUR, total 2026 items, on 12.03.2026.");
        assert!(!k.iter().any(|(_, k)| matches!(k, DetectedKind::Phone(_))), "{k:?}");
    }

    #[test]
    fn absolute_dates() {
        let k = kinds("Meeting on 12 March 2026, then 2026-03-12 14:00 and 12.03.2026 and 3/12.");
        let dates: Vec<(String, DetectedKind)> = k.into_iter().filter(|(_, k)| matches!(k, DetectedKind::Date(_))).collect();
        assert_eq!(dates[0], ("12 March 2026".into(), DetectedKind::Date(ts(2026, 3, 12, 0, 0))));
        assert_eq!(dates[1], ("2026-03-12 14:00".into(), DetectedKind::Date(ts(2026, 3, 12, 14, 0))));
        assert_eq!(dates[2], ("12.03.2026".into(), DetectedKind::Date(ts(2026, 3, 12, 0, 0))));
        // "3/12" without year in October → next March
        assert_eq!(dates[3], ("3/12".into(), DetectedKind::Date(ts(2027, 3, 12, 0, 0))));
        let k = kinds("Due October 20th, 2026 at 5pm.");
        assert_eq!(k[0], ("October 20th, 2026 at 5pm".into(), DetectedKind::Date(ts(2026, 10, 20, 17, 0))));
    }

    #[test]
    fn relative_dates() {
        let k = kinds("Can we talk tomorrow at 3pm?");
        assert_eq!(k, vec![("tomorrow at 3pm".into(), DetectedKind::Date(ts(2026, 10, 8, 15, 0)))]);
        let k = kinds("Let's meet next Monday.");
        assert_eq!(k, vec![("next Monday".into(), DetectedKind::Date(ts(2026, 10, 12, 0, 0)))]);
        let k = kinds("Wir sehen uns morgen um 10 Uhr.");
        assert_eq!(k, vec![("morgen um 10 Uhr".into(), DetectedKind::Date(ts(2026, 10, 8, 10, 0)))]);
        let k = kinds("Guten Morgen! Am Freitag, 9:30 passt.");
        assert_eq!(k, vec![("Freitag, 9:30".into(), DetectedKind::Date(ts(2026, 10, 9, 9, 30)))]);
        let k = kinds("Lunch at 12:30?");
        assert_eq!(k, vec![("12:30".into(), DetectedKind::Date(ts(2026, 10, 7, 12, 30)))]);
    }

    #[test]
    fn no_false_positives_and_no_panics() {
        assert!(kinds("Version 1.2.3 has 3.5 times the speed, 192.168.1.1").is_empty(), "{:?}", kinds("Version 1.2.3 has 3.5 times the speed, 192.168.1.1"));
        assert!(kinds("I may 5 times").is_empty());
        for s in ["", "€€€ ü", "+", "http://", "a@b", "99/99/9999", "31.02.2026", "25:61", "+49 (0) 30"] {
            let _ = detect_at(s, now());
        }
    }
}
