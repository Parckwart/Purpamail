//! Viewing preferences: layout, theme, message list, headers, conversations,
//! classic layout columns, flag names and toolbar customization.

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::form::{self, Form};
use super::page;
use crate::core::config::{ListLayout, Settings, Theme, ToolbarItem, ViewingPrefs};
use crate::core::model::FlagColor;
use crate::ui::rule_editor::opt_color_picker;
use crate::ui::theme::Palette;

const F: Form = Form::new(210.0);

/// Columns available in the classic layout: (id, title).
pub const CLASSIC_COLUMNS: [(&str, &str); 10] = [
    ("unread", "Unread"),
    ("flag", "Flag"),
    ("attachment", "Attachment"),
    ("from", "From"),
    ("to", "To"),
    ("subject", "Subject"),
    ("date", "Date Received"),
    ("size", "Size"),
    ("mailbox", "Mailbox"),
    ("category", "Category"),
];

#[derive(Default)]
pub struct ViewingState {
    /// Selected row in the current toolbar list.
    toolbar_sel: Option<usize>,
    /// Selected row in the available items list.
    available_sel: Option<ToolbarItem>,
    column_sel: Option<String>,
}

// ------------------------------------------------------------------ list editing helpers

/// Move the element at `idx` by `delta` positions. Returns the new index.
pub fn move_item<T>(list: &mut [T], idx: usize, delta: isize) -> Option<usize> {
    let to = idx as isize + delta;
    if idx >= list.len() || to < 0 || to as usize >= list.len() {
        return None;
    }
    list.swap(idx, to as usize);
    Some(to as usize)
}

/// Spaces may appear any number of times; other items only once.
pub fn toolbar_item_repeatable(item: ToolbarItem) -> bool {
    matches!(item, ToolbarItem::Space | ToolbarItem::FlexibleSpace)
}

/// Items that can still be added to the toolbar.
pub fn available_toolbar_items(current: &[ToolbarItem]) -> Vec<ToolbarItem> {
    ToolbarItem::ALL.iter().copied().filter(|i| toolbar_item_repeatable(*i) || !current.contains(i)).collect()
}

/// Insert an item after `after` (or at the end). Returns the index of the new
/// item, or None when it is already present.
pub fn toolbar_add(list: &mut Vec<ToolbarItem>, item: ToolbarItem, after: Option<usize>) -> Option<usize> {
    if !toolbar_item_repeatable(item) && list.contains(&item) {
        return None;
    }
    let at = after.map(|i| (i + 1).min(list.len())).unwrap_or(list.len());
    list.insert(at, item);
    Some(at)
}

/// Enable or disable a classic layout column. The subject column is always shown.
pub fn set_column(cols: &mut Vec<String>, id: &str, on: bool) {
    let present = cols.iter().any(|c| c == id);
    if on && !present {
        // insert at the position suggested by the canonical order
        let rank = |c: &str| CLASSIC_COLUMNS.iter().position(|(i, _)| *i == c).unwrap_or(usize::MAX);
        let at = cols.iter().position(|c| rank(c) > rank(id)).unwrap_or(cols.len());
        cols.insert(at, id.to_string());
    } else if !on && present && id != "subject" {
        cols.retain(|c| c != id);
    }
}

pub fn toolbar_icon(item: ToolbarItem) -> &'static str {
    match item {
        ToolbarItem::GetMail => ic::ENVELOPE_OPEN,
        ToolbarItem::NewMessage => ic::NOTE_PENCIL,
        ToolbarItem::Archive => ic::ARCHIVE,
        ToolbarItem::Delete => ic::TRASH,
        ToolbarItem::Junk => ic::PROHIBIT,
        ToolbarItem::Reply => ic::ARROW_BEND_UP_LEFT,
        ToolbarItem::ReplyAll => ic::ARROW_BEND_DOUBLE_UP_LEFT,
        ToolbarItem::Forward => ic::ARROW_BEND_UP_RIGHT,
        ToolbarItem::Flag => ic::FLAG,
        ToolbarItem::Mute => ic::BELL_SLASH,
        ToolbarItem::MoveTo => ic::FOLDER_SIMPLE,
        ToolbarItem::Mailboxes => ic::SIDEBAR,
        ToolbarItem::Filter => ic::FUNNEL_SIMPLE,
        ToolbarItem::Print => ic::PRINTER,
        ToolbarItem::Unread => ic::ENVELOPE_SIMPLE,
        ToolbarItem::Search => ic::MAGNIFYING_GLASS,
        ToolbarItem::Space => ic::DOTS_THREE,
        ToolbarItem::FlexibleSpace => ic::ARROWS_OUT_LINE_HORIZONTAL,
        ToolbarItem::Summarize => ic::SPARKLE,
        ToolbarItem::Remind => ic::ALARM,
    }
}

