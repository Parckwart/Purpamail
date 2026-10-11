//! The render tree: HTML (parsed with `scraper`) is turned into blocks of
//! styled inline content, tables, lists and quotes that the layout engine can
//! position without knowing anything about HTML.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Vec2};
use scraper::{ElementRef, Html, Node, Selector};

use super::css::{self, Decl, Len};

/// Generic font family classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Family {
    #[default]
    Body,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Vertical alignment of table cell content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Sub- / superscript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Script {
    #[default]
    Normal,
    Sub,
    Sup,
}

/// Style of a run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSt {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub family: Family,
    /// Explicit color; `None` uses the document's text (or link) color.
    pub color: Option<Color32>,
    pub bg: Option<Color32>,
    pub script: Script,
    /// Index into [`Doc::links`].
    pub link: Option<u32>,
    /// Absolute line height in points.
    pub line_height: Option<f32>,
    /// De-emphasized text (signatures in plain-text messages).
    pub dim: bool,
}

impl TextSt {
    pub fn new(size: f32) -> Self {
        Self { size, bold: false, italic: false, underline: false, strike: false, family: Family::Body, color: None, bg: None, script: Script::Normal, link: None, line_height: None, dim: false }
    }
}

/// An image reference inside a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct ImgRef {
    /// Index into [`Doc::images`].
    pub idx: u32,
    pub width: Option<Len>,
    pub height: Option<f32>,
    pub link: Option<u32>,
    pub alt: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String, TextSt),
    Image(ImgRef),
    Break,
}

/// A paragraph: inline content wrapped as one flow.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Para {
    pub inlines: Vec<Inline>,
    pub align: HAlign,
    /// Preformatted (whitespace preserved).
    pub pre: bool,
}

/// Box decoration of a block (margins, padding, border, background, sizes).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BoxStyle {
    /// Top, right, bottom, left.
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub border: [f32; 4],
    pub border_color: [Option<Color32>; 4],
    pub bg: Option<Color32>,
    pub radius: f32,
    pub width: Option<Len>,
    pub max_width: Option<Len>,
    pub min_height: Option<f32>,
    /// `margin-left/right: auto` — center horizontally.
    pub center: bool,
    /// Padding was given explicitly (table cells otherwise use `cellpadding`).
    pub padding_set: bool,
}

impl BoxStyle {
    pub fn is_plain(&self) -> bool {
        self.padding == [0.0; 4] && self.border == [0.0; 4] && self.bg.is_none() && self.width.is_none() && self.max_width.is_none() && !self.center && self.min_height.is_none()
    }
    pub fn h_extra(&self) -> f32 {
        self.padding[1] + self.padding[3] + self.border[1] + self.border[3]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableCell {
    pub style: BoxStyle,
    pub colspan: usize,
    pub rowspan: usize,
    pub width: Option<Len>,
    pub height: Option<f32>,
    pub valign: VAlign,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub bg: Option<Color32>,
    pub height: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    pub style: BoxStyle,
    pub rows: Vec<TableRow>,
    pub spacing: f32,
    pub width: Option<Len>,
    pub align: HAlign,
}

/// A block-level element of the render tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A generic block container (div, p, td content, ...).
    Box { style: BoxStyle, children: Vec<Block> },
    Para(Para),
    /// Quoted text (blockquote type=cite, `>` lines) drawn with a colored bar.
    Quote { level: u8, children: Vec<Block> },
    /// A list item with its marker ("•", "3.").
    ListItem { marker: String, marker_style: TextSt, children: Vec<Block> },
    Table(Box<Table>),
    Rule { color: Option<Color32>, thickness: f32 },
    /// Collapsible section (long quoted text in plain-text messages).
    Collapse { key: u64, label: String, children: Vec<Block> },
    /// Vertical space.
    Gap(f32),
}

/// A hyperlink target.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub href: String,
    /// Visible text (for phishing hints).
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImgSrc {
    Cid(String),
    Remote(String),
    Data,
    Missing,
}

/// An image referenced by the document.
#[derive(Clone, Debug)]
pub struct DocImage {
    pub src: ImgSrc,
    /// Bytes for embedded (cid / data) images.
    pub bytes: Option<Arc<[u8]>>,
    /// URI used with egui's image loaders.
    pub uri: String,
    /// Intrinsic size when known.
    pub size: Option<Vec2>,
    /// Tracking pixel (tiny or hidden remote image).
    pub tracking: bool,
}

/// Intrinsic (min, max) widths of blocks by address, for one pixels-per-point value.
pub(crate) type MeasureCache = (f32, HashMap<usize, (f32, f32)>);

/// A prepared document.
#[derive(Debug, Default)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
    pub images: Vec<DocImage>,
    /// Page background (body bgcolor).
    pub bg: Option<Color32>,
    /// Page text color (body text / color).
    pub text_color: Option<Color32>,
    /// References remote content (images, backgrounds) other than tracking pixels.
    pub has_remote: bool,
    /// Has remote tracking pixels.
    pub has_trackers: bool,
    /// Designed layout (colored backgrounds, images, layout tables): rendered
    /// on a light "paper" card in dark mode instead of adapting colors.
    pub designed: bool,
    /// Plain text approximation (for word counts, summaries).
    pub text_len: usize,
    pub(crate) measure: parking_lot::Mutex<MeasureCache>,
}

impl Doc {
    pub fn new(blocks: Vec<Block>, links: Vec<Link>) -> Self {
        Doc { blocks, links, ..Default::default() }
    }
}

/// Inputs for [`build_html`].
pub struct BuildOpts<'a> {
    /// Base font size in points (the message font size).
    pub base_size: f32,
    /// Resolve `cid:` references to embedded image bytes.
    pub resolve_cid: &'a dyn Fn(&str) -> Option<Arc<[u8]>>,
}

// ---------------------------------------------------------------------- images

/// File extension for image bytes (for egui's loaders).
pub fn sniff_ext(b: &[u8]) -> &'static str {
    if b.starts_with(b"\x89PNG") {
        "png"
    } else if b.starts_with(&[0xff, 0xd8]) {
        "jpg"
    } else if b.starts_with(b"GIF8") {
        "gif"
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "webp"
    } else if b.starts_with(b"BM") {
        "bmp"
    } else if String::from_utf8_lossy(&b[..b.len().min(512)]).contains("<svg") {
        "svg"
    } else {
        "png"
    }
}

/// Stable `bytes://` URI for image data.
pub fn bytes_uri(b: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    format!("bytes://pm-img/{:016x}.{}", h.finish(), sniff_ext(b))
}

/// Intrinsic pixel size of encoded image bytes.
pub fn image_dims(b: &[u8]) -> Option<Vec2> {
    let r = image::ImageReader::new(std::io::Cursor::new(b)).with_guessed_format().ok()?;
    let (w, h) = r.into_dimensions().ok()?;
    Some(Vec2::new(w as f32, h as f32))
}

