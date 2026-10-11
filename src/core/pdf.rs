//! Minimal PDF writer used for "Export as PDF" and printing.
//!
//! Produces dependency-free PDF 1.4 documents on A4 pages using the built-in
//! Type 1 fonts Helvetica, Helvetica-Bold and Courier with WinAnsiEncoding.
//! Text is wrapped using the standard Helvetica metrics, pages get a footer
//! with page numbers, and the file carries a correct cross-reference table.

use std::fmt::Write as _;

/// One rendered line of a document.
#[derive(Clone, Debug)]
pub struct PdfLine {
    pub text: String,
    pub bold: bool,
    pub size: f32,
    /// Gray level 0..1 (0 = black).
    pub gray: f32,
    pub indent: f32,
}

impl PdfLine {
    /// A regular black line of text at 11 pt.
    pub fn new(text: impl Into<String>) -> Self {
        PdfLine { text: text.into(), bold: false, size: 11.0, gray: 0.0, indent: 0.0 }
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn gray(mut self, gray: f32) -> Self {
        self.gray = gray;
        self
    }
    pub fn indent(mut self, indent: f32) -> Self {
        self.indent = indent;
        self
    }
}

/// The built-in fonts available to documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfFont {
    Helvetica,
    HelveticaBold,
    Courier,
}

impl PdfFont {
    fn resource(&self) -> &'static str {
        match self {
            PdfFont::Helvetica => "F1",
            PdfFont::HelveticaBold => "F2",
            PdfFont::Courier => "F3",
        }
    }
    fn base_font(&self) -> &'static str {
        match self {
            PdfFont::Helvetica => "Helvetica",
            PdfFont::HelveticaBold => "Helvetica-Bold",
            PdfFont::Courier => "Courier",
        }
    }
    /// Advance width of a WinAnsi byte in 1/1000 em.
    pub fn glyph_width(&self, b: u8) -> u16 {
        if b < 32 {
            return 0;
        }
        match self {
            PdfFont::Courier => 600,
            PdfFont::Helvetica => HELVETICA_WIDTHS[(b - 32) as usize],
            PdfFont::HelveticaBold => HELVETICA_BOLD_WIDTHS[(b - 32) as usize],
        }
    }
    /// Width of WinAnsi encoded text in points at `size`.
    pub fn text_width(&self, bytes: &[u8], size: f32) -> f32 {
        bytes.iter().map(|&b| self.glyph_width(b) as f32).sum::<f32>() * size / 1000.0
    }
}

/// Helvetica advance widths (Adobe AFM) for WinAnsi codes 32..=255.
#[rustfmt::skip]
const HELVETICA_WIDTHS: [u16; 224] = [
    // 32..=63:  space ! " # $ % & ' ( ) * + , - . / 0-9 : ; < = > ?
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    // 64..=95: @ A-Z [ \ ] ^ _
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    // 96..=127: ` a-z { | } ~ DEL
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 350,
    // 128..=159: € (129) ‚ ƒ „ … † ‡ ˆ ‰ Š ‹ Œ (141) Ž (143) (144) ‘ ’ “ ” • – — ˜ ™ š › œ (157) ž Ÿ
    556, 350, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 350, 611, 350,
    350, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 350, 500, 667,
    // 160..=191: nbsp ¡ ¢ £ ¤ ¥ ¦ § ¨ © ª « ¬ shy ® ¯ ° ± ² ³ ´ µ ¶ · ¸ ¹ º » ¼ ½ ¾ ¿
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    // 192..=223: À-Æ Ç È-Ë Ì-Ï Ð Ñ Ò-Ö × Ø Ù-Ü Ý Þ ß
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    // 224..=255: à-æ ç è-ë ì-ï ð ñ ò-ö ÷ ø ù-ü ý þ ÿ
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// Helvetica-Bold advance widths (Adobe AFM) for WinAnsi codes 32..=255.
#[rustfmt::skip]
const HELVETICA_BOLD_WIDTHS: [u16; 224] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 350,
    556, 350, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 350, 611, 350,
    350, 278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 350, 500, 667,
    278, 333, 556, 556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278,
    611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
];

