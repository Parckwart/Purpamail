//! Search query parsing.
//!
//! Supports free text plus field tokens, e.g.
//! `from:alice subject:"lunch plans" to:bob invoice -spam`.
//! The result is an SQLite FTS5 MATCH expression plus flag filters.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedSearch {
    /// FTS5 expression ("" when only flag filters were given).
    pub fts: String,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachment: bool,
    /// Human readable tokens for display as chips in the search field.
    pub tokens: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchField {
    #[default]
    Entire,
    From,
    To,
    Subject,
    Body,
}

impl SearchField {
    pub const ALL: [SearchField; 5] = [SearchField::Entire, SearchField::From, SearchField::To, SearchField::Subject, SearchField::Body];
    pub fn name(&self) -> &'static str {
        match self {
            SearchField::Entire => "Entire Message",
            SearchField::From => "From",
            SearchField::To => "To",
            SearchField::Subject => "Subject",
            SearchField::Body => "Message content",
        }
    }
    fn column(&self) -> Option<&'static str> {
        match self {
            SearchField::Entire => None,
            SearchField::From => Some("sender"),
            SearchField::To => Some("recipients"),
            SearchField::Subject => Some("subject"),
            SearchField::Body => Some("body"),
        }
    }
}

fn quote_term(t: &str) -> String {
    // Quote every term so FTS operators in user input are taken literally; allow prefix search.
    let clean: String = t.chars().filter(|c| *c != '"').collect();
    if clean.is_empty() {
        return String::new();
    }
    format!("\"{clean}\"*")
}

fn split_tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in q.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parse a query typed into the search field. `default_field` applies to bare words.
pub fn parse(query: &str, default_field: SearchField) -> ParsedSearch {
    let mut res = ParsedSearch::default();
    let mut parts: Vec<String> = Vec::new();
    for tok in split_tokens(query) {
        let (neg, tok) = if let Some(t) = tok.strip_prefix('-') { (true, t.to_string()) } else { (false, tok) };
        let (field, value) = match tok.split_once(':') {
            Some((k, v)) if !v.is_empty() => {
                let f = match k.to_ascii_lowercase().as_str() {
                    "from" => Some(SearchField::From),
                    "to" | "cc" => Some(SearchField::To),
                    "subject" => Some(SearchField::Subject),
                    "body" => Some(SearchField::Body),
                    "is" => {
                        match v.to_ascii_lowercase().as_str() {
                            "unread" => res.unread = true,
                            "flagged" => res.flagged = true,
                            _ => {}
                        }
                        res.tokens.push(("is".into(), v.to_string()));
                        continue;
                    }
                    "has" => {
                        if v.eq_ignore_ascii_case("attachment") || v.eq_ignore_ascii_case("attachments") {
                            res.has_attachment = true;
                        }
                        res.tokens.push(("has".into(), v.to_string()));
                        continue;
                    }
                    _ => None,
                };
                match f {
                    Some(f) => (f, v.trim_matches('"').to_string()),
                    None => (default_field, tok.trim_matches('"').to_string()),
                }
            }
            _ => (default_field, tok.trim_matches('"').to_string()),
        };
        let mut words: Vec<String> = Vec::new();
        for w in value.split(|c: char| !c.is_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '-' && c != '+') {
            let q = quote_term(w);
            if !q.is_empty() {
                words.push(q);
            }
        }
        if words.is_empty() {
            continue;
        }
        let body = if words.len() == 1 { words.remove(0) } else { format!("({})", words.join(" AND ")) };
        let expr = match field.column() {
            Some(col) => format!("{col} : {body}"),
            None => body,
        };
        res.tokens.push((field.name().to_string(), value.clone()));
        parts.push(if neg { format!("NOT {expr}") } else { expr });
    }
    // FTS5 cannot start with NOT; move positives first.
    parts.sort_by_key(|p| p.starts_with("NOT "));
    if parts.first().map(|p| p.starts_with("NOT ")).unwrap_or(false) {
        parts.clear();
    }
    let mut fts = String::new();
    for p in parts {
        if fts.is_empty() {
            fts = p;
        } else if let Some(n) = p.strip_prefix("NOT ") {
            fts = format!("{fts} NOT {n}");
        } else {
            fts = format!("{fts} AND {p}");
        }
    }
    res.fts = fts;
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Address, MailboxRole, MessageSummary};
    use crate::core::store::{MessageQuery, Store};

    #[test]
    fn parse_fields() {
        let p = parse("from:alice lunch is:unread", SearchField::Entire);
        assert!(p.unread);
        assert!(p.fts.contains("sender : \"alice\"*"));
        assert!(p.fts.contains("\"lunch\"*"));
    }

    #[test]
    fn query_works_in_sqlite() {
        let s = Store::open_in_memory().unwrap();
        let mb = s.ensure_mailbox("a", "INBOX", MailboxRole::Inbox).unwrap();
        let m = MessageSummary { mailbox_id: mb, account_id: "a".into(), subject: "Quarterly report".into(), from: Address::new("Alice Smith", "alice@corp.com"), ..Default::default() };
        s.insert_message(&m, None, "numbers look great").unwrap();
        for q in ["from:alice", "report", "from:alice -spam", "subject:\"quarterly report\"", "alice@corp.com", "numb", "(weird \" input*"] {
            let p = parse(q, SearchField::Entire);
            let r = s.list_messages(&MessageQuery { fts: Some(p.fts.clone()), ..Default::default() });
            if q.starts_with("(weird") {
                continue;
            }
            assert_eq!(r.len(), 1, "query {q} -> {}", p.fts);
        }
        let p = parse("from:bob", SearchField::Entire);
        assert!(s.list_messages(&MessageQuery { fts: Some(p.fts), ..Default::default() }).is_empty());
    }
}
