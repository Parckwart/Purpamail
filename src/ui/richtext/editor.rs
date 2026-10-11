//! The [`RichTextEditor`] widget: an egui `TextEdit` driven by a [`RichDoc`].

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::{Color32, Id, Key, Modifiers, Response, RichText, Ui, Vec2};
use egui_phosphor::regular as ic;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use super::html;
use super::layout::{self, InlineImage, LayoutParams};
use super::markup;
use super::model::*;
use crate::core::spell::SpellChecker;
use crate::ui::theme::Palette;

/// The shared spell checker (loaded on first use).
static SPELL: Lazy<Mutex<SpellChecker>> = Lazy::new(|| {
    let lang = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_else(|_| "en_US".into());
    let lang = lang.split(['.', '@']).next().unwrap_or("en_US").to_string();
    Mutex::new(SpellChecker::load(if lang.is_empty() || lang == "C" || lang == "POSIX" { "en_US" } else { &lang }))
});

/// Load the spell checker in the background so the first composer opens instantly.
pub fn preload_spell() {
    std::thread::Builder::new().name("purpamail-spell".into()).spawn(|| {
        Lazy::force(&SPELL);
    }).ok();
}

/// Is a spelling dictionary available?
pub fn spell_available() -> bool {
    SPELL.lock().is_available()
}

/// Color swatches offered by the format bar.
pub const SWATCHES: [Rgb; 12] = [
    [0x1d, 0x1b, 0x22],
    [0x6b, 0x67, 0x75],
    [0xe0, 0x3b, 0x3b],
    [0xe8, 0x59, 0x0c],
    [0xf0, 0xb4, 0x1c],
    [0x3c, 0xa5, 0x4a],
    [0x14, 0x9a, 0x8b],
    [0x2f, 0x7c, 0xf6],
    [0x7b, 0x4f, 0xd6],
    [0xd9, 0x48, 0x6b],
    [0x8e, 0x5c, 0x3c],
    [0xff, 0xff, 0xff],
];
/// Highlight swatches.
pub const HIGHLIGHTS: [Rgb; 6] = [[0xff, 0xf3, 0x8a], [0xc8, 0xf7, 0xc5], [0xc5, 0xe3, 0xff], [0xff, 0xd6, 0xe7], [0xe7, 0xdc, 0xff], [0xff, 0xe0, 0xb8]];

#[derive(Clone)]
struct Snapshot {
    doc: RichDoc,
    sel: Range<usize>,
}

/// State of the "Add Link" dialog.
struct LinkDialog {
    range: Range<usize>,
    text: String,
    url: String,
    focus: bool,
}

/// A rich text editing widget.
///
/// ```ignore
/// let mut ed = RichTextEditor::new(&signature.body);
/// ed.ui_with_toolbar(ui);
/// signature.body = ed.to_markup();
/// ```
pub struct RichTextEditor {
    doc: RichDoc,
    id: Id,
    /// Plain text mode: character formatting is neither shown nor editable.
    pub plain: bool,
    /// Text can be selected and copied but not changed.
    pub read_only: bool,
    /// Placeholder shown while the document is empty.
    pub hint: String,
    /// Base font size in points.
    pub base_size: f32,
    /// Minimum height of the editing area.
    pub min_height: f32,
    /// Underline misspelled words (if a dictionary is available).
    pub spell_check: bool,
    pending: Option<(usize, Style)>,
    sel: Range<usize>,
    set_sel: Option<Range<usize>>,
    want_focus: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: f64,
    version: u64,
    images: HashMap<String, InlineImage>,
    link_dialog: Option<LinkDialog>,
    ignored: HashSet<String>,
    spell_cache: Option<(u64, Vec<Range<usize>>)>,
    context_char: Option<usize>,
}

impl RichTextEditor {
    /// Create an editor from markup (see the module docs).
    pub fn new(markup: &str) -> Self {
        Self::from_doc(markup::from_markup(markup))
    }

