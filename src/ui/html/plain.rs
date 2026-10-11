//! Plain-text bodies as render trees: quote levels (`>` lines) become nested
//! quote blocks, long quoted sections become collapsible, the signature is
//! dimmed and links, addresses, phone numbers and dates become clickable.

use crate::core::datadetect::{self, DetectedKind};

use super::tree::{Block, Doc, Family, HAlign, Inline, Link, Para, TextSt};

/// Options for [`build_plain`].
#[derive(Clone, Debug, PartialEq)]
pub struct PlainOpts {
    pub size: f32,
    /// Use the fixed-width font.
    pub fixed: bool,
    /// Quoted sections at the top level with more lines than this are collapsed.
    pub collapse_over: usize,
}

/// URL scheme used for detected dates; the payload is the unix timestamp (or empty).
pub const DATE_SCHEME: &str = "x-purpamail-date:";

/// One parsed line: quote depth and the text after the quote markers.
#[derive(Clone, Debug, PartialEq)]
pub struct QuotedLine {
    pub level: usize,
    pub text: String,
    /// Inside the signature.
    pub sig: bool,
}

/// Split text into lines with quote levels; marks signature lines.
pub fn parse_lines(text: &str) -> Vec<QuotedLine> {
    let mut out = Vec::new();
    let mut in_sig = false;
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let mut level = 0;
        let mut rest = raw;
        loop {
            let t = rest.trim_start_matches(' ');
            if rest.len() - t.len() <= 3 && t.starts_with('>') {
                level += 1;
                rest = &t[1..];
            } else {
                break;
            }
        }
        if level > 0 {
            rest = rest.strip_prefix(' ').unwrap_or(rest);
        }
        if level == 0 && (raw == "-- " || raw == "--") {
            in_sig = true;
        }
        if level > 0 && in_sig {
            in_sig = false;
        }
        out.push(QuotedLine { level, text: rest.trim_end_matches('\r').to_string(), sig: in_sig && level == 0 });
    }
    // Drop trailing blank lines.
    while out.last().is_some_and(|l| l.text.trim().is_empty() && l.level == 0) {
        out.pop();
    }
    out
}

struct PlainBuilder<'a> {
    opts: &'a PlainOpts,
    links: Vec<Link>,
}

impl PlainBuilder<'_> {
    fn style(&self, dim: bool) -> TextSt {
        let mut st = TextSt::new(self.opts.size);
        if self.opts.fixed {
            st.family = Family::Mono;
        }
        st.dim = dim;
        st.line_height = Some(self.opts.size * 1.45);
        st
    }

    /// Text with data detectors applied.
    fn inlines(&mut self, text: &str, dim: bool, out: &mut Vec<Inline>) {
        let base = self.style(dim);
        let mut pos = 0;
        for d in datadetect::detect(text) {
            if d.start < pos || d.end > text.len() || !text.is_char_boundary(d.start) || !text.is_char_boundary(d.end) {
                continue;
            }
            push_text(out, &text[pos..d.start], &base);
            let shown = &text[d.start..d.end];
            let href = match &d.kind {
                DetectedKind::Url(u) => {
                    if u.to_ascii_lowercase().starts_with("www.") {
                        format!("https://{u}")
                    } else {
                        u.clone()
                    }
                }
                DetectedKind::Email(e) => format!("mailto:{e}"),
                DetectedKind::Phone(p) => format!("tel:{}", p.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect::<String>()),
                DetectedKind::Date(ts) => format!("{DATE_SCHEME}{}", ts.map(|t| t.to_string()).unwrap_or_default()),
            };
            self.links.push(Link { href, text: shown.to_string() });
            let mut st = base.clone();
            st.link = Some((self.links.len() - 1) as u32);
            st.underline = !matches!(d.kind, DetectedKind::Date(_) | DetectedKind::Phone(_));
            push_text(out, shown, &st);
            pos = d.end;
        }
        push_text(out, &text[pos..], &base);
    }

    fn para(&mut self, lines: &[&QuotedLine]) -> Option<Block> {
        if lines.is_empty() {
            return None;
        }
        let mut inlines = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            // Join soft-wrapped (format=flowed) lines that end with a space.
            let dim = lines[i].sig;
            let mut text = lines[i].text.clone();
            while text.ends_with(' ') && text != "-- " && i + 1 < lines.len() && lines[i + 1].sig == dim {
                i += 1;
                text.push_str(&lines[i].text);
            }
            if !inlines.is_empty() {
                inlines.push(Inline::Break);
            }
            self.inlines(text.trim_end(), dim, &mut inlines);
            i += 1;
        }
        Some(Block::Para(Para { inlines, align: HAlign::Left, pre: true }))
    }

    /// Build blocks for lines at quote depth >= `level`.
    fn blocks(&mut self, lines: &[QuotedLine], level: usize, start_index: usize) -> Vec<Block> {
        let mut out = Vec::new();
        let mut para: Vec<&QuotedLine> = Vec::new();
        let mut blank_run = 0;
        let mut i = 0;
        let gap = self.opts.size * 0.75;
        while i < lines.len() {
            let l = &lines[i];
            if l.level > level {
                if let Some(p) = self.para(&para) {
                    out.push(p);
                }
                para.clear();
                let start = i;
                while i < lines.len() && lines[i].level > level {
                    i += 1;
                }
                let run = &lines[start..i];
                let inner = self.blocks(run, level + 1, start_index + start);
                let quote = Block::Quote { level: (level + 1).min(255) as u8, children: inner };
                let n = run.iter().filter(|l| !l.text.trim().is_empty()).count();
                if level == 0 && n > self.opts.collapse_over {
                    out.push(Block::Collapse { key: (start_index + start) as u64, label: format!("{n} quoted lines"), children: vec![quote] });
                } else {
                    out.push(quote);
                }
                blank_run = 0;
                continue;
            }
            let is_sig_start = l.level == level && level == 0 && (l.text == "-- " || l.text == "--") && l.sig;
            if l.text.trim().is_empty() {
                if let Some(p) = self.para(&para) {
                    out.push(p);
                }
                para.clear();
                blank_run += 1;
                if blank_run <= 2 && !out.is_empty() {
                    out.push(Block::Gap(gap));
                }
            } else {
                if is_sig_start {
                    if let Some(p) = self.para(&para) {
                        out.push(p);
                    }
                    para.clear();
                    if !matches!(out.last(), Some(Block::Gap(_))) && !out.is_empty() {
                        out.push(Block::Gap(gap));
                    }
                }
                blank_run = 0;
                para.push(l);
            }
            i += 1;
        }
        if let Some(p) = self.para(&para) {
            out.push(p);
        }
        while matches!(out.last(), Some(Block::Gap(_))) {
            out.pop();
        }
        out
    }
}

