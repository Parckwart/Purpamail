//! Signatures preferences: accounts | signatures | editor with live preview.
//!
//! Signatures are written in Purpamail's lightweight markup (`**bold**`,
//! `*italic*`, `[text](url)`, `- item`). The plain text editor can later be
//! replaced by the rich text editor (`ui::richtext::RichTextEditor`).

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::form;
use crate::core::config::{Settings, Signature};
use crate::ui::theme::Palette;

#[derive(Default)]
pub struct SignaturesState {
    /// Selected account filter (None = "All Signatures").
    account: Option<String>,
    selected: Option<String>,
    /// Rich text editor for the selected signature: (signature id, editor, last seen version).
    editor: Option<(String, crate::ui::richtext::RichTextEditor, u64)>,
}

pub fn show(ui: &mut Ui, st: &mut SignaturesState, s: &mut Settings) {
    let pal = Palette::of(ui.ctx());
    if st.account.as_ref().is_some_and(|a| s.account(a).is_none()) {
        st.account = None;
    }
    let visible: Vec<usize> = s
        .signatures
        .iter()
        .enumerate()
        .filter(|(_, g)| match &st.account {
            None => true,
            Some(a) => g.accounts.is_empty() || g.accounts.contains(a),
        })
        .map(|(i, _)| i)
        .collect();
    if st.selected.as_ref().is_none_or(|id| !visible.iter().any(|i| &s.signatures[*i].id == id)) {
        st.selected = visible.first().map(|i| s.signatures[*i].id.clone());
    }
    let h = ui.available_height();
    ui.horizontal_top(|ui| {
        // ------------------------------------------------ accounts column
        ui.vertical(|ui| {
            ui.set_width(170.0);
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(h - 76.0);
                egui::ScrollArea::vertical().id_salt("sig_accounts").auto_shrink([false, false]).show(ui, |ui| {
                    let all_count = s.signatures.len();
                    if form::list_row(ui, st.account.is_none(), Some((ic::SIGNATURE, pal.accent)), "All Signatures", &count_label(all_count)).clicked() {
                        st.account = None;
                    }
                    for a in &s.accounts {
                        let n = s.signatures.iter().filter(|g| g.accounts.is_empty() || g.accounts.contains(&a.id)).count();
                        if form::list_row(ui, st.account.as_deref() == Some(a.id.as_str()), Some((ic::AT, pal.text_secondary)), a.display_name(), &count_label(n)).clicked() {
                            st.account = Some(a.id.clone());
                        }
                    }
                });
            });
            // default signature for the selected account
            if let Some(acct_id) = st.account.clone() {
                ui.label(RichText::new("Choose signature:").size(11.5).color(pal.text_secondary));
                let names: Vec<(Option<String>, String)> = std::iter::once((None, "None".to_string())).chain(visible.iter().map(|i| (Some(s.signatures[*i].id.clone()), s.signatures[*i].name.clone()))).collect();
                if let Some(acct) = s.account_mut(&acct_id) {
                    let refs: Vec<(Option<String>, &str)> = names.iter().map(|(i, n)| (i.clone(), n.as_str())).collect();
                    form::combo(ui, "sig_default", &mut acct.signature, &refs, 158.0);
                }
            }
        });
        // ------------------------------------------------ signatures column
        ui.vertical(|ui| {
            ui.set_width(180.0);
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(h - 40.0);
                egui::ScrollArea::vertical().id_salt("sig_list").auto_shrink([false, false]).show(ui, |ui| {
                    if visible.is_empty() {
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("No signatures").color(pal.text_tertiary)));
                    }
                    for i in &visible {
                        let g = &s.signatures[*i];
                        let preview = g.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").replace(['*', '[', ']'], "");
                        if form::list_row(ui, st.selected.as_deref() == Some(g.id.as_str()), None, &g.name, &preview).clicked() {
                            st.selected = Some(g.id.clone());
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if form::icon_button(ui, ic::PLUS, "New signature", true).clicked() {
                    let n = s.signatures.len() + 1;
                    let mut g = Signature { name: format!("Signature #{n}"), ..Default::default() };
                    let me = s.accounts.first().map(|a| (a.full_name.clone(), a.primary_email().to_string())).unwrap_or_default();
                    g.body = if me.0.is_empty() { String::new() } else { format!("**{}**\n{}", me.0, me.1) };
                    if let Some(a) = &st.account {
                        g.accounts = vec![a.clone()];
                    }
                    st.selected = Some(g.id.clone());
                    s.signatures.push(g);
                }
                let sel_idx = st.selected.as_ref().and_then(|id| s.signatures.iter().position(|g| &g.id == id));
                if form::icon_button(ui, ic::MINUS, "Delete signature", sel_idx.is_some()).clicked()
                    && let Some(i) = sel_idx {
                        let removed = s.signatures.remove(i);
                        for a in &mut s.accounts {
                            if a.signature.as_deref() == Some(removed.id.as_str()) {
                                a.signature = None;
                            }
                        }
                        st.selected = None;
                    }
                if form::icon_button(ui, ic::COPY, "Duplicate signature", sel_idx.is_some()).clicked()
                    && let Some(i) = sel_idx {
                        let mut g = s.signatures[i].clone();
                        g.id = uuid::Uuid::new_v4().to_string();
                        g.name = format!("{} copy", g.name);
                        st.selected = Some(g.id.clone());
                        s.signatures.insert(i + 1, g);
                    }
                ui.add_space(6.0);
                let pos = sel_idx;
                if form::icon_button(ui, ic::ARROW_UP, "Move up", pos.is_some_and(|i| i > 0)).clicked()
                    && let Some(i) = pos {
                        s.signatures.swap(i, i - 1);
                    }
                if form::icon_button(ui, ic::ARROW_DOWN, "Move down", pos.is_some_and(|i| i + 1 < s.signatures.len())).clicked()
                    && let Some(i) = pos {
                        s.signatures.swap(i, i + 1);
                    }
            });
        });
        // ------------------------------------------------ editor column
        ui.vertical(|ui| {
            let Some(idx) = st.selected.as_ref().and_then(|id| s.signatures.iter().position(|g| &g.id == id)) else {
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(ic::SIGNATURE).size(40.0).color(pal.text_tertiary));
                    ui.label(RichText::new("Click + to create a signature.").color(pal.text_secondary));
                });
                return;
            };
            let accounts: Vec<(String, String)> = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
            let g = &mut s.signatures[idx];
            egui::ScrollArea::vertical().id_salt("sig_editor").auto_shrink([false, false]).show(ui, |ui| {
                ui.label(RichText::new("Name").size(11.5).color(pal.text_secondary));
                form::text_field(ui, &mut g.name, ui.available_width(), "Signature name");
                ui.add_space(6.0);
                ui.label(RichText::new("Signature").size(11.5).color(pal.text_secondary));
                if st.editor.as_ref().map(|(id, _, _)| id != &g.id).unwrap_or(true) {
                    let ed = crate::ui::richtext::RichTextEditor::new(&g.body);
                    let v = ed.version();
                    st.editor = Some((g.id.clone(), ed, v));
                }
                if let Some((_, ed, seen)) = &mut st.editor {
                    ed.ui_with_toolbar(ui);
                    if ed.version() != *seen {
                        *seen = ed.version();
                        g.body = ed.to_markup();
                    }
                }
                ui.add_space(6.0);
                ui.checkbox(&mut g.use_default_font, "Always match my default message font");
                ui.add_space(6.0);
                ui.label(RichText::new("Offer for").size(11.5).color(pal.text_secondary));
                let mut all = g.accounts.is_empty();
                if ui.checkbox(&mut all, "All accounts").changed() {
                    g.accounts = if all { vec![] } else { accounts.first().map(|a| vec![a.0.clone()]).unwrap_or_default() };
                }
                if !all {
                    for (id, name) in &accounts {
                        let mut on = g.accounts.contains(id);
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                g.accounts.push(id.clone());
                            } else {
                                g.accounts.retain(|a| a != id);
                            }
                        }
                    }
                }
            });
        });
    });
}

fn count_label(n: usize) -> String {
    match n {
        0 => "No signatures".into(),
        1 => "1 signature".into(),
        n => format!("{n} signatures"),
    }
}