    pub fn from_doc(doc: RichDoc) -> Self {
        RichTextEditor {
            doc,
            id: Id::new(("richtext", uuid::Uuid::new_v4().as_u128())),
            plain: false,
            read_only: false,
            hint: String::new(),
            base_size: 14.0,
            min_height: 80.0,
            spell_check: false,
            pending: None,
            sel: 0..0,
            set_sel: None,
            want_focus: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: f64::NEG_INFINITY,
            version: 0,
            images: HashMap::new(),
            link_dialog: None,
            ignored: HashSet::new(),
            spell_cache: None,
            context_char: None,
        }
    }

    // ------------------------------------------------------------------ document access

    pub fn doc(&self) -> &RichDoc {
        &self.doc
    }

    /// Replace the whole document (clears undo history).
    pub fn set_doc(&mut self, doc: RichDoc) {
        self.doc = doc;
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        let n = self.doc.char_len();
        self.sel = self.sel.start.min(n)..self.sel.end.min(n);
        self.version += 1;
    }

    /// Modify the document as one undoable step.
    pub fn edit(&mut self, f: impl FnOnce(&mut RichDoc)) {
        self.push_undo();
        f(&mut self.doc);
        let n = self.doc.char_len();
        self.sel = self.sel.start.min(n)..self.sel.end.min(n);
        self.pending = None;
        self.version += 1;
    }

    /// Increases with every change; compare to detect edits.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Current (sorted) selection as char range.
    pub fn selection(&self) -> Range<usize> {
        self.sel.clone()
    }

    pub fn set_selection(&mut self, r: Range<usize>) {
        let n = self.doc.char_len();
        let r = r.start.min(n)..r.end.min(n);
        self.sel = r.clone();
        self.set_sel = Some(r);
    }

