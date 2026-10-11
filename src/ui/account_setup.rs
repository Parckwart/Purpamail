//! "Add Account" assistant.
//! STUB: to be implemented.

use super::actions::UiCx;

#[derive(Default)]
pub struct AccountSetup {}

impl AccountSetup {
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns false once finished or cancelled.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        let _ = (ctx, cx);
        false
    }
    /// Feed engine events (test results) to the assistant.
    pub fn on_event(&mut self, ev: &crate::net::Event) {
        let _ = ev;
    }
}
