//! Secondary windows: compose, preferences, activity, connection doctor,
//! message viewer windows, prompts and dialogs.

use egui::{RichText, Vec2};
use egui_phosphor::regular as ic;

use super::actions::*;
use super::app::{PromptKind, PurpamailApp};
use super::theme::Palette;
use super::widgets;
use crate::net::{ActivityState, ServiceStatus};

impl PurpamailApp {
    pub fn windows(&mut self, ctx: &egui::Context) {
        // Compose windows
        let mut composers = std::mem::take(&mut self.composers);
        {
            let mut cx = UiCx { store: &self.store, settings: &self.settings, secrets: &self.secrets, engine: &self.engine, actions: &mut self.actions };
            composers.retain_mut(|w| w.show(ctx, &mut cx));
        }
        self.composers.append(&mut composers);

        {
            let mut cx = UiCx { store: &self.store, settings: &self.settings, secrets: &self.secrets, engine: &self.engine, actions: &mut self.actions };
            if self.prefs.open {
                self.prefs.show(ctx, &mut cx);
            }
            if self.contacts_win.open {
                self.contacts_win.show(ctx, &mut cx);
            }
            if let Some(setup) = &mut self.account_setup {
                if !setup.show(ctx, &mut cx) {
                    self.account_setup = None;
                    let mut s = self.settings.write();
                    if !s.onboarded {
                        s.onboarded = true;
                        drop(s);
                        cx.act(Action::SettingsChanged);
                    }
                }
            }
            if let Some(d) = &mut self.smart_dialog {
                if !d.show(ctx, &mut cx) {
                    self.smart_dialog = None;
                    self.list_dirty = true;
                }
            }
            // raw source windows
            let mut raws = std::mem::take(&mut self.raw_windows);
            raws.retain(|id| super::viewer::raw_source_window(ctx, &mut cx, *id));
            self.raw_windows = raws;
        }

        self.message_windows_ui(ctx);
        self.prompt_ui(ctx);
        self.confirm_ui(ctx);
        if self.show_activity {
            self.activity_window(ctx);
        }
        if self.show_doctor {
            self.doctor_window(ctx);
        }
        if self.show_about {
            self.about_window(ctx);
        }
        if self.show_shortcuts {
            self.shortcuts_window(ctx);
        }
    }