fn decode_data_uri(src: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let rest = src.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if meta.ends_with(";base64") {
        let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(clean).ok()
    } else {
        Some(data.as_bytes().to_vec())
    }
}

// ---------------------------------------------------------------------- builder

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Display {
    Default,
    None,
    Block,
    Inline,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transform {
    None,
    Upper,
    Lower,
}

/// Inherited properties.
#[derive(Clone, Debug)]
struct Inh {
    text: TextSt,
    align: HAlign,
    pre: bool,
    transform: Transform,
    quote_level: u8,
    list_depth: u8,
    hidden: bool,
}

/// Computed style of one element.
#[derive(Clone, Debug)]
struct Computed {
    inh: Inh,
    display: Display,
    bx: BoxStyle,
    valign: Option<VAlign>,
    height: Option<f32>,
    list_style: Option<String>,
    border_collapse: bool,
    border_spacing: Option<f32>,
}

const SKIP: &[&str] = &[
    "head", "script", "style", "title", "meta", "link", "noscript", "template", "iframe", "object", "embed", "svg", "math", "canvas", "audio", "video", "input", "select",
    "textarea", "map", "area", "base", "frame", "frameset", "applet", "param", "source", "track", "datalist", "dialog",
];

const BLOCKS: &[&str] = &[
    "html", "body", "div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre", "ul", "ol", "li", "dl", "dt", "dd", "hr", "table", "center", "section", "article", "header",
    "footer", "nav", "main", "aside", "figure", "figcaption", "address", "form", "fieldset", "details", "summary", "legend", "menu", "dir", "listing", "xmp", "plaintext",
    "caption", "tbody", "thead", "tfoot", "tr", "td", "th", "hgroup",
];

fn is_invisible_char(c: char) -> bool {
    matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{34f}' | '\u{ad}' | '\u{180e}')
}

/// Accumulates inline content of one block.
struct ParaBuilder {
    inlines: Vec<Inline>,
    last_space: bool,
    /// Style of the last line break (for paragraphs holding only a break).
    break_style: Option<TextSt>,
}

impl ParaBuilder {
    fn new() -> Self {
        Self { inlines: Vec::new(), last_space: true, break_style: None }
    }

    fn push_str(&mut self, s: &str, st: &TextSt) {
        if s.is_empty() {
            return;
        }
        if let Some(Inline::Text(t, prev)) = self.inlines.last_mut()
            && prev == st {
                t.push_str(s);
                return;
            }
        self.inlines.push(Inline::Text(s.to_string(), st.clone()));
    }

    fn text(&mut self, raw: &str, inh: &Inh) {
        if inh.hidden {
            return;
        }
        let transformed;
        let raw = match inh.transform {
            Transform::None => raw,
            Transform::Upper => {
                transformed = raw.to_uppercase();
                &transformed
            }
            Transform::Lower => {
                transformed = raw.to_lowercase();
                &transformed
            }
        };
        let mut s = String::with_capacity(raw.len());
        if inh.pre {
            for c in raw.chars() {
                match c {
                    '\r' => {}
                    '\t' => s.push_str("    "),
                    c if is_invisible_char(c) => {}
                    c => s.push(c),
                }
            }
            self.last_space = s.ends_with('\n') || s.ends_with(' ');
        } else {
            for c in raw.chars() {
                if c.is_whitespace() && c != '\u{a0}' {
                    if !self.last_space {
                        s.push(' ');
                        self.last_space = true;
                    }
                } else if is_invisible_char(c) {
                } else {
                    s.push(c);
                    self.last_space = false;
                }
            }
        }
        self.push_str(&s, &inh.text);
    }

    fn line_break(&mut self, st: &TextSt) {
        self.break_style = Some(st.clone());
        // drop a trailing space before the break
        if let Some(Inline::Text(t, _)) = self.inlines.last_mut()
            && t.ends_with(' ') {
                t.pop();
            }
        self.inlines.push(Inline::Break);
        self.last_space = true;
    }

    fn image(&mut self, img: ImgRef) {
        self.inlines.push(Inline::Image(img));
        self.last_space = false;
    }

    /// Finish the paragraph; `None` if it holds nothing visible.
    fn finish(&mut self, align: HAlign, pre: bool) -> Option<Block> {
        let mut inlines = std::mem::take(&mut self.inlines);
        self.last_space = true;
        // Trim trailing whitespace.
        while let Some(Inline::Text(t, _)) = inlines.last_mut() {
            let trimmed = if pre { t.trim_end_matches('\n').len() } else { t.trim_end_matches(' ').len() };
            t.truncate(trimmed);
            if t.is_empty() {
                inlines.pop();
            } else {
                break;
            }
        }
        // A trailing <br> does not add a line, unless it is the only content.
        let has_content = inlines.iter().any(|i| !matches!(i, Inline::Break));
        if has_content && matches!(inlines.last(), Some(Inline::Break)) {
            inlines.pop();
        }
        if !has_content && !inlines.is_empty() {
            // Only line breaks: one empty line (a trailing break adds none).
            let st = self.break_style.take().unwrap_or_else(|| TextSt::new(14.0));
            inlines = vec![Inline::Text("\u{a0}".into(), st)];
        }
        if inlines.is_empty() {
            return None;
        }
        if inlines.iter().all(|i| matches!(i, Inline::Text(t, _) if t.chars().all(|c| c == ' ' || c == '\n'))) {
            return None;
        }
        Some(Block::Para(Para { inlines, align, pre }))
    }
}

struct Builder<'a> {
    opts: &'a BuildOpts<'a>,
    rules: HashMap<usize, Vec<(u32, usize, Decl)>>,
    links: Vec<Link>,
    images: Vec<DocImage>,
    cid_cache: HashMap<String, u32>,
    has_remote: bool,
    has_trackers: bool,
    designed: bool,
}

fn key(el: &ElementRef) -> usize {
    el.value() as *const _ as usize
}