// ------------------------------------------------------------------ UI

pub fn show(ui: &mut Ui, st: &mut ViewingState, s: &mut Settings) {
    let snapshot = s.clone();
    page(ui, "prefs_viewing", 660.0, |ui| {
        let v = &mut s.viewing;
        F.row(ui, "Layout:", |ui| {
            form::segmented(ui, &mut v.layout, &[(ListLayout::Columns, "Columns"), (ListLayout::Classic, "Classic")]);
            form::hint(ui, if v.layout == ListLayout::Columns { "Mailboxes, message list and message side by side." } else { "Message list with columns above the message." });
        });
        F.row(ui, "Appearance:", |ui| {
            form::segmented(ui, &mut v.theme, &[(Theme::System, "System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")]);
        });
        F.row(ui, "Interface size:", |ui| {
            let opts = [(80u32, "80%"), (90, "90%"), (100, "100%"), (110, "110%"), (125, "125%"), (150, "150%")];
            let mut pct = (v.ui_scale * 100.0).round() as u32;
            if !opts.iter().any(|(p, _)| *p == pct) {
                pct = 100;
            }
            if form::combo(ui, "ui_scale", &mut pct, &opts, 100.0) {
                v.ui_scale = pct as f32 / 100.0;
            }
        });
        form::section(ui);
        F.row(ui, "List preview:", |ui| {
            let opts = [(0u8, "None"), (1, "1 line"), (2, "2 lines"), (3, "3 lines"), (4, "4 lines"), (5, "5 lines")];
            form::combo(ui, "preview_lines", &mut v.list_preview_lines, &opts, 100.0);
        });
        F.row(ui, "Message list:", |ui| {
            ui.checkbox(&mut v.show_to_cc_labels, "Show To/Cc labels");
            ui.checkbox(&mut v.show_contact_photos, "Show contact photos");
            ui.checkbox(&mut v.show_message_size, "Show message size");
            ui.checkbox(&mut v.display_unread_bold, "Display unread messages with bold font");
            ui.checkbox(&mut v.show_categories, "Sort messages into categories");
            ui.checkbox(&mut v.show_deleted_messages, "Show deleted messages (IMAP)");
        });
        F.row(ui, "Messages:", |ui| {
            ui.checkbox(&mut v.show_header_detail_all, "Show all headers");
            ui.checkbox(&mut v.load_remote_content, "Load remote content in messages");
            ui.checkbox(&mut v.use_smart_addresses, "Use smart addresses");
            form::hint(ui, "Smart addresses show names only for people in your contacts.");
            ui.checkbox(&mut v.show_summary, "Show message summaries");
        });
        F.row(ui, "Window:", |ui| {
            ui.checkbox(&mut v.show_favorites_bar, "Show favorites bar");
            ui.checkbox(&mut v.show_sidebar, "Show mailbox sidebar");
            ui.checkbox(&mut v.show_tab_bar, "Show tab bar");
        });
        form::section(ui);
        F.row(ui, "Conversations:", |ui| {
            ui.checkbox(&mut v.organize_by_conversation, "Organize by conversation");
            ui.add_enabled_ui(v.organize_by_conversation, |ui| {
                ui.checkbox(&mut v.include_related_messages, "Include related messages");
                ui.checkbox(&mut v.mark_all_read_when_opening_conversation, "Mark all messages as read when opening a conversation");
                ui.checkbox(&mut v.show_most_recent_on_top, "Show most recent message at the top");
            });
        });
        F.row(ui, "Highlight conversations:", |ui| {
            opt_color_picker(ui, "highlight_conv", &mut v.highlight_conversations_color, &snapshot, 160.0);
        });
        form::section(ui);
        form::title(ui, "Classic Layout Columns");
        form::hint(ui, "Columns shown in the message list of the classic layout. Select a column to change its position; the Subject column is always shown.");
        ui.add_space(6.0);
        columns_editor(ui, st, v);
        form::section(ui);
        F.row(ui, "Flag names:", |ui| flags_editor(ui, s));
        form::section(ui);
        form::title(ui, "Toolbar");
        form::hint(ui, "Choose the buttons shown in the main window's toolbar. Double-click an available item to add it.");
        ui.add_space(6.0);
        toolbar_editor(ui, st, &mut s.toolbar);
    });
}

