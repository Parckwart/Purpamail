//! Conversion between [`RichDoc`] and HTML / plain text.

use std::fmt::Write as _;

use scraper::{ElementRef, Html, Node};

use super::model::*;
use crate::core::mime::{escape_html, html_to_text};

/// CSS color of the quote bar in generated HTML (the Purpamail accent).
const QUOTE_BAR: &str = "#7b4fd6";

pub fn rgb_hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Parse `#rgb`, `#rrggbb`, `rgb(r, g, b)` and a few color names.
pub fn parse_css_color(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
    if let Some(h) = s.strip_prefix('#') {
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        return match h.len() {
            3 => Some([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17]),
            6 => Some([v(0, 2)?, v(2, 2)?, v(4, 2)?]),
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgb(").or_else(|| s.strip_prefix("rgba(")).and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<u8> = inner.split(',').take(3).filter_map(|p| p.trim().parse::<f32>().ok().map(|f| f.clamp(0.0, 255.0) as u8)).collect();
        return (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]]);
    }
    Some(match s.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [0xff, 0, 0],
        "green" => [0, 0x80, 0],
        "blue" => [0, 0, 0xff],
        "gray" | "grey" => [0x80, 0x80, 0x80],
        "purple" => [0x80, 0, 0x80],
        "orange" => [0xff, 0xa5, 0],
        "yellow" => [0xff, 0xff, 0],
        "navy" => [0, 0, 0x80],
        "maroon" => [0x80, 0, 0],
        "teal" => [0, 0x80, 0x80],
        _ => return None,
    })
}

// ------------------------------------------------------------------ to HTML

fn span_html(out: &mut String, text: &str, st: &Style) {
    if let Some(cid) = &st.image {
        let _ = write!(out, "<img src=\"cid:{}\" alt=\"\" style=\"max-width:100%\">", escape_html(cid));
        return;
    }
    let mut css = String::new();
    if st.size != FontSize::Normal {
        let _ = write!(css, "font-size:{}%;", (st.size.scale() * 100.0).round());
    }
    if let Some(c) = st.color {
        let _ = write!(css, "color:{};", rgb_hex(c));
    }
    if let Some(c) = st.highlight {
        let _ = write!(css, "background-color:{};", rgb_hex(c));
    }
    if st.mono {
        css.push_str("font-family:monospace;");
    }
    let mut open = String::new();
    let mut close = String::new();
    if !css.is_empty() {
        let _ = write!(open, "<span style=\"{css}\">");
        close.insert_str(0, "</span>");
    }
    for (on, tag) in [(st.bold, "b"), (st.italic, "i"), (st.underline, "u"), (st.strike, "s")] {
        if on {
            let _ = write!(open, "<{tag}>");
            close.insert_str(0, &format!("</{tag}>"));
        }
    }
    out.push_str(&open);
    // preserve runs of spaces
    let mut prev_space = false;
    for c in text.chars() {
        match c {
            ' ' if prev_space => {
                out.push_str("&nbsp;");
                continue;
            }
            '\t' => out.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
        prev_space = c == ' ';
    }
    out.push_str(&close);
}

/// Inline HTML for the char range `r` (a single paragraph).
fn inline_html(doc: &RichDoc, r: std::ops::Range<usize>) -> String {
    let mut out = String::new();
    let mut cur_link: Option<String> = None;
    for (sr, st) in doc.spans() {
        let a = sr.start.max(r.start);
        let b = sr.end.min(r.end);
        if a >= b {
            continue;
        }
        if st.link != cur_link {
            if cur_link.is_some() {
                out.push_str("</a>");
            }
            if let Some(l) = &st.link {
                let _ = write!(out, "<a href=\"{}\">", escape_html(l));
            }
            cur_link = st.link.clone();
        }
        span_html(&mut out, doc.slice(a..b), st);
    }
    if cur_link.is_some() {
        out.push_str("</a>");
    }
    out
}

fn align_css(a: Align) -> &'static str {
    match a {
        Align::Left => "",
        Align::Center => "text-align:center;",
        Align::Right => "text-align:right;",
    }
}

