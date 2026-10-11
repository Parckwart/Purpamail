//! Layout of a render tree at a given width: produces positioned items
//! (backgrounds, borders, text galleys, images) relative to the document origin.

use std::collections::HashSet;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Color32, FontId, Galley, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use super::css::Len;
use super::remote::{self, RemoteImage};
use super::tree::*;
use crate::ui::fonts;

/// Colors and settings used for layout.
#[derive(Clone, Debug)]
pub struct LayoutOpts {
    pub width: f32,
    /// Adapt explicit colors to a dark background.
    pub adapt_dark: bool,
    /// Background the document is painted on (for contrast decisions).
    pub canvas: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub link: Color32,
    pub quote_colors: [Color32; 3],
    /// Tint quoted text with its quote level color.
    pub quote_text: bool,
    pub rule: Color32,
    pub placeholder_bg: Color32,
    /// Remote images may be downloaded.
    pub allow_remote: bool,
    /// Expanded collapsible sections.
    pub expanded: HashSet<u64>,
    /// Default line height as a multiple of the font size.
    pub line_height: f32,
    pub pixels_per_point: f32,
}

/// A laid out text galley.
#[derive(Clone, Debug)]
pub struct TextItem {
    pub pos: Pos2,
    pub galley: Arc<Galley>,
    /// Character ranges that are links: (start, end, link index).
    pub links: Vec<(usize, usize, u32)>,
    /// Underlines (relative to `pos`), drawn on the baseline rather than at
    /// the bottom of the (taller) line box.
    pub underlines: Vec<(Rect, Color32)>,
}

impl TextItem {
    pub fn rect(&self) -> Rect {
        self.galley.rect.translate(self.pos.to_vec2())
    }
}

/// What an image item shows.
#[derive(Clone, Debug)]
pub enum ImageContent {
    Bytes { uri: String, bytes: Arc<[u8]> },
    /// Not loaded (blocked, loading or failed).
    Placeholder { alt: String, loading: bool },
}

#[derive(Clone, Debug)]
pub enum Item {
    Rect { rect: Rect, fill: Color32, radius: f32 },
    Border { rect: Rect, widths: [f32; 4], colors: [Color32; 4], radius: f32 },
    /// Quote bar.
    Bar { rect: Rect, color: Color32 },
    Text(TextItem),
    Image { rect: Rect, img: u32, link: Option<u32>, content: ImageContent },
    Toggle { rect: Rect, key: u64, open: bool, label: String },
}

impl Item {
    fn translate(&mut self, d: Vec2) {
        match self {
            Item::Rect { rect, .. } | Item::Border { rect, .. } | Item::Bar { rect, .. } | Item::Image { rect, .. } | Item::Toggle { rect, .. } => *rect = rect.translate(d),
            Item::Text(t) => t.pos += d,
        }
    }

    pub fn rect(&self) -> Rect {
        match self {
            Item::Rect { rect, .. } | Item::Border { rect, .. } | Item::Bar { rect, .. } | Item::Image { rect, .. } | Item::Toggle { rect, .. } => *rect,
            Item::Text(t) => t.rect(),
        }
    }
}

/// The result of laying out a document.
#[derive(Clone, Debug, Default)]
pub struct LayoutOut {
    pub size: Vec2,
    pub items: Vec<Item>,
    /// Fill of image placeholders and toggles.
    pub placeholder: Color32,
    /// Secondary text color (placeholder labels).
    pub muted: Color32,
}

const QUOTE_INDENT: f32 = 14.0;
const LIST_GUTTER: f32 = 8.0;

// ------------------------------------------------------------------ colors

fn to_hsl(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32, a: u8) -> Color32 {
    let hue = |p: f32, q: f32, mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (r, g, b) = if s == 0.0 {
        (l, l, l)
    } else {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        (hue(p, q, h + 1.0 / 3.0), hue(p, q, h), hue(p, q, h - 1.0 / 3.0))
    };
    Color32::from_rgba_unmultiplied((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8, a)
}

fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Dark-mode version of a background color (`None`: let the canvas show).
pub fn adapt_bg(c: Color32) -> Option<Color32> {
    let (h, s, l) = to_hsl(c);
    if l > 0.9 && s < 0.5 {
        None
    } else if l > 0.45 {
        Some(from_hsl(h, s * 0.55, (1.0 - l) * 0.55 + 0.08, 255))
    } else {
        Some(c)
    }
}

/// Dark-mode version of a text color, keeping it readable on `bg`.
pub fn adapt_fg(c: Color32, bg: Color32, fallback: Color32) -> Color32 {
    if contrast(c, bg) >= 3.5 {
        return c;
    }
    let (h, s, l) = to_hsl(c);
    let inv = from_hsl(h, s, (1.0 - l).max(0.62), 255);
    if contrast(inv, bg) >= 3.0 { inv } else { fallback }
}

// ------------------------------------------------------------------ layout

/// Underline segments for sections with an underline color.
fn underlines(g: &Galley, unders: &[(usize, usize, Color32)]) -> Vec<(Rect, Color32)> {
    if unders.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut ci = 0usize;
    for row in &g.rows {
        let mut run: Option<(f32, f32, f32, Color32)> = None;
        for (i, glyph) in row.glyphs.iter().enumerate() {
            let c = ci + i;
            let u = unders.iter().find(|(s, e, _)| *s <= c && c < *e).map(|u| u.2);
            let x0 = row.pos.x + glyph.pos.x;
            let x1 = x0 + glyph.advance_width;
            let y = (row.pos.y + glyph.pos.y + 1.5).round();
            match (&mut run, u) {
                (Some(r), Some(col)) if r.3 == col && (r.2 - y).abs() < 0.5 => r.1 = x1,
                (_, u) => {
                    if let Some((a, b, y, col)) = run.take() {
                        out.push((Rect::from_min_max(pos2(a, y), pos2(b, y + 1.0)), col));
                    }
                    if let Some(col) = u {
                        run = Some((x0, x1, y, col));
                    }
                }
            }
        }
        if let Some((a, b, y, col)) = run {
            out.push((Rect::from_min_max(pos2(a, y), pos2(b, y + 1.0)), col));
        }
        ci += row.char_count_including_newline().0;
    }
    out
}

/// Underlined character ranges with their color.
type Unders = Vec<(usize, usize, Color32)>;

enum Seg<'a> {
    Text(LayoutJob, Vec<(usize, usize, u32)>, Unders),
    Img(&'a ImgRef),
}

struct Lx<'a> {
    ctx: &'a egui::Context,
    doc: &'a Doc,
    o: &'a LayoutOpts,
    items: Vec<Item>,
    bg_stack: Vec<Color32>,
    quote_depth: usize,
}