fn columns_editor(ui: &mut Ui, st: &mut ViewingState, v: &mut ViewingPrefs) {
    let pal = Palette::of(ui.ctx());
    ui.horizontal_top(|ui| {
        form::list_frame(ui).show(ui, |ui| {
            ui.set_width(260.0);
            ui.vertical(|ui| {
            // enabled columns in their order, then the disabled ones
            let mut order: Vec<String> = v.classic_columns.clone();
            for (id, _) in CLASSIC_COLUMNS {
                if !order.iter().any(|c| c == id) {
                    order.push(id.to_string());
                }
            }
            for id in order {
                let title = CLASSIC_COLUMNS.iter().find(|(i, _)| *i == id).map(|(_, t)| *t).unwrap_or(id.as_str());
                let mut on = v.classic_columns.contains(&id);
                let sel = st.column_sel.as_deref() == Some(id.as_str());
                let frame = egui::Frame::new().fill(if sel { pal.accent_soft } else { egui::Color32::TRANSPARENT }).corner_radius(4).inner_margin(egui::Margin::symmetric(4, 0));
                let r = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(id != "subject", |ui| {
                                if ui.checkbox(&mut on, "").changed() {
                                    set_column(&mut v.classic_columns, &id, on);
                                }
                            });
                            ui.add(egui::Label::new(title).sense(egui::Sense::click()))
                        })
                        .inner
                    })
                    .inner;
                if r.clicked() {
                    st.column_sel = Some(id.clone());
                }
            }
            });
        });
        ui.add_space(6.0);
        ui.vertical(|ui| {
            let idx = st.column_sel.as_ref().and_then(|c| v.classic_columns.iter().position(|x| x == c));
            if ui.add_enabled(idx.is_some_and(|i| i > 0), egui::Button::new(format!("{} Move Up", ic::ARROW_UP)).min_size(egui::vec2(110.0, 0.0))).clicked() {
                move_item(&mut v.classic_columns, idx.unwrap_or(0), -1);
            }
            if ui.add_enabled(idx.is_some_and(|i| i + 1 < v.classic_columns.len()), egui::Button::new(format!("{} Move Down", ic::ARROW_DOWN)).min_size(egui::vec2(110.0, 0.0))).clicked() {
                move_item(&mut v.classic_columns, idx.unwrap_or(0), 1);
            }
            if ui.add(egui::Button::new("Restore Default").min_size(egui::vec2(110.0, 0.0))).clicked() {
                v.classic_columns = ViewingPrefs::default().classic_columns;
            }
        });
    });
}

fn flags_editor(ui: &mut Ui, s: &mut Settings) {
    let pal = Palette::of(ui.ctx());
    egui::Grid::new("flag_names").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        for c in FlagColor::ALL {
            ui.label(RichText::new(ic::FLAG).size(16.0).color(pal.flag(c)));
            let mut name = s.flag_names.get(c.name()).cloned().unwrap_or_default();
            if form::text_field(ui, &mut name, 180.0, c.name()).changed() {
                if name.trim().is_empty() {
                    s.flag_names.remove(c.name());
                } else {
                    s.flag_names.insert(c.name().to_string(), name);
                }
            }
            ui.end_row();
        }
    });
    form::hint(ui, "Leave a field empty to use the color's name.");
}