/// Build the render tree for an HTML document.
pub fn build_html(html: &str, opts: &BuildOpts) -> Doc {
    let dom = Html::parse_document(html);
    let mut b = Builder { opts, rules: HashMap::new(), links: Vec::new(), images: Vec::new(), cid_cache: HashMap::new(), has_remote: false, has_trackers: false, designed: false };

    // Embedded style sheets.
    if let Ok(style_sel) = Selector::parse("style") {
        let mut order = 0usize;
        for st in dom.select(&style_sel) {
            let text: String = st.text().collect();
            if css::has_remote_url(&text) {
                b.has_remote = true;
            }
            for rule in css::parse_stylesheet(&text) {
                let Ok(sel) = Selector::parse(&rule.selector) else { continue };
                for el in dom.select(&sel) {
                    let e = b.rules.entry(key(&el)).or_default();
                    for d in &rule.decls {
                        e.push((rule.specificity, order, d.clone()));
                    }
                }
                order += 1;
            }
        }
    }

    let root = dom.root_element();
    let body = root.child_elements().find(|e| e.value().name() == "body").unwrap_or(root);
    let base = Inh { text: TextSt::new(opts.base_size), align: HAlign::Left, pre: false, transform: Transform::None, quote_level: 0, list_depth: 0, hidden: false };
    let html_style = b.compute(&root, &base);
    let body_style = b.compute(&body, &html_style.inh);
    let mut blocks = Vec::new();
    let mut para = ParaBuilder::new();
    b.walk(body, &body_style.inh, &mut para, &mut blocks, 0);
    if let Some(p) = para.finish(body_style.inh.align, body_style.inh.pre) {
        blocks.push(p);
    }
    // Keep the body's padding (but not its default margin).
    let mut bs = body_style.bx.clone();
    bs.margin = [0.0; 4];
    let bg = body_style.bx.bg.or(html_style.bx.bg).filter(|c| c.a() > 0);
    bs.bg = None;
    if !bs.is_plain() {
        blocks = vec![Block::Box { style: bs, children: blocks }];
    }
    let text_color = body_style.inh.text.color;
    let text_len = text_len(&blocks);
    let designed = b.designed || bg.is_some_and(|c| !near_white(c));
    Doc {
        blocks,
        links: b.links,
        images: b.images,
        bg,
        text_color,
        has_remote: b.has_remote,
        has_trackers: b.has_trackers,
        designed,
        text_len,
        measure: Default::default(),
    }
}

fn near_white(c: Color32) -> bool {
    c.r() > 240 && c.g() > 240 && c.b() > 240
}

fn text_len(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            Block::Box { children, .. } | Block::Quote { children, .. } | Block::ListItem { children, .. } | Block::Collapse { children, .. } => text_len(children),
            Block::Para(p) => p.inlines.iter().map(|i| if let Inline::Text(t, _) = i { t.len() } else { 0 }).sum(),
            Block::Table(t) => t.rows.iter().flat_map(|r| r.cells.iter()).map(|c| text_len(&c.blocks)).sum(),
            _ => 0,
        })
        .sum()
}

fn attr_len(v: &str) -> Option<Len> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(Len::Percent);
    }
    let n: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    n.parse::<f32>().ok().filter(|v| *v > 0.0).map(Len::Px)
}

fn attr_px(v: Option<&str>) -> Option<f32> {
    match attr_len(v?) {
        Some(Len::Px(p)) => Some(p),
        _ => None,
    }
}

fn halign_of(v: &str) -> Option<HAlign> {
    match v.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(HAlign::Left),
        "center" | "middle" | "-webkit-center" | "-moz-center" => Some(HAlign::Center),
        "right" | "end" => Some(HAlign::Right),
        "justify" => Some(HAlign::Justify),
        _ => None,
    }
}

fn valign_of(v: &str) -> Option<VAlign> {
    match v.trim().to_ascii_lowercase().as_str() {
        "top" | "text-top" => Some(VAlign::Top),
        "middle" | "center" => Some(VAlign::Middle),
        "bottom" | "text-bottom" | "baseline" => Some(VAlign::Bottom),
        _ => None,
    }
}

fn family_of(v: &str) -> Family {
    let l = v.to_ascii_lowercase();
    for f in l.split(',') {
        let f = f.trim().trim_matches(['"', '\'']);
        if f.contains("mono") || f.contains("courier") || f.contains("consolas") || f.contains("menlo") || f.contains("monaco") || f == "fixed" || f.contains("lucida console") {
            return Family::Mono;
        }
        if f == "serif" || f.contains("times") || f.contains("georgia") || f.contains("garamond") || f.contains("palatino") || f.contains("cambria") || f.contains("book antiqua") || f.contains("baskerville") || f.contains("merriweather") {
            return Family::Serif;
        }
        if f == "sans-serif" || f.contains("arial") || f.contains("helvetica") || f.contains("verdana") || f.contains("tahoma") || f.contains("segoe") || f.contains("calibri") || f.contains("sans") || f.contains("inter") || f.contains("roboto") || f.contains("system-ui") || f.contains("trebuchet") {
            return Family::Body;
        }
    }
    Family::Body
}

