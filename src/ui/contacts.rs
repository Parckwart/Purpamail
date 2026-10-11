//! Contacts window: address book, VIPs, blocked senders, previous recipients.
//! STUB: to be implemented.

use super::actions::UiCx;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContactsTab {
    #[default]
    Contacts,
    PreviousRecipients,
    Blocked,
}

#[derive(Default)]
pub struct ContactsWindow {
    pub open: bool,
    pub tab: ContactsTab,
}

impl ContactsWindow {
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let _ = (ctx, cx);
    }
}