    pub fn request_focus(&mut self) {
        self.want_focus = true;
    }

    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id))
    }

    pub fn to_markup(&self) -> String {
        markup::to_markup(&self.doc)
    }
    /// HTML body fragment.
    pub fn to_html(&self) -> String {
        html::to_html_fragment(&self.doc)
    }
    pub fn to_plain(&self) -> String {
        html::to_plain(&self.doc)
    }

    /// Switch between plain and rich text. Making plain text drops all
    /// character formatting (bullets and links become text, quotes stay).
    pub fn set_plain(&mut self, plain: bool) {
        if plain == self.plain {
            return;
        }
        if plain {
            let sig = self.doc.ranges_where(|s| s.signature);
            self.edit(|d| {
                // keep the signature marker so it can still be replaced
                let mut sig_text_ranges = Vec::new();
                for r in &sig {
                    sig_text_ranges.push(d.slice(r.clone()).to_string());
                }
                let plain_text = html::to_plain(d);
                *d = html::from_plain(&plain_text);
                if let Some(last) = sig_text_ranges.last()
                    && let Some(pos) = plain_text.rfind(last.as_str())
                {
                    let start = plain_text[..pos].chars().count();
                    d.update_styles(start, start + last.chars().count(), |s| s.signature = true);
                }
            });
        }
        self.plain = plain;
        self.version += 1;
    }

    // ------------------------------------------------------------------ images

    /// Register image data for a content id (does not insert it).
    pub fn add_image(&mut self, cid: &str, bytes: Vec<u8>) {
        self.images.insert(cid.to_string(), InlineImage::new(cid, bytes));
    }

    /// Insert an inline image at the cursor.
    pub fn insert_image(&mut self, cid: &str, bytes: Vec<u8>) {
        self.add_image(cid, bytes);
        let sel = self.sel.clone();
        let st = Style {
            image: Some(cid.to_string()),
            ..Default::default()
        };
        self.edit(|d| d.replace(sel.start, sel.end, &OBJ.to_string(), &st));
        let p = sel.start + 1;
        self.set_selection(p..p);
    }

    pub fn image_bytes(&self, cid: &str) -> Option<&[u8]> {
        self.images.get(cid).map(|i| &*i.bytes)
    }

    // ------------------------------------------------------------------ formatting commands

    /// Style at the insertion point / start of the selection.
    pub fn current_style(&self) -> Style {
        if self.sel.is_empty() {
            self.insertion_style(self.sel.start)
        } else {
            self.doc.style_at(self.sel.start)
        }
    }

    pub fn current_para(&self) -> ParaAttr {
        self.doc.paras()[self.doc.para_at(self.sel.start).min(self.doc.paras().len() - 1)]
    }

    fn insertion_style(&self, pos: usize) -> Style {
        let st = match &self.pending {
            Some((p, s)) if *p == pos => s.clone(),
            _ => self.doc.insertion_style(pos),
        };
        if self.plain {
            Style {
                signature: st.signature,
                ..Default::default()
            }
        } else {
            st
        }
    }

    /// Is `attr` active for the selection / insertion point?
    pub fn is_active(&self, attr: &Attr) -> bool {
        if self.sel.is_empty() {
            attr.is_set(&self.insertion_style(self.sel.start))
        } else {
            self.doc.range_has(self.sel.start, self.sel.end, attr)
        }
    }

    /// Toggle / set a character attribute on the selection, or on the
    /// insertion style when nothing is selected.
    pub fn apply(&mut self, attr: Attr) {
        if self.plain || self.read_only {
            return;
        }
        let sel = self.sel.clone();
        if sel.is_empty() {
            let mut st = self.insertion_style(sel.start);
            let on = !attr.is_toggle() || !attr.is_set(&st);
            attr.apply(&mut st, on);
            self.pending = Some((sel.start, st));
        } else {
            self.edit(|d| d.toggle(sel.start, sel.end, &attr));
            self.set_sel = Some(sel);
        }
        self.want_focus = true;
    }

    pub fn clear_formatting(&mut self) {
        let sel = self.sel.clone();
        if sel.is_empty() {
            self.pending = Some((sel.start, self.insertion_style(sel.start).cleared()));
        } else {
            self.edit(|d| d.clear_formatting(sel.start, sel.end));
            self.set_sel = Some(sel);
        }
        self.want_focus = true;
    }

    pub fn set_align(&mut self, a: Align) {
        let sel = self.sel.clone();
        self.edit(|d| d.update_paras(sel.start, sel.end, |p| p.align = a));
        self.set_sel = Some(sel);
        self.want_focus = true;
    }

    pub fn toggle_list(&mut self, kind: ListKind) {
        let sel = self.sel.clone();
        self.edit(|d| d.toggle_list(sel.start, sel.end, kind));
        self.set_sel = Some(sel);
        self.want_focus = true;
    }

    pub fn indent(&mut self, delta: i32) {
        let sel = self.sel.clone();
        self.edit(|d| d.indent(sel.start, sel.end, delta));
        self.set_sel = Some(sel);
        self.want_focus = true;
    }

    /// Open the link dialog for the selection.
    pub fn open_link_dialog(&mut self) {
        if self.plain || self.read_only {
            return;
        }
        let sel = self.sel.clone();
        let existing = self.doc.style_at(sel.start).link.unwrap_or_default();
        self.link_dialog = Some(LinkDialog {
            text: self.doc.slice(sel.clone()).to_string(),
            range: sel,
            url: existing,
            focus: true,
        });
    }

    /// Insert a document at the cursor (replacing the selection).
    pub fn insert_doc(&mut self, other: &RichDoc) {
        let sel = self.sel.clone();
        self.edit(|d| d.splice(sel.start, sel.end, other));
        let p = sel.start + other.char_len();
        self.set_selection(p..p);
    }

    // ------------------------------------------------------------------ undo

    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            doc: self.doc.clone(),
            sel: self.sel.clone(),
        });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot {
                doc: std::mem::replace(&mut self.doc, s.doc),
                sel: self.sel.clone(),
            });
            self.set_selection(s.sel);
            self.version += 1;
            self.last_edit = f64::NEG_INFINITY;
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot {
                doc: std::mem::replace(&mut self.doc, s.doc),
                sel: self.sel.clone(),
            });
            self.set_selection(s.sel);
            self.version += 1;
            self.last_edit = f64::NEG_INFINITY;
        }
    }

    // ------------------------------------------------------------------ spelling

    /// Misspelled words (char ranges), excluding quotes, links, code and ignored words.
    fn misspelled(&mut self) -> Vec<Range<usize>> {
        let text = self.doc.text();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(text, &mut h);
        std::hash::Hash::hash(&self.ignored.len(), &mut h);
        let key = std::hash::Hasher::finish(&h);
        if let Some((k, v)) = &self.spell_cache
            && *k == key
        {
            return v.clone();
        }
        let spell = SPELL.lock();
        let mut out = Vec::new();
        if spell.is_available() {
            let byte_to_char: HashMap<usize, usize> = text
                .char_indices()
                .enumerate()
                .map(|(ci, (b, _))| (b, ci))
                .chain(std::iter::once((text.len(), text.chars().count())))
                .collect();
            for (b0, b1) in spell.misspelled(text) {
                let (Some(&c0), Some(&c1)) = (byte_to_char.get(&b0), byte_to_char.get(&b1)) else { continue };
                let word = &text[b0..b1];
                if self.ignored.contains(&word.to_lowercase()) {
                    continue;
                }
                let st = self.doc.style_at(c0);
                let para = self.doc.paras()[self.doc.para_at(c0)];
                if st.link.is_some() || st.mono || para.quote > 0 || st.image.is_some() {
                    continue;
                }
                out.push(c0..c1);
            }
        }
        drop(spell);
        self.spell_cache = Some((key, out.clone()));
        out
    }

    /// Misspelled words in the document (for "check spelling before sending").
    pub fn misspelled_words(&mut self) -> Vec<String> {
        let mut words: Vec<String> = self.misspelled().into_iter().map(|r| self.doc.slice(r).to_string()).collect();
        words.dedup();
        words
    }

    // ------------------------------------------------------------------ the widget

    fn handle_shortcuts(&mut self, ui: &mut Ui) {
        let (undo, redo, b, i, u, k) = ui.input_mut(|inp| {
            let redo = inp.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || inp.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = inp.consume_key(Modifiers::COMMAND, Key::Z);
            let b = inp.consume_key(Modifiers::COMMAND, Key::B);
            let i = inp.consume_key(Modifiers::COMMAND, Key::I);
            let u = inp.consume_key(Modifiers::COMMAND, Key::U);
            let k = inp.consume_key(Modifiers::COMMAND, Key::K);
            // other emacs-style bindings of TextEdit that would surprise users
            inp.consume_key(Modifiers::COMMAND, Key::H);
            inp.consume_key(Modifiers::COMMAND, Key::W);
            (undo, redo, b, i, u, k)
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if !self.plain {
            if b {
                self.apply(Attr::Bold);
            }
            if i {
                self.apply(Attr::Italic);
            }
            if u {
                self.apply(Attr::Underline);
            }
            if k {
                self.open_link_dialog();
            }
        }
    }

    /// Show the editing area. Returns the text edit's response
    /// (`changed()` is set when the document was edited this frame).
    pub fn ui(&mut self, ui: &mut Ui) -> Response {
        let id = self.id;
        let focused = ui.memory(|m| m.has_focus(id));
        if focused && !self.read_only {
            self.handle_shortcuts(ui);
        }
        if let Some(r) = self.set_sel.take() {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(r.start), CCursor::new(r.end))));
            egui::TextEdit::store_state(ui.ctx(), id, state);
        }
        // a click elsewhere this frame (e.g. on a format bar button) would make
        // egui take the focus away again, so wait for a frame without clicks
        if self.want_focus && !ui.input(|i| i.pointer.any_click() || i.pointer.any_pressed() || i.pointer.any_released()) {
            ui.memory_mut(|m| m.request_focus(id));
            self.want_focus = false;
        } else if self.want_focus {
            ui.ctx().request_repaint();
        }
        let lp = LayoutParams::from_ui(ui, self.base_size, self.plain);
        let old_text = self.doc.text().to_string();
        let mut buf = old_text.clone();
        let mut ro_text: &str = &old_text;
        let output = {
            let doc = &self.doc;
            let images = &self.images;
            let pending = &self.pending;
            let plain = self.plain;
            let ins_style = |d: &RichDoc, pos: usize| -> Style {
                let st = match pending {
                    Some((p, s)) if *p == pos => s.clone(),
                    _ => d.insertion_style(pos),
                };
                if plain {
                    Style {
                        signature: st.signature,
                        ..Default::default()
                    }
                } else {
                    st
                }
            };
            let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                let t = text.as_str();
                if t == doc.text() {
                    layout::layout_doc(ui, doc, &lp, images, wrap)
                } else {
                    let mut tmp = doc.clone();
                    if let Some(ch) = diff(doc.text(), t, None) {
                        let st = ins_style(doc, ch.start);
                        tmp.apply_text_change(ch, t, &st);
                    }
                    layout::layout_doc(ui, &tmp, &lp, images, wrap)
                }
            };
            let te = if self.read_only {
                egui::TextEdit::multiline(&mut ro_text)
            } else {
                egui::TextEdit::multiline(&mut buf)
            };
            te.id(id)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .min_size(Vec2::new(0.0, self.min_height))
                .hint_text(RichText::new(self.hint.clone()).size(self.base_size))
                .layouter(&mut layouter)
                .show(ui)
        };
        let mut response = output.response.response.clone();
        // fold the edit into the model
        if !self.read_only && buf != old_text {
            let cursor_after = output.cursor_range.filter(|c| c.is_empty()).map(|c| c.primary.index.0);
            if let Some(ch) = diff(&old_text, &buf, cursor_after) {
                let now = ui.input(|i| i.time);
                let typing = ch.deleted() == 0 && ch.inserted() == 1 && !buf.chars().nth(ch.start).is_some_and(|c| c == '\n');
                if !typing || now - self.last_edit > 1.0 {
                    self.push_undo();
                }
                self.last_edit = now;
                let st = self.insertion_style(ch.start);
                let para_before = self.doc.para_at(ch.start);
                let para_was_empty = self.doc.para_ranges().get(para_before).is_some_and(|r| r.is_empty());
                let para_attr = self.doc.paras()[para_before];
                self.doc.apply_text_change(ch, &buf, &st);
                self.pending = None;
                self.version += 1;
                // Enter on an empty list item / quote line ends the list (or
                // leaves the quote) instead of adding another item
                let inserted = buf[byte_of(&buf, ch.start)..byte_of(&buf, ch.new_end)].to_string();
                if inserted.starts_with('\n') && ch.deleted() == 0 && para_was_empty && (para_attr.list != ListKind::None || para_attr.quote > 0) {
                    self.doc.delete(ch.start, ch.start + 1);
                    let mut a = para_attr;
                    if a.list != ListKind::None {
                        a.list = ListKind::None;
                    } else {
                        a.quote -= 1;
                    }
                    let p = self.doc.para_at(ch.start);
                    let extra = inserted.matches('\n').count() - 1;
                    for i in p..=p + extra {
                        self.doc.set_para(i, a);
                    }
                    let c = ch.new_end - 1;
                    self.set_selection(c..c);
                }
                response.mark_changed();
            }
        }
        // track the selection
        if let Some(r) = output.state.cursor.char_range() {
            let s = r.as_sorted_char_range();
            let new_sel = s.start.0.min(self.doc.char_len())..s.end.0.min(self.doc.char_len());
            if new_sel != self.sel {
                if let Some((p, _)) = &self.pending
                    && !(new_sel.is_empty() && new_sel.start == *p)
                {
                    self.pending = None;
                }
                self.sel = new_sel;
            }
        }
        // decorations
        let misspelled = if self.spell_check && !self.read_only {
            let mut m = self.misspelled();
            if focused && self.sel.is_empty() {
                // don't flag the word being typed
                let c = self.sel.start;
                m.retain(|r| !(r.start <= c && c <= r.end));
            }
            m
        } else {
            Vec::new()
        };
        if output.galley.text() == self.doc.text() {
            layout::paint_decorations(ui, &self.doc, &lp, &self.images, &output.galley, output.galley_pos, &misspelled);
        }
        // context menu
        if response.secondary_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let c = output.galley.cursor_from_pos(pos - output.galley_pos);
            self.context_char = Some(c.index.0);
        }
        if !self.read_only {
            response.context_menu(|ui| self.context_menu(ui, &misspelled));
        }
        if let Some(url) = self.link_at_click(&response, &output.galley, output.galley_pos) {
            crate::ui::widgets::open_external(&url);
        }
        self.link_dialog_ui(ui.ctx());
        response
    }

    /// Ctrl+click on a link opens it.
    fn link_at_click(&self, resp: &Response, galley: &egui::Galley, origin: egui::Pos2) -> Option<String> {
        if !resp.clicked() || !resp.ctx.input(|i| i.modifiers.command) {
            return None;
        }
        let pos = resp.interact_pointer_pos()?;
        let c = galley.cursor_from_pos(pos - origin).index.0;
        self.doc.style_at(c).link
    }

    fn context_menu(&mut self, ui: &mut Ui, misspelled: &[Range<usize>]) {
        ui.set_min_width(180.0);
        let c = self.context_char.unwrap_or(self.sel.start);
        if let Some(r) = misspelled.iter().find(|r| r.start <= c && c < r.end).cloned() {
            let word = self.doc.slice(r.clone()).to_string();
            let sugg = SPELL.lock().suggest(&word, 5);
            if sugg.is_empty() {
                ui.label(RichText::new("No Guesses Found").italics().weak());
            }
            for s in sugg {
                if ui.button(RichText::new(&s).strong()).clicked() {
                    let r2 = r.clone();
                    self.edit(|d| {
                        let st = d.style_at(r2.start);
                        d.replace(r2.start, r2.end, &s, &st);
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Learn Spelling").clicked() {
                SPELL.lock().learn(&word);
                self.spell_cache = None;
                ui.close();
            }
            if ui.button("Ignore Spelling").clicked() {
                self.ignored.insert(word.to_lowercase());
                self.spell_cache = None;
                ui.close();
            }
            ui.separator();
        }
        let has_sel = !self.sel.is_empty();
        if ui.add_enabled(has_sel, egui::Button::new("Cut").shortcut_text("Ctrl+X")).clicked() {
            let sel = self.sel.clone();
            ui.ctx().copy_text(self.doc.slice(sel.clone()).to_string());
            self.edit(|d| d.delete(sel.start, sel.end));
            self.set_selection(sel.start..sel.start);
            ui.close();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
            ui.ctx().copy_text(self.doc.slice(self.sel.clone()).to_string());
            ui.close();
        }
        if !self.plain {
            ui.separator();
            for (attr, label, sc) in [(Attr::Bold, "Bold", "Ctrl+B"), (Attr::Italic, "Italic", "Ctrl+I"), (Attr::Underline, "Underline", "Ctrl+U")] {
                let on = self.is_active(&attr);
                if ui.add(egui::Button::selectable(on, label).shortcut_text(sc)).clicked() {
                    self.apply(attr);
                    ui.close();
                }
            }
            ui.separator();
            let link = self.doc.style_at(c).link;
            if ui.add(egui::Button::new(if link.is_some() { "Edit Link…" } else { "Add Link…" }).shortcut_text("Ctrl+K")).clicked() {
                if let (Some(_), true) = (&link, self.sel.is_empty()) {
                    // select the whole link
                    if let Some(r) = self.doc.ranges_where(|s| s.link == link).into_iter().find(|r| r.start <= c && c <= r.end) {
                        self.sel = r;
                    }
                }
                self.open_link_dialog();
                ui.close();
            }
            if let Some(url) = link {
                if ui.button("Open Link").clicked() {
                    crate::ui::widgets::open_external(&url);
                    ui.close();
                }
                if ui.button("Copy Link").clicked() {
                    ui.ctx().copy_text(url.clone());
                    ui.close();
                }
                if ui.button("Remove Link").clicked() {
                    let target = Some(url);
                    if let Some(r) = self.doc.ranges_where(|s| s.link == target).into_iter().find(|r| r.start <= c && c <= r.end) {
                        self.edit(|d| d.update_styles(r.start, r.end, |s| s.link = None));
                    }
                    ui.close();
                }
            }
        }
    }

    fn link_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.link_dialog else { return };
        let mut result: Option<bool> = None;
        let pal = Palette::of(ctx);
        egui::Modal::new(self.id.with("link-dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Add Link").size(15.0).family(crate::ui::fonts::bold()));
            ui.add_space(8.0);
            egui::Grid::new(self.id.with("link-grid")).num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                if dlg.range.is_empty() {
                    ui.label(RichText::new("Text:").color(pal.text_secondary));
                    ui.add(egui::TextEdit::singleline(&mut dlg.text).desired_width(280.0).hint_text("Link text"));
                    ui.end_row();
                }
                ui.label(RichText::new("Address:").color(pal.text_secondary));
                let r = ui.add(egui::TextEdit::singleline(&mut dlg.url).desired_width(280.0).hint_text("https://"));
                if dlg.focus {
                    r.request_focus();
                    dlg.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    result = Some(true);
                }
                ui.end_row();
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::ui::widgets::primary_button(ui, "OK").clicked() {
                    result = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    result = Some(false);
                }
                if !dlg.url.is_empty() && !dlg.range.is_empty() && ui.button("Remove Link").clicked() {
                    dlg.url.clear();
                    result = Some(true);
                }
            });
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                result = Some(false);
            }
        });
        let Some(ok) = result else { return };
        let dlg = self.link_dialog.take().unwrap();
        self.want_focus = true;
        if !ok {
            self.set_sel = Some(dlg.range);
            return;
        }
        let url = normalize_url(dlg.url.trim());
        let r = dlg.range.clone();
        if r.is_empty() {
            if url.is_empty() {
                return;
            }
            let text = if dlg.text.trim().is_empty() { url.clone() } else { dlg.text.clone() };
            let mut st = self.insertion_style(r.start);
            st.link = Some(url);
            self.edit(|d| d.replace(r.start, r.start, &text, &st));
            let end = r.start + text.chars().count();
            self.set_selection(end..end);
        } else {
            let link = if url.is_empty() { None } else { Some(url) };
            self.edit(|d| d.update_styles(r.start, r.end, |s| s.link = link.clone()));
            self.set_selection(r);
        }
    }

    // ------------------------------------------------------------------ format bar

    /// A compact formatting toolbar acting on this editor.
    pub fn format_bar(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let enabled = !self.plain && !self.read_only;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let cur = self.current_style();
                let size_name = cur.size.name();
                egui::ComboBox::from_id_salt(self.id.with("size")).width(92.0).selected_text(size_name).show_ui(ui, |ui| {
                    for f in FontSize::ALL {
                        if ui.selectable_label(cur.size == f, f.name()).clicked() {
                            self.apply(Attr::Size(f));
                        }
                    }
                });
                ui.add_space(6.0);
                for (attr, icon, tip) in [
                    (Attr::Bold, ic::TEXT_B, "Bold (Ctrl+B)"),
                    (Attr::Italic, ic::TEXT_ITALIC, "Italic (Ctrl+I)"),
                    (Attr::Underline, ic::TEXT_UNDERLINE, "Underline (Ctrl+U)"),
                    (Attr::Strike, ic::TEXT_STRIKETHROUGH, "Strikethrough"),
                    (Attr::Mono, ic::CODE, "Monospace"),
                ] {
                    let on = self.is_active(&attr);
                    if fmt_button(ui, icon, tip, on).clicked() {
                        self.apply(attr);
                    }
                }
                ui.add_space(6.0);
                self.color_menu(ui, false, cur.color);
                self.color_menu(ui, true, cur.highlight);
                sep(ui, &pal);
                let para = self.current_para();
                for (a, icon, tip) in [
                    (Align::Left, ic::TEXT_ALIGN_LEFT, "Align Left"),
                    (Align::Center, ic::TEXT_ALIGN_CENTER, "Center"),
                    (Align::Right, ic::TEXT_ALIGN_RIGHT, "Align Right"),
                ] {
                    if fmt_button(ui, icon, tip, para.align == a).clicked() {
                        self.set_align(a);
                    }
                }
                sep(ui, &pal);
                if fmt_button(ui, ic::LIST_BULLETS, "Bulleted List", para.list == ListKind::Bullet).clicked() {
                    self.toggle_list(ListKind::Bullet);
                }
                if fmt_button(ui, ic::LIST_NUMBERS, "Numbered List", para.list == ListKind::Numbered).clicked() {
                    self.toggle_list(ListKind::Numbered);
                }
                if fmt_button(ui, ic::TEXT_OUTDENT, "Decrease Quote Level", false).clicked() {
                    self.indent(-1);
                }
                if fmt_button(ui, ic::TEXT_INDENT, "Increase Quote Level", false).clicked() {
                    self.indent(1);
                }
                sep(ui, &pal);
                if fmt_button(ui, ic::LINK, "Add Link (Ctrl+K)", cur.link.is_some()).clicked() {
                    self.open_link_dialog();
                }
                if fmt_button(ui, ic::TEXT_T_SLASH, "Clear Formatting", false).clicked() {
                    self.clear_formatting();
                }
            });
        });
    }

    fn color_menu(&mut self, ui: &mut Ui, highlight: bool, current: Option<Rgb>) {
        let pal = Palette::of(ui.ctx());
        let (icon, tip) = if highlight { (ic::HIGHLIGHTER, "Highlight Color") } else { (ic::TEXT_AA, "Text Color") };
        let resp = fmt_button(ui, icon, tip, false);
        // color underline below the icon
        let bar = egui::Rect::from_center_size(resp.rect.center_bottom() - Vec2::new(0.0, 4.0), Vec2::new(14.0, 3.0));
        let col = current.map(|c| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or(if highlight { Color32::TRANSPARENT } else { pal.text });
        ui.painter().rect_filled(bar, 1.0, col);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(170.0);
            let swatches: &[Rgb] = if highlight { &HIGHLIGHTS } else { &SWATCHES };
            egui::Grid::new(self.id.with(("swatches", highlight))).spacing([4.0, 4.0]).show(ui, |ui| {
                for (i, c) in swatches.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
                    let fill = Color32::from_rgb(c[0], c[1], c[2]);
                    ui.painter().rect_filled(rect, 5.0, fill);
                    ui.painter().rect_stroke(
                        rect,
                        5.0,
                        egui::Stroke::new(if current == Some(*c) { 2.0 } else { 1.0 }, if current == Some(*c) { pal.accent } else { pal.separator }),
                        egui::StrokeKind::Inside,
                    );
                    if r.clicked() {
                        self.apply(if highlight { Attr::Highlight(Some(*c)) } else { Attr::Color(Some(*c)) });
                        ui.close();
                    }
                    if i % 6 == 5 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            let mut custom = current.unwrap_or(if highlight { HIGHLIGHTS[0] } else { SWATCHES[7] });
            ui.horizontal(|ui| {
                if egui::color_picker::color_edit_button_srgb(ui, &mut custom).changed() {
                    self.apply(if highlight { Attr::Highlight(Some(custom)) } else { Attr::Color(Some(custom)) });
                }
                ui.label("Custom…");
            });
            if ui.button(if highlight { "No Highlight" } else { "Default Color" }).clicked() {
                self.apply(if highlight { Attr::Highlight(None) } else { Attr::Color(None) });
                ui.close();
            }
        });
    }

    /// Format bar plus editor in a framed box (for preferences panes).
    pub fn ui_with_toolbar(&mut self, ui: &mut Ui) -> Response {
        let pal = Palette::of(ui.ctx());
        egui::Frame::new()
            .fill(pal.field_bg)
            .stroke(egui::Stroke::new(1.0, pal.separator))
            .corner_radius(6.0)
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| self.format_bar(ui));
                crate::ui::widgets::hline(ui);
                egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| self.ui(ui)).inner
            })
            .inner
    }
}

fn normalize_url(u: &str) -> String {
    if u.is_empty() || u.contains("://") || u.starts_with("mailto:") || u.starts_with("tel:") {
        u.to_string()
    } else if u.contains('@') && !u.contains('/') {
        format!("mailto:{u}")
    } else {
        format!("https://{u}")
    }
}

fn sep(ui: &mut Ui, pal: &Palette) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 18.0), egui::Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, pal.separator));
    ui.add_space(4.0);
}

/// A small square toggle button with an icon.
pub fn fmt_button(ui: &mut Ui, icon: &str, tip: &str, on: bool) -> Response {
    let pal = Palette::of(ui.ctx());
    let enabled = ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(28.0, 26.0), egui::Sense::click());
    let resp = resp.on_hover_text(tip);
    if ui.is_rect_visible(rect) {
        let fill = if on {
            pal.accent_soft
        } else if resp.hovered() && enabled {
            pal.hover_bg
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 5.0, fill);
        let col = if !enabled {
            pal.text_tertiary.gamma_multiply(0.6)
        } else if on {
            pal.accent
        } else {
            pal.text_secondary
        };
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(16.0), col);
    }
    resp
}
