//! Privacy preferences.

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::form;
use super::page;
use crate::core::config::Settings;
use crate::ui::theme::Palette;

/// One option with an icon, a bold title and an explanation.
fn option(ui: &mut Ui, icon: &str, value: &mut bool, title: &str, text: &str) {
    let pal = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| {
        ui.add_sized([40.0, 26.0], egui::Label::new(RichText::new(icon).size(22.0).color(pal.accent)));
        ui.vertical(|ui| {
            ui.checkbox(value, RichText::new(title).family(crate::ui::fonts::bold()));
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.vertical(|ui| form::hint(ui, text));
            });
        });
    });
    ui.add_space(12.0);
}

pub fn show(ui: &mut Ui, s: &mut Settings) {
    let pal = Palette::of(ui.ctx());
    page(ui, "prefs_privacy", 600.0, |ui| {
        ui.add(egui::Label::new(RichText::new("Purpamail never loads remote images unless you allow them. Tracking pixels are always blocked.").color(pal.text_secondary)).wrap());
        ui.add_space(14.0);
        let p = &mut s.privacy;
        option(
            ui,
            ic::SHIELD_CHECK,
            &mut p.protect_mail_activity,
            "Protect Mail Activity",
            "When you choose to show remote content, it is downloaded in the background without cookies or identifying headers, so senders cannot tell when or whether you opened their message.",
        );
        option(
            ui,
            ic::IMAGE_BROKEN,
            &mut p.block_all_remote_content,
            "Block all remote content",
            "Never load images, fonts or styles from the internet, not even when you ask for them in a message. Some messages may look incomplete.",
        );
        option(
            ui,
            ic::EYE_SLASH,
            &mut p.hide_ip_address,
            "Hide IP address",
            "Remote content is requested without revealing your network address to the sender's servers where possible.",
        );
        option(
            ui,
            ic::LINK_BREAK,
            &mut p.strip_tracking_parameters,
            "Remove tracking from links",
            "Strips known tracking parameters (such as utm_source or fbclid) from links before they are opened.",
        );
        form::section(ui);
        form::hint(ui, "Purpamail keeps your mail only on this computer and on your mail servers. It does not send usage data anywhere.");
    });
}
