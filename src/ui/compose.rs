//! Compose window: every message being written lives in its own OS window
//! (an egui viewport) with a toolbar, the header fields (From, To, Cc, Bcc,
//! Reply-To, Subject, Priority), the rich text body, attachments and the
//! send / save-draft logic.

mod attachments;
mod init;
mod send_later;
mod tokens;

use std::hash::{Hash, Hasher};

use egui::{Align, Color32, Key, Layout, Modifiers, RichText, Ui, Vec2, ViewportCommand};
use egui_phosphor::regular as ic;

use self::attachments::ComposeAttachment;
use self::init::{Prefill, SigAnchor};
use self::send_later::SendLaterPicker;
use self::tokens::{TokenField, TokenStyle};
use super::actions::{Action, ComposeInit, UiCx};
use super::richtext::{self, RichDoc, RichTextEditor, Rgb};
use super::theme::Palette;
use super::widgets;
use crate::core::compose::{self, OutgoingAttachment, OutgoingMessage, ReplyKind};
use crate::core::config::{Settings, SpellCheck, Template};
use crate::core::mime::{self, ParsedMessage};
use crate::core::model::{Address, MessageId, Priority};
use crate::core::ops;
use crate::net::Command;

/// Width of the right-aligned header label column.
const LABEL_W: f32 = 78.0;
/// Seconds between automatic draft saves.
const AUTOSAVE_SECS: f64 = 30.0;

/// One selectable sender address.
#[derive(Clone)]
struct FromChoice {
    account_id: String,
    address: Address,
}

/// Modal dialogs shown inside the compose window.
enum Dialog {
    SaveDraft,
    Error { title: String, message: String },
    /// Confirmation during send validation; "OK" continues with `next_step`.
    Confirm { title: String, message: String, ok: String, next_step: u8, send_at: Option<i64> },
    SendLater,
}

pub struct ComposeWindow {
    pub id: u64,
    from_choices: Vec<FromChoice>,
    from_idx: usize,
    to: TokenField,
    cc: TokenField,
    bcc: TokenField,
    reply_to: TokenField,
    subject: String,
    show_cc: bool,
    show_bcc: bool,
    show_reply_to: bool,
    show_priority: bool,
    editor: RichTextEditor,
    attachments: Vec<ComposeAttachment>,
    priority: Priority,
    request_receipt: bool,
    message_id: String,
    in_reply_to: String,
    references: Vec<String>,
    draft_id: Option<MessageId>,
    redirect_raw: Option<Vec<u8>>,
    signature_id: Option<String>,
    sig_anchor: SigAnchor,
    background: Option<Rgb>,
    /// Template accent color, used to tint the body in dark mode.
    accent: Option<Rgb>,
    show_format_bar: bool,
    auto_cc: bool,
    auto_bcc: bool,
    saved_fingerprint: u64,
    dirty_since: Option<f64>,
    status: Option<(String, f64)>,
    dialog: Option<Dialog>,
    picker: SendLaterPicker,
    open: bool,
    first_frame: bool,
    focus_body: bool,
    font_size: f32,
}

fn kv_flag(cx: &UiCx, key: &str, default: bool) -> bool {
    cx.store.kv_get(key).map(|v| v == "1").unwrap_or(default)
}

impl ComposeWindow {
    pub fn new(cx: &mut UiCx, init: ComposeInit, id: u64) -> Self {
        let settings = cx.settings.read().clone();
        let mut draft_id = None;
        let mut auto_self = true;
        let prefill = match init {
            ComposeInit::New { to, cc, subject, body, attachments } => {
                let mut atts = Vec::new();
                for p in attachments {
                    match attachments::read_file(&p) {
                        Ok(a) => atts.push(a),
                        Err(e) => cx.act(Action::Toast(format!("Couldn't attach {}: {e}", p.display()))),
                    }
                }
                init::new_message(to, cc, subject, &body, atts, &settings)
            }
            ComposeInit::Mailto(url) => init::mailto(&url, &settings),
            ComposeInit::Template(t) => init::template(&t, &settings),
            ComposeInit::Outgoing(m) => {
                auto_self = false;
                init::outgoing(&m)
            }
            ComposeInit::ForwardMany(ids) => {
                let mut p = init::new_message(vec![], vec![], String::new(), "", vec![], &settings);
                for (i, mid) in ids.iter().enumerate() {
                    if let Some(raw) = cx.store.raw(*mid) {
                        if i == 0 {
                            if let Some(parsed) = mime::parse(&raw) {
                                p.subject = compose::prefixed_subject("Fwd:", &parsed.subject);
                                if let Some((a, e)) = init::receiving_address(&parsed, &settings) {
                                    p.account_id = Some(a);
                                    p.from_email = Some(e);
                                }
                            }
                        }
                        p.attachments.push(init::message_attachment(&raw));
                    } else {
                        cx.act(Action::FetchBody(*mid));
                    }
                }
                p
            }
            ComposeInit::Respond { message, kind, selected_text } => {
                let summary = cx.store.message(message);
                let raw = cx.store.raw(message);
                let (parsed, raw) = match raw.as_deref().and_then(mime::parse) {
                    Some(p) => (p, raw.unwrap_or_default()),
                    None => {
                        // body not downloaded yet: work from the summary
                        cx.act(Action::FetchBody(message));
                        let s = summary.clone().unwrap_or_default();
                        (
                            ParsedMessage {
                                subject: s.subject.clone(),
                                from: s.from.clone(),
                                to: s.to.clone(),
                                cc: s.cc.clone(),
                                reply_to: s.reply_to.clone(),
                                date: s.date,
                                message_id: s.message_id.clone(),
                                references: s.references.clone(),
                                text: Some(s.snippet.clone()),
                                ..Default::default()
                            },
                            Vec::new(),
                        )
                    }
                };
                if kind == ReplyKind::EditDraft {
                    draft_id = Some(message);
                }
                if matches!(kind, ReplyKind::EditDraft | ReplyKind::SendAgain | ReplyKind::Redirect) {
                    auto_self = false;
                }
                init::respond(&parsed, &raw, kind, selected_text.as_deref(), &settings, summary.as_ref().map(|s| s.account_id.as_str()))
            }
        };
        Self::from_prefill(cx, &settings, prefill, id, draft_id, auto_self)
    }

