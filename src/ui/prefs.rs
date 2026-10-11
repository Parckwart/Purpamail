//! Preferences window.
//! STUB: to be implemented.

use super::actions::{PrefsTab, UiCx};

#[derive(Default)]
pub struct PrefsWindow {
    pub open: bool,
    pub tab: PrefsTab,
}

impl PrefsWindow {
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let _ = (ctx, cx);
    }
}