/// Encode text as WinAnsi (CP-1252) bytes. Common typographic characters are mapped,
/// a few others get ASCII fallbacks, invisible characters are dropped and anything
/// else becomes '?'.
pub fn to_winansi(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let code = c as u32;
        match c {
            '\t' => out.extend_from_slice(b"    "),
            '\u{20}'..='\u{7e}' => out.push(code as u8),
            '\u{a0}'..='\u{ff}' => out.push(code as u8),
            '€' => out.push(0x80),
            '‚' => out.push(0x82),
            'ƒ' => out.push(0x83),
            '„' => out.push(0x84),
            '…' => out.push(0x85),
            '†' => out.push(0x86),
            '‡' => out.push(0x87),
            'ˆ' => out.push(0x88),
            '‰' => out.push(0x89),
            'Š' => out.push(0x8a),
            '‹' => out.push(0x8b),
            'Œ' => out.push(0x8c),
            'Ž' => out.push(0x8e),
            '‘' => out.push(0x91),
            '’' => out.push(0x92),
            '“' => out.push(0x93),
            '”' => out.push(0x94),
            '•' => out.push(0x95),
            '–' => out.push(0x96),
            '—' => out.push(0x97),
            '˜' => out.push(0x98),
            '™' => out.push(0x99),
            'š' => out.push(0x9a),
            '›' => out.push(0x9b),
            'œ' => out.push(0x9c),
            'ž' => out.push(0x9e),
            'Ÿ' => out.push(0x9f),
            // Fallbacks
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2212}' => out.push(b'-'),
            '\u{2015}' => out.push(0x97),
            '\u{2032}' => out.push(b'\''),
            '\u{2033}' => out.push(b'"'),
            '\u{201b}' => out.push(0x91),
            '\u{201f}' => out.push(0x93),
            '\u{25cf}' | '\u{2219}' | '\u{25aa}' => out.push(0x95),
            '\u{2192}' => out.extend_from_slice(b"->"),
            '\u{2190}' => out.extend_from_slice(b"<-"),
            '\u{2264}' => out.extend_from_slice(b"<="),
            '\u{2265}' => out.extend_from_slice(b">="),
            '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => out.push(b' '),
            '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{fe0f}' => {}
            c if c.is_control() => {}
            _ => out.push(b'?'),
        }
    }
    out
}

/// Escape WinAnsi bytes for a PDF literal string; non-ASCII bytes become octal escapes.
fn pdf_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() + 2);
    s.push('(');
    for &b in bytes {
        match b {
            b'(' => s.push_str("\\("),
            b')' => s.push_str("\\)"),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:03o}");
            }
        }
    }
    s.push(')');
    s
}

/// UTF-16BE hex string with BOM, for document information entries.
fn pdf_text_string(text: &str) -> String {
    let mut s = String::from("<FEFF");
    for u in text.encode_utf16() {
        let _ = write!(s, "{u:04X}");
    }
    s.push('>');
    s
}

/// A4 page size in points.
pub const PAGE_WIDTH: f32 = 595.28;
pub const PAGE_HEIGHT: f32 = 841.89;
const MARGIN_LEFT: f32 = 56.0;
const MARGIN_RIGHT: f32 = 56.0;
const MARGIN_TOP: f32 = 60.0;
const MARGIN_BOTTOM: f32 = 64.0;
const FOOTER_Y: f32 = 32.0;
const LINE_SPACING: f32 = 1.3;