    fn from_prefill(cx: &mut UiCx, settings: &Settings, p: Prefill, id: u64, draft_id: Option<MessageId>, auto_self: bool) -> Self {
        let from_choices: Vec<FromChoice> = settings
            .accounts
            .iter()
            .flat_map(|a| a.emails.iter().map(move |e| FromChoice { account_id: a.id.clone(), address: Address::new(a.full_name.clone(), e.clone()) }))
            .collect();
        let find = |acct: Option<&str>, email: Option<&str>| -> Option<usize> {
            from_choices.iter().position(|c| acct.is_none_or(|a| a == c.account_id) && email.is_none_or(|e| e.eq_ignore_ascii_case(&c.address.email)))
        };
        let from_idx = find(p.account_id.as_deref(), p.from_email.as_deref())
            .or_else(|| p.from_email.as_deref().and_then(|e| find(None, Some(e))))
            .or_else(|| find(p.account_id.as_deref(), None))
            .or_else(|| (!settings.composing.send_new_from.is_empty()).then(|| find(Some(&settings.composing.send_new_from), None)).flatten())
            .unwrap_or(0);
        let wid = egui::Id::new(("compose", id));
        let redirect = p.redirect_raw.is_some();
        let mut editor = RichTextEditor::from_doc(p.body.clone());
        editor.plain = p.plain;
        editor.read_only = redirect;
        editor.base_size = settings.composing.default_font_size.clamp(9.0, 32.0);
        editor.spell_check = settings.composing.spell_check == SpellCheck::AsITyped;
        editor.min_height = 240.0;
        for (cid, bytes) in &p.inline_images {
            editor.add_image(cid, bytes.clone());
        }
        let from_addr = from_choices.get(from_idx).map(|c| c.address.clone());
        let mut cc = p.cc.clone();
        let mut bcc = p.bcc.clone();
        let mut auto_cc = false;
        let mut auto_bcc = false;
        if auto_self && !redirect {
            if let Some(f) = &from_addr {
                let acct_bcc = from_choices.get(from_idx).and_then(|c| settings.account(&c.account_id)).is_some_and(|a| a.auto_bcc_myself);
                if settings.composing.auto_cc_myself && !cc.iter().any(|a| a.email.eq_ignore_ascii_case(&f.email)) {
                    cc.push(f.clone());
                    auto_cc = true;
                }
                if (settings.composing.auto_bcc_myself || acct_bcc) && !bcc.iter().any(|a| a.email.eq_ignore_ascii_case(&f.email)) {
                    bcc.push(f.clone());
                    auto_bcc = true;
                }
            }
        }
        let message_id = p.message_id.clone().unwrap_or_else(|| compose::new_message_id(from_addr.as_ref().unwrap_or(&Address::default())));
        let mut w = ComposeWindow {
            id,
            from_idx,
            to: TokenField::new(wid.with("to"), p.to.clone()),
            show_cc: kv_flag(cx, "compose.show_cc", true) || !cc.is_empty(),
            show_bcc: kv_flag(cx, "compose.show_bcc", false) || !bcc.is_empty(),
            show_reply_to: kv_flag(cx, "compose.show_reply_to", false) || !p.reply_to.is_empty(),
            show_priority: kv_flag(cx, "compose.show_priority", false) || p.priority != Priority::Normal,
            cc: TokenField::new(wid.with("cc"), cc),
            bcc: TokenField::new(wid.with("bcc"), bcc),
            reply_to: TokenField::new(wid.with("reply_to"), p.reply_to.clone()),
            subject: p.subject.clone(),
            editor,
            attachments: p.attachments.iter().cloned().map(ComposeAttachment::new).collect(),
            priority: p.priority,
            request_receipt: p.request_receipt,
            message_id,
            in_reply_to: p.in_reply_to.clone(),
            references: p.references.clone(),
            draft_id,
            redirect_raw: p.redirect_raw.clone(),
            signature_id: None,
            sig_anchor: p.sig_anchor,
            background: p.background,
            accent: p.accent,
            show_format_bar: kv_flag(cx, "compose.show_format_bar", false),
            auto_cc,
            auto_bcc,
            saved_fingerprint: 0,
            dirty_since: None,
            status: None,
            dialog: None,
            picker: SendLaterPicker::default(),
            open: true,
            first_frame: true,
            focus_body: p.focus_body,
            font_size: settings.composing.default_font_size,
            from_choices,
        };
        if p.insert_signature && !redirect {
            let default_sig = w.account_id().and_then(|a| settings.account(&a)).and_then(|a| a.signature.clone());
            if let Some(sig) = default_sig.filter(|s| settings.signature(s).is_some()) {
                w.set_signature(settings, Some(sig));
                w.editor.set_doc(w.editor.doc().clone()); // the inserted signature is not an undo step
            }
        }
        w.saved_fingerprint = w.fingerprint();
        w
    }

