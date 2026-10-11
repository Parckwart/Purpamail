//! The styled-span document model behind the rich text editor.
//!
//! A [`RichDoc`] is a plain `String` plus two parallel structures:
//!
//! * **runs** — a run-length list of character [`Style`]s that covers the
//!   whole text exactly (the sum of all run lengths equals the number of
//!   `char`s, there are no empty runs and no two neighbouring runs share a
//!   style);
//! * **paragraph attributes** — one [`ParaAttr`] per paragraph (paragraphs are
//!   separated by `\n`, so there are always `newlines + 1` of them).
//!
//! All positions are **char** indices (that is what egui's text cursors use).
//! Every mutation goes through [`RichDoc::splice`], which keeps both
//! structures consistent; edits made by a plain text widget are folded back
//! into the model with [`diff`] + [`RichDoc::apply_text_change`].

use std::ops::Range;

/// An sRGB color without alpha.
pub type Rgb = [u8; 3];

/// The object replacement character used as the placeholder for inline images.
pub const OBJ: char = '\u{FFFC}';

/// Font size presets (relative to the message font size).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum FontSize {
    Smaller,
    #[default]
    Normal,
    Bigger,
    Heading,
}

impl FontSize {
    pub const ALL: [FontSize; 4] = [FontSize::Smaller, FontSize::Normal, FontSize::Bigger, FontSize::Heading];

    /// Scale factor relative to the base font size.
    pub fn scale(self) -> f32 {
        match self {
            FontSize::Smaller => 0.85,
            FontSize::Normal => 1.0,
            FontSize::Bigger => 1.25,
            FontSize::Heading => 1.6,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FontSize::Smaller => "Smaller",
            FontSize::Normal => "Normal",
            FontSize::Bigger => "Bigger",
            FontSize::Heading => "Heading",
        }
    }

    /// Keyword used by the markup format.
    pub fn keyword(self) -> &'static str {
        match self {
            FontSize::Smaller => "smaller",
            FontSize::Normal => "normal",
            FontSize::Bigger => "bigger",
            FontSize::Heading => "heading",
        }
    }

    pub fn from_keyword(s: &str) -> Option<FontSize> {
        FontSize::ALL.into_iter().find(|f| f.keyword().eq_ignore_ascii_case(s.trim()))
    }
}

/// Character-level formatting.
#[derive(Clone, PartialEq, Eq, Debug, Default, Hash)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub size: FontSize,
    pub color: Option<Rgb>,
    pub highlight: Option<Rgb>,
    /// Hyperlink target.
    pub link: Option<String>,
    /// Content id of an inline image (only set on [`OBJ`] characters).
    pub image: Option<String>,
    /// Part of the inserted signature block (not serialized; lets the composer
    /// find and replace the signature).
    pub signature: bool,
}

impl Style {
    /// True if the style carries no visible formatting (ignoring the signature marker).
    pub fn is_plain(&self) -> bool {
        let Style {
            bold,
            italic,
            underline,
            strike,
            mono,
            size,
            color,
            highlight,
            link,
            image,
            signature: _,
        } = self;
        !bold && !italic && !underline && !strike && !mono && *size == FontSize::Normal && color.is_none() && highlight.is_none() && link.is_none() && image.is_none()
    }

    /// The same style with all formatting removed (keeps links, images and the signature marker).
    pub fn cleared(&self) -> Style {
        Style {
            link: self.link.clone(),
            image: self.image.clone(),
            signature: self.signature,
            ..Default::default()
        }
    }
}

/// A boolean / valued character attribute that can be toggled or set on a range.
#[derive(Clone, PartialEq, Debug)]
pub enum Attr {
    Bold,
    Italic,
    Underline,
    Strike,
    Mono,
    Size(FontSize),
    Color(Option<Rgb>),
    Highlight(Option<Rgb>),
    Link(Option<String>),
}