/// Break WinAnsi text into lines no wider than `max_width` points.
pub fn wrap(bytes: &[u8], font: PdfFont, size: f32, max_width: f32) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return vec![Vec::new()];
    }
    let space_w = font.text_width(b" ", size);
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut cur_w = 0.0f32;
    // Keep leading indentation of the original line.
    let lead = bytes.iter().take_while(|&&b| b == b' ').count();
    if lead > 0 {
        cur.extend_from_slice(&bytes[..lead]);
        cur_w = space_w * lead as f32;
    }
    for word in bytes[lead..].split(|&b| b == b' ') {
        let w = font.text_width(word, size);
        let needed = if cur.is_empty() || cur.iter().all(|&b| b == b' ') { w } else { space_w + w };
        if cur_w + needed <= max_width || cur.is_empty() {
            if w > max_width {
                // Break an over-long word (e.g. a URL) at character level.
                if !cur.is_empty() && !cur.iter().all(|&b| b == b' ') {
                    cur.push(b' ');
                    cur_w += space_w;
                }
                for &b in word {
                    let bw = font.glyph_width(b) as f32 * size / 1000.0;
                    if cur_w + bw > max_width && !cur.is_empty() {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0.0;
                    }
                    cur.push(b);
                    cur_w += bw;
                }
                continue;
            }
            if !cur.is_empty() && !cur.iter().all(|&b| b == b' ') {
                cur.push(b' ');
            }
            cur.extend_from_slice(word);
            cur_w += needed;
        } else {
            lines.push(std::mem::take(&mut cur));
            if w > max_width {
                cur_w = 0.0;
                for &b in word {
                    let bw = font.glyph_width(b) as f32 * size / 1000.0;
                    if cur_w + bw > max_width && !cur.is_empty() {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0.0;
                    }
                    cur.push(b);
                    cur_w += bw;
                }
            } else {
                cur.extend_from_slice(word);
                cur_w = w;
            }
        }
    }
    lines.push(cur);
    lines
}

enum Item {
    Text { text: String, font: PdfFont, size: f32, gray: f32, indent: f32 },
    Space(f32),
    PageBreak,
}

/// Incremental document builder.
pub struct Document {
    title: String,
    items: Vec<Item>,
    page_numbers: bool,
}

impl Document {
    pub fn new(title: impl Into<String>) -> Self {
        Document { title: title.into(), items: Vec::new(), page_numbers: true }
    }
    /// Disable the "Page n of N" footer.
    pub fn without_page_numbers(mut self) -> Self {
        self.page_numbers = false;
        self
    }
    /// Add a (wrapped) line of Helvetica / Helvetica-Bold text.
    pub fn line(&mut self, l: &PdfLine) -> &mut Self {
        let font = if l.bold { PdfFont::HelveticaBold } else { PdfFont::Helvetica };
        self.text(&l.text, font, l.size, l.gray, l.indent)
    }
    /// Add text in an explicit font; embedded newlines start new lines.
    pub fn text(&mut self, text: &str, font: PdfFont, size: f32, gray: f32, indent: f32) -> &mut Self {
        let size = if size.is_finite() && size > 0.0 { size.clamp(4.0, 72.0) } else { 11.0 };
        let gray = if gray.is_finite() { gray.clamp(0.0, 1.0) } else { 0.0 };
        let indent = if indent.is_finite() { indent.clamp(0.0, 300.0) } else { 0.0 };
        for part in text.split('\n') {
            self.items.push(Item::Text { text: part.trim_end_matches('\r').to_string(), font, size, gray, indent });
        }
        self
    }
    /// Monospaced text (e.g. plain-text message bodies with layout).
    pub fn mono(&mut self, text: &str, size: f32) -> &mut Self {
        self.text(text, PdfFont::Courier, size, 0.0, 0.0)
    }
    /// Vertical space in points.
    pub fn space(&mut self, points: f32) -> &mut Self {
        if points.is_finite() && points > 0.0 {
            self.items.push(Item::Space(points));
        }
        self
    }
    pub fn page_break(&mut self) -> &mut Self {
        self.items.push(Item::PageBreak);
        self
    }

