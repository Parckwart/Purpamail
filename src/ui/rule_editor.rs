//! Editors for criteria (rules and smart mailboxes).
//! STUB: to be implemented.

use super::actions::UiCx;
use crate::core::config::SmartMailbox;

/// Dialog for creating / editing a smart mailbox.
pub struct SmartMailboxDialog {
    pub mailbox: SmartMailbox,
    pub is_new: bool,
}

impl SmartMailboxDialog {
    pub fn new(mailbox: SmartMailbox, is_new: bool) -> Self {
        Self { mailbox, is_new }
    }
    /// Returns false once the dialog is closed (saved or cancelled).
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        let _ = (ctx, cx);
        false
    }
}
