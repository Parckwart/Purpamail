//! Fonts & Colors preferences with a live preview.

use std::thread::JoinHandle;

use egui::{Color32, FontId, RichText, Stroke, Ui, vec2};

use super::form::{self, Form};
use super::page;
use crate::core::config::Settings;
use crate::ui::fonts;
use crate::ui::theme::{self, Palette};

const F: Form = Form::new(190.0);
const SIZES: [f32; 14] = [9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 32.0];

#[derive(Default)]
pub struct FontsState {
    families: Option<Vec<String>>,
    job: Option<JoinHandle<Vec<String>>>,
}

impl FontsState {
    /// Installed font families (loaded once in the background).
    pub fn families(&mut self, ui: &Ui) -> Option<&[String]> {
        if self.families.is_none() && self.job.is_none() {
            self.job = Some(std::thread::spawn(fonts::system_families));
        }
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.families = self.job.take().and_then(|j| j.join().ok());
        }
        if self.job.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.families.as_deref()
    }
}

/// Searchable font family combo box ("" = default font). Returns true on change.
pub fn family_picker(ui: &mut Ui, id_salt: &str, value: &mut String, families: Option<&[String]>) -> bool {
    let pal = Palette::of(ui.ctx());
    let search_id = egui::Id::new(("font_search", id_salt));
    let mut search_text: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
    let search = &mut search_text;
    let mut changed = false;
    let shown = if value.is_empty() { "Default".to_string() } else { value.clone() };
    let inner = egui::ComboBox::from_id_salt(id_salt).width(220.0).truncate().selected_text(shown).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show_ui(ui, |ui| {
        ui.set_min_width(240.0);
        let r = ui.add(egui::TextEdit::singleline(search).hint_text(format!("{} Search fonts", egui_phosphor::regular::MAGNIFYING_GLASS)).desired_width(f32::INFINITY));
        if !r.has_focus() && search.is_empty() {
            r.request_focus();
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            let q = search.to_lowercase();
            if q.is_empty() && ui.selectable_label(value.is_empty(), "Default").clicked() {
                value.clear();
                changed = true;
                ui.close();
            }
            match families {
                None => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(RichText::new("Loading fonts…").color(pal.text_secondary));
                    });
                }
                Some(list) => {
                    let mut any = false;
                    for f in list.iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)) {
                        any = true;
                        if ui.selectable_label(value == f, f).clicked() {
                            *value = f.clone();
                            changed = true;
                            ui.close();
                        }
                    }
                    if !any {
                        ui.label(RichText::new("No matching fonts").color(pal.text_tertiary));
                    }
                }
            }
        });
    });
    if inner.inner.is_none() {
        search.clear();
    }
    ui.data_mut(|d| d.insert_temp(search_id, search_text));
    changed
}

/// Font size combo box. Returns true on change.
pub fn size_picker(ui: &mut Ui, id_salt: &str, size: &mut f32) -> bool {
    let mut opts: Vec<f32> = SIZES.to_vec();
    if !opts.iter().any(|s| (s - *size).abs() < 0.01) {
        opts.push(*size);
        opts.sort_by(|a, b| a.total_cmp(b));
    }
    let labels: Vec<String> = opts.iter().map(|s| format!("{s:.0} pt")).collect();
    let pairs: Vec<(u32, &str)> = opts.iter().zip(labels.iter()).map(|(s, l)| ((s * 10.0).round() as u32, l.as_str())).collect();
    let mut v = (*size * 10.0).round() as u32;
    if form::combo(ui, id_salt, &mut v, &pairs, 70.0) {
        *size = v as f32 / 10.0;
        true
    } else {
        false
    }
}

fn color_button(ui: &mut Ui, hex: &mut String) -> bool {
    let mut c = theme::hex(hex).unwrap_or(Color32::GRAY);
    if ui.color_edit_button_srgba(&mut c).changed() {
        *hex = theme::to_hex(c);
        true
    } else {
        false
    }
}