/// Lay out `doc` at `o.width`.
pub fn layout(ctx: &egui::Context, doc: &Doc, o: &LayoutOpts) -> LayoutOut {
    {
        let mut m = doc.measure.lock();
        if m.0 != o.pixels_per_point {
            m.0 = o.pixels_per_point;
            m.1.clear();
        }
    }
    let mut lx = Lx { ctx, doc, o, items: Vec::new(), bg_stack: vec![o.canvas], quote_depth: 0 };
    let h = lx.blocks(&doc.blocks, 0.0, 0.0, o.width.max(40.0), true, true);
    LayoutOut { size: vec2(o.width, h.ceil()), items: lx.items, placeholder: o.placeholder_bg, muted: o.text_secondary }
}

impl Lx<'_> {
    fn canvas(&self) -> Color32 {
        *self.bg_stack.last().unwrap_or(&self.o.canvas)
    }

    fn bg(&self, c: Color32) -> Option<Color32> {
        if self.o.adapt_dark { adapt_bg(c) } else { Some(c) }
    }

    fn text_color(&self, st: &TextSt) -> Color32 {
        if st.dim {
            return self.o.text_secondary;
        }
        let explicit = st.color.or(if st.link.is_some() { None } else { self.doc.text_color });
        match explicit {
            Some(c) if self.o.adapt_dark => adapt_fg(c, self.canvas(), self.o.text),
            Some(c) => c,
            None if st.link.is_some() => self.o.link,
            None if self.quote_depth > 0 && self.o.quote_text => {
                let q = self.o.quote_colors[(self.quote_depth - 1) % 3];
                let t = self.o.text;
                let mix = |a: u8, b: u8| (a as f32 * 0.55 + b as f32 * 0.45).round() as u8;
                Color32::from_rgb(mix(t.r(), q.r()), mix(t.g(), q.g()), mix(t.b(), q.b()))
            }
            None => self.o.text,
        }
    }

    fn font(&self, st: &TextSt) -> FontId {
        let fam = match st.family {
            Family::Body => fonts::message_styled(st.bold, st.italic),
            Family::Serif => fonts::serif(),
            Family::Mono => fonts::fixed(),
        };
        FontId::new(st.size.max(4.0), fam)
    }

    fn format(&self, st: &TextSt) -> TextFormat {
        let color = self.text_color(st);
        let mut f = TextFormat::simple(self.font(st), color);
        f.italics = st.italic && st.family != Family::Body;
        if st.strike {
            f.strikethrough = Stroke::new(1.0, color);
        }
        if let Some(bg) = st.bg.and_then(|b| self.bg(b)) {
            f.background = bg;
        }
        f.line_height = Some(st.line_height.unwrap_or(st.size * self.o.line_height).round());
        f.valign = match st.script {
            Script::Sup => Align::TOP,
            Script::Sub | Script::Normal => Align::BOTTOM,
        };
        f
    }

    fn galley(&self, job: LayoutJob) -> Arc<Galley> {
        self.ctx.fonts_mut(|f| f.layout_job(job))
    }

    // ---------------------------------------------------------- measuring

    /// Collapsed outer (top, bottom) margins of a block.
    fn margins(b: &Block) -> (f32, f32) {
        match b {
            Block::Box { style, children } => {
                let mut t = style.margin[0];
                let mut bo = style.margin[2];
                if style.padding[0] + style.border[0] == 0.0 && style.bg.is_none()
                    && let Some(f) = children.first() {
                        t = t.max(Self::margins(f).0);
                    }
                if style.padding[2] + style.border[2] == 0.0 && style.bg.is_none() && style.min_height.is_none()
                    && let Some(l) = children.last() {
                        bo = bo.max(Self::margins(l).1);
                    }
                (t, bo)
            }
            Block::Table(t) => (t.style.margin[0], t.style.margin[2]),
            _ => (0.0, 0.0),
        }
    }

    /// (min-content, max-content) widths of blocks stacked vertically.
    fn measure_blocks(&self, blocks: &[Block]) -> (f32, f32) {
        let mut r = (0.0f32, 0.0f32);
        for b in blocks {
            let m = self.measure(b);
            r.0 = r.0.max(m.0);
            r.1 = r.1.max(m.1);
        }
        r
    }

    fn measure(&self, b: &Block) -> (f32, f32) {
        match b {
            Block::Para(p) => self.cached(b, || self.measure_para(p)),
            Block::Table(t) => self.cached(b, || self.measure_table(t)),
            Block::Box { style, children } => {
                let (mut mn, mut mx) = self.measure_blocks(children);
                let ex = style.h_extra() + style.margin[1].max(0.0) + style.margin[3].max(0.0);
                mn += ex;
                mx += ex;
                if let Some(Len::Px(w)) = style.width {
                    mx = (w + style.h_extra()).max(mn);
                }
                if let Some(Len::Px(w)) = style.max_width {
                    mx = mx.min(w + style.h_extra()).max(mn);
                }
                (mn, mx)
            }
            Block::Quote { children, .. } => {
                let (a, b) = self.measure_blocks(children);
                (a + QUOTE_INDENT, b + QUOTE_INDENT)
            }
            Block::ListItem { children, .. } => self.measure_blocks(children),
            Block::Collapse { children, key, .. } => {
                if self.o.expanded.contains(key) {
                    let (a, b) = self.measure_blocks(children);
                    (a.max(140.0), b.max(140.0))
                } else {
                    (140.0, 140.0)
                }
            }
            Block::Rule { .. } | Block::Gap(_) => (0.0, 0.0),
        }
    }

    fn cached(&self, b: &Block, f: impl FnOnce() -> (f32, f32)) -> (f32, f32) {
        let k = b as *const Block as usize;
        if let Some(v) = self.doc.measure.lock().1.get(&k) {
            return *v;
        }
        let v = f();
        self.doc.measure.lock().1.insert(k, v);
        v
    }

    fn segments<'p>(&self, p: &'p Para) -> Vec<Seg<'p>> {
        let mut segs = Vec::new();
        let mut job = LayoutJob::default();
        let mut links: Vec<(usize, usize, u32)> = Vec::new();
        let mut unders: Unders = Vec::new();
        let mut chars = 0usize;
        let mut last_fmt: Option<TextFormat> = None;
        for inl in &p.inlines {
            match inl {
                Inline::Text(t, st) => {
                    if t.is_empty() {
                        continue;
                    }
                    let f = self.format(st);
                    let n = t.chars().count();
                    if st.underline {
                        unders.push((chars, chars + n, f.color.gamma_multiply(0.75)));
                    }
                    if let Some(l) = st.link {
                        links.push((chars, chars + n, l));
                    }
                    chars += n;
                    job.append(t, 0.0, f.clone());
                    last_fmt = Some(f);
                }
                Inline::Break => {
                    let f = last_fmt.clone().unwrap_or_else(|| self.format(&TextSt::new(14.0)));
                    job.append("\n", 0.0, f);
                    chars += 1;
                }
                Inline::Image(r) => {
                    if !job.text.is_empty() {
                        segs.push(Seg::Text(std::mem::take(&mut job), std::mem::take(&mut links), std::mem::take(&mut unders)));
                        chars = 0;
                    }
                    segs.push(Seg::Img(r));
                }
            }
        }
        if !job.text.is_empty() {
            segs.push(Seg::Text(job, links, unders));
        }
        segs
    }

    fn measure_para(&self, p: &Para) -> (f32, f32) {
        let mut mn = 0.0f32;
        let mut line = 0.0f32;
        let mut mx = 0.0f32;
        for s in self.segments(p) {
            match s {
                Seg::Text(job, _, _) => {
                    let mut j = job.clone();
                    j.wrap = TextWrapping::no_max_width();
                    let g = self.galley(j);
                    let rows = &g.rows;
                    if rows.len() > 1 {
                        mx = mx.max(line + rows[0].rect().width());
                        for r in &rows[1..rows.len() - 1] {
                            mx = mx.max(r.rect().width());
                        }
                        line = rows.last().map(|r| r.rect().width()).unwrap_or(0.0);
                    } else {
                        line += g.rect.width();
                    }
                    if !p.pre {
                        let mut j = job;
                        j.wrap = TextWrapping::wrap_at_width(1.0);
                        mn = mn.max(self.galley(j).rect.width());
                    } else {
                        mn = mn.max(g.rect.width().min(320.0));
                    }
                }
                Seg::Img(r) => {
                    let w = self.image_size(r, f32::INFINITY).x;
                    line += w;
                    mn = mn.max(w.min(64.0));
                }
            }
        }
        mx = mx.max(line);
        (mn.ceil(), mx.ceil().max(mn.ceil()))
    }

    // ---------------------------------------------------------- images

    fn image_content(&self, r: &ImgRef) -> (Option<Vec2>, ImageContent) {
        let di = &self.doc.images[r.idx as usize];
        match (&di.src, &di.bytes) {
            (_, Some(b)) => (di.size, ImageContent::Bytes { uri: di.uri.clone(), bytes: b.clone() }),
            (ImgSrc::Remote(url), None) => {
                if self.o.allow_remote {
                    match remote::get(url, self.ctx) {
                        RemoteImage::Ready { uri, bytes, size } => (size, ImageContent::Bytes { uri, bytes }),
                        RemoteImage::Loading => (None, ImageContent::Placeholder { alt: r.alt.clone(), loading: true }),
                        RemoteImage::Failed => (None, ImageContent::Placeholder { alt: r.alt.clone(), loading: false }),
                    }
                } else {
                    (None, ImageContent::Placeholder { alt: r.alt.clone(), loading: false })
                }
            }
            _ => (None, ImageContent::Placeholder { alt: r.alt.clone(), loading: false }),
        }
    }

    fn image_size(&self, r: &ImgRef, avail: f32) -> Vec2 {
        let di = &self.doc.images[r.idx as usize];
        let intrinsic = di.size.or_else(|| match &di.src {
            ImgSrc::Remote(u) if self.o.allow_remote => match remote::peek(u) {
                Some(RemoteImage::Ready { size, .. }) => size,
                _ => None,
            },
            _ => None,
        });
        let base = if avail.is_finite() { avail } else { 0.0 };
        let w = r.width.and_then(|l| l.resolve(base)).filter(|w| *w > 0.0);
        let h = r.height.filter(|h| *h > 0.0);
        let mut size = match (w, h, intrinsic) {
            (Some(w), Some(h), _) => vec2(w, h),
            (Some(w), None, Some(i)) => vec2(w, if i.x > 0.0 { w * i.y / i.x } else { w }),
            (None, Some(h), Some(i)) => vec2(if i.y > 0.0 { h * i.x / i.y } else { h }, h),
            (None, None, Some(i)) => i,
            (Some(w), None, None) => vec2(w, if r.alt.is_empty() { 0.0 } else { 28.0 }),
            (None, Some(h), None) => vec2(if r.alt.is_empty() { h } else { 120.0 }, h),
            (None, None, None) => {
                if r.alt.is_empty() {
                    Vec2::ZERO
                } else {
                    vec2((r.alt.chars().count() as f32 * 7.0 + 36.0).min(260.0), 28.0)
                }
            }
        };
        if avail.is_finite() && size.x > avail && size.x > 0.0 {
            size = vec2(avail, size.y * avail / size.x);
        }
        size
    }

    // ---------------------------------------------------------- blocks

    fn blocks(&mut self, blocks: &[Block], x: f32, y: f32, w: f32, trim_top: bool, trim_bottom: bool) -> f32 {
        let mut cy = y;
        let mut prev_mb = 0.0f32;
        for (i, b) in blocks.iter().enumerate() {
            let (mt, mb) = Self::margins(b);
            let gap = if i == 0 { if trim_top { 0.0 } else { mt } } else { prev_mb.max(mt) };
            cy += gap.max(0.0);
            cy += self.block(b, x, cy, w);
            prev_mb = mb;
        }
        if !trim_bottom {
            cy += prev_mb.max(0.0);
        }
        cy - y
    }

    fn block(&mut self, b: &Block, x: f32, y: f32, w: f32) -> f32 {
        match b {
            Block::Para(p) => self.para(p, x, y, w),
            Block::Gap(g) => *g,
            Block::Rule { color, thickness } => {
                let c = color.and_then(|c| self.bg(c)).unwrap_or(self.o.rule);
                self.items.push(Item::Rect { rect: Rect::from_min_size(pos2(x, y), vec2(w, *thickness)), fill: c, radius: 0.0 });
                *thickness
            }
            Block::Quote { level, children } => {
                self.quote_depth += 1;
                let h = self.blocks(children, x + QUOTE_INDENT, y, (w - QUOTE_INDENT).max(20.0), true, true);
                self.quote_depth -= 1;
                let color = self.o.quote_colors[((*level as usize).max(1) - 1) % 3];
                self.items.push(Item::Bar { rect: Rect::from_min_size(pos2(x + 1.0, y), vec2(3.0, h.max(4.0))), color });
                h
            }
            Block::ListItem { marker, marker_style, children } => {
                let h = self.blocks(children, x, y, w, true, true);
                if !marker.is_empty() {
                    let st = marker_style.clone();
                    let f = self.format(&st);
                    let g = self.galley(LayoutJob::single_section(marker.clone(), f));
                    // Align the marker with the first line of the item.
                    let first_line = self.first_line_height(children).unwrap_or(g.size().y);
                    let my = y + ((first_line - g.size().y) / 2.0).max(0.0);
                    let pos = pos2(x - LIST_GUTTER - g.size().x, my);
                    self.items.push(Item::Text(TextItem { pos, galley: g, links: vec![], underlines: vec![] }));
                }
                h.max(1.0)
            }
            Block::Collapse { key, label, children } => {
                let open = self.o.expanded.contains(key);
                let text = if open { format!("Hide {label}") } else { format!("Show {label}") };
                let g = self.galley(LayoutJob::simple_singleline(text.clone(), FontId::proportional(12.0), self.o.text_secondary));
                let rect = Rect::from_min_size(pos2(x, y + 4.0), vec2(g.size().x + 40.0, 22.0));
                self.items.push(Item::Toggle { rect, key: *key, open, label: text });
                let mut h = 30.0;
                if open {
                    h += 4.0 + self.blocks(children, x, y + h + 4.0, w, true, true);
                }
                h
            }
            Block::Box { style, children } => self.box_block(style, children, x, y, w),
            Block::Table(t) => self.table(t, x, y, w),
        }
    }

    fn first_line_height(&self, blocks: &[Block]) -> Option<f32> {
        match blocks.first()? {
            Block::Para(p) => p.inlines.iter().find_map(|i| if let Inline::Text(_, st) = i { Some(st.line_height.unwrap_or(st.size * self.o.line_height).round()) } else { None }),
            Block::Box { children, style } if style.padding[0] == 0.0 => self.first_line_height(children),
            _ => None,
        }
    }

    fn reserve(&mut self) -> usize {
        self.items.push(Item::Rect { rect: Rect::NOTHING, fill: Color32::TRANSPARENT, radius: 0.0 });
        self.items.len() - 1
    }

    fn decorate(&mut self, slot: usize, rect: Rect, style: &BoxStyle, bg: Option<Color32>) {
        if let Some(fill) = bg {
            self.items[slot] = Item::Rect { rect, fill, radius: style.radius };
        }
        if style.border.iter().any(|b| *b > 0.0) {
            let def = self.o.rule;
            let colors = style.border_color.map(|c| c.and_then(|c| self.bg(c)).unwrap_or(def));
            self.items.push(Item::Border { rect, widths: style.border, colors, radius: style.radius });
        }
    }

    fn box_block(&mut self, style: &BoxStyle, children: &[Block], x: f32, y: f32, w: f32) -> f32 {
        let ml = style.margin[3].max(0.0);
        let mr = style.margin[1].max(0.0);
        let mut x = x + ml;
        let avail = (w - ml - mr).max(10.0);
        let extra = style.h_extra();
        let mut bw = avail;
        if let Some(width) = style.width.and_then(|l| l.resolve(avail)) {
            bw = (width + extra).min(avail);
        }
        if let Some(mw) = style.max_width.and_then(|l| l.resolve(avail)) {
            bw = bw.min(mw + extra);
        }
        if style.center && bw < avail {
            x += (avail - bw) / 2.0;
        }
        let bg = style.bg.and_then(|c| self.bg(c));
        let slot = self.reserve();
        if let Some(b) = bg {
            self.bg_stack.push(b);
        }
        let cx = x + style.border[3] + style.padding[3];
        let top = style.border[0] + style.padding[0];
        let bottom = style.border[2] + style.padding[2];
        let cw = (bw - extra).max(4.0);
        let trim_top = top == 0.0 && bg.is_none();
        let trim_bottom = bottom == 0.0 && bg.is_none() && style.min_height.is_none();
        let ch = self.blocks(children, cx, y + top, cw, trim_top, trim_bottom);
        if bg.is_some() {
            self.bg_stack.pop();
        }
        let h = (top + ch + bottom).max(style.min_height.unwrap_or(0.0));
        self.decorate(slot, Rect::from_min_size(pos2(x, y), vec2(bw, h)), style, bg);
        h
    }

    // ---------------------------------------------------------- paragraphs

    fn para(&mut self, p: &Para, x: f32, y: f32, w: f32) -> f32 {
        let segs = self.segments(p);
        let w = w.max(8.0);
        let halign = match p.align {
            HAlign::Center => Align::Center,
            HAlign::Right => Align::RIGHT,
            _ => Align::LEFT,
        };
        if segs.is_empty() {
            return 0.0;
        }
        if segs.len() == 1 && matches!(segs[0], Seg::Text(..)) {
            let Some(Seg::Text(mut job, links, unders)) = segs.into_iter().next() else { return 0.0 };
            job.wrap = TextWrapping::wrap_at_width(w);
            job.halign = halign;
            job.justify = p.align == HAlign::Justify;
            let g = self.galley(job);
            let dx = match halign {
                Align::Center => w / 2.0,
                Align::RIGHT => w,
                _ => 0.0,
            };
            let h = g.size().y;
            let underlines = underlines(&g, &unders);
            self.items.push(Item::Text(TextItem { pos: pos2(x + dx, y), galley: g, links, underlines }));
            return h;
        }
        self.flow(segs, x, y, w, p.align)
    }

    /// Inline flow of text and images.
    fn flow(&mut self, segs: Vec<Seg>, x: f32, y: f32, w: f32, align: HAlign) -> f32 {
        struct LineItem {
            idx: usize,
            h: f32,
            vshift: bool,
            hshift: bool,
        }
        let mut cx = 0.0f32;
        let mut top = y;
        let mut line: Vec<LineItem> = Vec::new();
        let close = |lx: &mut Self, line: &mut Vec<LineItem>, top: f32, used: f32| -> f32 {
            let lh = line.iter().map(|l| l.h).fold(0.0, f32::max);
            let dx = match align {
                HAlign::Center => ((w - used) / 2.0).max(0.0),
                HAlign::Right => (w - used).max(0.0),
                _ => 0.0,
            };
            for l in line.iter() {
                let dy = if l.vshift { lh - l.h } else { 0.0 };
                let d = vec2(if l.hshift { dx } else { 0.0 }, dy);
                if d != Vec2::ZERO {
                    lx.items[l.idx].translate(d);
                }
            }
            line.clear();
            top + lh
        };
        for seg in segs {
            match seg {
                Seg::Img(r) => {
                    let size = self.image_size(r, w);
                    if size.x <= 0.0 || size.y <= 0.0 {
                        continue;
                    }
                    if cx > 0.0 && cx + size.x > w + 0.5 {
                        top = close(self, &mut line, top, cx);
                        cx = 0.0;
                    }
                    let (_, content) = self.image_content(r);
                    self.items.push(Item::Image { rect: Rect::from_min_size(pos2(x + cx, top), size), img: r.idx, link: r.link, content });
                    line.push(LineItem { idx: self.items.len() - 1, h: size.y, vshift: true, hshift: true });
                    cx += size.x;
                }
                Seg::Text(mut job, links, unders) => {
                    if let Some(s) = job.sections.first_mut() {
                        s.leading_space = cx;
                    }
                    job.wrap = TextWrapping::wrap_at_width(w);
                    let g = self.galley(job);
                    if g.rows.is_empty() {
                        continue;
                    }
                    let n = g.rows.len();
                    let first_h = g.rows[0].rect().height();
                    let last = g.rows[n - 1].clone();
                    let underlines = underlines(&g, &unders);
                    self.items.push(Item::Text(TextItem { pos: pos2(x, top), galley: g, links, underlines }));
                    let idx = self.items.len() - 1;
                    if n == 1 {
                        line.push(LineItem { idx, h: first_h, vshift: true, hshift: true });
                        cx = last.rect().max.x;
                    } else {
                        line.push(LineItem { idx, h: first_h, vshift: true, hshift: false });
                        let used = cx;
                        close(self, &mut line, top, used);
                        let gy = match &self.items[idx] {
                            Item::Text(t) => t.pos.y,
                            _ => top,
                        };
                        top = gy + last.pos.y;
                        cx = last.rect().max.x;
                        line.push(LineItem { idx, h: last.rect().height(), vshift: false, hshift: false });
                    }
                }
            }
        }
        let bottom = close(self, &mut line, top, cx);
        bottom - y
    }

    // ---------------------------------------------------------- tables

    fn grid(t: &Table) -> (usize, Vec<(usize, usize, &TableCell)>) {
        let mut occupied: Vec<Vec<bool>> = vec![Vec::new(); t.rows.len()];
        let mut places = Vec::new();
        let mut ncols = 0;
        for (ri, row) in t.rows.iter().enumerate() {
            let mut col = 0;
            for cell in &row.cells {
                while occupied[ri].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                let rs = cell.rowspan.min(t.rows.len() - ri).max(1);
                for occ in occupied.iter_mut().skip(ri).take(rs) {
                    if occ.len() < col + cell.colspan {
                        occ.resize(col + cell.colspan, false);
                    }
                    for o in occ.iter_mut().skip(col).take(cell.colspan) {
                        *o = true;
                    }
                }
                places.push((ri, col, cell));
                col += cell.colspan;
                ncols = ncols.max(col);
            }
        }
        (ncols, places)
    }

    /// Column constraints: (min, max, fixed px, percent).
    #[allow(clippy::type_complexity)]
    fn columns(&self, t: &Table, ncols: usize, places: &[(usize, usize, &TableCell)]) -> (Vec<f32>, Vec<f32>, Vec<Option<f32>>, Vec<Option<f32>>) {
        let mut mn = vec![0.0f32; ncols];
        let mut mx = vec![0.0f32; ncols];
        let mut fixed: Vec<Option<f32>> = vec![None; ncols];
        let mut pct: Vec<Option<f32>> = vec![None; ncols];
        let mut spanned = Vec::new();
        for &(_, c, cell) in places {
            let (a, b) = self.measure_blocks(&cell.blocks);
            let ex = cell.style.h_extra();
            let (a, b) = (a + ex, b + ex);
            if cell.colspan == 1 {
                mn[c] = mn[c].max(a);
                mx[c] = mx[c].max(b);
                match cell.width {
                    Some(Len::Px(px)) => fixed[c] = Some(fixed[c].unwrap_or(0.0).max(px)),
                    Some(Len::Percent(p)) => pct[c] = Some(pct[c].unwrap_or(0.0).max(p)),
                    _ => {}
                }
            } else {
                spanned.push((c, cell.colspan.min(ncols - c), a, b, cell.width));
            }
        }
        let sp = t.spacing;
        for (c, n, a, b, width) in spanned {
            let gaps = sp * (n as f32 - 1.0);
            let cur_min: f32 = mn[c..c + n].iter().sum::<f32>() + gaps;
            let cur_max: f32 = mx[c..c + n].iter().sum::<f32>() + gaps;
            let want_max = match width {
                Some(Len::Px(px)) => b.max(px),
                _ => b,
            };
            if a > cur_min {
                let add = (a - cur_min) / n as f32;
                for v in &mut mn[c..c + n] {
                    *v += add;
                }
            }
            if want_max > cur_max {
                let add = (want_max - cur_max) / n as f32;
                for v in &mut mx[c..c + n] {
                    *v += add;
                }
            }
        }
        for c in 0..ncols {
            if let Some(f) = fixed[c] {
                mx[c] = f.max(mn[c]);
            }
            mx[c] = mx[c].max(mn[c]);
        }
        (mn, mx, fixed, pct)
    }

    fn table_extras(t: &Table, ncols: usize) -> f32 {
        t.spacing * (ncols as f32 + 1.0) + t.style.h_extra()
    }

    fn measure_table(&self, t: &Table) -> (f32, f32) {
        let (ncols, places) = Self::grid(t);
        if ncols == 0 {
            return (t.style.h_extra(), t.style.h_extra());
        }
        let (mn, mx, _, _) = self.columns(t, ncols, &places);
        let ex = Self::table_extras(t, ncols) + t.style.margin[1].max(0.0) + t.style.margin[3].max(0.0);
        let smin = mn.iter().sum::<f32>() + ex;
        let mut smax = mx.iter().sum::<f32>() + ex;
        if let Some(Len::Px(w)) = t.width {
            smax = w.max(smin);
        }
        (smin, smax.max(smin))
    }

    fn table(&mut self, t: &Table, x: f32, y: f32, w: f32) -> f32 {
        let ml = t.style.margin[3].max(0.0);
        let mr = t.style.margin[1].max(0.0);
        let x = x + ml;
        let avail = (w - ml - mr).max(10.0);
        let (ncols, places) = Self::grid(t);
        if ncols == 0 {
            return 0.0;
        }
        let (mn, mx, fixed, pct) = self.columns(t, ncols, &places);
        let extras = Self::table_extras(t, ncols);
        let sum_min: f32 = mn.iter().sum();
        let sum_max: f32 = mx.iter().sum();
        let mut target = match t.width.and_then(|l| l.resolve(avail)) {
            Some(tw) => tw.min(avail),
            None => (sum_max + extras).min(avail),
        };
        target = target.max((sum_min + extras).min(avail));
        let content = (target - extras).max(sum_min.min(avail));
        // Distribute widths.
        let mut widths = mn.clone();
        let mut remaining = content - sum_min;
        if remaining > 0.0 {
            let desired: Vec<Option<f32>> = (0..ncols).map(|c| fixed[c].or(pct[c].map(|p| p / 100.0 * content))).collect();
            let need: Vec<f32> = (0..ncols).map(|c| desired[c].map(|d| (d - widths[c]).max(0.0)).unwrap_or(0.0)).collect();
            let total: f32 = need.iter().sum();
            if total > 0.0 {
                let give = remaining.min(total);
                for c in 0..ncols {
                    widths[c] += give * need[c] / total;
                }
                remaining -= give;
            }
            let auto: Vec<usize> = (0..ncols).filter(|c| desired[*c].is_none()).collect();
            if remaining > 0.0 {
                let need: Vec<f32> = auto.iter().map(|&c| (mx[c] - widths[c]).max(0.0)).collect();
                let total: f32 = need.iter().sum();
                if total > 0.0 {
                    let give = remaining.min(total);
                    for (i, &c) in auto.iter().enumerate() {
                        widths[c] += give * need[i] / total;
                    }
                    remaining -= give;
                }
            }
            if remaining > 0.5 {
                let cols: Vec<usize> = if auto.is_empty() { (0..ncols).collect() } else { auto };
                let weight: f32 = cols.iter().map(|&c| mx[c].max(1.0)).sum();
                for &c in &cols {
                    widths[c] += remaining * mx[c].max(1.0) / weight;
                }
            }
        }
        let table_w = widths.iter().sum::<f32>() + extras;
        let tx = match t.align {
            HAlign::Center if table_w < avail => x + (avail - table_w) / 2.0,
            HAlign::Right if table_w < avail => x + avail - table_w,
            _ => x,
        };
        let sp = t.spacing;
        let st = &t.style;
        let inner_x = tx + st.border[3] + st.padding[3];
        let mut col_x = vec![inner_x + sp; ncols + 1];
        for c in 0..ncols {
            col_x[c + 1] = col_x[c] + widths[c] + sp;
        }
        let tbg = st.bg.and_then(|c| self.bg(c));
        let tslot = self.reserve();
        if let Some(b) = tbg {
            self.bg_stack.push(b);
        }
        let top = y + st.border[0] + st.padding[0];
        let nrows = t.rows.len();
        let mut row_y = vec![top + sp; nrows + 1];
        let mut row_h = vec![0.0f32; nrows];
        struct Pending {
            ri: usize,
            rs: usize,
            slot: usize,
            range: (usize, usize),
            x: f32,
            w: f32,
            h: f32,
            bg: Option<Color32>,
            valign: VAlign,
            style: BoxStyle,
        }
        let mut pending: Vec<Pending> = Vec::new();
        let mut row_slots = vec![usize::MAX; nrows];
        let mut pi = 0;
        for ri in 0..nrows {
            let row = &t.rows[ri];
            let rbg = row.bg.and_then(|c| self.bg(c));
            if let Some(b) = rbg {
                row_slots[ri] = self.reserve();
                self.bg_stack.push(b);
            }
            while pi < places.len() && places[pi].0 == ri {
                let (_, c, cell) = places[pi];
                pi += 1;
                let cs = cell.colspan.min(ncols - c);
                let cw = col_x[c + cs] - col_x[c] - sp;
                let cx = col_x[c];
                let cbg = cell.style.bg.and_then(|b| self.bg(b));
                let slot = self.reserve();
                if let Some(b) = cbg {
                    self.bg_stack.push(b);
                }
                let s = &cell.style;
                let start = self.items.len();
                let ch = self.blocks(&cell.blocks, cx + s.border[3] + s.padding[3], row_y[ri] + s.border[0] + s.padding[0], (cw - s.h_extra()).max(2.0), false, false);
                if cbg.is_some() {
                    self.bg_stack.pop();
                }
                let h = (ch + s.border[0] + s.padding[0] + s.border[2] + s.padding[2]).max(cell.height.unwrap_or(0.0));
                let rs = cell.rowspan.min(nrows - ri).max(1);
                if rs == 1 {
                    row_h[ri] = row_h[ri].max(h);
                }
                pending.push(Pending { ri, rs, slot, range: (start, self.items.len()), x: cx, w: cw, h, bg: cbg, valign: cell.valign, style: cell.style.clone() });
            }
            if rbg.is_some() {
                self.bg_stack.pop();
            }
            row_h[ri] = row_h[ri].max(row.height.unwrap_or(0.0));
            // Row-spanning cells ending here may need more height.
            for p in pending.iter().filter(|p| p.ri + p.rs - 1 == ri && p.rs > 1) {
                let have: f32 = row_h[p.ri..=ri].iter().sum::<f32>() + sp * (p.rs as f32 - 1.0);
                if p.h > have {
                    row_h[ri] += p.h - have;
                }
            }
            row_y[ri + 1] = row_y[ri] + row_h[ri] + sp;
        }
        // Finalize cells: vertical alignment, backgrounds, borders.
        for p in pending {
            let full = row_y[p.ri + p.rs] - sp - row_y[p.ri];
            let dy = match p.valign {
                VAlign::Top => 0.0,
                VAlign::Middle => ((full - p.h) / 2.0).max(0.0),
                VAlign::Bottom => (full - p.h).max(0.0),
            };
            if dy > 0.0 {
                for it in &mut self.items[p.range.0..p.range.1] {
                    it.translate(vec2(0.0, dy));
                }
            }
            let rect = Rect::from_min_size(pos2(p.x, row_y[p.ri]), vec2(p.w, full));
            if let Some(fill) = p.bg {
                self.items[p.slot] = Item::Rect { rect, fill, radius: p.style.radius };
            }
            if p.style.border.iter().any(|b| *b > 0.0) {
                let def = self.o.rule;
                let colors = p.style.border_color.map(|c| c.and_then(|c| self.bg(c)).unwrap_or(def));
                self.items.push(Item::Border { rect, widths: p.style.border, colors, radius: p.style.radius });
            }
        }
        for ri in 0..nrows {
            if row_slots[ri] != usize::MAX
                && let Some(fill) = t.rows[ri].bg.and_then(|c| self.bg(c)) {
                    let rect = Rect::from_min_max(pos2(col_x[0] - sp, row_y[ri]), pos2(col_x[ncols], row_y[ri] + row_h[ri]));
                    self.items[row_slots[ri]] = Item::Rect { rect, fill, radius: 0.0 };
                }
        }
        if tbg.is_some() {
            self.bg_stack.pop();
        }
        let h = (row_y[nrows] - y) + st.padding[2] + st.border[2];
        let rect = Rect::from_min_size(pos2(tx, y), vec2(table_w, h));
        self.decorate(tslot, rect, st, tbg);
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context whose Purpamail font families map to egui's default fonts.
    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut defs = egui::FontDefinitions::default();
        let prop = defs.families[&egui::FontFamily::Proportional].clone();
        for name in [fonts::MESSAGE, fonts::MESSAGE_BOLD, fonts::MESSAGE_ITALIC, fonts::MESSAGE_BOLD_ITALIC, fonts::FIXED, fonts::SERIF, fonts::BOLD] {
            defs.families.insert(egui::FontFamily::Name(name.into()), prop.clone());
        }
        ctx.set_fonts(defs);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        ctx
    }

    fn opts(width: f32) -> LayoutOpts {
        LayoutOpts {
            width,
            adapt_dark: false,
            canvas: Color32::WHITE,
            text: Color32::BLACK,
            text_secondary: Color32::GRAY,
            link: Color32::BLUE,
            quote_colors: [Color32::RED; 3],
            quote_text: false,
            rule: Color32::GRAY,
            placeholder_bg: Color32::LIGHT_GRAY,
            allow_remote: false,
            expanded: HashSet::new(),
            line_height: 1.4,
            pixels_per_point: 1.0,
        }
    }

    fn run(doc: &Doc, width: f32) -> LayoutOut {
        let ctx = test_ctx();
        let mut out = None;
        ctx.run_ui(egui::RawInput::default(), |ui| out = Some(layout(ui.ctx(), doc, &opts(width)))).textures_delta.clear();
        out.unwrap()
    }

    fn text_items(l: &LayoutOut) -> Vec<(String, Rect)> {
        l.items.iter().filter_map(|i| if let Item::Text(t) = i { Some((t.galley.text().to_string(), t.rect())) } else { None }).collect()
    }

    fn doc(html: &str) -> Doc {
        let resolve = |_: &str| None;
        build_html(html, &BuildOpts { base_size: 14.0, resolve_cid: &resolve })
    }

    #[test]
    fn table_columns_and_centering() {
        let d = doc("<table width=600 align=center cellpadding=0 cellspacing=0><tr><td width='50%'>Left</td><td width='50%'>Right</td></tr></table>");
        let l = run(&d, 800.0);
        let t = text_items(&l);
        let left = t.iter().find(|(s, _)| s == "Left").unwrap().1;
        let right = t.iter().find(|(s, _)| s == "Right").unwrap().1;
        // 600px table centered in 800px: columns start at 100 and 400
        assert!((left.min.x - 100.0).abs() < 1.5, "{left:?}");
        assert!((right.min.x - 400.0).abs() < 1.5, "{right:?}");
        // narrower than the table: it shrinks to fit
        let l = run(&d, 300.0);
        let t = text_items(&l);
        let right = t.iter().find(|(s, _)| s == "Right").unwrap().1;
        assert!(right.min.x >= 140.0 && right.max.x <= 302.0, "{right:?}");
    }

    #[test]
    fn auto_table_shrinks_to_content() {
        let d = doc("<table cellspacing=0 cellpadding=4><tr><td>a</td><td>bb</td></tr><tr><td colspan=2>wide cell spanning both columns</td></tr></table><p>after</p>");
        let l = run(&d, 900.0);
        let t = text_items(&l);
        let wide = t.iter().find(|(s, _)| s.starts_with("wide")).unwrap().1;
        let after = t.iter().find(|(s, _)| s == "after").unwrap().1;
        assert!(wide.max.x < 400.0, "auto table should not stretch: {wide:?}");
        assert!(after.min.y > wide.max.y);
    }

    #[test]
    fn quotes_lists_and_valign() {
        let d = doc("<blockquote type=cite>quoted</blockquote><ul><li>item</li></ul><table cellspacing=0 cellpadding=0><tr><td valign=bottom>low</td><td><div style='height:100px'>tall</div></td></tr></table>");
        let l = run(&d, 500.0);
        assert!(l.items.iter().any(|i| matches!(i, Item::Bar { .. })));
        let t = text_items(&l);
        let quoted = t.iter().find(|(s, _)| s == "quoted").unwrap().1;
        assert!(quoted.min.x >= QUOTE_INDENT);
        let bullet = t.iter().find(|(s, _)| s == "•").unwrap().1;
        let item = t.iter().find(|(s, _)| s == "item").unwrap().1;
        assert!(bullet.max.x <= item.min.x);
        let low = t.iter().find(|(s, _)| s == "low").unwrap().1;
        let tall = t.iter().find(|(s, _)| s == "tall").unwrap().1;
        assert!(low.min.y > tall.min.y + 50.0, "bottom-aligned cell: {low:?} vs {tall:?}");
    }

    #[test]
    fn images_scale_to_width() {
        let d = doc("<p><img src='https://x.example/a.png' width=800 height=400 alt='Banner'></p>");
        let l = run(&d, 400.0);
        let img = l.items.iter().find_map(|i| if let Item::Image { rect, content, .. } = i { Some((*rect, content.clone())) } else { None }).unwrap();
        assert!((img.0.width() - 400.0).abs() < 0.5 && (img.0.height() - 200.0).abs() < 0.5, "{:?}", img.0);
        assert!(matches!(img.1, ImageContent::Placeholder { .. }), "remote images are blocked by default");
    }

    #[test]
    fn color_adaptation() {
        let dark = Color32::from_rgb(0x1a, 0x18, 0x1f);
        // black text becomes light on a dark canvas
        let c = adapt_fg(Color32::BLACK, dark, Color32::WHITE);
        assert!(contrast(c, dark) >= 3.0);
        // light text stays
        assert_eq!(adapt_fg(Color32::from_rgb(0xee, 0xee, 0xee), dark, Color32::WHITE), Color32::from_rgb(0xee, 0xee, 0xee));
        // white backgrounds disappear, colored ones darken, dark ones stay
        assert_eq!(adapt_bg(Color32::WHITE), None);
        let lb = adapt_bg(Color32::from_rgb(0xdd, 0xee, 0xff)).unwrap();
        assert!(luminance(lb) < 0.2);
        assert_eq!(adapt_bg(Color32::from_rgb(0x20, 0x20, 0x60)), Some(Color32::from_rgb(0x20, 0x20, 0x60)));
        let (h, s, l) = to_hsl(Color32::from_rgb(0x7b, 0x4f, 0xd6));
        assert_eq!(from_hsl(h, s, l, 255), Color32::from_rgb(0x7b, 0x4f, 0xd6));
    }
}