/// Body HTML (no `<html>` wrapper) with inline styles.
pub fn to_html_fragment(doc: &RichDoc) -> String {
    let mut out = String::new();
    let ranges = doc.para_ranges();
    let mut depth = 0u8;
    let mut list: ListKind = ListKind::None;
    let close_list = |out: &mut String, list: &mut ListKind| {
        match list {
            ListKind::Bullet => out.push_str("</ul>"),
            ListKind::Numbered => out.push_str("</ol>"),
            ListKind::None => {}
        }
        *list = ListKind::None;
    };
    for (i, r) in ranges.iter().enumerate() {
        let p = doc.paras()[i];
        if list != ListKind::None && (p.list != list || p.quote != depth) {
            close_list(&mut out, &mut list);
        }
        while depth > p.quote {
            out.push_str("</blockquote>");
            depth -= 1;
        }
        while depth < p.quote {
            let _ = write!(out, "<blockquote type=\"cite\" style=\"margin:0 0 0 0.8ex;border-left:2px solid {QUOTE_BAR};padding-left:1ex\">");
            depth += 1;
        }
        let content = inline_html(doc, r.clone());
        let content = if content.is_empty() { "<br>".to_string() } else { content };
        let style = align_css(p.align);
        let style_attr = if style.is_empty() { String::new() } else { format!(" style=\"{style}\"") };
        match p.list {
            ListKind::None => {
                let _ = write!(out, "<div{style_attr}>{content}</div>");
            }
            kind => {
                if list != kind {
                    out.push_str(if kind == ListKind::Bullet { "<ul style=\"margin:0;padding-left:1.6em\">" } else { "<ol style=\"margin:0;padding-left:1.6em\">" });
                    list = kind;
                }
                let _ = write!(out, "<li{style_attr}>{content}</li>");
            }
        }
    }
    close_list(&mut out, &mut list);
    while depth > 0 {
        out.push_str("</blockquote>");
        depth -= 1;
    }
    out
}

/// A complete HTML document for an email body.
pub fn to_html_document(doc: &RichDoc, font_px: f32, background: Option<Rgb>) -> String {
    let mut body_css = format!("font-family:sans-serif;font-size:{}px;line-height:1.4;", font_px.round());
    if let Some(bg) = background {
        let _ = write!(body_css, "background-color:{};", rgb_hex(bg));
    }
    format!(
        "<!DOCTYPE html><html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body style=\"{body_css}\">{}</body></html>",
        to_html_fragment(doc)
    )
}

// ------------------------------------------------------------------ to plain text

/// Plain text rendering: quotes as `> `, bullets as `• `, numbers as `1. `,
/// links as `text <url>`.
pub fn to_plain(doc: &RichDoc) -> String {
    let mut out = String::new();
    let ranges = doc.para_ranges();
    let mut number = 0;
    for (i, r) in ranges.iter().enumerate() {
        let p = doc.paras()[i];
        if i > 0 {
            out.push('\n');
        }
        if p.quote > 0 {
            out.push_str(&">".repeat(p.quote as usize));
            out.push(' ');
        }
        match p.list {
            ListKind::Bullet => out.push_str("• "),
            ListKind::Numbered => {
                number += 1;
                let _ = write!(out, "{number}. ");
            }
            ListKind::None => {}
        }
        if p.list != ListKind::Numbered {
            number = 0;
        }
        // text with links
        let mut cur_link: Option<(String, String)> = None; // (url, text so far)
        let flush = |out: &mut String, l: Option<(String, String)>| {
            if let Some((url, text)) = l {
                let bare = url.strip_prefix("mailto:").unwrap_or(&url);
                if text.trim() != url && text.trim() != bare {
                    let _ = write!(out, " <{url}>");
                }
            }
        };
        for (sr, st) in doc.spans() {
            let a = sr.start.max(r.start);
            let b = sr.end.min(r.end);
            if a >= b {
                continue;
            }
            let t = doc.slice(a..b);
            if cur_link.as_ref().map(|c| &c.0) != st.link.as_ref() {
                flush(&mut out, cur_link.take());
                cur_link = st.link.clone().map(|u| (u, String::new()));
            }
            if st.image.is_some() {
                continue;
            }
            out.push_str(t);
            if let Some(c) = &mut cur_link {
                c.1.push_str(t);
            }
        }
        flush(&mut out, cur_link.take());
    }
    out
}

