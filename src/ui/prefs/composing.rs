//! Composing preferences: format, spelling, addressing, responding,
//! attachments and the default font.

use egui::Ui;

use super::form::{self, Form};
use super::fonts_tab::FontsState;
use super::page;
use crate::core::config::{MessageFormat, Settings, SpellCheck};

const F: Form = Form::new(220.0);

pub fn show(ui: &mut Ui, fonts: &mut FontsState, s: &mut Settings) {
    let families: Option<Vec<String>> = fonts.families(ui).map(|f| f.to_vec());
    let fam = families.as_deref();
    let accounts: Vec<(String, String)> = s.accounts.iter().map(|a| (a.id.clone(), format!("{} <{}>", a.display_name(), a.primary_email()))).collect();
    page(ui, "prefs_composing", 640.0, |ui| {
        let c = &mut s.composing;
        F.row(ui, "Message format:", |ui| {
            form::segmented(ui, &mut c.format, &[(MessageFormat::RichText, "Rich Text"), (MessageFormat::PlainText, "Plain Text")]);
        });
        F.row(ui, "Check spelling:", |ui| {
            let opts = [(SpellCheck::AsITyped, "As I type"), (SpellCheck::BeforeSending, "Before sending"), (SpellCheck::Never, "Never")];
            form::combo(ui, "spell", &mut c.spell_check, &opts, 160.0);
        });
        F.row(ui, "Default font:", |ui| {
            ui.horizontal(|ui| {
                super::fonts_tab::family_picker(ui, "compose_font", &mut c.default_font, fam);
                super::fonts_tab::size_picker(ui, "compose_size", &mut c.default_font_size);
            });
        });
        form::section(ui);
        F.row(ui, "Addressing:", |ui| {
            ui.horizontal(|ui| {
                ui.label("Automatically");
                ui.checkbox(&mut c.auto_cc_myself, "Cc");
                ui.checkbox(&mut c.auto_bcc_myself, "Bcc");
                ui.label("myself");
            });
            ui.checkbox(&mut c.show_addresses_not_names, "Show email addresses instead of names");
            ui.horizontal(|ui| {
                ui.checkbox(&mut c.mark_addresses_enabled, "Mark addresses not ending with");
                ui.add_enabled(c.mark_addresses_enabled, egui::TextEdit::singleline(&mut c.mark_addresses_not_ending_with).desired_width(150.0).hint_text("example.com"));
            });
            form::hint(ui, "Highlights recipients outside your organization. Separate several domains with commas.");
        });
        F.row(ui, "Send new messages from:", |ui| {
            let mut opts: Vec<(String, &str)> = vec![(String::new(), "Automatically")];
            opts.extend(accounts.iter().map(|(id, n)| (id.clone(), n.as_str())));
            form::combo(ui, "send_from", &mut c.send_new_from, &opts, 300.0);
            form::hint(ui, "“Automatically” picks the account of the selected mailbox.");
        });
        form::section(ui);
        F.row(ui, "When responding:", |ui| {
            ui.checkbox(&mut c.reply_same_format, "Use the same message format as the original message");
            ui.checkbox(&mut c.quote_original, "Quote the text of the original message");
            ui.add_enabled_ui(c.quote_original, |ui| {
                ui.checkbox(&mut c.increase_quote_level, "Increase quote level");
            });
        });
        F.row(ui, "When quoting text:", |ui| {
            ui.radio_value(&mut c.include_selected_text_only, false, "Include all of the original message text");
            ui.radio_value(&mut c.include_selected_text_only, true, "Include selected text, if any; otherwise include all text");
        });
        form::section(ui);
        F.row(ui, "Attachments:", |ui| {
            ui.checkbox(&mut c.include_attachments_in_reply, "Include original attachments in replies");
            ui.checkbox(&mut c.insert_attachments_at_end, "Insert attachments at the end of the message");
            ui.checkbox(&mut c.send_windows_friendly_attachments, "Send attachments compatible with all mail apps");
        });
        F.row(ui, "Suggestions:", |ui| {
            ui.checkbox(&mut c.follow_up_suggestions, "Suggest following up on messages without a reply");
        });
    });
}