fn push_text(out: &mut Vec<Inline>, s: &str, st: &TextSt) {
    if s.is_empty() {
        return;
    }
    if let Some(Inline::Text(t, prev)) = out.last_mut()
        && prev == st {
            t.push_str(s);
            return;
        }
    out.push(Inline::Text(s.to_string(), st.clone()));
}

/// Build a render tree for a plain-text body.
pub fn build_plain(text: &str, opts: &PlainOpts) -> Doc {
    let lines = parse_lines(text);
    let mut b = PlainBuilder { opts, links: Vec::new() };
    let blocks = b.blocks(&lines, 0, 0);
    let mut doc = Doc::new(blocks, b.links);
    doc.text_len = text.len();
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> PlainOpts {
        PlainOpts { size: 14.0, fixed: false, collapse_over: 3 }
    }

    #[test]
    fn quote_levels() {
        let l = parse_lines("Hi\n> one\n>> two\n > > also two\n>no space\nBye\n-- \nSig line\n\n");
        let levels: Vec<usize> = l.iter().map(|l| l.level).collect();
        assert_eq!(levels, vec![0, 1, 2, 2, 1, 0, 0, 0]);
        assert_eq!(l[1].text, "one");
        assert_eq!(l[3].text, "also two");
        assert_eq!(l[4].text, "no space");
        assert!(!l[5].sig);
        assert!(l[6].sig && l[7].sig);
    }

    #[test]
    fn nested_blocks_and_collapse() {
        let d = build_plain("Reply text\n\nOn Monday Bob wrote:\n> a\n> b\n>> c\n> d\n> e\nThanks", &opts());
        // Reply para, gap, attribution para, collapsed quote, thanks
        let kinds: Vec<&str> = d
            .blocks
            .iter()
            .map(|b| match b {
                Block::Para(_) => "p",
                Block::Gap(_) => "gap",
                Block::Collapse { .. } => "collapse",
                Block::Quote { .. } => "quote",
                _ => "?",
            })
            .collect();
        assert_eq!(kinds, vec!["p", "gap", "p", "collapse", "p"]);
        let Block::Collapse { children, label, .. } = &d.blocks[3] else { panic!() };
        assert_eq!(label, "5 quoted lines");
        let Block::Quote { level: 1, children: inner } = &children[0] else { panic!() };
        assert!(inner.iter().any(|b| matches!(b, Block::Quote { level: 2, .. })));
        // Short quotes are not collapsed.
        let d = build_plain("x\n> a\ny", &opts());
        assert!(matches!(d.blocks[1], Block::Quote { level: 1, .. }));
    }

    #[test]
    fn signature_is_dimmed() {
        let d = build_plain("Body\n-- \nJane Doe\nACME", &opts());
        let Some(Block::Para(p)) = d.blocks.last() else { panic!() };
        assert!(p.inlines.iter().all(|i| match i {
            Inline::Text(_, st) => st.dim,
            _ => true,
        }));
        let Block::Para(first) = &d.blocks[0] else { panic!() };
        assert!(matches!(&first.inlines[0], Inline::Text(t, st) if t == "Body" && !st.dim));
    }

    #[test]
    fn flowed_lines_join() {
        let d = build_plain("This is a \nsoft wrapped line.\nHard", &opts());
        let Block::Para(p) = &d.blocks[0] else { panic!() };
        let text: String = p.inlines.iter().map(|i| if let Inline::Text(t, _) = i { t.clone() } else { "\n".into() }).collect();
        assert_eq!(text, "This is a soft wrapped line.\nHard");
    }
}
