//! Rules preferences: the ordered list of mail rules and the rule dialog.

use egui::{RichText, Sense, Ui, vec2};
use egui_phosphor::regular as ic;

use super::Edit;
use super::form;
use crate::core::config::{Rule, Settings};
use crate::core::criteria::Criteria;
use crate::core::model::MailboxRole;
use crate::core::store::MessageQuery;
use crate::ui::actions::{Action, UiCx};
use crate::ui::rule_editor::{DialogResult, RuleDialog};
use crate::ui::theme::Palette;

#[derive(Default)]
pub struct RulesState {
    selected: Option<String>,
    dialog: Option<RuleDialog>,
    ask_apply: bool,
    confirm_remove: Option<String>,
}

/// One-line summary of a rule's conditions.
pub fn criteria_summary(c: &Criteria) -> String {
    match c.conditions.len() {
        0 => "No conditions".into(),
        1 => c.conditions[0].describe(),
        n => format!("{} of {n} conditions, e.g. {}", if c.match_all { "All" } else { "Any" }, c.conditions[0].describe()),
    }
}

pub fn show(ui: &mut Ui, st: &mut RulesState, s: &mut Settings, cx: &mut UiCx, _edit: &mut Edit) {
    let pal = Palette::of(ui.ctx());
    super::header(ui, "Rules", "Rules are applied in order to new messages as they arrive. Use the arrow buttons to change their order.");
    let sel_idx = st.selected.as_ref().and_then(|id| s.rules.iter().position(|r| &r.id == id));
    let mut edit_idx: Option<usize> = None;
    let list_h = ui.available_height() - 44.0;
    form::list_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_height(list_h);
        egui::ScrollArea::vertical().id_salt("rules_list").auto_shrink([false, false]).show(ui, |ui| {
            if s.rules.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(ic::LIST_CHECKS).size(40.0).color(pal.text_tertiary));
                    ui.label(RichText::new("No rules").family(crate::ui::fonts::bold()));
                    ui.label(RichText::new("Rules can file, flag, forward or answer messages automatically.").color(pal.text_secondary));
                });
            }
            for (i, r) in s.rules.iter_mut().enumerate() {
                let sel = sel_idx == Some(i);
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
                if sel {
                    ui.painter().rect_filled(rect, 5.0, pal.selection_bg);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 5.0, pal.hover_bg);
                }
                let (fg, fg2) = if sel { (pal.selection_text, pal.selection_text.gamma_multiply(0.85)) } else { (pal.text, pal.text_secondary) };
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                child.checkbox(&mut r.enabled, "").on_hover_text("Active");
                child.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(&r.name).family(crate::ui::fonts::bold()).color(fg));
                    let actions: Vec<&str> = r.actions.iter().map(|a| a.name()).collect();
                    ui.add(egui::Label::new(RichText::new(format!("{}  →  {}", criteria_summary(&r.criteria), actions.join(", "))).size(11.5).color(fg2)).truncate());
                });
                if resp.clicked() {
                    st.selected = Some(r.id.clone());
                }
                if resp.double_clicked() {
                    edit_idx = Some(i);
                }
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(format!("{} Add Rule", ic::PLUS)).clicked() {
            let n = s.rules.len() + 1;
            st.dialog = Some(RuleDialog::new(Rule { name: format!("Rule {n}"), ..Default::default() }, true));
        }
        if ui.add_enabled(sel_idx.is_some(), egui::Button::new("Edit…")).clicked() {
            edit_idx = sel_idx;
        }
        if ui.add_enabled(sel_idx.is_some(), egui::Button::new("Duplicate")).clicked()
            && let Some(i) = sel_idx {
                let mut r = s.rules[i].clone();
                r.id = uuid::Uuid::new_v4().to_string();
                r.name = format!("{} copy", r.name);
                st.selected = Some(r.id.clone());
                s.rules.insert(i + 1, r);
            }
        if ui.add_enabled(sel_idx.is_some(), egui::Button::new("Remove")).clicked() {
            st.confirm_remove = st.selected.clone();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(sel_idx.is_some_and(|i| i + 1 < s.rules.len()), egui::Button::new(ic::ARROW_DOWN)).on_hover_text("Move down").clicked()
                && let Some(i) = sel_idx {
                    s.rules.swap(i, i + 1);
                }
            if ui.add_enabled(sel_idx.is_some_and(|i| i > 0), egui::Button::new(ic::ARROW_UP)).on_hover_text("Move up").clicked()
                && let Some(i) = sel_idx {
                    s.rules.swap(i, i - 1);
                }
        });
    });
    if let Some(i) = edit_idx {
        st.dialog = Some(RuleDialog::new(s.rules[i].clone(), false));
    }

    // ---------------------------------------------------------- dialogs
    if let Some(d) = &mut st.dialog {
        match d.show(ui.ctx(), cx) {
            DialogResult::Open => {}
            DialogResult::Cancel => st.dialog = None,
            DialogResult::Ok => {
                let rule = d.rule.clone();
                upsert_rule(&mut s.rules, rule.clone());
                st.selected = Some(rule.id.clone());
                st.ask_apply = rule.enabled;
                st.dialog = None;
            }
        }
    }
    if st.ask_apply {
        let mut decision = None;
        egui::Modal::new(egui::Id::new("apply_rules_prompt")).show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::LIST_CHECKS).size(32.0).color(pal.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Do you want to apply your rules to messages in selected mailboxes?").family(crate::ui::fonts::bold()).size(14.0));
                    form::hint(ui, "Your rules are applied to the messages in all inboxes.");
                });
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::ui::widgets::primary_button(ui, "Apply").clicked() {
                    decision = Some(true);
                }
                if ui.add(egui::Button::new("Don't Apply").min_size(vec2(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        if let Some(apply) = decision {
            st.ask_apply = false;
            if apply {
                let inboxes: Vec<_> = cx.store.mailboxes(None).into_iter().filter(|m| m.role == MailboxRole::Inbox).map(|m| m.id).collect();
                if !inboxes.is_empty() {
                    let ids: Vec<_> = cx.store.list_messages(&MessageQuery { mailboxes: inboxes, ..Default::default() }).into_iter().map(|m| m.id).collect();
                    if !ids.is_empty() {
                        cx.act(Action::ApplyRules(ids));
                    }
                }
            }
        }
    }
    if let Some(id) = st.confirm_remove.clone() {
        let name = s.rules.iter().find(|r| r.id == id).map(|r| r.name.clone()).unwrap_or_default();
        let mut decision = None;
        egui::Modal::new(egui::Id::new("confirm_remove_rule")).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(format!("Remove the rule “{name}”?")).family(crate::ui::fonts::bold()).size(14.0));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Remove").color(egui::Color32::WHITE)).fill(pal.danger).min_size(vec2(72.0, 26.0))).clicked() {
                    decision = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 26.0))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        if let Some(d) = decision {
            if d {
                s.rules.retain(|r| r.id != id);
                st.selected = None;
            }
            st.confirm_remove = None;
        }
    }
}

/// Insert or replace a rule (by id), trimming its name.
fn upsert_rule(rules: &mut Vec<Rule>, mut rule: Rule) {
    rule.name = rule.name.trim().to_string();
    match rules.iter_mut().find(|r| r.id == rule.id) {
        Some(r) => *r = rule,
        None => rules.push(rule),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::criteria::{Condition, Field, Op};

    #[test]
    fn upsert_rules() {
        let mut rules = vec![Rule { name: "A".into(), ..Default::default() }];
        let mut r = rules[0].clone();
        r.name = "  Renamed ".into();
        upsert_rule(&mut rules, r);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Renamed");
        upsert_rule(&mut rules, Rule::default());
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn summaries() {
        let c = Criteria { match_all: true, conditions: vec![Condition { field: Field::From, op: Op::Contains, value: "bob".into(), header: String::new() }] };
        assert_eq!(criteria_summary(&c), "From contains “bob”");
        let mut c2 = c.clone();
        c2.conditions.push(Condition { field: Field::IsUnread, ..Default::default() });
        assert!(criteria_summary(&c2).starts_with("All of 2 conditions"));
    }
}
