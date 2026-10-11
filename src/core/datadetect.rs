//! Data detectors: find links, email addresses, phone numbers and dates in text.
//! STUB: to be implemented.

#[derive(Clone, Debug, PartialEq)]
pub enum DetectedKind {
    Url(String),
    Email(String),
    Phone(String),
    /// A date/time mention with the parsed unix timestamp when known.
    Date(Option<i64>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    /// Byte range in the input text.
    pub start: usize,
    pub end: usize,
    pub kind: DetectedKind,
}

/// Find all detectable items in `text`, sorted by position and non-overlapping.
pub fn detect(_text: &str) -> Vec<Detected> {
    Vec::new()
}