    fn message_windows_ui(&mut self, ctx: &egui::Context) {
        let mut wins = std::mem::take(&mut self.message_windows);
        let mut cx = UiCx { store: &self.store, settings: &self.settings, secrets: &self.secrets, engine: &self.engine, actions: &mut self.actions };
        wins.retain_mut(|(wid, id, viewer)| {
            let Some(m) = cx.store.message(*id) else { return false };
            let mut open = true;
            let vid = egui::ViewportId::from_hash_of(("message-window", *wid));
            ctx.show_viewport_immediate(vid, egui::ViewportBuilder::default().with_title(if m.subject.is_empty() { "(No Subject)".to_string() } else { m.subject.clone() }).with_inner_size([760.0, 720.0]), |ui, _class| {
                let pal = Palette::of(ui.ctx());
                egui::Panel::top("msgwin_toolbar").frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(8, 6))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        use crate::core::compose::ReplyKind;
                        if widgets::tool_button(ui, ic::TRASH, "Delete", true).clicked() {
                            cx.act(Action::Delete(vec![*id]));
                            open = false;
                        }
                        if widgets::tool_button(ui, ic::ARCHIVE, "Archive", true).clicked() {
                            cx.act(Action::Archive(vec![*id]));
                            open = false;
                        }
                        if widgets::tool_button(ui, ic::PROHIBIT, "Junk", true).clicked() {
                            cx.act(Action::MarkJunk { ids: vec![*id], junk: true });
                            open = false;
                        }
                        ui.add_space(10.0);
                        if widgets::tool_button(ui, ic::ARROW_BEND_UP_LEFT, "Reply", true).clicked() {
                            cx.act(Action::Compose(ComposeInit::Respond { message: *id, kind: ReplyKind::Reply, selected_text: viewer.selected_text() }));
                        }
                        if widgets::tool_button(ui, ic::ARROW_BEND_DOUBLE_UP_LEFT, "Reply All", true).clicked() {
                            cx.act(Action::Compose(ComposeInit::Respond { message: *id, kind: ReplyKind::ReplyAll, selected_text: viewer.selected_text() }));
                        }
                        if widgets::tool_button(ui, ic::ARROW_BEND_UP_RIGHT, "Forward", true).clicked() {
                            cx.act(Action::Compose(ComposeInit::Respond { message: *id, kind: ReplyKind::Forward, selected_text: None }));
                        }
                        ui.add_space(10.0);
                        if widgets::tool_button(ui, ic::PRINTER, "Print", true).clicked() {
                            cx.act(Action::Print(*id));
                        }
                    });
                });
                egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.viewer_bg)).show(ui, |ui| {
                    viewer.show(ui, &mut cx, std::slice::from_ref(&m), Some(*id));
                });
                if ui.input(|i| i.viewport().close_requested()) {
                    open = false;
                }
            });
            open
        });
        self.message_windows = wins;
    }

    fn prompt_ui(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.prompt else { return };
        let mut done: Option<bool> = None;
        egui::Modal::new(egui::Id::new("prompt")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(&p.title);
            ui.add_space(6.0);
            ui.label(&p.label);
            let r = ui.add(egui::TextEdit::singleline(&mut p.text).desired_width(f32::INFINITY));
            if !r.has_focus() && p.text.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = match p.kind {
                        PromptKind::NewMailbox { .. } => "Create",
                        PromptKind::RenameMailbox(_) => "Rename",
                        PromptKind::RemindCustom(_) => "Remind Me",
                    };
                    if widgets::primary_button(ui, ok).clicked() {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
        if let Some(ok) = done {
            let p = self.prompt.take().unwrap();
            if ok {
                if let Err(e) = self.finish_prompt_internal(ctx, p) {
                    self.toast(ctx, e.to_string());
                }
            }
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.confirm else { return };
        let mut result: Option<bool> = None;
        let pal = Palette::of(ctx);
        egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::WARNING_CIRCLE).size(32.0).color(pal.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&c.title).family(super::fonts::bold()).size(15.0));
                    ui.label(&c.message);
                });
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(&c.ok).color(egui::Color32::WHITE)).fill(pal.danger)).clicked() {
                    result = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    result = Some(false);
                }
            });
        });
        if let Some(ok) = result {
            let c = self.confirm.take().unwrap();
            if ok {
                self.actions.push(c.action);
            }
        }
    }

    fn activity_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let now = ctx.input(|i| i.time);
        // forget finished activities after a while
        self.activities.retain(|_, (a, t)| a.state == ActivityState::Running || now - *t < 120.0);
        let pal = Palette::of(ctx);
        egui::Window::new("Activity").open(&mut open).default_size([460.0, 360.0]).show(ctx, |ui| {
            if self.activities.is_empty() {
                ui.label(RichText::new("No activity").color(pal.text_tertiary));
            }
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for (a, _) in self.activities.values().rev() {
                    ui.horizontal(|ui| {
                        match &a.state {
                            ActivityState::Running => {
                                ui.add(egui::Spinner::new().size(14.0));
                            }
                            ActivityState::Done => {
                                ui.label(RichText::new(ic::CHECK_CIRCLE).color(egui::Color32::from_rgb(0x3c, 0xb8, 0x4a)));
                            }
                            ActivityState::Failed(_) => {
                                ui.label(RichText::new(ic::X_CIRCLE).color(pal.danger));
                            }
                        }
                        ui.vertical(|ui| {
                            let acct = self.account_name(&a.account);
                            ui.label(RichText::new(format!("{} — {}", acct, a.title)).strong());
                            let detail = match &a.state {
                                ActivityState::Failed(e) => e.clone(),
                                _ => a.detail.clone(),
                            };
                            if !detail.is_empty() {
                                ui.label(RichText::new(detail).small().color(pal.text_secondary));
                            }
                            if let (ActivityState::Running, Some(p)) = (&a.state, a.progress) {
                                ui.add(egui::ProgressBar::new(p).desired_height(6.0));
                            }
                        });
                    });
                    ui.separator();
                }
            });
            ui.collapsing("Mail Log", |ui| {
                let mut logging = self.settings.read().logging;
                if ui.checkbox(&mut logging, "Log all connection activity").changed() {
                    self.settings.write().logging = logging;
                    self.actions.push(Action::SettingsChanged);
                }
                egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in self.log.iter().rev().take(300).rev() {
                        ui.label(RichText::new(l).monospace().small());
                    }
                });
                if ui.button("Copy Log").clicked() {
                    ui.ctx().copy_text(self.log.join("\n"));
                }
            });
        });
        self.show_activity = open;
    }

    fn doctor_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let pal = Palette::of(ctx);
        egui::Window::new("Connection Doctor").open(&mut open).default_size([560.0, 420.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::STETHOSCOPE).size(28.0).color(pal.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new(if self.doctor_running { "Checking connection status…" } else { "Connection check complete" }).strong());
                    ui.label(RichText::new("Purpamail checks each account's incoming and outgoing servers.").small().color(pal.text_secondary));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!self.doctor_running, egui::Button::new("Check Again")).clicked() {
                        self.actions.push(Action::ShowConnectionDoctor);
                    }
                    if self.doctor_running {
                        ui.add(egui::Spinner::new());
                    }
                });
            });
            ui.separator();
            let status = |ui: &mut egui::Ui, s: &ServiceStatus, what: &str| {
                let (icon, col, text) = match s {
                    ServiceStatus::Ok => (ic::CHECK_CIRCLE, egui::Color32::from_rgb(0x3c, 0xb8, 0x4a), format!("{what}: connected and logged in")),
                    ServiceStatus::Failed(e) => (ic::X_CIRCLE, pal.danger, format!("{what}: {e}")),
                    ServiceStatus::Unknown => (ic::QUESTION, pal.text_tertiary, format!("{what}: not checked")),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).color(col));
                    ui.label(text);
                });
            };
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.settings.read().accounts.is_empty() {
                    ui.label("No accounts configured.");
                }
                for r in &self.doctor {
                    ui.label(RichText::new(&r.account_name).strong());
                    status(ui, &r.incoming, "Incoming");
                    status(ui, &r.outgoing, "Outgoing");
                    ui.collapsing(format!("Details##{}", r.account), |ui| {
                        ui.label(RichText::new(&r.log).monospace().small());
                    });
                    ui.separator();
                }
            });
        });
        self.show_doctor = open;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let pal = Palette::of(ctx);
        egui::Window::new("About Purpamail").open(&mut open).collapsible(false).resizable(false).fixed_size([340.0, 230.0]).show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(ic::ENVELOPE_SIMPLE_OPEN).size(56.0).color(pal.accent));
                ui.label(RichText::new("Purpamail").family(super::fonts::bold()).size(22.0));
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(pal.text_secondary));
                ui.add_space(6.0);
                ui.label("A fast, full-featured mail client for Linux.");
                ui.label(RichText::new("Built with Rust and egui.").small().color(pal.text_tertiary));
            });
        });
        self.show_about = open;
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Keyboard Shortcuts").open(&mut open).default_size([420.0, 520.0]).show(ctx, |ui| {
            let rows = [
                ("New message", "Ctrl+N"),
                ("Get all new mail", "Ctrl+Shift+N"),
                ("Reply", "Ctrl+R"),
                ("Reply all", "Ctrl+Shift+R"),
                ("Forward", "Ctrl+Shift+F"),
                ("Redirect", "Ctrl+Shift+E"),
                ("Send (in compose window)", "Ctrl+Shift+D / Ctrl+Enter"),
                ("Delete", "Delete / Backspace"),
                ("Archive", "Ctrl+Shift+A"),
                ("Mark as read / unread", "Ctrl+Shift+U"),
                ("Flag / unflag", "Ctrl+Shift+L"),
                ("Move to Junk", "Ctrl+Shift+J"),
                ("Apply rules", "Ctrl+L"),
                ("Search", "Ctrl+Alt+F"),
                ("Go to favorite mailbox 1–9", "Ctrl+1 … Ctrl+9"),
                ("Show / hide sidebar", "Ctrl+Shift+M"),
                ("Activity", "Ctrl+Alt+0"),
                ("Raw source", "Ctrl+Alt+U"),
                ("Open in new window", "Enter"),
                ("Expand / collapse conversation", "→ / ←"),
                ("Previous / next message", "↑ / ↓"),
                ("Extend selection", "Shift+↑ / Shift+↓"),
                ("Undo", "Ctrl+Z"),
                ("Print", "Ctrl+P"),
                ("Save as", "Ctrl+Shift+S"),
                ("Preferences", "Ctrl+,"),
                ("Quit", "Ctrl+Q"),
            ];
            egui::Grid::new("shortcuts").striped(true).num_columns(2).spacing(Vec2::new(24.0, 6.0)).show(ui, |ui| {
                for (a, k) in rows {
                    ui.label(a);
                    ui.label(RichText::new(k).monospace());
                    ui.end_row();
                }
            });
        });
        self.show_shortcuts = open;
    }
}
