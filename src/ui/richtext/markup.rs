//! The lightweight markup used to persist signatures and templates.
//! The syntax is documented in the parent module.

use std::fmt::Write as _;

use super::html::{parse_css_color, rgb_hex};
use super::model::*;

/// An open inline formatting marker.
#[derive(Clone, PartialEq, Debug)]
enum Mark {
    Size(FontSize),
    Color(Rgb),
    Highlight(Rgb),
    Bold,
    Italic,
    Underline,
    Strike,
    Mono,
}

impl Mark {
    fn open(&self) -> String {
        match self {
            Mark::Size(f) => format!("{{size={}}}", f.keyword()),
            Mark::Color(c) => format!("{{color={}}}", rgb_hex(*c)),
            Mark::Highlight(c) => format!("{{highlight={}}}", rgb_hex(*c)),
            Mark::Bold => "**".into(),
            Mark::Italic => "*".into(),
            Mark::Underline => "__".into(),
            Mark::Strike => "~~".into(),
            Mark::Mono => "`".into(),
        }
    }
    fn close(&self) -> &'static str {
        match self {
            Mark::Size(_) => "{/size}",
            Mark::Color(_) => "{/color}",
            Mark::Highlight(_) => "{/highlight}",
            Mark::Bold => "**",
            Mark::Italic => "*",
            Mark::Underline => "__",
            Mark::Strike => "~~",
            Mark::Mono => "`",
        }
    }
    fn apply(&self, s: &mut Style) {
        match self {
            Mark::Size(f) => s.size = *f,
            Mark::Color(c) => s.color = Some(*c),
            Mark::Highlight(c) => s.highlight = Some(*c),
            Mark::Bold => s.bold = true,
            Mark::Italic => s.italic = true,
            Mark::Underline => s.underline = true,
            Mark::Strike => s.strike = true,
            Mark::Mono => s.mono = true,
        }
    }
}

/// The marks needed to express `st` on top of `base`, in canonical order.
fn marks_for(st: &Style, base: &Style) -> Vec<Mark> {
    let mut v = Vec::new();
    if st.size != base.size {
        v.push(Mark::Size(st.size));
    }
    if let Some(c) = st.color.filter(|_| st.color != base.color) {
        v.push(Mark::Color(c));
    }
    if let Some(c) = st.highlight.filter(|_| st.highlight != base.highlight) {
        v.push(Mark::Highlight(c));
    }
    for (on, b, m) in [
        (st.bold, base.bold, Mark::Bold),
        (st.italic, base.italic, Mark::Italic),
        (st.underline, base.underline, Mark::Underline),
        (st.strike, base.strike, Mark::Strike),
        (st.mono, base.mono, Mark::Mono),
    ] {
        if on && !b {
            v.push(m);
        }
    }
    v
}

const SPECIAL: &[char] = &['\\', '*', '`', '[', ']', '{', '}'];