impl Attr {
    /// Does `s` already carry this attribute?
    pub fn is_set(&self, s: &Style) -> bool {
        match self {
            Attr::Bold => s.bold,
            Attr::Italic => s.italic,
            Attr::Underline => s.underline,
            Attr::Strike => s.strike,
            Attr::Mono => s.mono,
            Attr::Size(f) => s.size == *f,
            Attr::Color(c) => s.color == *c,
            Attr::Highlight(c) => s.highlight == *c,
            Attr::Link(l) => s.link == *l,
        }
    }

    /// Set (`on == true`) or clear the attribute on `s`.
    pub fn apply(&self, s: &mut Style, on: bool) {
        match self {
            Attr::Bold => s.bold = on,
            Attr::Italic => s.italic = on,
            Attr::Underline => s.underline = on,
            Attr::Strike => s.strike = on,
            Attr::Mono => s.mono = on,
            Attr::Size(f) => s.size = if on { *f } else { FontSize::Normal },
            Attr::Color(c) => s.color = if on { *c } else { None },
            Attr::Highlight(c) => s.highlight = if on { *c } else { None },
            Attr::Link(l) => s.link = if on { l.clone() } else { None },
        }
    }

    /// Valued attributes are always *set*; flags toggle.
    pub fn is_toggle(&self) -> bool {
        matches!(self, Attr::Bold | Attr::Italic | Attr::Underline | Attr::Strike | Attr::Mono)
    }
}

/// Horizontal paragraph alignment.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// List membership of a paragraph.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum ListKind {
    #[default]
    None,
    Bullet,
    Numbered,
}

/// Paragraph-level formatting.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct ParaAttr {
    pub align: Align,
    pub list: ListKind,
    /// Quote (indentation) level; 0 = not quoted.
    pub quote: u8,
}

/// A run of characters sharing one style.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Run {
    /// Length in chars (never 0 inside a normalized document).
    pub len: usize,
    pub style: Style,
}

/// A rich text document. See the module docs for the invariants.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct RichDoc {
    text: String,
    runs: Vec<Run>,
    paras: Vec<ParaAttr>,
}

impl Default for RichDoc {
    fn default() -> Self {
        RichDoc {
            text: String::new(),
            runs: Vec::new(),
            paras: vec![ParaAttr::default()],
        }
    }
}

/// A text change found by [`diff`]: chars `start..old_end` of the old text were
/// replaced by chars `start..new_end` of the new text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextChange {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

impl TextChange {
    pub fn inserted(&self) -> usize {
        self.new_end - self.start
    }
    pub fn deleted(&self) -> usize {
        self.old_end - self.start
    }
}

