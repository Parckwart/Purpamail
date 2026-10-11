//! Junk Mail preferences: filtering, exemptions, sensitivity, training and
//! blocked senders.

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::form::{self, Form};
use super::{Edit, page};
use crate::core::config::{JunkAction, Settings};
use crate::ui::actions::{Action, PrefsTab, UiCx};
use crate::ui::theme::Palette;

const F: Form = Form::new(210.0);

#[derive(Default)]
pub struct JunkState {
    new_blocked: String,
    confirm_reset: bool,
    reset_done: Option<String>,
}

pub fn show(ui: &mut Ui, st: &mut JunkState, s: &mut Settings, cx: &mut UiCx, _edit: &mut Edit) {
    let pal = Palette::of(ui.ctx());
    page(ui, "prefs_junk", 640.0, |ui| {
        F.indented(ui, |ui| {
            ui.checkbox(&mut s.junk.enabled, RichText::new("Enable junk mail filtering").family(crate::ui::fonts::bold()));
            form::hint(ui, "Purpamail learns from the messages you mark as junk or not junk.");
        });
        ui.add_space(6.0);
        ui.add_enabled_ui(s.junk.enabled, |ui| {
            F.row(ui, "When junk mail arrives:", |ui| {
                ui.radio_value(&mut s.junk.action, JunkAction::MarkOnly, "Mark as junk, but leave it in my Inbox");
                ui.radio_value(&mut s.junk.action, JunkAction::MoveToJunk, "Move it to the Junk mailbox");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.junk.action, JunkAction::Custom, "Perform custom actions");
                    if ui.small_button("Rules…").on_hover_text("Junk messages are passed to your rules").clicked() {
                        cx.act(Action::ShowPreferences(Some(PrefsTab::Rules)));
                    }
                });
            });
            form::section(ui);
            F.row(ui, "Exempt from filtering:", |ui| {
                ui.checkbox(&mut s.junk.exempt_sender_in_contacts, "Sender of message is in my Contacts");
                ui.checkbox(&mut s.junk.exempt_previous_recipients, "Sender of message is in my Previous Recipients");
                ui.checkbox(&mut s.junk.exempt_full_name_addressed, "Message is addressed using my full name");
            });
            F.row(ui, "Server filtering:", |ui| {
                ui.checkbox(&mut s.junk.trust_server_headers, "Trust junk mail headers in messages");
                form::hint(ui, "Uses the spam verdict that many servers add to incoming messages.");
            });
            F.row(ui, "Order:", |ui| ui.checkbox(&mut s.junk.filter_before_rules, "Filter junk mail before applying my rules"));
            F.row(ui, "Sensitivity:", |ui| {
                // Higher sensitivity = lower probability threshold.
                let mut sens = 1.0 - s.junk.threshold;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Relaxed").size(11.5).color(pal.text_secondary));
                    if ui.add(egui::Slider::new(&mut sens, 0.01..=0.5).show_value(false)).changed() {
                        s.junk.threshold = ((1.0 - sens) * 100.0).round() / 100.0;
                    }
                    ui.label(RichText::new("Aggressive").size(11.5).color(pal.text_secondary));
                });
                form::hint(ui, &format!("Messages with a junk probability above {:.0}% are treated as junk.", s.junk.threshold * 100.0));
            });
            F.row(ui, "Training:", |ui| {
                if ui.button(format!("{} Reset Junk Filter Training…", ic::ARROW_COUNTER_CLOCKWISE)).clicked() {
                    st.confirm_reset = true;
                }
                match &st.reset_done {
                    Some(m) => form::hint(ui, m),
                    None => form::hint(ui, "Forgets everything the filter has learned."),
                }
            });
        });
        form::section(ui);
        F.row(ui, "Blocked senders:", |ui| {
            let blocked = cx.store.blocked_senders();
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(320.0);
                egui::ScrollArea::vertical().id_salt("blocked").max_height(140.0).min_scrolled_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
                    if blocked.is_empty() {
                        ui.label(RichText::new("No blocked senders").color(pal.text_tertiary));
                    }
                    for b in &blocked {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ic::PROHIBIT).color(pal.text_tertiary));
                            ui.label(b);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if form::icon_button(ui, ic::MINUS, "Unblock", true).clicked() {
                                    let _ = cx.store.set_blocked(b, false);
                                }
                            });
                        });
                    }
                });
            });
            ui.horizontal(|ui| {
                let r = form::text_field(ui, &mut st.new_blocked, 240.0, "name@example.com");
                let valid = crate::core::model::Address::parse_one(&st.new_blocked).is_some_and(|a| a.is_valid());
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(valid, egui::Button::new("Block")).clicked() || (enter && valid))
                    && let Some(a) = crate::core::model::Address::parse_one(&st.new_blocked)
                {
                    let _ = cx.store.set_blocked(&a.email, true);
                    st.new_blocked.clear();
                }
            });
            form::hint(ui, "Messages from blocked senders are moved to the Trash.");
        });
    });

    if st.confirm_reset {
        let mut decision = None;
        egui::Modal::new(egui::Id::new("confirm_junk_reset")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::WARNING_CIRCLE).size(32.0).color(pal.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Reset junk filter training?").family(crate::ui::fonts::bold()).size(14.0));
                    form::hint(ui, "The filter starts learning again from scratch.");
                });
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Reset").color(egui::Color32::WHITE)).fill(pal.danger).min_size(egui::vec2(72.0, 26.0))).clicked() {
                    decision = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        if let Some(d) = decision {
            st.confirm_reset = false;
            if d {
                let r = cx.store.with_conn(|c| c.execute("DELETE FROM junk_tokens", []));
                st.reset_done = Some(match r {
                    Ok(n) => format!("Training reset ({n} learned words removed)."),
                    Err(e) => format!("Could not reset: {e}"),
                });
            }
        }
    }
}
