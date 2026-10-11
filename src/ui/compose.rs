//! Compose window.
//! STUB: to be implemented.

use super::actions::{ComposeInit, UiCx};

pub struct ComposeWindow {
    pub id: u64,
}

impl ComposeWindow {
    pub fn new(cx: &mut UiCx, init: ComposeInit, id: u64) -> Self {
        let _ = (cx, init);
        Self { id }
    }

    /// Draw the window (as its own OS window / viewport). Returns false once closed.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        let _ = (ctx, cx);
        false
    }
}
