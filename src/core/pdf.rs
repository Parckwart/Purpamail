//! Minimal PDF writer used for "Export as PDF" and printing.
//! STUB: to be implemented.

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

/// Lay out the lines on A4 pages (wrapping long lines) and return the PDF bytes.
pub fn render(_title: &str, _lines: &[PdfLine]) -> Vec<u8> {
    Vec::new()
}
