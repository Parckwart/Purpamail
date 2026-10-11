//! Rendering of HTML (and plain-text) email bodies with egui.
//!
//! There is no browser engine: HTML is parsed with `scraper` (html5ever) and
//! turned into a small render tree ([`tree`]) using a CSS subset ([`css`]):
//! blocks, paragraphs of styled inline text and images, lists, quotes and real
//! tables. The tree is laid out once per width ([`layout`]) into positioned
//! items that are painted with only the visible ones touched ([`paint`]).
//! Plain-text bodies use the same machinery ([`plain`]), so both get quote
//! bars, clickable links and multi-paragraph text selection.
//!
//! Remote images are only downloaded when the caller allows it ([`remote`]).

#[path = "html/css.rs"]
pub mod css;
#[path = "html/layout.rs"]
pub mod layout;
#[path = "html/paint.rs"]
pub mod paint;
#[path = "html/plain.rs"]
pub mod plain;
#[path = "html/remote.rs"]
pub mod remote;
#[path = "html/tree.rs"]
pub mod tree;

use std::sync::Arc;

pub use layout::{LayoutOpts, LayoutOut};
pub use paint::{DocEvent, DocState, MenuTarget};
pub use plain::{DATE_SCHEME, PlainOpts, build_plain};
pub use tree::{BuildOpts, Doc, build_html};

/// Everything that influences a layout besides the document itself.
#[derive(Clone, Debug, PartialEq)]
struct LayoutKey {
    width: i32,
    adapt_dark: bool,
    canvas: egui::Color32,
    text: egui::Color32,
    link: egui::Color32,
    quote_colors: [egui::Color32; 3],
    quote_text: bool,
    allow_remote: bool,
    expanded: Vec<u64>,
    remote_generation: u64,
    pixels_per_point: f32,
    line_height: f32,
}

impl LayoutKey {
    fn of(o: &LayoutOpts, doc: &Doc) -> Self {
        let mut expanded: Vec<u64> = o.expanded.iter().copied().collect();
        expanded.sort_unstable();
        let uses_remote = o.allow_remote && doc.has_remote;
        LayoutKey {
            width: o.width.round() as i32,
            adapt_dark: o.adapt_dark,
            canvas: o.canvas,
            text: o.text,
            link: o.link,
            quote_colors: o.quote_colors,
            quote_text: o.quote_text,
            allow_remote: o.allow_remote,
            expanded,
            remote_generation: if uses_remote { remote::generation() } else { 0 },
            pixels_per_point: o.pixels_per_point,
            line_height: o.line_height,
        }
    }
}

/// A prepared document together with its most recent layout.
pub struct Rendered {
    pub doc: Arc<Doc>,
    cache: Option<(LayoutKey, Arc<LayoutOut>)>,
}

impl Rendered {
    pub fn new(doc: Doc) -> Self {
        Self { doc: Arc::new(doc), cache: None }
    }

    /// Layout for these options, reusing the previous one when nothing changed.
    pub fn layout(&mut self, ctx: &egui::Context, o: &LayoutOpts) -> Arc<LayoutOut> {
        let key = LayoutKey::of(o, &self.doc);
        if let Some((k, l)) = &self.cache
            && *k == key {
                return l.clone();
            }
        let mut o = o.clone();
        o.width = key.width as f32;
        let l = Arc::new(layout::layout(ctx, &self.doc, &o));
        self.cache = Some((key, l.clone()));
        l
    }
}