    /// Lay out all items and serialize the PDF.
    pub fn finish(&self) -> Vec<u8> {
        // Lay out into pages of content streams.
        let mut pages: Vec<String> = Vec::new();
        let mut cur = String::new();
        let top = PAGE_HEIGHT - MARGIN_TOP;
        let mut y = top;
        let width = PAGE_WIDTH - MARGIN_LEFT - MARGIN_RIGHT;
        let new_page = |cur: &mut String, pages: &mut Vec<String>, y: &mut f32| {
            pages.push(std::mem::take(cur));
            *y = top;
        };
        for item in &self.items {
            match item {
                Item::PageBreak => new_page(&mut cur, &mut pages, &mut y),
                Item::Space(p) => {
                    y -= p;
                    if y < MARGIN_BOTTOM {
                        new_page(&mut cur, &mut pages, &mut y);
                    }
                }
                Item::Text { text, font, size, gray, indent } => {
                    let bytes = to_winansi(text);
                    let leading = size * LINE_SPACING;
                    for l in wrap(&bytes, *font, *size, (width - indent).max(40.0)) {
                        if y - size < MARGIN_BOTTOM {
                            new_page(&mut cur, &mut pages, &mut y);
                        }
                        y -= leading;
                        if !l.is_empty() {
                            let _ = writeln!(
                                cur,
                                "BT /{} {:.2} Tf {:.3} g {:.2} {:.2} Td {} Tj ET",
                                font.resource(),
                                size,
                                gray,
                                MARGIN_LEFT + indent,
                                y + (leading - size),
                                pdf_string(&l)
                            );
                        }
                    }
                }
            }
        }
        pages.push(cur);
        if pages.len() > 1 && pages.last().is_some_and(|p| p.is_empty()) {
            pages.pop();
        }
        let total = pages.len();
        if self.page_numbers {
            for (i, p) in pages.iter_mut().enumerate() {
                let label = to_winansi(&format!("Page {} of {}", i + 1, total));
                let w = PdfFont::Helvetica.text_width(&label, 8.0);
                let _ = writeln!(p, "BT /F1 8.00 Tf 0.500 g {:.2} {:.2} Td {} Tj ET", (PAGE_WIDTH - w) / 2.0, FOOTER_Y, pdf_string(&label));
            }
        }
        serialize(&self.title, &pages)
    }
}

fn serialize(title: &str, pages: &[String]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    // Object numbers: 1 catalog, 2 pages, 3-5 fonts, 6 info, then (page, content) pairs.
    let first_page = 7;
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", first_page + 2 * i)).collect();
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).as_bytes());
    for f in [PdfFont::Helvetica, PdfFont::HelveticaBold, PdfFont::Courier] {
        obj(&mut out, format!("<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>", f.base_font()).as_bytes());
    }
    let date = chrono::Utc::now().format("D:%Y%m%d%H%M%SZ").to_string();
    obj(&mut out, format!("<< /Title {} /Producer (Purpamail) /Creator (Purpamail) /CreationDate ({date}) >>", pdf_text_string(title)).as_bytes());
    for (i, content) in pages.iter().enumerate() {
        let page_no = first_page + 2 * i;
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH:.2} {PAGE_HEIGHT:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> /ProcSet [/PDF /Text] >> /Contents {} 0 R >>",
                page_no + 1
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        body.extend_from_slice(content.as_bytes());
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
    }
    let xref = out.len();
    let mut x = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in &offsets {
        let _ = writeln!(x, "{o:010} 00000 n ");
    }
    let _ = write!(x, "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1);
    out.extend_from_slice(x.as_bytes());
    out
}