    // ------------------------------------------------------------------ state helpers

    fn account_id(&self) -> Option<String> {
        self.from_choices.get(self.from_idx).map(|c| c.account_id.clone())
    }

    fn from_address(&self) -> Option<Address> {
        self.from_choices.get(self.from_idx).map(|c| c.address.clone())
    }

    fn is_redirect(&self) -> bool {
        self.redirect_raw.is_some()
    }

    /// Hash of everything the user can change; differs from the saved value when dirty.
    fn fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.from_idx.hash(&mut h);
        for f in [&self.to, &self.cc, &self.bcc, &self.reply_to] {
            f.addresses().hash(&mut h);
        }
        self.subject.hash(&mut h);
        self.editor.doc().hash(&mut h);
        self.editor.plain.hash(&mut h);
        for a in &self.attachments {
            a.att.filename.hash(&mut h);
            a.att.data.len().hash(&mut h);
        }
        self.priority.to_i32().hash(&mut h);
        self.request_receipt.hash(&mut h);
        h.finish()
    }

    fn is_dirty(&self) -> bool {
        self.fingerprint() != self.saved_fingerprint
    }

    fn set_status(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.status = Some((text.into(), ctx.input(|i| i.time)));
    }

    /// Signatures offered for the current account.
    fn signatures<'a>(&self, settings: &'a Settings) -> Vec<&'a crate::core::config::Signature> {
        let acct = self.account_id().unwrap_or_default();
        settings.signatures.iter().filter(|s| s.accounts.is_empty() || s.accounts.contains(&acct)).collect()
    }

    fn set_signature(&mut self, settings: &Settings, id: Option<String>) {
        let markup = id.as_deref().and_then(|i| settings.signature(i)).map(|s| s.body.clone());
        let markup = match (markup, self.editor.plain) {
            (Some(m), true) => Some(richtext::to_markup(&RichDoc::plain(&richtext::markup_to_plain(&m)))),
            (m, _) => m,
        };
        let anchor = self.sig_anchor;
        self.editor.edit(|d| init::replace_signature(d, markup.as_deref(), anchor));
        self.signature_id = id;
    }

    fn set_from(&mut self, settings: &Settings, idx: usize) {
        if idx == self.from_idx || idx >= self.from_choices.len() {
            return;
        }
        let old = self.from_address();
        let old_acct = self.account_id();
        self.from_idx = idx;
        let new = self.from_address();
        if let (Some(o), Some(n)) = (old, new) {
            for (auto, field) in [(self.auto_cc, &mut self.cc), (self.auto_bcc, &mut self.bcc)] {
                if auto {
                    for t in &mut field.tokens {
                        if t.email.eq_ignore_ascii_case(&o.email) {
                            *t = n.clone();
                        }
                    }
                }
            }
        }
        if self.account_id() != old_acct && !self.is_redirect() {
            let def = self.account_id().and_then(|a| settings.account(&a)).and_then(|a| a.signature.clone()).filter(|s| settings.signature(s).is_some());
            if def != self.signature_id {
                self.set_signature(settings, def);
            }
        }
    }

    // ------------------------------------------------------------------ attachments

    fn attach_paths(&mut self, cx: &mut UiCx, paths: Vec<std::path::PathBuf>) {
        for p in paths {
            match attachments::read_file(&p) {
                Ok(a) => self.attachments.push(ComposeAttachment::new(a)),
                Err(e) => cx.act(Action::Toast(format!("Couldn't attach {}: {e}", p.display()))),
            }
        }
    }

    fn pick_attachments(&mut self, cx: &mut UiCx) {
        if let Some(paths) = rfd::FileDialog::new().set_title("Attach Files").pick_files() {
            self.attach_paths(cx, paths);
        }
    }

    fn insert_photo(&mut self, cx: &mut UiCx) {
        let Some(paths) = rfd::FileDialog::new().set_title("Insert Photo").add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).pick_files() else { return };
        if self.editor.plain {
            self.attach_paths(cx, paths);
            return;
        }
        for p in paths {
            match std::fs::read(&p) {
                Ok(bytes) => {
                    let cid = format!("{}@purpamail", uuid::Uuid::new_v4().simple());
                    self.editor.insert_image(&cid, bytes);
                }
                Err(e) => cx.act(Action::Toast(format!("Couldn't read {}: {e}", p.display()))),
            }
        }
    }

    // ------------------------------------------------------------------ building & sending

    fn commit_fields(&mut self) {
        for f in [&mut self.to, &mut self.cc, &mut self.bcc, &mut self.reply_to] {
            f.commit();
        }
    }

    /// The message as it would be sent now.
    fn build_message(&self) -> OutgoingMessage {
        let from = self.from_address().unwrap_or_default();
        let mut attachments: Vec<OutgoingAttachment> = self.attachments.iter().map(|a| a.att.clone()).collect();
        let html = if self.editor.plain {
            None
        } else {
            let mut seen = std::collections::HashSet::new();
            for (n, cid) in self.editor.doc().image_ids().into_iter().enumerate() {
                if !seen.insert(cid.clone()) {
                    continue;
                }
                if let Some(bytes) = self.editor.image_bytes(&cid) {
                    let mime_type = attachments::image_mime(bytes, "");
                    let ext = mime_type.rsplit('/').next().unwrap_or("png").replace("jpeg", "jpg");
                    attachments.push(OutgoingAttachment { filename: format!("image{}.{ext}", n + 1), mime_type, data: bytes.to_vec(), content_id: Some(cid) });
                }
            }
            Some(richtext::to_html_document(self.editor.doc(), self.font_size, self.background))
        };
        let mut msg = OutgoingMessage {
            account_id: self.account_id().unwrap_or_default(),
            from: from.clone(),
            to: self.to.addresses(),
            cc: self.cc.addresses(),
            bcc: self.bcc.addresses(),
            reply_to: self.reply_to.addresses(),
            subject: self.subject.trim().to_string(),
            text: self.editor.to_plain(),
            html,
            attachments,
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            priority: self.priority,
            request_receipt: self.request_receipt,
            message_id: self.message_id.clone(),
            extra_headers: Vec::new(),
            redirect_raw: None,
        };
        if let Some(raw) = &self.redirect_raw {
            msg.extra_headers = compose::redirect_headers(&from, &msg.all_recipients());
            msg.redirect_raw = Some(raw.clone());
            msg.attachments.clear();
            msg.html = None;
        }
        msg
    }

    fn error(&mut self, title: &str, message: impl Into<String>) {
        self.dialog = Some(Dialog::Error { title: title.into(), message: message.into() });
    }

    /// Validate and send, starting at validation `step` (later steps resume
    /// after the user confirmed a warning).
    fn send(&mut self, cx: &mut UiCx, send_at: Option<i64>, step: u8) {
        self.commit_fields();
        if step == 0 {
            if self.from_address().is_none() {
                self.error("No account to send from", "Add a mail account in Preferences before sending messages.");
                return;
            }
            if self.is_redirect() && self.redirect_raw.as_ref().is_some_and(|r| r.is_empty()) {
                self.error("Message not downloaded", "The original message hasn't been downloaded yet. Try again in a moment.");
                return;
            }
            let all: Vec<Address> = [&self.to, &self.cc, &self.bcc].iter().flat_map(|f| f.addresses()).collect();
            if all.is_empty() {
                self.error("This message has no recipients", "Enter at least one address in the To, Cc or Bcc field.");
                return;
            }
            let invalid: Vec<String> = all.iter().chain(self.reply_to.addresses().iter()).filter(|a| !a.is_valid()).map(|a| a.to_header()).collect();
            if !invalid.is_empty() {
                self.error("Invalid address", format!("These addresses aren't valid email addresses:\n\n{}", invalid.join("\n")));
                return;
            }
        }
        if step <= 1 && !self.is_redirect() && self.subject.trim().is_empty() {
            self.dialog = Some(Dialog::Confirm { title: "Send without a subject?".into(), message: "This message has no subject. Do you want to send it anyway?".into(), ok: "Send".into(), next_step: 2, send_at });
            return;
        }
        if step <= 2 && !self.is_redirect() && self.attachments.is_empty() && init::mentions_attachment(self.editor.doc()) {
            self.dialog = Some(Dialog::Confirm {
                title: "Did you forget to attach something?".into(),
                message: "The message mentions an attachment, but nothing is attached.".into(),
                ok: "Send Anyway".into(),
                next_step: 3,
                send_at,
            });
            return;
        }
        if step <= 3 && !self.is_redirect() && cx.settings.read().composing.spell_check == SpellCheck::BeforeSending && richtext::spell_available() {
            let words = self.editor.misspelled_words();
            if !words.is_empty() {
                let list: Vec<String> = words.iter().take(12).cloned().collect();
                self.dialog = Some(Dialog::Confirm {
                    title: "The message contains spelling errors".into(),
                    message: format!("Possible misspellings: {}{}.\nDo you want to send it anyway?", list.join(", "), if words.len() > 12 { ", …" } else { "" }),
                    ok: "Send Anyway".into(),
                    next_step: 4,
                    send_at,
                });
                return;
            }
        }
        let message = self.build_message();
        cx.act(Action::Send { message: Box::new(message), send_at, draft: self.draft_id });
        self.open = false;
    }

    fn save_draft(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        if self.is_redirect() {
            return true;
        }
        let msg = self.build_message();
        let settings = cx.settings.read().clone();
        match ops::save_draft(cx.store, &settings, &msg, self.draft_id) {
            Ok((id, accounts)) => {
                self.draft_id = Some(id);
                for a in accounts {
                    cx.engine.send(Command::FlushOps(a));
                }
                self.saved_fingerprint = self.fingerprint();
                self.dirty_since = None;
                self.set_status(ctx, "Draft saved");
                true
            }
            Err(e) => {
                cx.act(Action::Toast(format!("Couldn't save draft: {e}")));
                false
            }
        }
    }

    /// Close without saving; removes an autosaved draft.
    fn discard(&mut self, cx: &mut UiCx) {
        if let Some(d) = self.draft_id.take() {
            if let Ok(accts) = ops::erase_messages(cx.store, &[d]) {
                for a in accts {
                    cx.engine.send(Command::FlushOps(a));
                }
            }
        }
        self.open = false;
    }

    fn apply_template(&mut self, settings: &Settings, t: &Template) {
        if !t.subject.is_empty() && self.subject.trim().is_empty() {
            self.subject = t.subject.clone();
        }
        let tdoc = richtext::from_markup(&t.body);
        let user_empty = {
            let d = self.editor.doc();
            d.para_ranges().iter().enumerate().all(|(i, r)| d.paras()[i].quote > 0 || d.slice(r.clone()).chars().enumerate().all(|(ci, c)| c.is_whitespace() || d.style_at(r.start + ci).signature))
        };
        if self.editor.plain && !tdoc.is_unformatted() {
            self.editor.set_plain(false);
        }
        if user_empty && self.sig_anchor == SigAnchor::End {
            let sig = self.signature_id.clone();
            self.editor.edit(|d| *d = tdoc.clone());
            if sig.is_some() {
                self.set_signature(settings, sig);
            }
        } else {
            self.editor.insert_doc(&tdoc);
        }
        self.background = crate::ui::theme::hex(&t.background).map(|c| [c.r(), c.g(), c.b()]);
        self.accent = crate::ui::theme::hex(&t.accent).map(|c| [c.r(), c.g(), c.b()]);
    }

    // ------------------------------------------------------------------ UI

    /// Draw the window (as its own OS window / viewport). Returns false once closed.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        if !self.open {
            return false;
        }
        let title = if self.subject.trim().is_empty() { "New Message".to_string() } else { self.subject.trim().to_string() };
        let vid = egui::ViewportId::from_hash_of(("compose", self.id));
        let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([760.0, 640.0]).with_min_inner_size([480.0, 360.0]);
        ctx.show_viewport_immediate(vid, builder, |ui, _class| self.window_ui(ui, cx));
        if !self.open {
            // let the parent process the outcome (and drop this viewport) right away
            ctx.request_repaint();
        }
        self.open
    }

    fn window_ui(&mut self, ui: &mut Ui, cx: &mut UiCx) {
        let ctx = ui.ctx().clone();
        let pal = Palette::of(&ctx);
        let now = ctx.input(|i| i.time);
        // closing
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.is_dirty() && !self.is_redirect() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.dialog = Some(Dialog::SaveDraft);
            } else {
                self.open = false;
                return;
            }
        }
        // window shortcuts
        if self.dialog.is_none() {
            let (send, attach, save, close) = ctx.input_mut(|i| {
                let send = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::D) || i.consume_key(Modifiers::COMMAND, Key::Enter);
                let attach = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
                let save = i.consume_key(Modifiers::COMMAND, Key::S);
                let close = i.consume_key(Modifiers::COMMAND, Key::W);
                (send, attach, save, close)
            });
            if send {
                self.send(cx, None, 0);
            }
            if attach && !self.is_redirect() {
                self.pick_attachments(cx);
            }
            if save {
                self.save_draft(&ctx, cx);
            }
            if close {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        // dropped files
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_file()).collect());
        if !dropped.is_empty() && !self.is_redirect() {
            self.attach_paths(cx, dropped);
        }
        // autosave
        if self.is_dirty() {
            let since = *self.dirty_since.get_or_insert(now);
            if now - since >= AUTOSAVE_SECS && self.dialog.is_none() && !self.is_redirect() {
                self.save_draft(&ctx, cx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        } else {
            self.dirty_since = None;
        }

        egui::Panel::top(egui::Id::new(("compose-toolbar", self.id)))
            .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(10, 7)))
            .show(ui, |ui| self.toolbar(ui, cx));
        if self.show_format_bar && !self.editor.plain && !self.is_redirect() {
            egui::Panel::top(egui::Id::new(("compose-format", self.id)))
                .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 6 }))
                .show(ui, |ui| self.editor.format_bar(ui));
        }
        egui::Panel::bottom(egui::Id::new(("compose-status", self.id)))
            .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| self.status_bar(ui, now));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.viewer_bg)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.header_fields(ui, cx);
                let settings_end = cx.settings.read().composing.insert_attachments_at_end;
                if !self.attachments.is_empty() && !settings_end {
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                        if let Some(i) = attachments::tiles(ui, &self.attachments) {
                            self.attachments.remove(i);
                        }
                    });
                    widgets::hline(ui);
                }
                self.body(ui, settings_end);
            });
        });
        // drop overlay
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) && !self.is_redirect() {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("compose-drop", self.id))));
            painter.rect_filled(rect.shrink(6.0), 12.0, pal.accent.gamma_multiply(0.12));
            painter.rect_stroke(rect.shrink(6.0), 12.0, egui::Stroke::new(2.0, pal.accent), egui::StrokeKind::Inside);
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, format!("{}  Drop files to attach", ic::PAPERCLIP), egui::FontId::proportional(18.0), pal.accent);
        }
        if self.first_frame {
            self.first_frame = false;
            if self.focus_body {
                self.editor.set_selection(0..0);
                self.editor.request_focus();
            } else {
                self.to.request_focus();
            }
        }
        self.dialogs(&ctx, cx);
    }

    fn toolbar(&mut self, ui: &mut Ui, cx: &mut UiCx) {
        let pal = Palette::of(ui.ctx());
        let redirect = self.is_redirect();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let send_label = RichText::new(format!("{}  Send", ic::PAPER_PLANE_RIGHT)).color(pal.accent_text).size(13.5);
            let send = ui.add(egui::Button::new(send_label).fill(pal.accent).corner_radius(egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }).min_size(Vec2::new(78.0, 28.0)));
            if send.on_hover_text("Send (Ctrl+Shift+D)").clicked() {
                self.send(cx, None, 0);
            }
            ui.add_space(-3.0);
            let more = ui.add(egui::Button::new(RichText::new(ic::CARET_DOWN).color(pal.accent_text).size(12.0)).fill(pal.accent.gamma_multiply(0.85)).corner_radius(egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }).min_size(Vec2::new(24.0, 28.0)));
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(200.0);
                if ui.add(egui::Button::new("Send Now").shortcut_text("Ctrl+Shift+D")).clicked() {
                    self.send(cx, None, 0);
                    ui.close();
                }
                ui.separator();
                if let Some(t) = send_later::tonight_at(21) {
                    if ui.button("Send 21:00 Tonight").clicked() {
                        self.send(cx, Some(t), 0);
                        ui.close();
                    }
                }
                if let Some(t) = send_later::tomorrow_at(8) {
                    if ui.button("Send 8:00 Tomorrow").clicked() {
                        self.send(cx, Some(t), 0);
                        ui.close();
                    }
                }
                if ui.button("Send Later…").clicked() {
                    self.picker = SendLaterPicker::default();
                    self.dialog = Some(Dialog::SendLater);
                    ui.close();
                }
            });
            ui.add_space(14.0);
            if widgets::tool_button(ui, ic::PAPERCLIP, "Attach Files (Ctrl+Shift+A)", !redirect).clicked() {
                self.pick_attachments(cx);
            }
            if widgets::tool_button(ui, ic::IMAGE, "Insert Photo", !redirect).clicked() {
                self.insert_photo(cx);
            }
            ui.add_space(8.0);
            let fmt = richtext::fmt_button(ui, ic::TEXT_AA, if self.show_format_bar { "Hide Format Bar" } else { "Show Format Bar" }, self.show_format_bar && !self.editor.plain);
            if fmt.clicked() && !redirect {
                if self.editor.plain {
                    self.editor.set_plain(false);
                    self.show_format_bar = true;
                } else {
                    self.show_format_bar = !self.show_format_bar;
                }
                let _ = cx.store.kv_set("compose.show_format_bar", if self.show_format_bar { "1" } else { "0" });
            }
            let fields = widgets::tool_button(ui, ic::ROWS, "Header Fields", true);
            egui::Popup::menu(&fields).show(|ui| {
                ui.set_min_width(180.0);
                for (label, key, flag) in [
                    ("Cc Address Field", "compose.show_cc", &mut self.show_cc),
                    ("Bcc Address Field", "compose.show_bcc", &mut self.show_bcc),
                    ("Reply-To Address Field", "compose.show_reply_to", &mut self.show_reply_to),
                    ("Priority Field", "compose.show_priority", &mut self.show_priority),
                ] {
                    if ui.checkbox(flag, label).changed() {
                        let _ = cx.store.kv_set(key, if *flag { "1" } else { "0" });
                    }
                }
            });
            if !redirect {
                let tmpl = widgets::tool_button(ui, ic::SWATCHES, "Templates & Stationery", true);
                egui::Popup::menu(&tmpl).show(|ui| {
                    ui.set_min_width(200.0);
                    let settings = cx.settings.read().clone();
                    let mut cats: Vec<&str> = settings.templates.iter().map(|t| t.category.as_str()).collect();
                    cats.dedup();
                    let mut chosen: Option<Template> = None;
                    for cat in cats {
                        ui.label(RichText::new(if cat.is_empty() { "Templates" } else { cat }).size(11.5).color(pal.text_tertiary));
                        for t in settings.templates.iter().filter(|t| t.category == cat) {
                            let swatch = crate::ui::theme::hex(&t.background).unwrap_or(pal.field_bg);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                                ui.painter().rect_filled(r, 3.0, swatch);
                                ui.painter().rect_stroke(r, 3.0, egui::Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
                                if ui.selectable_label(false, &t.name).clicked() {
                                    chosen = Some(t.clone());
                                    ui.close();
                                }
                            });
                        }
                    }
                    if let Some(t) = chosen {
                        self.apply_template(&settings, &t);
                    }
                });
            }
            let prio_icon = match self.priority {
                Priority::High => ic::ARROW_FAT_UP,
                Priority::Low => ic::ARROW_FAT_DOWN,
                Priority::Normal => ic::EXCLAMATION_MARK,
            };
            let prio = widgets::tool_button(ui, prio_icon, "Priority", true);
            egui::Popup::menu(&prio).show(|ui| {
                for (p, label) in [(Priority::High, "High Priority"), (Priority::Normal, "Normal Priority"), (Priority::Low, "Low Priority")] {
                    if ui.selectable_label(self.priority == p, label).clicked() {
                        self.priority = p;
                        ui.close();
                    }
                }
            });
            let more = widgets::tool_button(ui, ic::DOTS_THREE_CIRCLE, "More", true);
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(210.0);
                ui.checkbox(&mut self.request_receipt, "Request Read Receipt");
                ui.separator();
                if !redirect {
                    let label = if self.editor.plain { "Make Rich Text" } else { "Make Plain Text" };
                    if ui.add(egui::Button::new(label).shortcut_text("Ctrl+Shift+T")).clicked() {
                        let plain = !self.editor.plain;
                        self.editor.set_plain(plain);
                        ui.close();
                    }
                    if ui.add(egui::Button::new("Save as Draft").shortcut_text("Ctrl+S")).clicked() {
                        let ctx = ui.ctx().clone();
                        self.save_draft(&ctx, cx);
                        ui.close();
                    }
                }
            });
            if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::T)) && !redirect {
                let plain = !self.editor.plain;
                self.editor.set_plain(plain);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.priority != Priority::Normal {
                    let (txt, col) = if self.priority == Priority::High { ("High Priority", pal.danger) } else { ("Low Priority", pal.text_secondary) };
                    ui.label(RichText::new(txt).size(12.0).color(col));
                }
                if self.request_receipt {
                    ui.label(RichText::new(format!("{} Read receipt", ic::CHECKS)).size(12.0).color(pal.text_secondary));
                }
            });
        });
    }

    /// One header row: right-aligned label column + content, followed by a separator.
    fn header_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
        let pal = Palette::of(ui.ctx());
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 14, top: 5, bottom: 5 }).show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                ui.allocate_ui_with_layout(Vec2::new(LABEL_W, 24.0), Layout::right_to_left(Align::Center), |ui| {
                    ui.set_min_size(Vec2::new(LABEL_W, 24.0));
                    ui.label(RichText::new(label).size(13.0).color(pal.text_secondary));
                });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_min_height(24.0);
                    add(ui);
                });
            });
        });
        widgets::hline(ui);
    }

    fn header_fields(&mut self, ui: &mut Ui, cx: &mut UiCx) {
        let pal = Palette::of(ui.ctx());
        let settings = cx.settings.read().clone();
        let style = TokenStyle {
            show_addresses: settings.composing.show_addresses_not_names,
            mark_not_ending_with: (settings.composing.mark_addresses_enabled && !settings.composing.mark_addresses_not_ending_with.trim().is_empty()).then(|| settings.composing.mark_addresses_not_ending_with.clone()),
        };
        let redirect = self.is_redirect();
        ui.add_space(4.0);
        if redirect {
            egui::Frame::new().fill(pal.warning_bg).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{}  Redirect: the message is sent unchanged. Replies go to the original sender.", ic::ARROW_ELBOW_RIGHT)).size(12.5).color(if pal.dark { pal.text } else { Color32::from_rgb(0x6b, 0x4e, 0x00) }));
            });
        }
        let mut changed_to = false;
        Self::header_row(ui, "To:", |ui| changed_to |= self.to.show(ui, cx, &style));
        if self.show_cc {
            Self::header_row(ui, "Cc:", |ui| {
                self.cc.show(ui, cx, &style);
            });
        }
        if self.show_bcc {
            Self::header_row(ui, "Bcc:", |ui| {
                self.bcc.show(ui, cx, &style);
            });
        }
        if self.show_reply_to && !redirect {
            Self::header_row(ui, "Reply-To:", |ui| {
                self.reply_to.show(ui, cx, &style);
            });
        }
        if !redirect {
            Self::header_row(ui, "Subject:", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.subject).id(egui::Id::new(("compose-subject", self.id))).frame(egui::Frame::NONE).desired_width(f32::INFINITY).margin(egui::Margin { left: 2, right: 2, top: 4, bottom: 2 }).font(egui::FontId::proportional(14.0)));
            });
        } else {
            Self::header_row(ui, "Subject:", |ui| {
                ui.label(RichText::new(&self.subject).size(14.0).color(pal.text_secondary));
            });
        }
        // From + signature
        let choices: Vec<String> = self.from_choices.iter().map(|c| c.address.to_header()).collect();
        let sigs: Vec<(String, String)> = self.signatures(&settings).into_iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        let mut new_from: Option<usize> = None;
        let mut new_sig: Option<Option<String>> = None;
        let cur_sig = self.signature_id.clone();
        let from_idx = self.from_idx;
        let id = self.id;
        Self::header_row(ui, "From:", |ui| {
            ui.horizontal(|ui| {
                if choices.is_empty() {
                    ui.label(RichText::new("No account configured").color(pal.danger));
                } else {
                    egui::ComboBox::from_id_salt(("compose-from", id)).width(300.0).selected_text(choices.get(from_idx).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                        for (i, c) in choices.iter().enumerate() {
                            if ui.selectable_label(i == from_idx, c).clicked() {
                                new_from = Some(i);
                            }
                        }
                    });
                }
                if !redirect {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let current = cur_sig.as_ref().and_then(|s| sigs.iter().find(|(i, _)| i == s)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
                        egui::ComboBox::from_id_salt(("compose-sig", id)).width(130.0).selected_text(current).show_ui(ui, |ui| {
                            if ui.selectable_label(cur_sig.is_none(), "None").clicked() {
                                new_sig = Some(None);
                            }
                            for (sid, name) in &sigs {
                                if ui.selectable_label(cur_sig.as_ref() == Some(sid), name).clicked() {
                                    new_sig = Some(Some(sid.clone()));
                                }
                            }
                        });
                        ui.label(RichText::new("Signature:").size(13.0).color(pal.text_secondary));
                    });
                }
            });
        });
        if let Some(i) = new_from {
            self.set_from(&settings, i);
        }
        if let Some(s) = new_sig {
            if s != self.signature_id {
                self.set_signature(&settings, s);
            }
        }
        if self.show_priority {
            Self::header_row(ui, "Priority:", |ui| {
                egui::ComboBox::from_id_salt(("compose-prio", id)).width(120.0).selected_text(match self.priority {
                    Priority::High => "High",
                    Priority::Normal => "Normal",
                    Priority::Low => "Low",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.priority, Priority::High, "High");
                    ui.selectable_value(&mut self.priority, Priority::Normal, "Normal");
                    ui.selectable_value(&mut self.priority, Priority::Low, "Low");
                });
            });
        }
        let _ = changed_to;
    }

    fn body(&mut self, ui: &mut Ui, attachments_at_end: bool) {
        let pal = Palette::of(ui.ctx());
        let rgb = |c: Rgb| Color32::from_rgb(c[0], c[1], c[2]);
        let tint = self.background.filter(|_| !self.editor.plain).map(|c| {
            if pal.dark { pal.viewer_bg.lerp_to_gamma(rgb(self.accent.unwrap_or(c)), 0.10) } else { rgb(c) }
        });
        let avail = ui.available_height();
        let frame = egui::Frame::new().fill(tint.unwrap_or(Color32::TRANSPARENT)).inner_margin(egui::Margin { left: 22, right: 22, top: 16, bottom: 16 });
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let att_h = if attachments_at_end && !self.attachments.is_empty() { 80.0 } else { 0.0 };
            self.editor.min_height = (avail - 32.0 - att_h).max(200.0);
            self.editor.ui(ui);
            if attachments_at_end && !self.attachments.is_empty() {
                ui.add_space(12.0);
                if let Some(i) = attachments::tiles(ui, &self.attachments) {
                    self.attachments.remove(i);
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, now: f64) {
        let pal = Palette::of(ui.ctx());
        ui.horizontal(|ui| {
            ui.set_min_height(20.0);
            let status = self.status.as_ref().filter(|(_, t)| now - t < 4.0).map(|(s, _)| s.clone());
            if let Some(s) = status {
                ui.label(RichText::new(format!("{} {s}", ic::CHECK)).size(12.0).color(pal.text_secondary));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                let d = self.editor.doc();
                let words: usize = d.para_ranges().iter().enumerate().filter(|(i, _)| d.paras()[*i].quote == 0).map(|(_, r)| d.slice(r.clone()).split_whitespace().count()).sum();
                let mode = if self.editor.plain { "Plain Text" } else { "Rich Text" };
                ui.label(RichText::new(format!("{words} {} · {mode}", if words == 1 { "word" } else { "words" })).size(12.0).color(pal.text_tertiary));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                attachments::size_summary(ui, &self.attachments);
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let Some(dialog) = &mut self.dialog else { return };
        let pal = Palette::of(ctx);
        enum Outcome {
            None,
            Close,
            SaveAndClose,
            DiscardAndClose,
            Continue(u8, Option<i64>),
            Schedule(i64),
        }
        let mut outcome = Outcome::None;
        let mid = egui::Id::new(("compose-dialog", self.id));
        let heading = |ui: &mut Ui, icon: &str, color: Color32, title: &str, msg: &str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(34.0).color(color));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(15.0).family(super::fonts::bold()));
                    ui.add_space(2.0);
                    ui.label(RichText::new(msg).color(pal.text_secondary));
                });
            });
            ui.add_space(14.0);
        };
        match dialog {
            Dialog::SaveDraft => {
                egui::Modal::new(mid).show(ctx, |ui| {
                    ui.set_width(400.0);
                    heading(ui, ic::FLOPPY_DISK, pal.accent, "Do you want to save this message as a draft?", "If you don't save it, your changes will be lost.");
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Don't Save").min_size(Vec2::new(90.0, 26.0))).clicked() {
                            outcome = Outcome::DiscardAndClose;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::primary_button(ui, "Save").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                                outcome = Outcome::SaveAndClose;
                            }
                            if ui.add(egui::Button::new("Cancel").min_size(Vec2::new(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                                outcome = Outcome::Close;
                            }
                        });
                    });
                });
            }
            Dialog::Error { title, message } => {
                egui::Modal::new(mid).show(ctx, |ui| {
                    ui.set_width(380.0);
                    heading(ui, ic::WARNING_CIRCLE, pal.danger, title, message);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::Confirm { title, message, ok, next_step, send_at } => {
                let (step, at) = (*next_step, *send_at);
                egui::Modal::new(mid).show(ctx, |ui| {
                    ui.set_width(400.0);
                    heading(ui, ic::WARNING, Color32::from_rgb(0xe8, 0x90, 0x0c), title, message);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, ok).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            outcome = Outcome::Continue(step, at);
                        }
                        if ui.add(egui::Button::new("Cancel").min_size(Vec2::new(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            outcome = Outcome::Close;
                        }
                    });
                });
            }
            Dialog::SendLater => {
                let picker = &mut self.picker;
                egui::Modal::new(mid).show(ctx, |ui| match picker.ui(ui) {
                    Some(true) => {
                        if let Some(t) = picker.timestamp() {
                            outcome = Outcome::Schedule(t);
                        }
                    }
                    Some(false) => outcome = Outcome::Close,
                    None => {}
                });
            }
        }
        match outcome {
            Outcome::None => {}
            Outcome::Close => self.dialog = None,
            Outcome::SaveAndClose => {
                self.dialog = None;
                if self.save_draft(ctx, cx) {
                    self.open = false;
                }
            }
            Outcome::DiscardAndClose => {
                self.dialog = None;
                self.discard(cx);
            }
            Outcome::Continue(step, at) => {
                self.dialog = None;
                self.send(cx, at, step);
            }
            Outcome::Schedule(t) => {
                self.dialog = None;
                self.send(cx, Some(t), 0);
            }
        }
    }
}
