//! Preferences window.
//!
//! A separate OS window with a toolbar of tabs (General, Accounts, Junk Mail,
//! Fonts & Colors, Viewing, Composing, Signatures, Rules, Privacy). Every tab
//! edits a copy of the settings; changes are written back immediately (they
//! apply live) and [`Action::SettingsChanged`] is pushed once per edit burst.

mod accounts;
mod composing;
mod fonts_tab;
pub mod form;
mod general;
mod junk;
mod privacy;
mod rules;
mod signatures;
mod viewing;

use egui::{Align2, FontId, RichText, Sense, Vec2, pos2};
use egui_phosphor::regular as ic;

use super::actions::{Action, PrefsTab, UiCx};
use super::theme::Palette;
use crate::core::config::Settings;
use crate::net::Event;

/// Seconds without further edits before settings are saved.
const COMMIT_DELAY: f64 = 0.4;

impl PrefsTab {
    pub const ALL: [PrefsTab; 9] =
        [PrefsTab::General, PrefsTab::Accounts, PrefsTab::Junk, PrefsTab::Fonts, PrefsTab::Viewing, PrefsTab::Composing, PrefsTab::Signatures, PrefsTab::Rules, PrefsTab::Privacy];

    pub fn title(&self) -> &'static str {
        match self {
            PrefsTab::General => "General",
            PrefsTab::Accounts => "Accounts",
            PrefsTab::Junk => "Junk Mail",
            PrefsTab::Fonts => "Fonts & Colors",
            PrefsTab::Viewing => "Viewing",
            PrefsTab::Composing => "Composing",
            PrefsTab::Signatures => "Signatures",
            PrefsTab::Rules => "Rules",
            PrefsTab::Privacy => "Privacy",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            PrefsTab::General => ic::GEAR_SIX,
            PrefsTab::Accounts => ic::AT,
            PrefsTab::Junk => ic::PROHIBIT,
            PrefsTab::Fonts => ic::TEXT_AA,
            PrefsTab::Viewing => ic::EYEGLASSES,
            PrefsTab::Composing => ic::PENCIL_SIMPLE_LINE,
            PrefsTab::Signatures => ic::SIGNATURE,
            PrefsTab::Rules => ic::LIST_CHECKS,
            PrefsTab::Privacy => ic::HAND_PALM,
        }
    }
}

/// Bookkeeping shared by all tabs during one frame.
#[derive(Default)]
pub(crate) struct Edit {
    /// Something outside the settings struct changed (e.g. a password): save soon.
    pub dirty: bool,
    /// Save immediately (structural changes like removing an account).
    pub now: bool,
}

#[derive(Default)]
pub struct PrefsWindow {
    pub open: bool,
    pub tab: PrefsTab,
    general: general::GeneralState,
    accounts: accounts::AccountsState,
    junk: junk::JunkState,
    fonts: fonts_tab::FontsState,
    viewing: viewing::ViewingState,
    signatures: signatures::SignaturesState,
    rules: rules::RulesState,
    /// Time of the last uncommitted edit.
    pending_since: Option<f64>,
}

impl PrefsWindow {
    /// Feed engine events (connection test results, account status) to the window.
    pub fn on_event(&mut self, ev: &Event) {
        self.accounts.on_event(ev);
    }

    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let title = format!("Preferences — {}", self.tab.title());
        let vid = egui::ViewportId::from_hash_of("purpamail-preferences");
        let builder = egui::ViewportBuilder::default().with_title(title.clone()).with_inner_size([760.0, 560.0]).with_min_inner_size([680.0, 440.0]);
        let mut close = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            let now = ui.input(|i| i.time);
            form::window_style(ui);
            let pal = Palette::of(ui.ctx());
            egui::Panel::top("prefs_tabs")
                .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(10, 6)))
                .show(ui, |ui| self.toolbar(ui));

            let before = cx.settings.read().clone();
            let mut s: Settings = before.clone();
            let mut edit = Edit::default();
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window_bg).inner_margin(egui::Margin::symmetric(18, 14))).show(ui, |ui| match self.tab {
                PrefsTab::General => general::show(ui, &mut self.general, &mut s, cx, &mut edit),
                PrefsTab::Accounts => accounts::show(ui, &mut self.accounts, &mut s, cx, &mut edit),
                PrefsTab::Junk => junk::show(ui, &mut self.junk, &mut s, cx, &mut edit),
                PrefsTab::Fonts => fonts_tab::show(ui, &mut self.fonts, &mut s),
                PrefsTab::Viewing => viewing::show(ui, &mut self.viewing, &mut s),
                PrefsTab::Composing => composing::show(ui, &mut self.fonts, &mut s),
                PrefsTab::Signatures => signatures::show(ui, &mut self.signatures, &mut s),
                PrefsTab::Rules => rules::show(ui, &mut self.rules, &mut s, cx, &mut edit),
                PrefsTab::Privacy => privacy::show(ui, &mut s),
            });

            if s != before {
                *cx.settings.write() = s;
                edit.dirty = true;
            }
            if edit.dirty {
                self.pending_since = Some(now);
            }
            let close_req = ui.input(|i| i.viewport().close_requested() || (i.key_pressed(egui::Key::W) && i.modifiers.command));
            if let Some(t) = self.pending_since {
                let busy = ui.input(|i| i.pointer.any_down());
                if edit.now || close_req || (now - t >= COMMIT_DELAY && !busy) {
                    cx.act(Action::SettingsChanged);
                    self.pending_since = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                }
            }
            close = close_req;
        });
        if close {
            self.open = false;
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let pal = Palette::of(ui.ctx());
        let item = Vec2::new(74.0, 52.0);
        let total = item.x * PrefsTab::ALL.len() as f32;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 0.0;
            for t in PrefsTab::ALL {
                let (rect, resp) = ui.allocate_exact_size(item, Sense::click());
                let sel = self.tab == t;
                if sel {
                    ui.painter().rect_filled(rect.shrink(2.0), 8.0, pal.accent_soft);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect.shrink(2.0), 8.0, pal.hover_bg);
                }
                let col = if sel { pal.accent } else { pal.text_secondary };
                ui.painter().text(pos2(rect.center().x, rect.top() + 18.0), Align2::CENTER_CENTER, t.icon(), FontId::proportional(22.0), col);
                ui.painter().text(pos2(rect.center().x, rect.bottom() - 11.0), Align2::CENTER_CENTER, t.title(), FontId::proportional(11.5), if sel { pal.accent } else { pal.text });
                if resp.clicked() {
                    self.tab = t;
                }
            }
        });
    }
}

/// A vertically scrolling page with a centered form, used by most tabs.
pub(crate) fn page<R>(ui: &mut egui::Ui, id: &str, max_w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let r = form::centered(ui, max_w, add);
            ui.add_space(16.0);
            r
        })
        .inner
}

/// Title + subtitle header used at the top of list-style tabs.
pub(crate) fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let pal = Palette::of(ui.ctx());
    ui.label(RichText::new(title).family(super::fonts::bold()).size(15.0));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).size(12.0).color(pal.text_secondary));
    }
    ui.add_space(4.0);
}
