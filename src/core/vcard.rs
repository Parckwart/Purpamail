//! vCard (RFC 6350 / 2426) import and export for contacts.
//!
//! [`parse`] reads vCard 2.1, 3.0 and 4.0 (several cards per file, folded lines,
//! grouped properties such as `item1.EMAIL`, quoted-printable values with soft
//! line breaks, `CHARSET` parameters, escaped text and embedded photos given as
//! base64 or `data:` URIs). [`to_vcard`] writes vCard 3.0 with proper escaping
//! and 75-octet line folding.

use base64::Engine;

use super::ics::{Property, fold_line, parse_line};
use super::store::Contact;

/// Property name used to round-trip the VIP flag.
const VIP_PROPERTY: &str = "X-PURPAMAIL-VIP";

/// Split a structured value at unescaped `sep` characters and unescape each component.
pub fn split_components(value: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => cur.push('\n'),
                Some(o) => cur.push(o),
                None => {}
            }
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Unescape a vCard TEXT value.
pub fn unescape(value: &str) -> String {
    split_components(value, '\u{0}').join("")
}

/// Escape a value for a vCard TEXT property.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

fn decode_quoted_printable(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
            // A trailing '=' is a soft line break remnant; other stray '=' are kept.
            if i + 1 == bytes.len() {
                break;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn decode_charset(bytes: Vec<u8>, charset: Option<&str>) -> String {
    let latin1 = |b: &[u8]| b.iter().map(|&c| c as char).collect::<String>();
    match charset.map(|c| c.to_ascii_lowercase()) {
        Some(c) if c.contains("8859") || c.contains("latin") || c.contains("1252") => latin1(&bytes),
        _ => String::from_utf8(bytes).unwrap_or_else(|e| latin1(e.as_bytes())),
    }
}

fn is_qp(p: &Property) -> bool {
    p.param("ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("QUOTED-PRINTABLE")) || p.params.iter().any(|(k, _)| k.eq_ignore_ascii_case("QUOTED-PRINTABLE"))
}

fn is_base64(p: &Property) -> bool {
    p.param("ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("b") || e.eq_ignore_ascii_case("base64"))
        || p.params.iter().any(|(k, _)| k.eq_ignore_ascii_case("BASE64"))
}

/// Unfold vCard lines: RFC folding (leading whitespace) and quoted-printable soft breaks.
fn logical_lines(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut qp_continues = false;
    for raw in s.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if qp_continues && let Some(last) = out.last_mut() {
            last.pop(); // the soft-break '='
            last.push_str(raw.trim_start());
            qp_continues = last.ends_with('=') && line_is_qp(last);
            continue;
        }
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !out.is_empty() {
            let last = out.last_mut().unwrap();
            last.push_str(&raw[1..]);
        } else if raw.trim().is_empty() {
            continue;
        } else {
            out.push(raw.to_string());
        }
        let last = out.last().unwrap();
        qp_continues = last.ends_with('=') && line_is_qp(last);
    }
    out
}

fn line_is_qp(line: &str) -> bool {
    let head = line.split(':').next().unwrap_or("");
    head.to_ascii_uppercase().contains("QUOTED-PRINTABLE")
}

/// Decoded text value of a property (quoted-printable and charset aware).
fn text_value(p: &Property) -> String {
    if is_qp(p) { decode_charset(decode_quoted_printable(&p.value), p.param("CHARSET")) } else { p.value.clone() }
}

fn has_type(p: &Property, t: &str) -> bool {
    p.params
        .iter()
        .any(|(k, v)| (k.eq_ignore_ascii_case("TYPE") && v.split(',').any(|x| x.trim().eq_ignore_ascii_case(t))) || (v.is_empty() && k.eq_ignore_ascii_case(t)))
}

fn is_preferred(p: &Property) -> bool {
    has_type(p, "pref") || p.param("PREF").is_some_and(|v| v.trim() == "1")
}

fn decode_photo(p: &Property) -> Option<Vec<u8>> {
    let v = p.value.trim();
    let engine = base64::engine::general_purpose::STANDARD;
    let clean = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if let Some(rest) = v.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if meta.to_ascii_lowercase().ends_with(";base64") {
            return engine.decode(clean(data)).ok().filter(|d| !d.is_empty());
        }
        return None;
    }
    if is_base64(p) {
        return engine.decode(clean(v)).ok().filter(|d| !d.is_empty());
    }
    None
}

fn add_unique(list: &mut Vec<String>, value: String, front: bool) {
    let v = value.trim().to_string();
    if v.is_empty() || list.iter().any(|x| x.eq_ignore_ascii_case(&v)) {
        return;
    }
    if front { list.insert(0, v) } else { list.push(v) }
}

pub fn parse(s: &str) -> Vec<Contact> {
    let mut out = Vec::new();
    let mut cur: Option<Contact> = None;
    let mut fn_name = String::new();
    let mut has_n = false;
    for line in logical_lines(s) {
        let Some(mut p) = parse_line(&line) else { continue };
        // Strip a property group ("item1.EMAIL").
        if let Some((_, name)) = p.name.rsplit_once('.') {
            p.name = name.to_string();
        }
        match p.name.as_str() {
            "BEGIN" if p.value.trim().eq_ignore_ascii_case("VCARD") => {
                cur = Some(Contact::default());
                fn_name.clear();
                has_n = false;
                continue;
            }
            "END" if p.value.trim().eq_ignore_ascii_case("VCARD") => {
                if let Some(mut c) = cur.take() {
                    if !has_n && !fn_name.is_empty() && fn_name != c.organization {
                        let name = fn_name.trim();
                        match name.rsplit_once(' ') {
                            Some((first, last)) => {
                                c.first_name = first.trim().to_string();
                                c.last_name = last.trim().to_string();
                            }
                            None => c.first_name = name.to_string(),
                        }
                    }
                    let empty = c.first_name.is_empty() && c.last_name.is_empty() && c.organization.is_empty() && c.emails.is_empty() && c.phones.is_empty();
                    if !empty {
                        out.push(c);
                    }
                }
                continue;
            }
            _ => {}
        }
        let Some(c) = cur.as_mut() else { continue };
        match p.name.as_str() {
            "N" => {
                let parts = split_components(&text_value(&p), ';');
                let get = |i: usize| parts.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
                let mut first = get(1);
                let additional = get(2);
                if !additional.is_empty() {
                    first = format!("{first} {additional}").trim().to_string();
                }
                if !first.is_empty() || !get(0).is_empty() {
                    c.first_name = first;
                    c.last_name = get(0);
                    has_n = true;
                }
            }
            "FN" => fn_name = unescape(&text_value(&p)).trim().to_string(),
            "NICKNAME" => c.nickname = split_components(&text_value(&p), ',').into_iter().next().unwrap_or_default().trim().to_string(),
            "ORG" => {
                c.organization = split_components(&text_value(&p), ';').into_iter().map(|s| s.trim().to_string()).find(|s| !s.is_empty()).unwrap_or_default()
            }
            "EMAIL" => {
                let v = unescape(&text_value(&p));
                let v = v.trim().trim_start_matches("mailto:").to_string();
                add_unique(&mut c.emails, v, is_preferred(&p));
            }
            "TEL" => {
                let v = unescape(&text_value(&p));
                let v = v.trim().trim_start_matches("tel:").to_string();
                add_unique(&mut c.phones, v, is_preferred(&p));
            }
            "NOTE" => {
                let note = unescape(&text_value(&p));
                if c.notes.is_empty() {
                    c.notes = note;
                } else {
                    c.notes.push('\n');
                    c.notes.push_str(&note);
                }
            }
            "PHOTO" => {
                if c.photo.is_none() {
                    c.photo = decode_photo(&p);
                }
            }
            n if n == VIP_PROPERTY => c.vip = p.value.trim().eq_ignore_ascii_case("TRUE"),
            _ => {}
        }
    }
    out
}

fn photo_type(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "PNG"
    } else if data.starts_with(b"GIF8") {
        "GIF"
    } else {
        "JPEG"
    }
}

pub fn to_vcard(c: &Contact) -> String {
    let mut lines: Vec<String> = vec!["BEGIN:VCARD".into(), "VERSION:3.0".into()];
    lines.push(format!("N:{};{};;;", escape(&c.last_name), escape(&c.first_name)));
    lines.push(format!("FN:{}", escape(&c.display_name())));
    if !c.nickname.is_empty() {
        lines.push(format!("NICKNAME:{}", escape(&c.nickname)));
    }
    if !c.organization.is_empty() {
        lines.push(format!("ORG:{}", escape(&c.organization)));
    }
    for (i, e) in c.emails.iter().enumerate() {
        let t = if i == 0 && c.emails.len() > 1 { "INTERNET,PREF" } else { "INTERNET" };
        lines.push(format!("EMAIL;TYPE={t}:{}", escape(e)));
    }
    for p in &c.phones {
        lines.push(format!("TEL;TYPE=VOICE:{}", escape(p)));
    }
    if !c.notes.is_empty() {
        lines.push(format!("NOTE:{}", escape(&c.notes)));
    }
    if let Some(photo) = c.photo.as_ref().filter(|p| !p.is_empty()) {
        lines.push(format!("PHOTO;ENCODING=b;TYPE={}:{}", photo_type(photo), base64::engine::general_purpose::STANDARD.encode(photo)));
    }
    if c.vip {
        lines.push(format!("{VIP_PROPERTY}:TRUE"));
    }
    lines.push("END:VCARD".into());
    lines.iter().map(|l| fold_line(l)).collect()
}

/// Serialize all contacts into a single vCard file.
pub fn export_all(contacts: &[Contact]) -> String {
    contacts.iter().map(to_vcard).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vcard3_with_photo_and_escapes() {
        let s = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Berger;Anna;Maria;;\r\nFN:Anna Maria Berger\r\nNICKNAME:Anni,AB\r\nORG:Fern & Co\\, Ltd;Design\r\n\
item1.EMAIL;TYPE=INTERNET:anna@work.example\r\nEMAIL;TYPE=INTERNET,PREF:anna@home.example\r\nTEL;TYPE=CELL:+49 151 2345678\r\n\
NOTE:Met at the conference\\nLikes tea\\; no sugar\r\nPHOTO;ENCODING=b;TYPE=PNG:iVBORw0K\r\n Ggo=\r\nEND:VCARD\r\n";
        let cs = parse(s);
        assert_eq!(cs.len(), 1);
        let c = &cs[0];
        assert_eq!(c.first_name, "Anna Maria");
        assert_eq!(c.last_name, "Berger");
        assert_eq!(c.nickname, "Anni");
        assert_eq!(c.organization, "Fern & Co, Ltd");
        assert_eq!(c.emails, vec!["anna@home.example".to_string(), "anna@work.example".to_string()]);
        assert_eq!(c.phones, vec!["+49 151 2345678".to_string()]);
        assert_eq!(c.notes, "Met at the conference\nLikes tea; no sugar");
        assert_eq!(c.photo.as_deref(), Some(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..]));
    }

    #[test]
    fn parses_vcard21_quoted_printable() {
        let s = "BEGIN:VCARD\nVERSION:2.1\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:M=C3=BCller;J=C3=BCrgen\nTEL;WORK;VOICE:030 1234567\n\
EMAIL;INTERNET:juergen@example.de\nNOTE;ENCODING=QUOTED-PRINTABLE:First line=0D=0Asecond=\n line continues\nEND:VCARD\n\
BEGIN:VCARD\nVERSION:2.1\nN;CHARSET=ISO-8859-1;ENCODING=QUOTED-PRINTABLE:Gr=FC=DFe;Eva\nEND:VCARD\n";
        let cs = parse(s);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].last_name, "Müller");
        assert_eq!(cs[0].first_name, "Jürgen");
        assert_eq!(cs[0].phones, vec!["030 1234567".to_string()]);
        assert_eq!(cs[0].emails, vec!["juergen@example.de".to_string()]);
        assert_eq!(cs[0].notes, "First line\r\nsecondline continues");
        assert_eq!(cs[1].last_name, "Grüße");
    }

    #[test]
    fn parses_vcard4_data_uri_and_fn_only() {
        let s = "BEGIN:VCARD\nVERSION:4.0\nFN:Tom Example\nEMAIL;PREF=1:tom@example.com\nTEL;VALUE=uri;TYPE=cell:tel:+1-555-123-4567\nPHOTO:data:image/png;base64,iVBORw0KGgo=\nEND:VCARD";
        let c = &parse(s)[0];
        assert_eq!(c.first_name, "Tom");
        assert_eq!(c.last_name, "Example");
        assert_eq!(c.phones, vec!["+1-555-123-4567".to_string()]);
        assert!(c.photo.is_some());
    }

    #[test]
    fn roundtrip() {
        let c = Contact {
            first_name: "Zoë".into(),
            last_name: "O'Neil; Jr".into(),
            nickname: "Zo".into(),
            organization: "Acme, Inc".into(),
            emails: vec!["zoe@example.com".into(), "z@example.org".into()],
            phones: vec!["+44 20 7946 0000".into()],
            notes: "Line one\nLine two with a very long text that definitely needs folding because it exceeds seventy five octets".into(),
            vip: true,
            photo: Some(vec![
                0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
                33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60,
            ]),
            ..Default::default()
        };
        let v = to_vcard(&c);
        for l in v.split("\r\n") {
            assert!(l.len() <= 75, "{l}");
        }
        assert!(v.contains("VERSION:3.0\r\n"));
        let back = parse(&v);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], c);
        let all = export_all(&[c.clone(), Contact { first_name: "B".into(), emails: vec!["b@x.org".into()], ..Default::default() }]);
        assert_eq!(parse(&all).len(), 2);
    }

    #[test]
    fn malformed_input_does_not_panic() {
        assert!(parse("").is_empty());
        let _ = parse("BEGIN:VCARD\nN;ENCODING=QUOTED-PRINTABLE:=\nPHOTO;ENCODING=b:!!!\nEND:VCARD");
        assert!(parse("BEGIN:VCARD\nPHOTO;ENCODING=b:!!!\nEND:VCARD").is_empty());
        assert!(parse("garbage\n:\n;;;\nEND:VCARD").is_empty());
        let _ = decode_quoted_printable("=4");
        let _ = decode_quoted_printable("=ZZ=");
    }
}
