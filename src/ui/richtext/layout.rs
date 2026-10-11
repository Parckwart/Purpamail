//! Turning a [`RichDoc`] into an egui [`Galley`] (used as the `TextEdit`
//! layouter) and painting the decorations a galley cannot express: list
//! markers, quote bars, inline images and spelling underlines.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{Color32, FontFamily, FontId, Galley, Pos2, Rect, Shape, Stroke, Ui, Vec2};

use super::model::*;
use crate::ui::fonts;

/// Horizontal indentation per quote level.
pub const QUOTE_INDENT: f32 = 16.0;
/// Space reserved for list markers.
pub const LIST_INDENT: f32 = 24.0;

/// An inline image known to the editor.
#[derive(Clone)]
pub struct InlineImage {
    pub bytes: Arc<[u8]>,
    /// Pixel size of the decoded image.
    pub size: Vec2,
    pub uri: String,
}

impl InlineImage {
    pub fn new(cid: &str, bytes: Vec<u8>) -> Self {
        let size = image::load_from_memory(&bytes).map(|i| Vec2::new(i.width() as f32, i.height() as f32)).unwrap_or(Vec2::new(64.0, 64.0));
        InlineImage {
            bytes: bytes.into(),
            size,
            uri: format!("bytes://inline-{}-{cid}", uuid::Uuid::new_v4().simple()),
        }
    }

    /// Display size when the available width is `avail`.
    pub fn display_size(&self, avail: f32) -> Vec2 {
        let max_w = avail.clamp(40.0, 560.0);
        let scale = (max_w / self.size.x).min(1.0).min(360.0 / self.size.y.max(1.0));
        (self.size * scale).max(Vec2::splat(8.0))
    }
}

/// Visual parameters for laying out a document.
#[derive(Clone)]
pub struct LayoutParams {
    pub base_size: f32,
    pub text: Color32,
    pub link: Color32,
    pub quote: [Color32; 3],
    pub code_bg: Color32,
    /// Ignore character formatting (plain text mode).
    pub plain: bool,
}

impl LayoutParams {
    pub fn from_ui(ui: &Ui, base_size: f32, plain: bool) -> Self {
        let pal = crate::ui::theme::Palette::of(ui.ctx());
        let (link, q2, q3) = if pal.dark {
            (Color32::from_rgb(0x6c, 0xa8, 0xff), Color32::from_rgb(0x4f, 0xc3, 0xb5), Color32::from_rgb(0x8c, 0xc8, 0x6a))
        } else {
            (Color32::from_rgb(0x1f, 0x6f, 0xeb), Color32::from_rgb(0x14, 0x8a, 0x7d), Color32::from_rgb(0x3c, 0x8c, 0x3a))
        };
        LayoutParams {
            base_size,
            text: pal.text,
            link,
            quote: [pal.accent, q2, q3],
            code_bg: pal.hover_bg,
            plain,
        }
    }