pub fn show(ui: &mut Ui, st: &mut FontsState, s: &mut Settings) {
    let families: Option<Vec<String>> = st.families(ui).map(|f| f.to_vec());
    let fam = families.as_deref();
    page(ui, "prefs_fonts", 640.0, |ui| {
        F.row(ui, "Message list font:", |ui| {
            ui.horizontal(|ui| {
                family_picker(ui, "list_font", &mut s.fonts.list_font, fam);
                size_picker(ui, "list_size", &mut s.fonts.list_font_size);
            });
        });
        F.row(ui, "Message font:", |ui| {
            ui.horizontal(|ui| {
                family_picker(ui, "msg_font", &mut s.fonts.message_font, fam);
                size_picker(ui, "msg_size", &mut s.fonts.message_font_size);
            });
        });
        F.row(ui, "Fixed-width font:", |ui| {
            ui.horizontal(|ui| {
                family_picker(ui, "fixed_font", &mut s.fonts.fixed_font, fam);
                size_picker(ui, "fixed_size", &mut s.fonts.fixed_font_size);
            });
            ui.checkbox(&mut s.fonts.use_fixed_for_plain_text, "Use fixed-width font for plain text messages");
        });
        form::section(ui);
        F.row(ui, "Quoted text:", |ui| {
            ui.checkbox(&mut s.fonts.color_quoted_text, "Color quoted text");
            ui.add_enabled_ui(s.fonts.color_quoted_text, |ui| {
                ui.horizontal(|ui| {
                    for (i, c) in s.fonts.quote_colors.iter_mut().enumerate() {
                        color_button(ui, c);
                        ui.label(format!("Level {}", i + 1));
                        ui.add_space(6.0);
                    }
                });
            });
            if ui.small_button("Restore Default Colors").clicked() {
                s.fonts.quote_colors = crate::core::config::FontsColorsPrefs::default().quote_colors;
            }
        });
        form::section(ui);
        F.row(ui, "Preview:", |ui| preview(ui, s));
        F.note(ui, "Font changes apply to the whole app a moment after you make them.");
    });
}

fn preview(ui: &mut Ui, s: &Settings) {
    let pal = Palette::of(ui.ctx());
    form::list_frame(ui).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width().min(380.0));
        // list row
        let ls = s.fonts.list_font_size;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Alex Morgan").font(FontId::new(ls, fonts::list_bold())));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new("09:41").font(FontId::new(ls - 1.0, fonts::list())).color(pal.text_secondary)));
        });
        ui.label(RichText::new("Weekend hiking plans").font(FontId::new(ls, fonts::list())));
        ui.label(RichText::new("Are we still on for Saturday? The forecast looks great…").font(FontId::new(ls - 0.5, fonts::list())).color(pal.text_secondary));
        ui.add_space(6.0);
        crate::ui::widgets::hline(ui);
        ui.add_space(6.0);
        // message body
        let ms = s.fonts.message_font_size;
        let body_font = if s.fonts.use_fixed_for_plain_text { FontId::new(s.fonts.fixed_font_size, fonts::fixed()) } else { FontId::new(ms, fonts::message()) };
        ui.label(RichText::new("Sounds good! I'll bring the map.").font(body_font.clone()));
        for (level, text) in [(1usize, "Are we still on for Saturday?"), (2, "Let's meet at the trailhead at eight."), (3, "Who's coming along?")] {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                for l in 0..level {
                    let col = if s.fonts.color_quoted_text { theme::hex(&s.fonts.quote_colors[l.min(2)]).unwrap_or(pal.quote_bar) } else { pal.separator };
                    let (r, _) = ui.allocate_exact_size(vec2(3.0, ms + 4.0), egui::Sense::hover());
                    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(2.5, col));
                    ui.add_space(4.0);
                }
                let col = if s.fonts.color_quoted_text { theme::hex(&s.fonts.quote_colors[(level - 1).min(2)]).unwrap_or(pal.text) } else { pal.text_secondary };
                ui.label(RichText::new(text).font(FontId::new(ms, fonts::message())).color(col));
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("let total = items.iter().sum::<u32>();").font(FontId::new(s.fonts.fixed_font_size, fonts::fixed())));
    });
}