fn escape_into(out: &mut String, text: &str) {
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        // `_` and `~` only matter when doubled
        let doubled = (c == '_' || c == '~') && (chars.get(i + 1) == Some(&c) || (i > 0 && chars[i - 1] == c));
        if SPECIAL.contains(&c) || doubled {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Would this paragraph content be mistaken for a paragraph prefix?
fn needs_line_escape(s: &str) -> bool {
    if s.starts_with("# ") || s.starts_with("## ") || s.starts_with("- ") || s.starts_with('>') || s.starts_with("{center} ") || s.starts_with("{right} ") {
        return true;
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && s[digits..].starts_with(". ")
}

/// Serialize a document to markup.
pub fn to_markup(doc: &RichDoc) -> String {
    let mut out = String::new();
    let ranges = doc.para_ranges();
    let mut number = 0;
    for (i, r) in ranges.iter().enumerate() {
        let p = doc.paras()[i];
        if i > 0 {
            out.push('\n');
        }
        for _ in 0..p.quote {
            out.push_str("> ");
        }
        match p.list {
            ListKind::Bullet => out.push_str("- "),
            ListKind::Numbered => {
                number += 1;
                let _ = write!(out, "{number}. ");
            }
            ListKind::None => {}
        }
        if p.list != ListKind::Numbered {
            number = 0;
        }
        match p.align {
            Align::Center => out.push_str("{center} "),
            Align::Right => out.push_str("{right} "),
            Align::Left => {}
        }
        // heading paragraphs
        let spans: Vec<(std::ops::Range<usize>, Style)> = doc
            .spans()
            .filter_map(|(sr, st)| {
                let a = sr.start.max(r.start);
                let b = sr.end.min(r.end);
                (a < b).then(|| (a..b, st.clone()))
            })
            .collect();
        let mut base = Style::default();
        if !spans.is_empty() {
            if spans.iter().all(|(_, s)| s.size == FontSize::Heading && s.bold) {
                out.push_str("# ");
                base.size = FontSize::Heading;
                base.bold = true;
            } else if spans.iter().all(|(_, s)| s.size == FontSize::Bigger && s.bold) {
                out.push_str("## ");
                base.size = FontSize::Bigger;
                base.bold = true;
            }
        }
        let mut body = String::new();
        let mut stack: Vec<Mark> = Vec::new();
        let mut link: Option<String> = None;
        for (sr, st) in &spans {
            if st.link != link {
                while let Some(m) = stack.pop() {
                    body.push_str(m.close());
                }
                if let Some(l) = &link {
                    let _ = write!(body, "]({})", l.replace(')', "%29").replace(' ', "%20"));
                }
                if st.link.is_some() {
                    body.push('[');
                }
                link = st.link.clone();
            }
            if let Some(cid) = &st.image {
                while let Some(m) = stack.pop() {
                    body.push_str(m.close());
                }
                let _ = write!(body, "![](cid:{cid})");
                continue;
            }
            let want = marks_for(st, &base);
            // pop until the stack is a prefix of `want`
            let keep = stack.iter().zip(want.iter()).take_while(|(a, b)| a == b).count();
            while stack.len() > keep {
                body.push_str(stack.pop().unwrap().close());
            }
            for m in &want[keep..] {
                body.push_str(&m.open());
                stack.push(m.clone());
            }
            escape_into(&mut body, doc.slice(sr.clone()));
        }
        while let Some(m) = stack.pop() {
            body.push_str(m.close());
        }
        if let Some(l) = &link {
            let _ = write!(body, "]({})", l.replace(')', "%29").replace(' ', "%20"));
        }
        if needs_line_escape(&body) {
            out.push('\\');
        }
        out.push_str(&body);
    }
    out
}

struct Parser {
    text: String,
    runs: Vec<Run>,
    paras: Vec<ParaAttr>,
}

impl Parser {
    fn push(&mut self, c: char, st: &Style) {
        self.text.push(c);
        match self.runs.last_mut() {
            Some(r) if r.style == *st => r.len += 1,
            _ => self.runs.push(Run { len: 1, style: st.clone() }),
        }
    }
}

fn starts(chars: &[char], i: usize, pat: &str) -> bool {
    let n = pat.chars().count();
    i + n <= chars.len() && chars[i..i + n].iter().copied().eq(pat.chars())
}

/// Find an unescaped occurrence of `pat` at or after `from`.
fn find_unescaped(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if starts(chars, i, pat) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_inline(p: &mut Parser, line: &str, base: &Style) {
    let chars: Vec<char> = line.chars().collect();
    let mut stack: Vec<Mark> = Vec::new();
    // (position of "](", url, index after ")")
    let mut link: Option<(usize, String, usize)> = None;
    let style = |stack: &[Mark], link: &Option<(usize, String, usize)>| {
        let mut s = base.clone();
        for m in stack {
            m.apply(&mut s);
        }
        s.link = link.as_ref().map(|l| l.1.clone());
        s
    };
    let is_open = |stack: &[Mark], m: &Mark| stack.iter().any(|x| std::mem::discriminant(x) == std::mem::discriminant(m));
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((end, _, after)) = &link
            && i == *end
        {
            i = *after;
            link = None;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            let st = style(&stack, &link);
            p.push(chars[i + 1], &st);
            i += 2;
            continue;
        }
        // inline image
        if starts(&chars, i, "![](")
            && let Some(close) = find_unescaped(&chars, i + 4, ")")
        {
            let target: String = chars[i + 4..close].iter().collect();
            if let Some(cid) = target.strip_prefix("cid:") {
                p.push(
                    OBJ,
                    &Style {
                        image: Some(cid.to_string()),
                        ..Default::default()
                    },
                );
            }
            i = close + 1;
            continue;
        }
        // link
        if c == '['
            && link.is_none()
            && let Some(mid) = find_unescaped(&chars, i + 1, "](")
            && let Some(close) = find_unescaped(&chars, mid + 2, ")")
        {
            let url: String = chars[mid + 2..close].iter().collect::<String>().replace("%29", ")").replace("%20", " ");
            link = Some((mid, url, close + 1));
            i += 1;
            continue;
        }
        // value marks
        if c == '{' {
            let mut handled = false;
            for (kw, close_kw) in [("{size=", "{/size}"), ("{color=", "{/color}"), ("{highlight=", "{/highlight}")] {
                if starts(&chars, i, kw) {
                    if let Some(end) = find_unescaped(&chars, i, "}") {
                        let val: String = chars[i + kw.chars().count()..end].iter().collect();
                        let m = match kw {
                            "{size=" => FontSize::from_keyword(&val).map(Mark::Size),
                            "{color=" => parse_css_color(&val).map(Mark::Color),
                            _ => parse_css_color(&val).map(Mark::Highlight),
                        };
                        if let Some(m) = m
                            && find_unescaped(&chars, end, close_kw).is_some()
                        {
                            stack.push(m);
                            i = end + 1;
                            handled = true;
                        }
                    }
                    break;
                }
                if starts(&chars, i, close_kw) {
                    let target = match close_kw {
                        "{/size}" => Mark::Size(FontSize::Normal),
                        "{/color}" => Mark::Color([0; 3]),
                        _ => Mark::Highlight([0; 3]),
                    };
                    if let Some(pos) = stack.iter().rposition(|x| std::mem::discriminant(x) == std::mem::discriminant(&target)) {
                        stack.remove(pos);
                        i += close_kw.chars().count();
                        handled = true;
                    }
                    break;
                }
            }
            if handled {
                continue;
            }
        }
        // flag marks: close the top of the stack first, then open
        let flag = [("**", Mark::Bold), ("__", Mark::Underline), ("~~", Mark::Strike), ("*", Mark::Italic), ("`", Mark::Mono)];
        if let Some(top) = stack.last()
            && let Some((pat, _)) = flag.iter().find(|(pat, m)| m == top && starts(&chars, i, pat))
        {
            stack.pop();
            i += pat.len();
            continue;
        }
        let mut opened = false;
        for (pat, m) in &flag {
            if starts(&chars, i, pat) && !is_open(&stack, m) {
                // only a marker if the closing counterpart follows in this paragraph
                let after = i + pat.len();
                if after < chars.len() && !chars[after].is_whitespace() && find_unescaped(&chars, after + 1, pat).is_some() {
                    stack.push(m.clone());
                    i = after;
                    opened = true;
                }
                break;
            }
        }
        if opened {
            continue;
        }
        let st = style(&stack, &link);
        p.push(c, &st);
        i += 1;
    }
}

/// Parse markup into a document.
pub fn from_markup(src: &str) -> RichDoc {
    let src = src.replace("\r\n", "\n");
    let mut p = Parser {
        text: String::new(),
        runs: Vec::new(),
        paras: Vec::new(),
    };
    for (li, line) in src.split('\n').enumerate() {
        if li > 0 {
            p.push('\n', &Style::default());
        }
        let mut rest = line;
        let mut attr = ParaAttr::default();
        while let Some(r) = rest.strip_prefix('>') {
            attr.quote = attr.quote.saturating_add(1);
            rest = r.strip_prefix(' ').unwrap_or(r);
        }
        if let Some(r) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("• ")) {
            attr.list = ListKind::Bullet;
            rest = r;
        } else {
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && rest[digits..].starts_with(". ") {
                attr.list = ListKind::Numbered;
                rest = &rest[digits + 2..];
            }
        }
        if let Some(r) = rest.strip_prefix("{center} ") {
            attr.align = Align::Center;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("{right} ") {
            attr.align = Align::Right;
            rest = r;
        }
        let mut base = Style::default();
        if let Some(r) = rest.strip_prefix("# ") {
            base.size = FontSize::Heading;
            base.bold = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("## ") {
            base.size = FontSize::Bigger;
            base.bold = true;
            rest = r;
        }
        p.paras.push(attr);
        parse_inline(&mut p, rest, &base);
    }
    RichDoc::from_parts(p.text, p.runs, p.paras)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(d: &RichDoc) -> RichDoc {
        let m = to_markup(d);
        let back = from_markup(&m);
        assert_eq!(back.text(), d.text(), "markup: {m}");
        assert_eq!(back.paras(), d.paras(), "markup: {m}");
        assert_eq!(back.runs(), d.runs(), "markup: {m}");
        back
    }

    #[test]
    fn parse_template_style() {
        let d = from_markup("# Happy Birthday!\n\nWishing you a **wonderful** day *full* of joy.\n- one\n1. first\n> quoted [link](https://x.org)");
        d.check().unwrap();
        assert_eq!(d.text(), "Happy Birthday!\n\nWishing you a wonderful day full of joy.\none\nfirst\nquoted link");
        assert!(d.style_at(0).bold && d.style_at(0).size == FontSize::Heading);
        let t = d.text();
        assert!(d.style_at(t.find("wonderful").unwrap()).bold);
        assert!(d.style_at(t.find("full").unwrap()).italic);
        assert_eq!(d.paras()[3].list, ListKind::Bullet);
        assert_eq!(d.paras()[4].list, ListKind::Numbered);
        assert_eq!(d.paras()[5].quote, 1);
        assert_eq!(d.style_at(t.chars().count() - 1).link.as_deref(), Some("https://x.org"));
    }

    #[test]
    fn unmatched_markers_are_literal() {
        let d = from_markup("2 * 3 = 6 and snake_case and a ** b");
        assert_eq!(d.text(), "2 * 3 = 6 and snake_case and a ** b");
        assert!(d.is_unformatted());
        let d = from_markup("[not a link] {color=nope}x");
        assert_eq!(d.text(), "[not a link] {color=nope}x");
    }

    #[test]
    fn round_trip_all_attributes() {
        let mut d = RichDoc::plain("Big title\nbold italic both under strike mono\ncolored and highlighted text\nlink here\n- not a list\n3. item\ncentered\nright");
        d.toggle(0, 9, &Attr::Size(FontSize::Heading));
        d.toggle(0, 9, &Attr::Bold);
        d.toggle(10, 14, &Attr::Bold);
        d.toggle(15, 21, &Attr::Italic);
        d.toggle(22, 26, &Attr::Bold);
        d.toggle(22, 26, &Attr::Italic);
        d.toggle(27, 32, &Attr::Underline);
        d.toggle(33, 39, &Attr::Strike);
        d.toggle(40, 44, &Attr::Mono);
        d.toggle(45, 52, &Attr::Color(Some([0xe0, 0x3b, 0x3b])));
        d.toggle(57, 68, &Attr::Highlight(Some([0xff, 0xf6, 0xdb])));
        d.toggle(57, 62, &Attr::Size(FontSize::Smaller));
        d.update_styles(74, 78, |s| s.link = Some("https://example.com/a b)".into()));
        d.toggle_list(97, 97, ListKind::Numbered);
        d.update_paras(105, 105, |p| p.align = Align::Center);
        d.update_paras(115, 115, |p| p.align = Align::Right);
        d.indent(115, 115, 2);
        rt(&d);
    }

    #[test]
    fn round_trip_escapes() {
        let d = RichDoc::plain("Use *stars*, __under__, `ticks`, [brackets](x) and {braces}\n# not heading\n> not quote\n12. not list\n\\backslash");
        rt(&d);
        let m = to_markup(&d);
        assert!(m.contains("\\*stars\\*"));
    }

    #[test]
    fn round_trip_adjacent_styles() {
        let mut d = RichDoc::plain("abcdef");
        d.toggle(0, 4, &Attr::Bold);
        d.toggle(2, 6, &Attr::Italic);
        rt(&d);
        let mut d = RichDoc::plain("link bold");
        d.update_styles(0, 9, |s| s.link = Some("https://x".into()));
        d.toggle(5, 9, &Attr::Bold);
        rt(&d);
        let mut d = RichDoc::plain("x\u{FFFC}y");
        d.update_styles(1, 2, |s| s.image = Some("img1@purpamail".into()));
        rt(&d);
    }

    #[test]
    fn plain_signature_text_survives() {
        let s = "Jane Doe\nACME Corp. — +49 123 456\nhttps://acme.example";
        assert_eq!(from_markup(s).text(), s);
        assert_eq!(to_markup(&RichDoc::plain(s)), s);
    }
}