    pub fn quote_color(&self, level: u8) -> Color32 {
        self.quote[(level.max(1) as usize - 1) % 3]
    }
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Left indentation of a paragraph.
pub fn para_indent(p: &ParaAttr) -> f32 {
    p.quote as f32 * QUOTE_INDENT + if p.list != ListKind::None { LIST_INDENT } else { 0.0 }
}

fn format_for(st: &Style, para: &ParaAttr, lp: &LayoutParams) -> TextFormat {
    if lp.plain {
        let mut f = TextFormat::simple(FontId::new(lp.base_size, fonts::message()), if para.quote > 0 { lp.quote_color(para.quote) } else { lp.text });
        f.line_height = Some((lp.base_size * 1.4).round());
        return f;
    }
    let size = (lp.base_size * st.size.scale()).round();
    let family: FontFamily = if st.mono { fonts::fixed() } else { fonts::message_styled(st.bold, st.italic) };
    let mut color = match st.color {
        Some(c) => rgb(c),
        None if st.link.is_some() => lp.link,
        None if para.quote > 0 => lp.quote_color(para.quote),
        None => lp.text,
    };
    let mut f = TextFormat::simple(FontId::new(size, family), color);
    f.line_height = Some((size * 1.4).round());
    if let Some(h) = st.highlight {
        f.background = rgb(h);
        if st.color.is_none() {
            color = Color32::from_rgb(0x1d, 0x1b, 0x22);
            f.color = color;
        }
    } else if st.mono {
        f.background = lp.code_bg;
    }
    if st.mono && st.italic {
        f.italics = true;
    }
    if st.underline || st.link.is_some() {
        f.underline = Stroke::new(1.0, color);
    }
    if st.strike {
        f.strikethrough = Stroke::new(1.0, color);
    }
    f
}

/// Lay out `doc` for a `TextEdit` of width `wrap_width`.
pub fn layout_doc(ui: &Ui, doc: &RichDoc, lp: &LayoutParams, images: &HashMap<String, InlineImage>, wrap_width: f32) -> Arc<Galley> {
    let text = doc.text();
    let ranges = doc.para_ranges();
    let spans: Vec<(Range<usize>, &Style)> = doc.spans().collect();
    let mut galleys: Vec<Arc<Galley>> = Vec::with_capacity(ranges.len());
    // char -> byte offsets, computed incrementally
    let byte_idx: Vec<usize> = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect();
    let mut si = 0;
    ui.fonts_mut(|f| {
        for (pi, r) in ranges.iter().enumerate() {
            let para = doc.paras()[pi];
            let indent = para_indent(&para);
            let avail = (wrap_width - indent).max(30.0);
            let b0 = byte_idx[r.start];
            let b1 = byte_idx[r.end];
            let mut job = LayoutJob {
                text: text[b0..b1].to_string(),
                wrap: TextWrapping {
                    max_width: avail,
                    ..Default::default()
                },
                halign: match para.align {
                    Align::Left => egui::Align::LEFT,
                    Align::Center => egui::Align::Center,
                    Align::Right => egui::Align::RIGHT,
                },
                keep_trailing_whitespace: true,
                ..Default::default()
            };
            while si < spans.len() && spans[si].0.end <= r.start {
                si += 1;
            }
            let mut j = si;
            while j < spans.len() && spans[j].0.start < r.end {
                let (sr, st) = &spans[j];
                let a = sr.start.max(r.start);
                let b = sr.end.min(r.end);
                if a < b {
                    let mut fmt = format_for(st, &para, lp);
                    let byte_range = ByteIndex(byte_idx[a] - b0)..ByteIndex(byte_idx[b] - b0);
                    if let Some(img) = st.image.as_ref().and_then(|cid| images.get(cid)) {
                        // one section per image char so each gets its own size
                        let ds = img.display_size(avail);
                        let gw = f.glyph_width(&fmt.font_id, OBJ);
                        fmt.color = Color32::TRANSPARENT;
                        fmt.extra_letter_spacing = (ds.x - gw).max(0.0);
                        fmt.line_height = Some(ds.y + 4.0);
                        fmt.underline = Stroke::NONE;
                        fmt.valign = egui::Align::BOTTOM;
                        job.sections.push(LayoutSection {
                            leading_space: 0.0,
                            byte_range,
                            format: fmt,
                        });
                    } else {
                        job.sections.push(LayoutSection {
                            leading_space: 0.0,
                            byte_range,
                            format: fmt,
                        });
                    }
                }
                j += 1;
            }
            if job.sections.is_empty() {
                // empty paragraph: use the style of its terminating newline (keeps heading height)
                let st = if r.start < doc.char_len() {
                    doc.style_at(r.start)
                } else if r.start > 0 {
                    doc.style_at(r.start - 1)
                } else {
                    Style::default()
                };
                let st = Style {
                    image: None,
                    link: None,
                    highlight: None,
                    ..st
                };
                job.sections.push(LayoutSection {
                    leading_space: 0.0,
                    byte_range: ByteIndex(0)..ByteIndex(job.text.len()),
                    format: format_for(&st, &para, lp),
                });
            }
            let g = f.layout_job(job);
            let dx = indent
                + match para.align {
                    Align::Left => 0.0,
                    Align::Center => avail / 2.0,
                    Align::Right => avail,
                };
            if dx != 0.0 {
                let mut g2: Galley = (*g).clone();
                for row in &mut g2.rows {
                    row.pos.x += dx;
                }
                galleys.push(Arc::new(g2));
            } else {
                galleys.push(g);
            }
        }
    });
    let full = LayoutJob {
        text: text.to_string(),
        sections: vec![LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(0)..ByteIndex(text.len()),
            format: TextFormat::simple(FontId::new(lp.base_size, fonts::message()), lp.text),
        }],
        wrap: TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        keep_trailing_whitespace: true,
        ..Default::default()
    };
    let ppp = ui.ctx().pixels_per_point();
    Arc::new(Galley::concat(Arc::new(full), &galleys, ppp))
}

/// Screen rect of every char of the galley (newlines get a zero-width rect at
/// the row end). `origin` is where the galley was painted.
pub fn char_rects(galley: &Galley, origin: Pos2) -> Vec<Rect> {
    let mut out = Vec::with_capacity(galley.text().len());
    for row in &galley.rows {
        let top = origin.y + row.pos.y;
        let h = row.size.y;
        for g in &row.glyphs {
            let x = origin.x + row.pos.x + g.pos.x;
            out.push(Rect::from_min_size(Pos2::new(x, top), Vec2::new(g.advance_width, h)));
        }
        if row.ends_with_newline {
            let x = origin.x + row.pos.x + row.size.x;
            out.push(Rect::from_min_size(Pos2::new(x, top), Vec2::new(0.0, h)));
        }
    }
    out
}