/// Lay out the lines on A4 pages (wrapping long lines) and return the PDF bytes.
pub fn render(title: &str, lines: &[PdfLine]) -> Vec<u8> {
    let mut doc = Document::new(title);
    for l in lines {
        doc.line(l);
    }
    doc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Check the cross-reference table points at the right objects.
    fn check_structure(pdf: &[u8]) -> usize {
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(pdf);
        let sx = s.rfind("startxref\n").unwrap();
        let xref_off: usize = s[sx + 10..].lines().next().unwrap().trim().parse().unwrap();
        assert!(pdf[xref_off..].starts_with(b"xref\n"));
        let table = String::from_utf8_lossy(&pdf[xref_off..]).to_string();
        let mut lines = table.lines().skip(1);
        let header = lines.next().unwrap();
        let count: usize = header.split(' ').nth(1).unwrap().parse().unwrap();
        lines.next(); // free entry
        for i in 1..count {
            let entry = lines.next().unwrap();
            assert_eq!(entry.len() + 1, 20, "xref entries are 20 bytes");
            let off: usize = entry[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{i} 0 obj\n").as_bytes()), "object {i} at {off}");
        }
        // Stream lengths are correct
        let mut pos = 0;
        while let Some(p) = find(&pdf[pos..], b"/Length ") {
            let start = pos + p + 8;
            let len: usize = String::from_utf8_lossy(&pdf[start..start + 12]).split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
            let stream = start + find(&pdf[start..], b"stream\n").unwrap() + 7;
            assert!(pdf[stream + len..].starts_with(b"\nendstream"));
            pos = stream + len;
        }
        count
    }

    #[test]
    fn single_page_structure() {
        let pdf =
            render("Hello (World)", &[PdfLine::new("Subject: Grüße “quoted” – €5 … • ✓").bold().size(14.0), PdfLine::new("Body text").gray(0.4).indent(20.0)]);
        let n = check_structure(&pdf);
        assert_eq!(n, 9); // 6 fixed objects + page + content + free entry
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Count 1"));
        assert!(s.contains("/BaseFont /Helvetica-Bold"));
        assert!(s.contains("Page 1 of 1"));
        // WinAnsi mapping: ü = \374, “ = \223, – = \226, € = \200, … = \205, • = \225, ✓ = ?
        assert!(s.contains("Gr\\374\\337e \\223quoted\\224 \\226 \\2005 \\205 \\225 ?"), "{s}");
    }

    #[test]
    fn wrapping_and_pagination() {
        let long = "lorem ipsum dolor sit amet ".repeat(40);
        let mut lines = vec![PdfLine::new(long.clone())];
        for i in 0..120 {
            lines.push(PdfLine::new(format!("Line {i}")));
        }
        lines.push(PdfLine::new(format!("https://example.com/{}", "x".repeat(300))));
        let pdf = render("Long", &lines);
        check_structure(&pdf);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Count 3"), "expected 3 pages");
        assert!(s.contains("Page 3 of 3"));
        // Every wrapped line fits in the text area
        let w = wrap(&to_winansi(&long), PdfFont::Helvetica, 11.0, 400.0);
        assert!(w.len() > 5);
        for l in &w {
            assert!(PdfFont::Helvetica.text_width(l, 11.0) <= 400.0 + 0.01);
        }
        let w = wrap(&to_winansi(&"x".repeat(200)), PdfFont::Helvetica, 11.0, 100.0);
        assert!(w.iter().all(|l| PdfFont::Helvetica.text_width(l, 11.0) <= 100.01));
        assert_eq!(w.iter().map(|l| l.len()).sum::<usize>(), 200);
    }

    #[test]
    fn widths_and_builder() {
        assert_eq!(PdfFont::Helvetica.glyph_width(b'W'), 944);
        assert_eq!(PdfFont::Helvetica.glyph_width(b'i'), 222);
        assert_eq!(PdfFont::HelveticaBold.glyph_width(b'i'), 278);
        assert_eq!(PdfFont::Helvetica.glyph_width(0x80), 556);
        assert_eq!(PdfFont::HelveticaBold.glyph_width(0xff), 556);
        assert_eq!(PdfFont::Courier.glyph_width(b'i'), 600);
        let mut d = Document::new("Built");
        d.text("Heading", PdfFont::HelveticaBold, 16.0, 0.0, 0.0).space(10.0).mono("  fn main() {}\n  // done", 9.0).page_break().line(&PdfLine::new("p2"));
        let pdf = d.finish();
        check_structure(&pdf);
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 2"));
        // Degenerate inputs
        check_structure(&render("", &[]));
        check_structure(&render("x", &[PdfLine { text: "nan".into(), bold: false, size: f32::NAN, gray: f32::INFINITY, indent: -5.0 }]));
    }

    /// Validate with poppler when available (not required for the test to pass).
    #[test]
    fn validates_with_external_tool_if_present() {
        let Ok(out) = std::process::Command::new("pdftotext").arg("-v").output() else { return };
        if !out.status.success() && out.stderr.is_empty() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("purpamail-pdf-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pdf");
        std::fs::write(&path, render("Test", &[PdfLine::new("Grüße aus Köln – “Hallo” €"), PdfLine::new("second line")])).unwrap();
        let res = std::process::Command::new("pdftotext").arg(&path).arg("-").output().unwrap();
        let text = String::from_utf8_lossy(&res.stdout);
        assert!(res.status.success());
        assert!(text.contains("Grüße aus Köln – “Hallo” €"), "{text}");
        assert!(text.contains("Page 1 of 1"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