impl Builder<'_> {
    fn add_link(&mut self, href: &str, text: String) -> u32 {
        self.links.push(Link { href: href.trim().to_string(), text });
        (self.links.len() - 1) as u32
    }

    /// Compute the style of `el` given the inherited properties of its parent.
    fn compute(&self, el: &ElementRef, parent: &Inh) -> Computed {
        let tag = el.value().name();
        let mut inh = parent.clone();
        if tag == "a" && el.value().attr("href").is_some() {
            // Links get the link color unless they (or descendants) set a color.
            inh.text.color = None;
            inh.text.underline = true;
        }
        let mut c = Computed { inh, display: Display::Default, bx: BoxStyle::default(), valign: None, height: None, list_style: None, border_collapse: false, border_spacing: None };
        let size = parent.text.size;
        let base = self.opts.base_size;
        let t = &mut c.inh.text;
        // ---- tag defaults
        match tag {
            "b" | "strong" => t.bold = true,
            "th" => {
                t.bold = true;
                c.inh.align = HAlign::Center;
            }
            "i" | "em" | "cite" | "var" | "dfn" | "address" => t.italic = true,
            "u" | "ins" => t.underline = true,
            "s" | "strike" | "del" => t.strike = true,
            "code" | "tt" | "kbd" | "samp" => t.family = Family::Mono,
            "pre" | "listing" | "xmp" | "plaintext" => {
                t.family = Family::Mono;
                c.bx.margin = [size * 0.6, 0.0, size * 0.6, 0.0];
            }
            "small" => t.size = size * 0.85,
            "big" => t.size = size * 1.2,
            "sub" => {
                t.script = Script::Sub;
                t.size = size * 0.75;
            }
            "sup" => {
                t.script = Script::Sup;
                t.size = size * 0.75;
            }
            "mark" => t.bg = Some(Color32::from_rgb(0xff, 0xf1, 0x76)),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                t.bold = true;
                let (f, m) = match tag {
                    "h1" => (2.0, 0.67),
                    "h2" => (1.5, 0.83),
                    "h3" => (1.17, 1.0),
                    "h4" => (1.0, 1.33),
                    "h5" => (0.83, 1.67),
                    _ => (0.67, 2.33),
                };
                t.size = base * f;
                c.bx.margin = [t.size * m * 0.6, 0.0, t.size * m * 0.6, 0.0];
            }
            "p" => c.bx.margin = [size * 0.8, 0.0, size * 0.8, 0.0],
            "ul" | "ol" | "menu" | "dir" => {
                let m = if parent.list_depth == 0 { size * 0.6 } else { 0.0 };
                c.bx.margin = [m, 0.0, m, 0.0];
                c.bx.padding[3] = 26.0;
                c.inh.list_depth += 1;
            }
            "dl" => c.bx.margin = [size * 0.6, 0.0, size * 0.6, 0.0],
            "dd" => c.bx.margin[3] = 30.0,
            "figure" => c.bx.margin = [size * 0.6, 20.0, size * 0.6, 20.0],
            "center" => c.inh.align = HAlign::Center,
            "td" => c.inh.align = parent.align,
            "table" => {
                // Tables do not inherit text-align in standards mode, but email clients
                // (quirks mode) do; keep it.
            }
            _ => {}
        }
        if tag == "pre" || tag == "listing" || tag == "xmp" || tag == "plaintext" {
            c.inh.pre = true;
        }
        // ---- presentational attributes
        let a = |n: &str| el.value().attr(n);
        if let Some(v) = a("align")
            && tag != "img" && tag != "table"
                && let Some(h) = halign_of(v) {
                    c.inh.align = h;
                }
        if let Some(v) = a("bgcolor").and_then(css::parse_color) {
            c.bx.bg = Some(v).filter(|c| c.a() > 0);
        }
        if let Some(v) = a("valign").and_then(valign_of) {
            c.valign = Some(v);
        }
        if tag == "font" {
            if let Some(v) = a("color").and_then(css::parse_color) {
                c.inh.text.color = Some(v);
            }
            if let Some(v) = a("face") {
                c.inh.text.family = family_of(v);
            }
            if let Some(v) = a("size").and_then(|v| css::font_tag_size(v, size, base)) {
                c.inh.text.size = v;
            }
        }
        if tag == "body"
            && let Some(v) = a("text").and_then(css::parse_color) {
                c.inh.text.color = Some(v);
            }
        if matches!(tag, "td" | "th" | "table" | "div" | "col") {
            if let Some(w) = a("width").and_then(attr_len) {
                c.bx.width = Some(w);
            }
            if let Some(h) = attr_px(a("height")) {
                c.height = Some(h);
            }
        }
        if (tag == "td" || tag == "th")
            && a("nowrap").is_some() {
                // treated as preference only
            }
        if tag == "hr"
            && let Some(v) = a("color").and_then(css::parse_color) {
                c.bx.bg = Some(v);
            }
        if tag == "blockquote" && a("type").is_some_and(|t| t.eq_ignore_ascii_case("cite")) {
            // handled by the quote block
        }
        if (tag == "ol" || tag == "ul")
            && let Some(t) = a("type") {
                c.list_style = Some(match t {
                    "1" => "decimal".into(),
                    "a" => "lower-alpha".into(),
                    "A" => "upper-alpha".into(),
                    "i" => "lower-roman".into(),
                    "I" => "upper-roman".into(),
                    o => o.to_ascii_lowercase(),
                });
            }

        // ---- CSS: rules (by specificity and order), then the style attribute; important last.
        let mut decls: Vec<(u8, u32, usize, &Decl)> = Vec::new();
        if let Some(rs) = self.rules.get(&key(el)) {
            for (spec, order, d) in rs {
                decls.push((if d.important { 2 } else { 0 }, *spec, *order, d));
            }
        }
        let inline = a("style").map(css::parse_declarations).unwrap_or_default();
        for (i, d) in inline.iter().enumerate() {
            decls.push((if d.important { 3 } else { 1 }, 0, i, d));
        }
        decls.sort_by_key(|(imp, spec, order, _)| (*imp, *spec, *order));
        for (_, _, _, d) in decls {
            self.apply(&mut c, d, parent);
        }
        if c.bx.bg.is_some_and(|bg| !near_white(bg)) {
            // colored panels: a designed message
        }
        c
    }

    fn apply(&self, c: &mut Computed, d: &Decl, parent: &Inh) {
        let v = d.value.as_str();
        let lv = v.to_ascii_lowercase();
        let size = c.inh.text.size;
        let base = self.opts.base_size;
        match d.name.as_str() {
            "color" => {
                if let Some(col) = css::parse_color(v).filter(|c| c.a() > 0) {
                    c.inh.text.color = Some(col);
                } else if lv == "inherit" {
                    c.inh.text.color = parent.text.color;
                }
            }
            "background-color" => c.bx.bg = css::parse_color(v).filter(|c| c.a() > 0),
            "background" => {
                c.bx.bg = css::background_color(v).filter(|c| c.a() > 0);
            }
            "font-weight" => {
                c.inh.text.bold = match lv.as_str() {
                    "bold" | "bolder" => true,
                    "normal" | "lighter" => false,
                    n => n.parse::<u32>().map(|w| w >= 600).unwrap_or(c.inh.text.bold),
                }
            }
            "font-style" => c.inh.text.italic = lv.contains("italic") || lv.contains("oblique"),
            "text-decoration" | "text-decoration-line" => {
                if lv.contains("none") {
                    c.inh.text.underline = false;
                    c.inh.text.strike = false;
                }
                if lv.contains("underline") {
                    c.inh.text.underline = true;
                }
                if lv.contains("line-through") {
                    c.inh.text.strike = true;
                }
            }
            "font-size" => {
                if let Some(sz) = css::parse_len(v, parent.text.size).and_then(|l| l.resolve(parent.text.size))
                    && sz <= 1.5 {
                        c.inh.hidden = true;
                    }
                if let Some(sz) = css::parse_font_size(v, parent.text.size, base) {
                    c.inh.text.size = sz;
                }
            }
            "font-family" => c.inh.text.family = family_of(v),
            "font" => {
                let parts = css::split_value(&lv);
                for (i, p) in parts.iter().enumerate() {
                    match p.as_str() {
                        "bold" | "bolder" | "600" | "700" | "800" | "900" => c.inh.text.bold = true,
                        "italic" | "oblique" => c.inh.text.italic = true,
                        p if p.chars().next().is_some_and(|ch| ch.is_ascii_digit() || ch == '.') => {
                            let sz = p.split('/').next().unwrap_or(p);
                            if let Some(s) = css::parse_font_size(sz, parent.text.size, base) {
                                c.inh.text.size = s;
                                c.inh.text.family = family_of(&parts[i + 1..].join(" "));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "text-align" => {
                if let Some(h) = halign_of(v) {
                    c.inh.align = h;
                }
            }
            "line-height" => {
                c.inh.text.line_height = if lv == "normal" {
                    None
                } else if let Ok(n) = lv.parse::<f32>() {
                    Some(n * size)
                } else {
                    match css::parse_len(&lv, size) {
                        Some(Len::Px(p)) => Some(p),
                        Some(Len::Percent(p)) => Some(size * p / 100.0),
                        _ => None,
                    }
                }
                .map(|lh| lh.clamp(size * 0.8, size * 3.0));
            }
            "white-space" => c.inh.pre = lv.starts_with("pre"),
            "text-transform" => {
                c.inh.transform = match lv.as_str() {
                    "uppercase" => Transform::Upper,
                    "lowercase" => Transform::Lower,
                    _ => Transform::None,
                }
            }
            "display" => {
                c.display = match lv.as_str() {
                    "none" => Display::None,
                    "block" | "flex" | "grid" | "list-item" | "table" | "table-row" | "table-cell" => Display::Block,
                    "inline" | "inline-block" | "inline-flex" | "inline-table" => Display::Inline,
                    _ => c.display,
                }
            }
            "visibility" if lv == "hidden" || lv == "collapse" => c.display = Display::None,
            "opacity" if lv.parse::<f32>().is_ok_and(|o| o < 0.05) => c.display = Display::None,
            "mso-hide" if lv == "all" => c.display = Display::None,
            "max-height" | "height" if matches!(css::parse_len(&lv, size), Some(Len::Px(p)) if p < 1.0) => {
                c.inh.hidden = true;
                if d.name == "max-height" {
                    c.display = Display::None;
                }
            }
            "height" => {
                if let Some(Len::Px(p)) = css::parse_len(&lv, size) {
                    c.height = Some(p.min(4000.0));
                }
            }
            "margin" => {
                let s = css::parse_sides(&lv, size);
                for (i, l) in s.iter().enumerate() {
                    c.bx.margin[i] = l.and_then(|l| if let Len::Px(p) = l { Some(p) } else { None }).unwrap_or(0.0).clamp(-50.0, 200.0);
                }
                c.bx.center = matches!(s[1], Some(Len::Auto)) && matches!(s[3], Some(Len::Auto));
            }
            "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
                let i = side_index(&d.name);
                let l = css::parse_len(&lv, size);
                c.bx.margin[i] = match l {
                    Some(Len::Px(p)) => p.clamp(-50.0, 200.0),
                    _ => 0.0,
                };
                if (i == 1 || i == 3) && matches!(l, Some(Len::Auto)) {
                    c.bx.center = true;
                }
            }
            "padding" => {
                let s = css::parse_sides(&lv, size);
                for (i, l) in s.iter().enumerate() {
                    c.bx.padding[i] = l.and_then(|l| l.resolve(0.0)).unwrap_or(0.0).clamp(0.0, 200.0);
                }
                c.bx.padding_set = true;
            }
            "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
                c.bx.padding[side_index(&d.name)] = css::parse_len(&lv, size).and_then(|l| l.resolve(0.0)).unwrap_or(0.0).clamp(0.0, 200.0);
                c.bx.padding_set = true;
            }
            "border" => {
                if let Some((w, col)) = css::parse_border(&lv, size) {
                    c.bx.border = [w; 4];
                    c.bx.border_color = [col; 4];
                }
            }
            "border-top" | "border-right" | "border-bottom" | "border-left" => {
                if let Some((w, col)) = css::parse_border(&lv, size) {
                    let i = side_index(&d.name);
                    c.bx.border[i] = w;
                    c.bx.border_color[i] = col;
                }
            }
            "border-width" => {
                let s = css::parse_sides(&lv, size);
                for (i, l) in s.iter().enumerate() {
                    if let Some(Len::Px(p)) = l {
                        c.bx.border[i] = p.clamp(0.0, 20.0);
                    }
                }
            }
            "border-color" => {
                if let Some(col) = css::parse_color(css::split_value(&lv).first().map(|s| s.as_str()).unwrap_or("")) {
                    c.bx.border_color = [Some(col); 4];
                }
            }
            "border-style" if lv == "none" || lv == "hidden" => c.bx.border = [0.0; 4],
            "border-radius" => {
                if let Some(Len::Px(r)) = css::split_value(&lv).first().and_then(|p| css::parse_len(p, size)) {
                    c.bx.radius = r.clamp(0.0, 40.0);
                }
            }
            "border-collapse" => c.border_collapse = lv == "collapse",
            "border-spacing" => {
                if let Some(Len::Px(p)) = css::split_value(&lv).first().and_then(|p| css::parse_len(p, size)) {
                    c.border_spacing = Some(p.clamp(0.0, 40.0));
                }
            }
            "width" => {
                if let Some(l) = css::parse_len(&lv, size) {
                    c.bx.width = if l == Len::Auto { None } else { Some(l) };
                }
            }
            "max-width" => c.bx.max_width = css::parse_len(&lv, size).filter(|l| *l != Len::Auto),
            "min-height" => c.bx.min_height = css::parse_len(&lv, size).and_then(|l| if let Len::Px(p) = l { Some(p) } else { None }),
            "vertical-align" => match lv.as_str() {
                "sub" => c.inh.text.script = Script::Sub,
                "super" => c.inh.text.script = Script::Sup,
                o => {
                    if let Some(va) = valign_of(o) {
                        c.valign = Some(va);
                    }
                }
            },
            "list-style-type" | "list-style" => c.list_style = Some(lv.split_whitespace().next().unwrap_or("disc").to_string()),
            _ => {}
        }
        if d.name.starts_with("background") && css::has_remote_url(v) {
            // Background images are not rendered, but they would reveal the reader.
        }
    }

    /// Process the children of `el` into `out`, accumulating inline content in `para`.
    fn walk(&mut self, el: ElementRef, inh: &Inh, para: &mut ParaBuilder, out: &mut Vec<Block>, depth: usize) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => para.text(t, inh),
                Node::Element(_) => {
                    let Some(ce) = ElementRef::wrap(child) else { continue };
                    self.element(ce, inh, para, out, depth + 1);
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, el: ElementRef, inh: &Inh, para: &mut ParaBuilder, out: &mut Vec<Block>, depth: usize) {
        let tag = el.value().name();
        if SKIP.contains(&tag) {
            return;
        }
        if depth > 160 {
            // Pathologically deep markup: keep the text only.
            let text: String = el.text().collect();
            para.text(&text, inh);
            return;
        }
        let cs = self.compute(&el, inh);
        if tag == "img" {
            self.image(el, &cs, inh, para, out);
            return;
        }
        if cs.display == Display::None {
            return;
        }
        if cs.bx.bg.is_some_and(|bg| !near_white(bg)) && tag != "body" && tag != "html" {
            self.designed = true;
        }
        if tag == "br" {
            para.line_break(&cs.inh.text);
            return;
        }
        if tag == "wbr" {
            return;
        }
        let is_block = match cs.display {
            Display::Block => true,
            Display::Inline => matches!(tag, "table"),
            _ => BLOCKS.contains(&tag),
        };
        if !is_block {
            // Inline element: its background applies to the text.
            let mut cs = cs;
            if let Some(bg) = cs.bx.bg {
                cs.inh.text.bg = Some(bg);
            }
            if tag == "a"
                && let Some(href) = el.value().attr("href") {
                    let text: String = el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
                    let idx = self.add_link(href, text);
                    let mut inh2 = cs.inh.clone();
                    inh2.text.link = Some(idx);
                    self.walk(el, &inh2, para, out, depth);
                    return;
                }
            if tag == "q" {
                para.text("“", &cs.inh);
                self.walk(el, &cs.inh, para, out, depth);
                para.text("”", &cs.inh);
                return;
            }
            self.walk(el, &cs.inh, para, out, depth);
            return;
        }
        // Block element: flush the pending paragraph.
        if let Some(p) = para.finish(inh.align, inh.pre) {
            out.push(p);
        }
        if let Some(b) = self.block(el, cs, depth) {
            out.push(b);
        }
    }

    fn children_blocks(&mut self, el: ElementRef, inh: &Inh, depth: usize) -> Vec<Block> {
        let mut out = Vec::new();
        let mut para = ParaBuilder::new();
        self.walk(el, inh, &mut para, &mut out, depth);
        if let Some(p) = para.finish(inh.align, inh.pre) {
            out.push(p);
        }
        out
    }

    fn block(&mut self, el: ElementRef, cs: Computed, depth: usize) -> Option<Block> {
        let tag = el.value().name();
        match tag {
            "table" => return Some(self.table(el, cs, depth)),
            "hr" => {
                let mut bx = cs.bx.clone();
                if bx.margin == [0.0; 4] {
                    bx.margin = [8.0, 0.0, 8.0, 0.0];
                }
                let color = cs.bx.border_color.iter().flatten().next().copied().or(cs.bx.bg);
                let thickness = cs.bx.border.iter().cloned().fold(0.0, f32::max).max(1.0);
                return Some(Block::Box { style: BoxStyle { bg: None, border: [0.0; 4], ..bx }, children: vec![Block::Rule { color, thickness }] });
            }
            "blockquote" => {
                let mut inh = cs.inh.clone();
                inh.quote_level = inh.quote_level.saturating_add(1);
                let children = self.children_blocks(el, &inh, depth);
                let mut bx = BoxStyle { margin: [6.0, 0.0, 6.0, 0.0], ..Default::default() };
                bx.bg = cs.bx.bg;
                let q = Block::Quote { level: inh.quote_level, children };
                return Some(Block::Box { style: bx, children: vec![q] });
            }
            "ul" | "ol" | "menu" | "dir" => {
                let ordered = tag == "ol";
                let mut n: i64 = el.value().attr("start").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
                let reversed = el.value().attr("reversed").is_some();
                let style = cs.list_style.clone().unwrap_or_else(|| {
                    if ordered {
                        "decimal".into()
                    } else {
                        ["disc", "circle", "square"][((cs.inh.list_depth.max(1) - 1) % 3) as usize].into()
                    }
                });
                let mut children = Vec::new();
                let mut para = ParaBuilder::new();
                for child in el.children() {
                    match child.value() {
                        Node::Text(t) => para.text(t, &cs.inh),
                        Node::Element(e) if e.name() == "li" => {
                            if let Some(p) = para.finish(cs.inh.align, cs.inh.pre) {
                                children.push(p);
                            }
                            let Some(li) = ElementRef::wrap(child) else { continue };
                            let lcs = self.compute(&li, &cs.inh);
                            if lcs.display == Display::None {
                                continue;
                            }
                            let item_style = lcs.list_style.clone().unwrap_or_else(|| style.clone());
                            if let Some(v) = li.value().attr("value").and_then(|v| v.trim().parse().ok()) {
                                n = v;
                            }
                            let marker = list_marker(&item_style, n);
                            n += if reversed { -1 } else { 1 };
                            let inner = self.children_blocks(li, &lcs.inh, depth + 1);
                            let item = Block::ListItem { marker, marker_style: marker_style(&lcs.inh.text), children: inner };
                            children.push(if lcs.bx.is_plain() && lcs.bx.margin == [0.0; 4] { item } else { Block::Box { style: lcs.bx, children: vec![item] } });
                        }
                        Node::Element(_) => {
                            let Some(ce) = ElementRef::wrap(child) else { continue };
                            self.element(ce, &cs.inh, &mut para, &mut children, depth + 1);
                        }
                        _ => {}
                    }
                }
                if let Some(p) = para.finish(cs.inh.align, cs.inh.pre) {
                    children.push(p);
                }
                return Some(Block::Box { style: cs.bx, children });
            }
            "li" => {
                let inner = self.children_blocks(el, &cs.inh, depth);
                let marker = list_marker(cs.list_style.as_deref().unwrap_or("disc"), 1);
                return Some(Block::Box { style: cs.bx, children: vec![Block::ListItem { marker, marker_style: marker_style(&cs.inh.text), children: inner }] });
            }
            _ => {}
        }
        let children = self.children_blocks(el, &cs.inh, depth);
        if children.is_empty() && cs.bx.is_plain() && cs.height.is_none() {
            // Empty block: only its margins matter.
            let m = cs.bx.margin[0].max(cs.bx.margin[2]);
            return if m > 0.0 { Some(Block::Gap(m)) } else { None };
        }
        let mut bx = cs.bx;
        if let Some(h) = cs.height {
            bx.min_height = Some(bx.min_height.unwrap_or(0.0).max(h));
        }
        if bx.is_plain() && bx.margin == [0.0; 4] && children.len() == 1 {
            return children.into_iter().next();
        }
        Some(Block::Box { style: bx, children })
    }

    fn table(&mut self, el: ElementRef, cs: Computed, depth: usize) -> Block {
        let a = |n: &str| el.value().attr(n);
        let cellpadding = a("cellpadding").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.0, 60.0);
        let mut spacing = a("cellspacing").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(2.0).clamp(0.0, 60.0);
        if cs.border_collapse {
            spacing = 0.0;
        }
        if let Some(s) = cs.border_spacing {
            spacing = s;
        }
        let border_attr = a("border").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0).clamp(0.0, 10.0);
        let border_color = Color32::from_rgb(0xb0, 0xb0, 0xb0);
        let align = match a("align").map(|s| s.to_ascii_lowercase()) {
            Some(s) if s == "center" => HAlign::Center,
            Some(s) if s == "right" => HAlign::Right,
            _ if cs.bx.center => HAlign::Center,
            // Quirk: tables follow the alignment of their container (`<td align=center>`).
            _ if cs.inh.align == HAlign::Center => HAlign::Center,
            _ => HAlign::Left,
        };
        let mut style = cs.bx.clone();
        if border_attr > 0.0 && style.border == [0.0; 4] {
            style.border = [border_attr; 4];
            style.border_color = [Some(border_color); 4];
        }
        if style.width.is_some() || style.bg.is_some() {
            self.designed = true;
        }
        let width = style.width.take();
        let mut table = Table { style, rows: Vec::new(), spacing, width, align };
        let mut rows: Vec<ElementRef> = Vec::new();
        for child in el.child_elements() {
            match child.value().name() {
                "tr" => rows.push(child),
                "thead" | "tbody" | "tfoot" => rows.extend(child.child_elements().filter(|e| e.value().name() == "tr")),
                _ => {}
            }
        }
        // Inherited properties for rows: table-level color/font, but alignment only via attributes.
        let mut tinh = cs.inh.clone();
        if table.align == HAlign::Center && !el.value().attr("style").is_some_and(|s| s.contains("text-align")) {
            tinh.align = cs.inh.align;
        }
        for tr in rows {
            let rcs = self.compute(&tr, &tinh);
            if rcs.display == Display::None {
                continue;
            }
            if rcs.bx.bg.is_some() {
                self.designed = true;
            }
            let mut row = TableRow { cells: Vec::new(), bg: rcs.bx.bg, height: rcs.height };
            for td in tr.child_elements() {
                let name = td.value().name();
                if name != "td" && name != "th" {
                    continue;
                }
                let ccs = self.compute(&td, &rcs.inh);
                if ccs.display == Display::None {
                    continue;
                }
                if ccs.bx.bg.is_some_and(|bg| !near_white(bg)) {
                    self.designed = true;
                }
                let colspan = td.value().attr("colspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 50);
                let rowspan = td.value().attr("rowspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 50);
                let mut st = ccs.bx.clone();
                if !st.padding_set {
                    st.padding = [cellpadding; 4];
                }
                if border_attr > 0.0 && st.border == [0.0; 4] {
                    st.border = [1.0; 4];
                    st.border_color = [Some(border_color); 4];
                }
                let width = st.width.take();
                let blocks = self.children_blocks(td, &ccs.inh, depth + 1);
                row.cells.push(TableCell { style: st, colspan, rowspan, width, height: ccs.height, valign: ccs.valign.or(rcs.valign).unwrap_or(VAlign::Middle), blocks });
            }
            table.rows.push(row);
        }
        Block::Table(Box::new(table))
    }

    fn image(&mut self, el: ElementRef, cs: &Computed, inh: &Inh, para: &mut ParaBuilder, out: &mut Vec<Block>) {
        let a = |n: &str| el.value().attr(n);
        let src = a("src").unwrap_or("").trim();
        let w_attr = a("width").and_then(attr_len);
        let h_attr = attr_px(a("height"));
        let width = cs.bx.width.or(w_attr);
        let height = cs.height.or(h_attr);
        let tiny = |l: Option<Len>| matches!(l, Some(Len::Px(p)) if p <= 2.0);
        let hidden = cs.display == Display::None || inh.hidden || cs.inh.hidden;
        let tracking = hidden || (tiny(width) && (height.is_none() || height.is_some_and(|h| h <= 2.0))) || (height.is_some_and(|h| h <= 1.0) && width.is_none());
        let lsrc = src.to_ascii_lowercase();
        let (img_src, bytes) = if lsrc.starts_with("cid:") {
            let cid = src[4..].to_string();
            let b = (self.opts.resolve_cid)(&cid);
            (ImgSrc::Cid(cid), b)
        } else if lsrc.starts_with("data:") {
            (ImgSrc::Data, decode_data_uri(src).map(Arc::from))
        } else if lsrc.starts_with("http://") || lsrc.starts_with("https://") || lsrc.starts_with("//") {
            let url = if src.starts_with("//") { format!("https:{src}") } else { src.to_string() };
            (ImgSrc::Remote(url), None)
        } else {
            (ImgSrc::Missing, None)
        };
        if matches!(img_src, ImgSrc::Remote(_)) {
            if tracking {
                self.has_trackers = true;
            } else {
                self.has_remote = true;
            }
        }
        if hidden {
            return;
        }
        let idx = match &img_src {
            ImgSrc::Cid(c) if self.cid_cache.contains_key(c) => self.cid_cache[c],
            _ => {
                let (uri, size) = match &bytes {
                    Some(b) => (bytes_uri(b), image_dims(b)),
                    None => (String::new(), None),
                };
                let uri = match &img_src {
                    ImgSrc::Remote(u) if uri.is_empty() => u.clone(),
                    _ => uri,
                };
                self.images.push(DocImage { src: img_src.clone(), bytes, uri, size, tracking });
                let idx = (self.images.len() - 1) as u32;
                if let ImgSrc::Cid(c) = &img_src {
                    self.cid_cache.insert(c.clone(), idx);
                }
                idx
            }
        };
        if tracking {
            return;
        }
        let w_px = width.and_then(|l| l.resolve(0.0));
        if w_px.is_some_and(|w| w >= 120.0) || self.images[idx as usize].size.is_some_and(|s| s.x >= 120.0) {
            self.designed = true;
        }
        let img = ImgRef { idx, width, height, link: inh.text.link, alt: a("alt").unwrap_or("").trim().to_string() };
        let block = cs.display == Display::Block;
        if block {
            if let Some(p) = para.finish(inh.align, inh.pre) {
                out.push(p);
            }
            para.image(img);
            if let Some(p) = para.finish(inh.align, inh.pre) {
                out.push(p);
            }
        } else {
            para.image(img);
        }
    }
}

/// Style of a list marker: the item's text style without decorations.
fn marker_style(st: &TextSt) -> TextSt {
    TextSt { underline: false, strike: false, link: None, bg: None, script: Script::Normal, ..st.clone() }
}

fn side_index(name: &str) -> usize {
    if name.ends_with("top") {
        0
    } else if name.ends_with("right") {
        1
    } else if name.ends_with("bottom") {
        2
    } else {
        3
    }
}

fn roman(mut n: i64, upper: bool) -> String {
    if n <= 0 || n > 3999 {
        return n.to_string();
    }
    let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if upper { s.to_uppercase() } else { s }
}

fn alpha(n: i64, upper: bool) -> String {
    if n <= 0 {
        return n.to_string();
    }
    let mut n = n;
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    let s: String = s.into_iter().rev().collect();
    if upper { s.to_uppercase() } else { s }
}

/// Marker text for a list item.
pub fn list_marker(style: &str, n: i64) -> String {
    match style {
        "none" => String::new(),
        "circle" => "◦".into(),
        "square" => "▪".into(),
        "decimal" | "decimal-leading-zero" => format!("{n}."),
        "lower-alpha" | "lower-latin" => format!("{}.", alpha(n, false)),
        "upper-alpha" | "upper-latin" => format!("{}.", alpha(n, true)),
        "lower-roman" => format!("{}.", roman(n, false)),
        "upper-roman" => format!("{}.", roman(n, true)),
        _ => "•".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(html: &str) -> Doc {
        let png: Arc<[u8]> = Arc::from(&b"\x89PNG\r\n\x1a\n"[..]);
        let resolve = move |cid: &str| if cid == "logo@x" { Some(png.clone()) } else { None };
        build_html(html, &BuildOpts { base_size: 14.0, resolve_cid: &resolve })
    }

    fn texts(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Box { children, .. } | Block::Quote { children, .. } | Block::ListItem { children, .. } | Block::Collapse { children, .. } => texts(children, out),
                Block::Para(p) => out.push(p.inlines.iter().map(|i| match i {
                    Inline::Text(t, _) => t.clone(),
                    Inline::Break => "\n".into(),
                    Inline::Image(_) => "[img]".into(),
                }).collect()),
                Block::Table(t) => {
                    for r in &t.rows {
                        for c in &r.cells {
                            texts(&c.blocks, out)
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn all_text(d: &Doc) -> Vec<String> {
        let mut v = Vec::new();
        texts(&d.blocks, &mut v);
        v
    }

    fn find<'a>(blocks: &'a [Block], pred: &dyn Fn(&Block) -> bool) -> Option<&'a Block> {
        for b in blocks {
            if pred(b) {
                return Some(b);
            }
            let kids: Option<&Vec<Block>> = match b {
                Block::Box { children, .. } | Block::Quote { children, .. } | Block::ListItem { children, .. } | Block::Collapse { children, .. } => Some(children),
                _ => None,
            };
            if let Some(k) = kids {
                if let Some(f) = find(k, pred) {
                    return Some(f);
                }
            }
            if let Block::Table(t) = b {
                for r in &t.rows {
                    for c in &r.cells {
                        if let Some(f) = find(&c.blocks, pred) {
                            return Some(f);
                        }
                    }
                }
            }
        }
        None
    }

    #[test]
    fn whitespace_and_inline_styles() {
        let d = build("<p>Hello   <b>bold\n world</b> and <i>it</i><br>next</p><p></p>");
        let t = all_text(&d);
        assert_eq!(t, vec!["Hello bold world and it\nnext"]);
        let Block::Para(p) = find(&d.blocks, &|b| matches!(b, Block::Para(_))).unwrap() else { unreachable!() };
        let bold: Vec<&str> = p.inlines.iter().filter_map(|i| if let Inline::Text(t, s) = i { s.bold.then_some(t.as_str()) } else { None }).collect();
        assert_eq!(bold, vec!["bold world"]);
    }

    #[test]
    fn hidden_content_and_css() {
        let d = build(
            "<html><head><style>.pre{display:none} p.big { font-size: 20px; color: #ff0000 } #x span { font-weight: bold }</style></head>\
             <body><div class=pre>Preheader</div><p class=big>Big</p><div id=x><span>B</span></div><div style='display:none'>No</div></body></html>",
        );
        let t = all_text(&d);
        assert_eq!(t, vec!["Big", "B"]);
        let big = find(&d.blocks, &|b| matches!(b, Block::Para(p) if matches!(&p.inlines[0], Inline::Text(t, _) if t == "Big"))).unwrap();
        let Block::Para(p) = big else { unreachable!() };
        let Inline::Text(_, st) = &p.inlines[0] else { unreachable!() };
        assert_eq!(st.size, 20.0);
        assert_eq!(st.color, Some(Color32::from_rgb(255, 0, 0)));
        let b = find(&d.blocks, &|b| matches!(b, Block::Para(p) if matches!(&p.inlines[0], Inline::Text(t, s) if t == "B" && s.bold)));
        assert!(b.is_some());
    }

    #[test]
    fn newsletter_table() {
        let d = build(
            r##"<body bgcolor="#eeeeee"><table width="600" align="center" cellpadding="0" cellspacing="0" bgcolor="#ffffff">
               <tr><td colspan="2" bgcolor="#7b4fd6" style="padding:20px;color:#fff" align="center"><img src="cid:logo@x" width="120" height="40" alt="Logo"></td></tr>
               <tr><td width="50%" valign="top">Left <a href="https://example.com/a">link</a></td><td width="50%">Right</td></tr>
               <tr><td colspan=2><img src="https://tracker.example/p.gif" width="1" height="1"></td></tr>
               </table></body>"##,
        );
        assert_eq!(d.bg, Some(Color32::from_rgb(0xee, 0xee, 0xee)));
        assert!(d.designed);
        assert!(d.has_trackers);
        assert!(!d.has_remote);
        let Some(Block::Table(t)) = find(&d.blocks, &|b| matches!(b, Block::Table(_))) else { panic!("no table") };
        assert_eq!(t.align, HAlign::Center);
        assert_eq!(t.width, Some(Len::Px(600.0)));
        assert_eq!(t.spacing, 0.0);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.rows[0].cells[0].colspan, 2);
        assert_eq!(t.rows[0].cells[0].style.bg, Some(Color32::from_rgb(0x7b, 0x4f, 0xd6)));
        assert_eq!(t.rows[0].cells[0].style.padding, [20.0; 4]);
        assert_eq!(t.rows[1].cells[0].valign, VAlign::Top);
        assert_eq!(t.rows[1].cells[1].valign, VAlign::Middle);
        assert_eq!(t.rows[1].cells[0].width, Some(Len::Percent(50.0)));
        // cid image resolved and linked
        assert_eq!(d.images.len(), 2);
        assert!(matches!(d.images[0].src, ImgSrc::Cid(ref c) if c == "logo@x"));
        assert!(d.images[0].bytes.is_some());
        assert!(d.images[0].uri.starts_with("bytes://") && d.images[0].uri.ends_with(".png"));
        assert!(d.images[1].tracking);
        let Block::Para(p) = &t.rows[0].cells[0].blocks[0] else { panic!() };
        assert_eq!(p.align, HAlign::Center);
        assert!(matches!(&p.inlines[0], Inline::Image(i) if i.width == Some(Len::Px(120.0)) && i.alt == "Logo"));
        assert_eq!(d.links[0].href, "https://example.com/a");
        assert_eq!(d.links[0].text, "link");
    }

    #[test]
    fn nested_quotes_and_lists() {
        let d = build("<p>Reply</p><blockquote type=cite>One<blockquote type=cite>Two</blockquote></blockquote><ol start=3><li>a</li><li>b<ul><li>c</li></ul></li></ol>");
        let Some(Block::Quote { level: 1, children }) = find(&d.blocks, &|b| matches!(b, Block::Quote { .. })) else { panic!() };
        assert!(find(children, &|b| matches!(b, Block::Quote { level: 2, .. })).is_some());
        let markers: Vec<String> = {
            let mut v = Vec::new();
            fn walk(bs: &[Block], v: &mut Vec<String>) {
                for b in bs {
                    match b {
                        Block::ListItem { marker, children, .. } => {
                            v.push(marker.clone());
                            walk(children, v)
                        }
                        Block::Box { children, .. } | Block::Quote { children, .. } => walk(children, v),
                        _ => {}
                    }
                }
            }
            walk(&d.blocks, &mut v);
            v
        };
        assert_eq!(markers, vec!["3.", "4.", "◦"]);
        assert_eq!(all_text(&d), vec!["Reply", "One", "Two", "a", "b", "c"]);
    }

    #[test]
    fn remote_and_malformed() {
        let d = build("<div><img src='http://example.com/x.png' width=200 height=100><p>unclosed <b>bold <i>both</div></td></table>&amp; more");
        assert!(d.has_remote);
        assert!(matches!(d.images[0].src, ImgSrc::Remote(_)));
        assert!(all_text(&d).concat().contains("& more"));
        // Deep nesting does not overflow.
        let deep = "<div>".repeat(3000) + "deep" + &"</div>".repeat(3000);
        let d = build(&deep);
        assert!(all_text(&d).concat().contains("deep"));
    }

    #[test]
    fn markers() {
        assert_eq!(list_marker("upper-roman", 4), "IV.");
        assert_eq!(list_marker("lower-alpha", 28), "ab.");
        assert_eq!(list_marker("disc", 1), "•");
    }
}
