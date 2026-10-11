//! Message viewer: headers, body (HTML / plain), attachments, conversation view.
//! STUB: to be implemented.

use egui::Ui;

use super::actions::UiCx;
use crate::core::model::{MessageId, MessageSummary};

#[derive(Default)]
pub struct MessageViewer {}

impl MessageViewer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show one message, or a whole conversation when `msgs` has several entries
    /// (ordered oldest first). `focus` is the message the user selected.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut UiCx, msgs: &[MessageSummary], focus: Option<MessageId>) {
        let _ = (cx, focus);
        for m in msgs {
            ui.heading(&m.subject);
            ui.label(&m.snippet);
        }
    }

    /// Forget cached rendering for a message (body arrived, flags changed, ...).
    pub fn invalidate(&mut self, id: MessageId) {
        let _ = id;
    }

    /// Text currently selected by the user in the viewer (for "quote selected text" replies).
    pub fn selected_text(&self) -> Option<String> {
        None
    }
}

/// Window showing the raw RFC 5322 source of a message. Returns false when closed.
pub fn raw_source_window(ctx: &egui::Context, cx: &mut UiCx, id: MessageId) -> bool {
    let _ = (ctx, cx, id);
    false
}
