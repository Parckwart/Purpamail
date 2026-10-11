//! Rich text editing used by the composer and the signature / template editors.
//!
//! * [`RichDoc`] — the styled-span document model (text + style runs +
//!   paragraph attributes), see [`model`].
//! * [`RichTextEditor`] — an egui widget editing a `RichDoc` on top of
//!   `TextEdit::multiline` with a custom layouter, plus a format bar.
//! * Serialization: [`to_html_fragment`] / [`to_html_document`] (inline-styled
//!   email HTML), [`to_plain`], [`from_html_lite`], [`from_plain`], and the
//!   persisted markup ([`to_markup`] / [`from_markup`]).
//!
//! # Markup format
//!
//! `Signature.body` and `Template.body` are stored in a small markdown-like
//! markup. Plain text is valid markup (stray markers that are not closed on
//! the same line are kept literally).
//!
//! Paragraph prefixes, in this order (each line is one paragraph):
//!
//! | prefix              | meaning                                   |
//! |---------------------|-------------------------------------------|
//! | `> ` (repeatable)   | quote level                               |
//! | `- ` / `1. `        | bulleted / numbered list item             |
//! | `{center} ` / `{right} ` | paragraph alignment                  |
//! | `# ` / `## `        | heading / bigger bold paragraph           |
//!
//! Inline formatting:
//!
//! | markup                             | meaning                     |
//! |------------------------------------|-----------------------------|
//! | `**bold**`                         | bold                        |
//! | `*italic*`                         | italic                      |
//! | `__underline__`                    | underline                   |
//! | `~~strike~~`                       | strikethrough               |
//! | `` `mono` ``                       | monospace                   |
//! | `[text](url)`                      | link                        |
//! | `{color=#rrggbb}…{/color}`         | text color                  |
//! | `{highlight=#rrggbb}…{/highlight}` | highlight color             |
//! | `{size=smaller\|bigger\|heading}…{/size}` | font size preset     |
//! | `![](cid:content-id)`              | inline image                |
//!
//! A backslash escapes the next character (`\*`, `\[`, `\{`, …); a
//! backslash at the start of a line prevents it from being read as a prefix.

mod editor;
pub use editor::preload_spell;
pub mod html;
pub mod layout;
pub mod markup;
pub mod model;

pub use editor::{RichTextEditor, fmt_button, spell_available};
pub use html::{from_html_lite, from_plain, to_html_document, to_html_fragment, to_plain};
pub use markup::{from_markup, to_markup};
pub use model::{Align, Attr, FontSize, ListKind, OBJ, ParaAttr, Rgb, RichDoc, Style};

/// Convert stored markup (signature / template body) to an HTML fragment.
pub fn markup_to_html(markup: &str) -> String {
    to_html_fragment(&from_markup(markup))
}

/// Convert stored markup to plain text.
pub fn markup_to_plain(markup: &str) -> String {
    to_plain(&from_markup(markup))
}