// ------------------------------------------------------------------ from plain text

/// Split a leading quote prefix (`> `, `>> `, `> > `) off a line.
pub fn split_quote_prefix(line: &str) -> (u8, &str) {
    let mut level = 0u8;
    let mut rest = line;
    loop {
        if let Some(r) = rest.strip_prefix('>') {
            level = level.saturating_add(1);
            rest = r;
        } else if level > 0 && rest.starts_with(" >") {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    if level > 0 {
        rest = rest.strip_prefix(' ').unwrap_or(rest);
    }
    (level, rest)
}

/// Build a document from plain text; `>` quoted lines become quote levels.
pub fn from_plain(text: &str) -> RichDoc {
    let text = text.replace("\r\n", "\n");
    let mut out = String::new();
    let mut paras = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (q, rest) = split_quote_prefix(line);
        out.push_str(rest);
        paras.push(ParaAttr { quote: q, ..Default::default() });
    }
    RichDoc::from_parts(out, vec![], paras)
}

// ------------------------------------------------------------------ from HTML

struct Builder {
    text: String,
    runs: Vec<Run>,
    paras: Vec<ParaAttr>,
    /// The current paragraph has no content yet.
    para_empty: bool,
    /// A collapsed whitespace run is waiting, with the style it had.
    pending_space: Option<Style>,
    /// Insert an empty paragraph before the next content (paragraph margins).
    blank_pending: bool,
}

impl Builder {
    fn new() -> Self {
        Builder { text: String::new(), runs: Vec::new(), paras: vec![ParaAttr::default()], para_empty: true, pending_space: None, blank_pending: false }
    }

    fn push_raw(&mut self, c: char, st: &Style) {
        self.text.push(c);
        match self.runs.last_mut() {
            Some(r) if r.style == *st => r.len += 1,
            _ => self.runs.push(Run { len: 1, style: st.clone() }),
        }
    }

    fn newline(&mut self, attr: ParaAttr) {
        let st = Style::default();
        self.push_raw('\n', &st);
        self.paras.push(attr);
        self.para_empty = true;
        self.pending_space = None;
    }

    fn start_content(&mut self, ctx: ParaAttr) {
        if self.para_empty {
            if self.blank_pending && !self.text.is_empty() {
                *self.paras.last_mut().unwrap() = ParaAttr { quote: ctx.quote, ..Default::default() };
                self.newline(ctx);
            }
            self.blank_pending = false;
            *self.paras.last_mut().unwrap() = ctx;
            self.para_empty = false;
        }
    }

    fn text(&mut self, s: &str, st: &Style, ctx: ParaAttr, pre: bool) {
        for c in s.chars() {
            if pre {
                if c == '\n' {
                    self.start_content(ctx);
                    self.newline(ctx);
                    continue;
                }
                self.start_content(ctx);
                self.push_raw(c, st);
                continue;
            }
            if c.is_whitespace() && c != '\u{a0}' {
                if !self.para_empty && self.pending_space.is_none() {
                    self.pending_space = Some(Style { image: None, ..st.clone() });
                }
                continue;
            }
            self.start_content(ctx);
            if let Some(sp) = self.pending_space.take() {
                // a space between two different links belongs to neither
                let sp = if sp.link.is_some() && sp.link != st.link { Style { link: None, ..sp } } else { sp };
                self.push_raw(' ', &sp);
            }
            self.push_raw(if c == '\u{a0}' { ' ' } else { c }, st);
        }
    }

    fn block_break(&mut self, ctx: ParaAttr) {
        if !self.para_empty {
            self.newline(ctx);
        }
        self.pending_space = None;
    }

    fn hard_break(&mut self, ctx: ParaAttr) {
        self.start_content(ctx);
        self.newline(ctx);
    }

    fn finish(mut self) -> RichDoc {
        // drop trailing empty paragraphs
        while self.text.ends_with('\n') {
            self.text.pop();
            self.paras.pop();
            if let Some(r) = self.runs.last_mut() {
                r.len -= 1;
                if r.len == 0 {
                    self.runs.pop();
                }
            }
        }
        RichDoc::from_parts(self.text, self.runs, self.paras)
    }
}

fn apply_css(css: &str, st: &mut Style, ctx: &mut ParaAttr) {
    for decl in css.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().to_ascii_lowercase();
        match k.as_str() {
            "font-weight" => st.bold = v == "bold" || v == "bolder" || v.parse::<u32>().map(|w| w >= 600).unwrap_or(false),
            "font-style" => st.italic = v.contains("italic") || v.contains("oblique"),
            "text-decoration" | "text-decoration-line" => {
                if v.contains("underline") {
                    st.underline = true;
                }
                if v.contains("line-through") {
                    st.strike = true;
                }
                if v == "none" {
                    st.underline = false;
                    st.strike = false;
                }
            }
            "color" => st.color = parse_css_color(&v).filter(|c| !is_default_ink(*c)),
            "background-color" | "background" => st.highlight = parse_css_color(&v).filter(|c| *c != [255, 255, 255]),
            "font-family" => {
                if v.contains("mono") || v.contains("courier") || v.contains("consol") {
                    st.mono = true;
                }
            }
            "font-size" => {
                if let Some(f) = css_size(&v) {
                    st.size = f;
                }
            }
            "text-align" => {
                ctx.align = match v.as_str() {
                    "center" => Align::Center,
                    "right" | "end" => Align::Right,
                    _ => Align::Left,
                }
            }
            _ => {}
        }
    }
}