/// Byte offset of char index `ci` in `s` (clamped to the end).
pub fn byte_of(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

/// Find the single contiguous change between `old` and `new`.
///
/// Repeated characters make the location ambiguous ("a" typed into "aa"); the
/// collapsed text cursor after the edit (`cursor_after`, a char index into
/// `new`) is used to pick the location the user actually edited.
pub fn diff(old: &str, new: &str, cursor_after: Option<usize>) -> Option<TextChange> {
    if old == new {
        return None;
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let max_common = a.len().min(b.len());
    let mut p = 0;
    while p < max_common && a[p] == b[p] {
        p += 1;
    }
    let mut s = 0;
    while s < max_common - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    let mut change = TextChange {
        start: p,
        old_end: a.len() - s,
        new_end: b.len() - s,
    };
    if let Some(c) = cursor_after {
        if b.len() > a.len() && change.deleted() == 0 {
            // pure insertion of n chars ending at the cursor?
            let n = b.len() - a.len();
            if c >= n && c <= b.len() {
                let st = c - n;
                if a[..st] == b[..st] && a[st..] == b[c..] {
                    change = TextChange { start: st, old_end: st, new_end: c };
                }
            }
        } else if a.len() > b.len() && change.inserted() == 0 {
            // pure deletion of d chars starting at the cursor?
            let d = a.len() - b.len();
            if c + d <= a.len() && c <= b.len() && a[..c] == b[..c] && a[c + d..] == b[c..] {
                change = TextChange { start: c, old_end: c + d, new_end: c };
            }
        }
    }
    Some(change)
}

impl RichDoc {
    /// A document with uniform `style` and paragraph attributes `para`.
    pub fn styled(text: &str, style: Style, para: ParaAttr) -> RichDoc {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let n = text.chars().count();
        let paras = vec![para; text.matches('\n').count() + 1];
        let runs = if n > 0 { vec![Run { len: n, style }] } else { vec![] };
        RichDoc { text, runs, paras }
    }

    /// An unformatted document.
    pub fn plain(text: &str) -> RichDoc {
        Self::styled(text, Style::default(), ParaAttr::default())
    }

    /// Build a document from raw parts, normalizing them. Missing run coverage
    /// is filled with the default style; extra paragraphs are dropped.
    pub fn from_parts(text: String, runs: Vec<Run>, mut paras: Vec<ParaAttr>) -> RichDoc {
        let n = text.chars().count();
        let mut runs: Vec<Run> = runs.into_iter().filter(|r| r.len > 0).collect();
        let mut total: usize = runs.iter().map(|r| r.len).sum();
        while total > n {
            let last = runs.last_mut().unwrap();
            let cut = (total - n).min(last.len);
            last.len -= cut;
            total -= cut;
            if last.len == 0 {
                runs.pop();
            }
        }
        if total < n {
            runs.push(Run {
                len: n - total,
                style: Style::default(),
            });
        }
        let want = text.matches('\n').count() + 1;
        paras.resize(want, paras.last().copied().unwrap_or_default());
        let mut d = RichDoc { text, runs, paras };
        d.normalize();
        d
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
    pub fn paras(&self) -> &[ParaAttr] {
        &self.paras
    }
    pub fn char_len(&self) -> usize {
        self.runs.iter().map(|r| r.len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Check all invariants (used by tests and debug assertions).
    pub fn check(&self) -> Result<(), String> {
        let n = self.text.chars().count();
        let total: usize = self.runs.iter().map(|r| r.len).sum();
        if total != n {
            return Err(format!("runs cover {total} chars, text has {n}"));
        }
        if self.runs.iter().any(|r| r.len == 0) {
            return Err("empty run".into());
        }
        if self.runs.windows(2).any(|w| w[0].style == w[1].style) {
            return Err("unmerged neighbouring runs".into());
        }
        let want = self.text.matches('\n').count() + 1;
        if self.paras.len() != want {
            return Err(format!("{} paragraph attrs for {want} paragraphs", self.paras.len()));
        }
        Ok(())
    }

    fn normalize(&mut self) {
        self.runs.retain(|r| r.len > 0);
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for r in self.runs.drain(..) {
            match out.last_mut() {
                Some(l) if l.style == r.style => l.len += r.len,
                _ => out.push(r),
            }
        }
        self.runs = out;
        debug_assert!(self.check().is_ok(), "{:?}", self.check());
    }

    /// Style of the char at index `i` (default style when out of range).
    pub fn style_at(&self, i: usize) -> Style {
        let mut o = 0;
        for r in &self.runs {
            if i < o + r.len {
                return r.style.clone();
            }
            o += r.len;
        }
        Style::default()
    }

    /// Iterate `(char range, style)` for all runs.
    pub fn spans(&self) -> impl Iterator<Item = (Range<usize>, &Style)> {
        let mut o = 0;
        self.runs.iter().map(move |r| {
            let range = o..o + r.len;
            o += r.len;
            (range, &r.style)
        })
    }

    /// The style newly typed text at `pos` should get: that of the preceding
    /// character within the paragraph, else of the following one. Images are
    /// never inherited; a link only continues when typing inside it.
    pub fn insertion_style(&self, pos: usize) -> Style {
        let chars: Vec<char> = self.text.chars().collect();
        let prev = pos.checked_sub(1).filter(|&p| p < chars.len() && chars[p] != '\n');
        let next = Some(pos).filter(|&p| p < chars.len() && chars[p] != '\n');
        let mut s = match (prev, next) {
            (Some(p), _) => self.style_at(p),
            (None, Some(n)) => self.style_at(n),
            (None, None) => {
                // empty paragraph: inherit from the newline before it (so a
                // new line continues the formatting), but never links / images
                let mut st = if pos > 0 {
                    self.style_at(pos - 1)
                } else if pos < chars.len() {
                    self.style_at(pos)
                } else {
                    Style::default()
                };
                st.link = None;
                st
            }
        };
        s.image = None;
        if s.link.is_some() {
            let next_link = next.map(|n| self.style_at(n).link);
            if prev.is_none() || next_link.flatten() != s.link {
                s.link = None;
            }
        }
        s
    }

    /// Paragraph index containing char `pos`.
    pub fn para_at(&self, pos: usize) -> usize {
        self.text.chars().take(pos).filter(|&c| c == '\n').count()
    }

    /// Char ranges of all paragraphs (excluding their terminating newline).
    pub fn para_ranges(&self) -> Vec<Range<usize>> {
        let mut out = Vec::with_capacity(self.paras.len());
        let mut start = 0;
        let mut i = 0;
        for c in self.text.chars() {
            if c == '\n' {
                out.push(start..i);
                start = i + 1;
            }
            i += 1;
        }
        out.push(start..i);
        out
    }

    /// Text of char range `r`.
    pub fn slice(&self, r: Range<usize>) -> &str {
        let b0 = byte_of(&self.text, r.start);
        let b1 = byte_of(&self.text, r.end);
        &self.text[b0..b1.max(b0)]
    }

    /// A sub-document covering char range `r`.
    pub fn sub_doc(&self, r: Range<usize>) -> RichDoc {
        let n = self.char_len();
        let r = r.start.min(n)..r.end.min(n).max(r.start.min(n));
        let text = self.slice(r.clone()).to_string();
        let mut runs = Vec::new();
        for (sr, st) in self.spans() {
            let a = sr.start.max(r.start);
            let b = sr.end.min(r.end);
            if a < b {
                runs.push(Run { len: b - a, style: st.clone() });
            }
        }
        let p0 = self.para_at(r.start);
        let k = text.matches('\n').count();
        let paras = self.paras[p0..=p0 + k].to_vec();
        RichDoc::from_parts(text, runs, paras)
    }

    /// Replace chars `start..end` with the document `other`.
    ///
    /// Paragraph attributes: the paragraph containing `start` keeps its
    /// attributes unless the insertion begins at its very start (then it takes
    /// `other`'s first paragraph attributes); every further inserted paragraph
    /// takes the corresponding attributes from `other`.
    pub fn splice(&mut self, start: usize, end: usize, other: &RichDoc) {
        let n = self.char_len();
        let start = start.min(n);
        let end = end.clamp(start, n);
        // --- text
        let b0 = byte_of(&self.text, start);
        let b1 = byte_of(&self.text, end);
        let p = self.text[..b0].matches('\n').count();
        let para_start = self.text[..b0].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let at_para_start = para_start == b0;
        let removed_nl = self.text[b0..b1].matches('\n').count();
        self.text.replace_range(b0..b1, &other.text);
        // --- runs: delete start..end, then insert other's runs at start
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len() + other.runs.len() + 2);
        let mut o = 0;
        let mut inserted = false;
        for r in self.runs.drain(..) {
            let (rs, re) = (o, o + r.len);
            o = re;
            // part before start
            if rs < start {
                out.push(Run {
                    len: re.min(start) - rs,
                    style: r.style.clone(),
                });
            }
            if !inserted && re > start {
                out.extend(other.runs.iter().cloned());
                inserted = true;
            }
            // part after end
            if re > end {
                out.push(Run {
                    len: re - rs.max(end),
                    style: r.style,
                });
            }
        }
        if !inserted {
            out.extend(other.runs.iter().cloned());
        }
        self.runs = out;
        // --- paragraphs
        let k = other.text.matches('\n').count();
        let first = if at_para_start { other.paras[0] } else { self.paras[p] };
        self.paras.splice(p..=p + removed_nl, std::iter::once(first).chain(other.paras[1..=k].iter().copied()));
        self.normalize();
    }

    /// Replace chars `start..end` with `text` in uniform `style`. New
    /// paragraphs continue the attributes of the paragraph at `start`.
    pub fn replace(&mut self, start: usize, end: usize, text: &str, style: &Style) {
        let p = self.para_at(start.min(self.char_len()));
        let para = self.paras.get(p).copied().unwrap_or_default();
        let ins = RichDoc::styled(text, style.clone(), para);
        self.splice(start, end, &ins);
    }

    /// Insert `text` at `pos` with the natural insertion style.
    pub fn insert(&mut self, pos: usize, text: &str) {
        let st = self.insertion_style(pos);
        self.replace(pos, pos, text, &st);
    }

    pub fn delete(&mut self, start: usize, end: usize) {
        self.splice(start, end, &RichDoc::default());
    }

    /// Fold a change made to the plain text (e.g. by a text widget) into the
    /// model. `new_text` is the full new text, `style` the style for inserted chars.
    pub fn apply_text_change(&mut self, change: TextChange, new_text: &str, style: &Style) {
        let b0 = byte_of(new_text, change.start);
        let b1 = byte_of(new_text, change.new_end);
        let ins = new_text[b0..b1].to_string();
        self.replace(change.start, change.old_end, &ins, style);
        debug_assert_eq!(self.text, new_text);
    }

    /// Modify the style of every char in `start..end`.
    pub fn update_styles(&mut self, start: usize, end: usize, f: impl Fn(&mut Style)) {
        if start >= end {
            return;
        }
        let mut out = Vec::with_capacity(self.runs.len() + 2);
        let mut o = 0;
        for r in self.runs.drain(..) {
            let (rs, re) = (o, o + r.len);
            o = re;
            let a = rs.max(start);
            let b = re.min(end);
            if a >= b {
                out.push(r);
                continue;
            }
            if rs < a {
                out.push(Run { len: a - rs, style: r.style.clone() });
            }
            let mut st = r.style.clone();
            f(&mut st);
            out.push(Run { len: b - a, style: st });
            if b < re {
                out.push(Run { len: re - b, style: r.style });
            }
        }
        self.runs = out;
        self.normalize();
    }

    /// Does every non-newline char in `start..end` carry `attr`?
    pub fn range_has(&self, start: usize, end: usize, attr: &Attr) -> bool {
        let chars: Vec<char> = self.text.chars().collect();
        let mut any = false;
        for (r, st) in self.spans() {
            let a = r.start.max(start);
            let b = r.end.min(end);
            if a < b {
                if chars[a..b].iter().all(|&c| c == '\n') {
                    continue;
                }
                any = true;
                if !attr.is_set(st) {
                    return false;
                }
            }
        }
        any
    }

    /// Toggle a flag attribute on a range (set it everywhere unless every
    /// char already has it), or set a valued attribute.
    pub fn toggle(&mut self, start: usize, end: usize, attr: &Attr) {
        let on = !attr.is_toggle() || !self.range_has(start, end, attr);
        self.update_styles(start, end, |s| attr.apply(s, on));
    }

    /// Remove character formatting in a range (links / images stay).
    pub fn clear_formatting(&mut self, start: usize, end: usize) {
        self.update_styles(start, end, |s| *s = s.cleared());
    }

    /// Paragraph indices touched by the char range `start..end`.
    pub fn para_span(&self, start: usize, end: usize) -> Range<usize> {
        let a = self.para_at(start);
        let b = self.para_at(end.max(start));
        a..b + 1
    }

    /// Modify the attributes of the paragraphs touched by `start..end`.
    pub fn update_paras(&mut self, start: usize, end: usize, f: impl Fn(&mut ParaAttr)) {
        let r = self.para_span(start, end);
        for p in &mut self.paras[r] {
            f(p);
        }
    }

    /// Toggle a list kind on the paragraphs touched by the range.
    pub fn toggle_list(&mut self, start: usize, end: usize, kind: ListKind) {
        let r = self.para_span(start, end);
        let all = self.paras[r.clone()].iter().all(|p| p.list == kind);
        for p in &mut self.paras[r] {
            p.list = if all { ListKind::None } else { kind };
        }
    }

    /// Increase (`delta > 0`) or decrease the quote level of the touched paragraphs.
    pub fn indent(&mut self, start: usize, end: usize, delta: i32) {
        self.update_paras(start, end, |p| p.quote = (p.quote as i32 + delta).clamp(0, 8) as u8);
    }

    /// Set the attributes of paragraph `i` directly.
    pub fn set_para(&mut self, i: usize, attr: ParaAttr) {
        if let Some(p) = self.paras.get_mut(i) {
            *p = attr;
        }
    }

    /// Append another document (starting a new paragraph if `new_para`).
    pub fn append(&mut self, other: &RichDoc, new_para: bool) {
        let n = self.char_len();
        if new_para && !self.text.is_empty() {
            let st = self.style_at(n.saturating_sub(1));
            let para = *self.paras.last().unwrap();
            self.splice(n, n, &RichDoc::styled("\n", Style { image: None, link: None, ..st }, para));
        }
        let n = self.char_len();
        self.splice(n, n, other);
    }

    /// Char ranges of maximal runs whose style matches `pred`.
    pub fn ranges_where(&self, pred: impl Fn(&Style) -> bool) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for (r, st) in self.spans() {
            if pred(st) {
                match out.last_mut() {
                    Some(l) if l.end == r.start => l.end = r.end,
                    _ => out.push(r),
                }
            }
        }
        out
    }

    /// Is the whole document free of formatting?
    pub fn is_unformatted(&self) -> bool {
        self.runs.iter().all(|r| r.style.is_plain()) && self.paras.iter().all(|p| *p == ParaAttr::default())
    }

    /// Content ids of all inline images, in order.
    pub fn image_ids(&self) -> Vec<String> {
        self.spans().filter_map(|(_, s)| s.image.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Style {
        Style { bold: true, ..Default::default() }
    }

    fn styles(d: &RichDoc) -> Vec<(String, bool)> {
        d.spans().map(|(r, s)| (d.slice(r).to_string(), s.bold)).collect()
    }

    #[test]
    fn diff_basic() {
        assert_eq!(diff("abc", "abc", None), None);
        assert_eq!(diff("abc", "abXc", None), Some(TextChange { start: 2, old_end: 2, new_end: 3 }));
        assert_eq!(diff("abc", "ac", None), Some(TextChange { start: 1, old_end: 2, new_end: 1 }));
        assert_eq!(diff("hello world", "hello there", None), Some(TextChange { start: 6, old_end: 11, new_end: 11 }));
        assert_eq!(diff("", "x", None), Some(TextChange { start: 0, old_end: 0, new_end: 1 }));
        assert_eq!(diff("äöü", "äü", None), Some(TextChange { start: 1, old_end: 2, new_end: 1 }));
    }

    #[test]
    fn diff_uses_cursor_hint() {
        // "a" typed at the start of "aa": without hint the change is found at the end
        assert_eq!(diff("aa", "aaa", None), Some(TextChange { start: 2, old_end: 2, new_end: 3 }));
        assert_eq!(diff("aa", "aaa", Some(1)), Some(TextChange { start: 0, old_end: 0, new_end: 1 }));
        // backspace of the first "a" in "aab"
        assert_eq!(diff("aab", "ab", Some(0)), Some(TextChange { start: 0, old_end: 1, new_end: 0 }));
        // invalid hints are ignored
        assert_eq!(diff("abc", "abXc", Some(0)), Some(TextChange { start: 2, old_end: 2, new_end: 3 }));
    }

    #[test]
    fn insert_inherits_preceding_style() {
        let mut d = RichDoc::plain("hello world");
        d.toggle(0, 5, &Attr::Bold);
        assert_eq!(styles(&d), vec![("hello".into(), true), (" world".into(), false)]);
        d.insert(5, "!!");
        assert_eq!(styles(&d), vec![("hello!!".into(), true), (" world".into(), false)]);
        d.insert(0, ">");
        // at paragraph start: takes the following char's style
        assert_eq!(styles(&d)[0], (">hello!!".into(), true));
        d.check().unwrap();
    }

    #[test]
    fn delete_across_span_boundaries() {
        let mut d = RichDoc::plain("aaaBBBccc");
        d.toggle(3, 6, &Attr::Bold);
        d.delete(2, 7);
        assert_eq!(d.text(), "aacc");
        assert_eq!(styles(&d), vec![("aacc".into(), false)]);
        let mut d = RichDoc::plain("aaaBBBccc");
        d.toggle(3, 6, &Attr::Bold);
        d.delete(4, 5);
        assert_eq!(styles(&d), vec![("aaa".into(), false), ("BB".into(), true), ("ccc".into(), false)]);
        d.delete(0, d.char_len());
        assert!(d.runs().is_empty());
        d.check().unwrap();
    }

    #[test]
    fn replace_across_spans() {
        let mut d = RichDoc::plain("one two three");
        d.toggle(4, 7, &Attr::Bold);
        d.replace(2, 9, "X", &bold());
        assert_eq!(d.text(), "onXhree");
        assert_eq!(styles(&d), vec![("on".into(), false), ("X".into(), true), ("hree".into(), false)]);
        d.check().unwrap();
    }

    #[test]
    fn insert_at_run_boundaries_and_ends() {
        let mut d = RichDoc::plain("ab");
        d.replace(1, 1, "X", &bold());
        assert_eq!(styles(&d), vec![("a".into(), false), ("X".into(), true), ("b".into(), false)]);
        d.replace(3, 3, "Z", &bold());
        assert_eq!(styles(&d).last().unwrap(), &("Z".to_string(), true));
        d.replace(0, 0, "Q", &Style::default());
        assert_eq!(d.text(), "QaXbZ");
        d.check().unwrap();
    }

    #[test]
    fn apply_text_change_matches_widget_edit() {
        let mut d = RichDoc::plain("Hello world");
        d.toggle(6, 11, &Attr::Italic);
        let new = "Hello brave world";
        let ch = diff(d.text(), new, Some(12)).unwrap();
        let st = d.insertion_style(ch.start);
        d.apply_text_change(ch, new, &st);
        assert_eq!(d.text(), new);
        // inserted at 6 (start of the italic word, preceded by plain space) -> plain
        assert!(!d.style_at(6).italic);
        assert!(d.style_at(12).italic);
        d.check().unwrap();
    }

    #[test]
    fn toggle_bold_on_and_off() {
        let mut d = RichDoc::plain("abcdef");
        d.toggle(0, 3, &Attr::Bold);
        d.toggle(0, 6, &Attr::Bold); // partially bold -> all bold
        assert!(d.range_has(0, 6, &Attr::Bold));
        d.toggle(2, 4, &Attr::Bold); // all bold -> off
        assert_eq!(styles(&d), vec![("ab".into(), true), ("cd".into(), false), ("ef".into(), true)]);
        d.toggle(0, 6, &Attr::Size(FontSize::Heading));
        assert!(d.range_has(0, 6, &Attr::Size(FontSize::Heading)));
        d.clear_formatting(0, 6);
        assert!(d.is_unformatted());
        d.check().unwrap();
    }

    #[test]
    fn paragraphs_split_and_merge() {
        let mut d = RichDoc::plain("item one");
        d.toggle_list(0, 0, ListKind::Bullet);
        // Enter at the end continues the list
        d.insert(8, "\nitem two");
        assert_eq!(d.paras().len(), 2);
        assert_eq!(d.paras()[1].list, ListKind::Bullet);
        // add a plain paragraph after it, then merge it back
        d.insert(d.char_len(), "\nplain");
        d.set_para(2, ParaAttr::default());
        assert_eq!(d.paras()[2].list, ListKind::None);
        // delete the newline between "two" and "plain" -> merged paragraph keeps the upper attrs
        let nl = d.text().chars().position(|c| c == 'p').unwrap() - 1;
        d.delete(nl, nl + 1);
        assert_eq!(d.text(), "item one\nitem twoplain");
        assert_eq!(d.paras().len(), 2);
        assert_eq!(d.paras()[1].list, ListKind::Bullet);
        // delete across several paragraphs
        d.delete(2, 12);
        assert_eq!(d.paras().len(), 1);
        d.check().unwrap();
    }

    #[test]
    fn quote_levels_and_alignment() {
        let mut d = RichDoc::plain("a\nb\nc");
        d.indent(2, 4, 1);
        assert_eq!(d.paras().iter().map(|p| p.quote).collect::<Vec<_>>(), vec![0, 1, 1]);
        d.indent(0, 5, 1);
        d.indent(0, 0, -5);
        assert_eq!(d.paras().iter().map(|p| p.quote).collect::<Vec<_>>(), vec![0, 2, 2]);
        d.update_paras(4, 4, |p| p.align = Align::Right);
        assert_eq!(d.paras()[2].align, Align::Right);
        assert_eq!(d.para_span(1, 4), 0..3);
        d.toggle_list(0, 5, ListKind::Numbered);
        assert!(d.paras().iter().all(|p| p.list == ListKind::Numbered));
        d.toggle_list(0, 5, ListKind::Numbered);
        assert!(d.paras().iter().all(|p| p.list == ListKind::None));
    }

    #[test]
    fn splice_documents() {
        let mut d = RichDoc::plain("start end");
        let mut q = RichDoc::plain("quoted 1\nquoted 2");
        q.indent(0, q.char_len(), 1);
        q.toggle(0, 6, &Attr::Bold);
        d.splice(6, 6, &q);
        assert_eq!(d.text(), "start quoted 1\nquoted 2end");
        // mid-paragraph insertion keeps the host paragraph's attrs for the first line
        assert_eq!(d.paras()[0].quote, 0);
        assert_eq!(d.paras()[1].quote, 1);
        assert!(d.style_at(6).bold);
        // insertion at a paragraph start takes the inserted attrs
        let mut e = RichDoc::plain("x\n");
        e.splice(2, 2, &q);
        assert_eq!(e.paras()[1].quote, 1);
        e.check().unwrap();
        let sub = d.sub_doc(6..21);
        assert_eq!(sub.text(), "quoted 1\nquoted");
        assert!(sub.style_at(0).bold);
        sub.check().unwrap();
    }

    #[test]
    fn insertion_style_rules() {
        let mut d = RichDoc::plain("see link here");
        d.update_styles(4, 8, |s| s.link = Some("https://x.org".into()));
        // typing right after the link does not extend it
        assert_eq!(d.insertion_style(8).link, None);
        // typing inside the link does
        assert!(d.insertion_style(6).link.is_some());
        d.replace(
            0,
            0,
            &OBJ.to_string(),
            &Style {
                image: Some("cid1".into()),
                ..Default::default()
            },
        );
        assert_eq!(d.insertion_style(1).image, None);
        assert_eq!(d.image_ids(), vec!["cid1".to_string()]);
    }

    #[test]
    fn randomized_edits_keep_invariants() {
        // deterministic pseudo random sequence of edits
        let mut seed: u64 = 0x1234_5678;
        let mut rnd = |m: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % (m as u64).max(1)) as usize
        };
        let mut d = RichDoc::plain("The quick brown fox\njumps over\nthe lazy dog");
        let pieces = ["a", "\n", "xyz", "ü\nö", "", "  "];
        for _ in 0..500 {
            let n = d.char_len();
            let a = rnd(n + 1);
            let b = (a + rnd(4)).min(n);
            match rnd(5) {
                0 => d.toggle(a, b, &Attr::Bold),
                1 => d.toggle_list(a, b, ListKind::Bullet),
                2 => d.indent(a, b, if rnd(2) == 0 { 1 } else { -1 }),
                _ => {
                    let p = pieces[rnd(pieces.len())];
                    d.replace(
                        a,
                        b,
                        p,
                        &Style {
                            italic: rnd(2) == 0,
                            ..Default::default()
                        },
                    )
                }
            }
            d.check().unwrap();
        }
    }
}