fn toolbar_editor(ui: &mut Ui, st: &mut ViewingState, list: &mut Vec<ToolbarItem>) {
    let pal = Palette::of(ui.ctx());
    let row = |ui: &mut Ui, item: ToolbarItem, sel: bool| -> egui::Response { form::list_row(ui, sel, Some((toolbar_icon(item), pal.text_secondary)), item.name(), "") };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Current items").size(11.5).color(pal.text_secondary));
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(230.0);
                egui::ScrollArea::vertical().id_salt("tb_current").max_height(240.0).min_scrolled_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
                    for (i, item) in list.iter().enumerate() {
                        if row(ui, *item, st.toolbar_sel == Some(i)).clicked() {
                            st.toolbar_sel = Some(i);
                        }
                    }
                });
            });
        });
        ui.vertical(|ui| {
            ui.add_space(18.0);
            let sel = st.toolbar_sel.filter(|i| *i < list.len());
            let can_add = st.available_sel.is_some_and(|a| available_toolbar_items(list).contains(&a));
            if ui.add_enabled(can_add, egui::Button::new(format!("{} Add", ic::ARROW_LEFT)).min_size(egui::vec2(96.0, 0.0))).clicked()
                && let Some(a) = st.available_sel {
                    st.toolbar_sel = toolbar_add(list, a, sel);
                }
            if ui.add_enabled(sel.is_some(), egui::Button::new(format!("{} Remove", ic::ARROW_RIGHT)).min_size(egui::vec2(96.0, 0.0))).clicked()
                && let Some(i) = sel {
                    list.remove(i);
                    st.toolbar_sel = if list.is_empty() { None } else { Some(i.min(list.len() - 1)) };
                }
            ui.add_space(6.0);
            if ui.add_enabled(sel.is_some_and(|i| i > 0), egui::Button::new(format!("{} Up", ic::ARROW_UP)).min_size(egui::vec2(96.0, 0.0))).clicked() {
                st.toolbar_sel = sel.and_then(|i| move_item(list, i, -1));
            }
            if ui.add_enabled(sel.is_some_and(|i| i + 1 < list.len()), egui::Button::new(format!("{} Down", ic::ARROW_DOWN)).min_size(egui::vec2(96.0, 0.0))).clicked() {
                st.toolbar_sel = sel.and_then(|i| move_item(list, i, 1));
            }
            ui.add_space(6.0);
            if ui.add(egui::Button::new("Restore Default").min_size(egui::vec2(96.0, 0.0))).clicked() {
                *list = ToolbarItem::default_set();
                st.toolbar_sel = None;
            }
        });
        ui.vertical(|ui| {
            ui.label(RichText::new("Available items").size(11.5).color(pal.text_secondary));
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(230.0);
                egui::ScrollArea::vertical().id_salt("tb_avail").max_height(240.0).min_scrolled_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
                    for item in available_toolbar_items(list) {
                        let r = row(ui, item, st.available_sel == Some(item));
                        if r.clicked() {
                            st.available_sel = Some(item);
                        }
                        if r.double_clicked() {
                            st.toolbar_sel = toolbar_add(list, item, st.toolbar_sel);
                        }
                    }
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_items() {
        let mut v = vec![1, 2, 3];
        assert_eq!(move_item(&mut v, 0, 1), Some(1));
        assert_eq!(v, [2, 1, 3]);
        assert_eq!(move_item(&mut v, 0, -1), None);
        assert_eq!(move_item(&mut v, 2, 1), None);
        assert_eq!(move_item(&mut v, 2, -2), Some(0));
        assert_eq!(v, [3, 1, 2]);
    }

    #[test]
    fn toolbar_editing() {
        let mut list = vec![ToolbarItem::GetMail, ToolbarItem::Delete];
        assert!(!available_toolbar_items(&list).contains(&ToolbarItem::GetMail));
        assert!(available_toolbar_items(&list).contains(&ToolbarItem::Space));
        assert_eq!(toolbar_add(&mut list, ToolbarItem::GetMail, None), None);
        assert_eq!(toolbar_add(&mut list, ToolbarItem::Archive, Some(0)), Some(1));
        assert_eq!(list, [ToolbarItem::GetMail, ToolbarItem::Archive, ToolbarItem::Delete]);
        assert_eq!(toolbar_add(&mut list, ToolbarItem::Space, None), Some(3));
        assert_eq!(toolbar_add(&mut list, ToolbarItem::Space, Some(99)), Some(4));
        assert_eq!(list.iter().filter(|i| **i == ToolbarItem::Space).count(), 2);
        assert_eq!(available_toolbar_items(&ToolbarItem::ALL).len(), 2);
    }

    #[test]
    fn classic_columns() {
        let mut cols: Vec<String> = ["from", "subject", "date"].iter().map(|s| s.to_string()).collect();
        set_column(&mut cols, "flag", true);
        assert_eq!(cols, ["flag", "from", "subject", "date"]);
        set_column(&mut cols, "size", true);
        assert_eq!(cols, ["flag", "from", "subject", "date", "size"]);
        set_column(&mut cols, "subject", false);
        assert!(cols.contains(&"subject".to_string()));
        set_column(&mut cols, "from", false);
        assert_eq!(cols, ["flag", "subject", "date", "size"]);
        set_column(&mut cols, "flag", true);
        assert_eq!(cols.len(), 4);
    }
}