/// Colors that just mean "normal text" and should follow the theme instead.
fn is_default_ink(c: Rgb) -> bool {
    c.iter().all(|&v| v < 0x40) && c.iter().max().unwrap() - c.iter().min().unwrap() < 0x18
}

fn css_size(v: &str) -> Option<FontSize> {
    let rel = if let Some(p) = v.strip_suffix('%') {
        p.trim().parse::<f32>().ok()? / 100.0
    } else if let Some(p) = v.strip_suffix("em").or_else(|| v.strip_suffix("rem")) {
        p.trim().parse::<f32>().ok()?
    } else if let Some(p) = v.strip_suffix("px") {
        p.trim().parse::<f32>().ok()? / 14.0
    } else if let Some(p) = v.strip_suffix("pt") {
        p.trim().parse::<f32>().ok()? / 10.5
    } else {
        match v {
            "x-small" | "xx-small" | "small" | "smaller" => 0.8,
            "medium" => 1.0,
            "large" | "larger" => 1.25,
            "x-large" | "xx-large" => 1.6,
            _ => return None,
        }
    };
    Some(if rel < 0.93 {
        FontSize::Smaller
    } else if rel < 1.12 {
        FontSize::Normal
    } else if rel < 1.45 {
        FontSize::Bigger
    } else {
        FontSize::Heading
    })
}

fn walk_children(b: &mut Builder, el: ElementRef<'_>, st: &Style, ctx: ParaAttr, pre: bool, list: ListKind) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => b.text(t, st, ctx, pre),
            Node::Element(_) => {
                if let Some(ce) = ElementRef::wrap(child) {
                    walk(b, ce, st, ctx, pre, list);
                }
            }
            _ => {}
        }
    }
}