/// Row index ranges per paragraph.
pub fn para_rows(galley: &Galley) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, row) in galley.rows.iter().enumerate() {
        if row.ends_with_newline {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    out.push(start..galley.rows.len());
    out
}

/// Paint list markers, quote bars, inline images and spelling underlines.
pub fn paint_decorations(ui: &Ui, doc: &RichDoc, lp: &LayoutParams, images: &HashMap<String, InlineImage>, galley: &Galley, origin: Pos2, misspelled: &[Range<usize>]) {
    let painter = ui.painter();
    let rows = para_rows(galley);
    let paras = doc.paras();
    if rows.len() != paras.len() {
        return; // galley from a different text (one frame lag); skip this frame
    }
    // list markers
    let mut number = 0;
    for (i, p) in paras.iter().enumerate() {
        if p.list != ListKind::Numbered {
            number = 0;
        }
        if p.list == ListKind::None {
            continue;
        }
        let Some(row) = galley.rows.get(rows[i].start) else { continue };
        let first = row.glyphs.first();
        let first_x = first.map(|g| g.pos.x).unwrap_or(0.0);
        let x = origin.x + row.pos.x + first_x - 6.0;
        let font_size = first.map(|g| g.font_height / 1.2).unwrap_or(lp.base_size).clamp(lp.base_size * 0.8, lp.base_size * 1.6);
        let color = if p.quote > 0 { lp.quote_color(p.quote) } else { lp.text };
        let marker = if p.list == ListKind::Bullet {
            "•".to_string()
        } else {
            number += 1;
            format!("{number}.")
        };
        let mg = painter.layout_no_wrap(marker, FontId::new(font_size, fonts::message()), color);
        let marker_baseline = mg.rows.first().and_then(|r| r.glyphs.first()).map(|g| r_pos_y(&mg) + g.pos.y).unwrap_or(mg.size().y * 0.8);
        let baseline = match first {
            Some(g) => origin.y + row.pos.y + g.pos.y,
            None => origin.y + row.pos.y + row.size.y * 0.8,
        };
        painter.galley(Pos2::new(x - mg.size().x, baseline - marker_baseline), mg, color);
    }
    // quote bars
    let max_q = paras.iter().map(|p| p.quote).max().unwrap_or(0);
    for level in 1..=max_q {
        let mut i = 0;
        while i < paras.len() {
            if paras[i].quote < level {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < paras.len() && paras[j + 1].quote >= level {
                j += 1;
            }
            let (Some(r0), Some(r1)) = (galley.rows.get(rows[i].start), galley.rows.get(rows[j].end.saturating_sub(1).max(rows[j].start))) else {
                break;
            };
            let x = origin.x + (level as f32 - 1.0) * QUOTE_INDENT + 3.0;
            let y0 = origin.y + r0.pos.y + 1.0;
            let y1 = origin.y + r1.pos.y + r1.size.y - 1.0;
            painter.line_segment([Pos2::new(x, y0), Pos2::new(x, y1)], Stroke::new(2.0, lp.quote_color(level).gamma_multiply(0.85)));
            i = j + 1;
        }
    }
    let needs_rects = !misspelled.is_empty() || (!lp.plain && doc.runs().iter().any(|r| r.style.image.is_some()));
    if !needs_rects {
        return;
    }
    let rects = char_rects(galley, origin);
    // inline images
    if !lp.plain {
        for (r, st) in doc.spans() {
            let Some(img) = st.image.as_ref().and_then(|c| images.get(c)) else { continue };
            for ci in r {
                let Some(cr) = rects.get(ci) else { continue };
                let p = doc.paras()[doc.para_at(ci)];
                let ds = img.display_size(galley.job.wrap.max_width - para_indent(&p));
                let rect = Rect::from_min_size(Pos2::new(cr.left(), cr.bottom() - ds.y - 2.0), ds);
                egui::Image::from_bytes(img.uri.clone(), img.bytes.clone()).corner_radius(4.0).paint_at(ui, rect);
            }
        }
    }
    // spelling
    let red = Color32::from_rgb(0xe0, 0x3b, 0x3b);
    for r in misspelled {
        let mut seg: Option<(f32, f32, f32)> = None; // x0, x1, y
        let flush = |seg: Option<(f32, f32, f32)>| {
            if let Some((x0, x1, y)) = seg {
                wavy(painter, x0, x1, y, red);
            }
        };
        for ci in r.clone() {
            let Some(cr) = rects.get(ci) else { continue };
            let y = cr.bottom() - 2.0;
            seg = match seg {
                Some((x0, _, sy)) if (sy - y).abs() < 0.5 => Some((x0, cr.right(), sy)),
                other => {
                    flush(other);
                    Some((cr.left(), cr.right(), y))
                }
            };
        }
        flush(seg);
    }
}

fn r_pos_y(g: &Galley) -> f32 {
    g.rows.first().map(|r| r.pos.y).unwrap_or(0.0)
}

fn wavy(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    let mut pts = Vec::new();
    let step = 2.0;
    let mut x = x0;
    let mut up = true;
    while x <= x1 {
        pts.push(Pos2::new(x, if up { y - 1.2 } else { y + 1.2 }));
        up = !up;
        x += step;
    }
    if pts.len() >= 2 {
        painter.add(Shape::line(pts, Stroke::new(1.0, color)));
    }
}