fn walk(b: &mut Builder, node: ElementRef<'_>, st: &Style, ctx: ParaAttr, pre: bool, list: ListKind) {
    let e = node.value();
    {
        {
            let name = e.name().to_ascii_lowercase();
            let mut st = st.clone();
            let mut ctx = ctx;
            let mut pre = pre;
            let mut list = list;
            let mut block = false;
            let mut paragraph = false;
            match name.as_str() {
                "head" | "style" | "script" | "title" | "meta" | "link" | "noscript" | "template" | "xml" => return,
                "br" => {
                    b.hard_break(ctx);
                    return;
                }
                "img" => {
                    if let Some(cid) = e.attr("src").and_then(|s| s.strip_prefix("cid:")) {
                        b.start_content(ctx);
                        let ist = Style { image: Some(cid.to_string()), ..Default::default() };
                        b.push_raw(OBJ, &ist);
                    }
                    return;
                }
                "hr" => {
                    b.block_break(ctx);
                    return;
                }
                "b" | "strong" => st.bold = true,
                "i" | "em" | "cite" | "var" => st.italic = true,
                "u" | "ins" => st.underline = true,
                "s" | "strike" | "del" => st.strike = true,
                "code" | "tt" | "kbd" | "samp" => st.mono = true,
                "small" => st.size = FontSize::Smaller,
                "big" => st.size = FontSize::Bigger,
                "a" => {
                    if let Some(h) = e.attr("href").map(str::trim).filter(|h| !h.is_empty() && !h.starts_with('#') && !h.starts_with("javascript:")) {
                        st.link = Some(h.to_string());
                    }
                }
                "font" => {
                    if let Some(c) = e.attr("color").and_then(parse_css_color).filter(|c| !is_default_ink(*c)) {
                        st.color = Some(c);
                    }
                    if let Some(sz) = e.attr("size").and_then(|s| s.trim().parse::<i32>().ok()) {
                        st.size = match sz {
                            ..=2 => FontSize::Smaller,
                            3 => FontSize::Normal,
                            4 | 5 => FontSize::Bigger,
                            _ => FontSize::Heading,
                        };
                    }
                    if e.attr("face").is_some_and(|f| f.to_ascii_lowercase().contains("mono") || f.to_ascii_lowercase().contains("courier")) {
                        st.mono = true;
                    }
                }
                "h1" | "h2" => {
                    block = true;
                    paragraph = true;
                    st.size = FontSize::Heading;
                    st.bold = true;
                }
                "h3" | "h4" => {
                    block = true;
                    paragraph = true;
                    st.size = FontSize::Bigger;
                    st.bold = true;
                }
                "h5" | "h6" => {
                    block = true;
                    st.bold = true;
                }
                "p" => {
                    block = true;
                    paragraph = true;
                }
                "pre" => {
                    block = true;
                    pre = true;
                    st.mono = true;
                }
                "blockquote" => {
                    block = true;
                    ctx.quote = ctx.quote.saturating_add(1);
                    ctx.list = ListKind::None;
                    list = ListKind::None;
                }
                "ul" | "menu" => {
                    block = true;
                    list = ListKind::Bullet;
                }
                "ol" => {
                    block = true;
                    list = ListKind::Numbered;
                }
                "li" => {
                    block = true;
                    ctx.list = if list == ListKind::None { ListKind::Bullet } else { list };
                }
                "center" => {
                    block = true;
                    ctx.align = Align::Center;
                }
                "div" | "table" | "tr" | "section" | "article" | "header" | "footer" | "address" | "dd" | "dt" | "dl" | "figure" | "figcaption" | "main" | "nav" | "aside" | "tbody" | "thead" | "tfoot" | "caption" => {
                    block = true;
                }
                _ => {}
            }
            if let Some(a) = e.attr("align") {
                ctx.align = match a.to_ascii_lowercase().as_str() {
                    "center" => Align::Center,
                    "right" => Align::Right,
                    _ => ctx.align,
                };
            }
            if let Some(css) = e.attr("style") {
                apply_css(css, &mut st, &mut ctx);
            }
            if block {
                b.block_break(ctx);
            }
            walk_children(b, node, &st, ctx, pre, list);
            if name == "td" || name == "th" {
                if !b.para_empty && b.pending_space.is_none() {
                    b.pending_space = Some(Style::default());
                }
            }
            if block {
                b.block_break(ctx);
                if paragraph {
                    b.blank_pending = true;
                }
            }
        }
    }
}

/// Convert an existing HTML body into the document model (paragraphs, basic
/// inline formatting, links, lists, quotes). Falls back to the plain text
/// rendering when nothing usable comes out.
pub fn from_html_lite(html: &str) -> RichDoc {
    let dom = Html::parse_document(html);
    let mut b = Builder::new();
    walk_children(&mut b, dom.root_element(), &Style::default(), ParaAttr::default(), false, ListKind::None);
    let doc = b.finish();
    if doc.text().trim().is_empty() && !html.trim().is_empty() {
        let t = html_to_text(html);
        if !t.trim().is_empty() {
            return from_plain(&t);
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_inline_and_blocks() {
        let mut d = RichDoc::plain("Hello bold world\nsecond <line>");
        d.toggle(6, 10, &Attr::Bold);
        d.update_styles(11, 16, |s| s.link = Some("https://example.com/?a=1&b=2".into()));
        d.update_paras(17, 17, |p| p.align = Align::Center);
        let h = to_html_fragment(&d);
        assert_eq!(
            h,
            "<div>Hello <b>bold</b> <a href=\"https://example.com/?a=1&amp;b=2\">world</a></div><div style=\"text-align:center;\">second &lt;line&gt;</div>"
        );
    }

    #[test]
    fn html_lists_and_quotes_nest() {
        let mut d = RichDoc::plain("intro\none\ntwo\nquoted\n\nend");
        d.toggle_list(6, 13, ListKind::Bullet);
        d.indent(14, 21, 1);
        let h = to_html_fragment(&d);
        assert!(h.contains("<ul style=\"margin:0;padding-left:1.6em\"><li>one</li><li>two</li></ul>"), "{h}");
        assert!(h.contains("<blockquote type=\"cite\""));
        assert!(h.contains("<div>quoted</div><div><br></div></blockquote><div>end</div>"), "{h}");
        let doc = to_html_document(&d, 14.0, Some([255, 244, 230]));
        assert!(doc.starts_with("<!DOCTYPE html>") && doc.contains("background-color:#fff4e6"));
    }

    #[test]
    fn plain_rendering() {
        let mut d = RichDoc::plain("Visit site now\na\nb\nc\nquote");
        d.update_styles(6, 10, |s| s.link = Some("https://x.org".into()));
        d.toggle_list(15, 15, ListKind::Bullet);
        d.toggle_list(17, 19, ListKind::Numbered);
        d.indent(21, 21, 2);
        assert_eq!(to_plain(&d), "Visit site <https://x.org> now\n• a\n1. b\n2. c\n>> quote");
        // a link whose text is the URL is not repeated
        let mut d = RichDoc::plain("https://x.org");
        d.update_styles(0, 13, |s| s.link = Some("https://x.org".into()));
        assert_eq!(to_plain(&d), "https://x.org");
    }

    #[test]
    fn plain_round_trip_quotes() {
        let src = "Hi,\n\nOn Monday Bob wrote:\n> first\n>> nested\n> > also nested\n>back";
        let d = from_plain(src);
        assert_eq!(d.paras().iter().map(|p| p.quote).collect::<Vec<_>>(), vec![0, 0, 0, 1, 2, 2, 1]);
        assert_eq!(d.text(), "Hi,\n\nOn Monday Bob wrote:\nfirst\nnested\nalso nested\nback");
        assert_eq!(to_plain(&d), "Hi,\n\nOn Monday Bob wrote:\n> first\n>> nested\n>> also nested\n> back");
        assert_eq!(to_plain(&from_plain(&to_plain(&d))), to_plain(&d));
    }

    #[test]
    fn from_html_basic_structure() {
        let html = r#"<html><head><style>p{color:red}</style><title>x</title></head><body>
            <p>Hello <b>bold</b> and <i>italic</i>,
            <a href="https://example.com">a link</a>.</p>
            <ul><li>one</li><li>two</li></ul>
            <blockquote type="cite"><div>quoted <u>text</u></div><blockquote>deeper</blockquote></blockquote>
            <div style="text-align:center"><span style="color:#ff0000;font-weight:bold">red</span></div>
            line<br>break
        </body></html>"#;
        let d = from_html_lite(html);
        d.check().unwrap();
        let t = d.text();
        assert_eq!(t, "Hello bold and italic, a link.\n\none\ntwo\nquoted text\ndeeper\nred\nline\nbreak");
        let bold_pos = t.find("bold").unwrap();
        assert!(d.style_at(bold_pos).bold);
        assert!(d.style_at(t.find("italic").unwrap()).italic);
        assert_eq!(d.style_at(t.find("a link").unwrap() + 2).link.as_deref(), Some("https://example.com"));
        let paras = d.paras();
        assert_eq!(paras[2].list, ListKind::Bullet);
        assert_eq!(paras[3].list, ListKind::Bullet);
        assert_eq!(paras[4].quote, 1);
        assert_eq!(paras[5].quote, 2);
        assert_eq!(paras[6].align, Align::Center);
        let red = d.style_at(t.chars().count() - "red\nline\nbreak".len());
        assert_eq!(red.color, Some([255, 0, 0]));
        assert!(red.bold);
    }

    #[test]
    fn html_round_trip_preserves_model() {
        let mut d = RichDoc::plain("Title\nSome bold, italic and mono text\nitem\nquoted line\ncentered");
        d.toggle(0, 5, &Attr::Size(FontSize::Heading));
        d.toggle(11, 15, &Attr::Bold);
        d.toggle(17, 23, &Attr::Italic);
        d.toggle(28, 32, &Attr::Mono);
        d.toggle(6, 10, &Attr::Color(Some([0x12, 0x80, 0x40])));
        d.toggle_list(39, 39, ListKind::Numbered);
        d.indent(44, 44, 1);
        d.update_paras(57, 57, |p| p.align = Align::Center);
        let back = from_html_lite(&to_html_document(&d, 14.0, None));
        assert_eq!(back.text(), d.text());
        assert_eq!(back.paras(), d.paras());
        let strip = |doc: &RichDoc| doc.spans().map(|(r, s)| (doc.slice(r).to_string(), s.clone())).filter(|(t, _)| !t.trim().is_empty()).collect::<Vec<_>>();
        assert_eq!(strip(&back), strip(&d));
    }

    #[test]
    fn from_html_falls_back_and_handles_images() {
        let d = from_html_lite("<p>see <img src=\"cid:abc@x\"> here</p>");
        assert_eq!(d.image_ids(), vec!["abc@x".to_string()]);
        assert!(d.text().contains(OBJ));
        let d = from_html_lite("plain words only");
        assert_eq!(d.text(), "plain words only");
        assert_eq!(parse_css_color("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_css_color("rgb(1, 2, 300)"), Some([1, 2, 255]));
        assert_eq!(parse_css_color("nonsense"), None);
    }

    #[test]
    fn split_quote_prefix_variants() {
        assert_eq!(split_quote_prefix("> a"), (1, "a"));
        assert_eq!(split_quote_prefix(">>a"), (2, "a"));
        assert_eq!(split_quote_prefix("> > a"), (2, "a"));
        assert_eq!(split_quote_prefix("a > b"), (0, "a > b"));
    }
}
