//! Form layout helpers shared by the preferences window, the account
//! assistant, the rule editors and the contacts window.
//!
//! Forms follow the classic settings-window layout: a right-aligned label
//! column, controls on the right, thin section separators and explanatory
//! secondary text below the controls they describe.


use egui::{Align2, Color32, CornerRadius, FontId, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::ui::theme::Palette;

/// A form with a fixed-width, right-aligned label column.
#[derive(Clone, Copy, Debug)]
pub struct Form {
    pub label_w: f32,
}

impl Form {
    pub const fn new(label_w: f32) -> Self {
        Self { label_w }
    }

    /// One labelled row. The label is vertically aligned with the first line of
    /// the controls; the controls are laid out top-down.
    pub fn row<R>(&self, ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
        ui.horizontal_top(|ui| {
            let h = ui.spacing().interact_size.y;
            let (rect, _) = ui.allocate_exact_size(vec2(self.label_w, h), Sense::hover());
            if !label.is_empty() {
                let pal = Palette::of(ui.ctx());
                let col = if ui.is_enabled() { pal.text } else { pal.text_tertiary };
                let font = egui::TextStyle::Body.resolve(ui.style());
                let galley = ui.painter().layout_no_wrap(label.to_string(), font, col);
                let pos = pos2(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0);
                ui.painter().galley(pos, galley, col);
            }
            ui.vertical(|ui| add(ui)).inner
        })
        .inner
    }

    /// Secondary explanatory text aligned with the controls column.
    pub fn note(&self, ui: &mut Ui, text: &str) {
        ui.horizontal_top(|ui| {
            ui.add_space(self.label_w + ui.spacing().item_spacing.x);
            ui.vertical(|ui| hint(ui, text));
        });
    }

    /// A row that only contains controls (aligned with the controls column).
    pub fn indented<R>(&self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
        self.row(ui, "", add)
    }
}

/// Style tweaks for settings-like windows: text fields, combo boxes and
/// checkboxes get a hairline border so they stand out on the window background.
pub fn window_style(ui: &mut Ui) {
    let pal = Palette::of(ui.ctx());
    let border = if pal.dark { Color32::from_rgb(0x4a, 0x45, 0x55) } else { Color32::from_rgb(0xd4, 0xce, 0xdf) };
    let v = ui.visuals_mut();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.accent.gamma_multiply(0.6));
}

/// Secondary text that wraps within the available width.
pub fn hint(ui: &mut Ui, text: &str) {
    let pal = Palette::of(ui.ctx());
    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(pal.text_secondary)).wrap());
}

/// Error text in the danger color.
pub fn error_text(ui: &mut Ui, text: &str) {
    let pal = Palette::of(ui.ctx());
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(pal.danger)).wrap());
}

/// A thin separator with vertical breathing room.
pub fn section(ui: &mut Ui) {
    ui.add_space(10.0);
    crate::ui::widgets::hline(ui);
    ui.add_space(10.0);
}

/// A bold group title (used inside larger panes such as account details).
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).family(crate::ui::fonts::bold()).size(14.0));
}

/// Lay out a form centered horizontally with at most `max_w` width.
pub fn centered<R>(ui: &mut Ui, max_w: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_width();
    let w = avail.min(max_w);
    let pad = ((avail - w) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(w);
            add(ui)
        })
        .inner
    })
    .inner
}

/// Combo box over a fixed list of `(value, label)` options. Returns true when
/// the selection changed.
pub fn combo<T: PartialEq + Clone>(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut T, options: &[(T, &str)], width: f32) -> bool {
    let selected = options.iter().find(|(v, _)| v == value).map(|(_, n)| *n).unwrap_or("—");
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt).width(width).selected_text(selected).show_ui(ui, |ui| {
        for (v, n) in options {
            if ui.selectable_label(v == value, *n).clicked() && v != value {
                *value = v.clone();
                changed = true;
            }
        }
    });
    changed
}

/// Single-line text field of the given width. Returns the response.
pub fn text_field(ui: &mut Ui, text: &mut String, width: f32, hint_text: &str) -> Response {
    ui.add(egui::TextEdit::singleline(text).desired_width(width).hint_text(RichText::new(hint_text.to_string())))
}

/// Segmented control (a row of joined toggle buttons). Returns true on change.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let pal = Palette::of(ui.ctx());
    let mut changed = false;
    let font = FontId::proportional(13.0);
    let widths: Vec<f32> = options.iter().map(|(_, n)| ui.painter().layout_no_wrap(n.to_string(), font.clone(), pal.text).size().x + 22.0).collect();
    let total: f32 = widths.iter().sum();
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
    let bg = if pal.dark { Color32::from_rgb(0x35, 0x31, 0x3d) } else { Color32::from_rgb(0xee, 0xeb, 0xf3) };
    ui.painter().rect_filled(rect, 7.0, bg);
    let mut x = rect.left();
    for (i, (v, n)) in options.iter().enumerate() {
        let r = egui::Rect::from_min_size(pos2(x, rect.top()), vec2(widths[i], h));
        let resp = ui.interact(r, ui.id().with(("seg", i, n)), Sense::click());
        let sel = v == value;
        if sel {
            let inner = r.shrink(2.0);
            ui.painter().rect(inner, 5.0, pal.window_bg, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r.shrink(2.0), 5.0, pal.hover_bg);
        }
        let col = if sel { pal.text } else { pal.text_secondary };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *n, font.clone(), col);
        if resp.clicked() && !sel {
            *value = v.clone();
            changed = true;
        }
        x += widths[i];
    }
    changed
}

/// A small square icon button (for + / − / reorder controls).
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, enabled: bool) -> Response {
    let pal = Palette::of(ui.ctx());
    let size = Vec2::splat(24.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let resp = if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) };
    if ui.is_rect_visible(rect) {
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, pal.hover_bg);
        }
        let col = if enabled { pal.text_secondary } else { pal.text_tertiary.gamma_multiply(0.6) };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(15.0), col);
    }
    resp
}

/// A bordered list box frame (used for account lists, signature lists, ...).
pub fn list_frame(ui: &Ui) -> egui::Frame {
    let pal = Palette::of(ui.ctx());
    egui::Frame::new().fill(pal.field_bg).stroke(Stroke::new(1.0, pal.separator)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(4))
}

/// A selectable list row with an optional leading icon and secondary line.
pub fn list_row(ui: &mut Ui, selected: bool, icon: Option<(&str, Color32)>, title: &str, subtitle: &str) -> Response {
    let pal = Palette::of(ui.ctx());
    let h = if subtitle.is_empty() { 26.0 } else { 38.0 };
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 5.0, pal.selection_bg);
        } else if resp.hovered() {
            p.rect_filled(rect, 5.0, pal.hover_bg);
        }
        let (fg, fg2) = if selected { (pal.selection_text, pal.selection_text.gamma_multiply(0.8)) } else { (pal.text, pal.text_secondary) };
        let mut x = rect.left() + 8.0;
        if let Some((ic, col)) = icon {
            p.text(pos2(x + 9.0, rect.center().y), Align2::CENTER_CENTER, ic, FontId::proportional(17.0), if selected { fg } else { col });
            x += 26.0;
        }
        let maxw = rect.right() - x - 6.0;
        let title_g = p.layout(title.to_string(), FontId::new(13.0, crate::ui::fonts::bold()), fg, f32::INFINITY);
        let title_g = truncate_galley(p, title_g, title, FontId::new(13.0, crate::ui::fonts::bold()), fg, maxw);
        if subtitle.is_empty() {
            p.galley(pos2(x, rect.center().y - title_g.size().y / 2.0), title_g, fg);
        } else {
            p.galley(pos2(x, rect.top() + 4.0), title_g, fg);
            let sub = p.layout_no_wrap(subtitle.to_string(), FontId::proportional(11.5), fg2);
            let sub = truncate_galley(p, sub, subtitle, FontId::proportional(11.5), fg2, maxw);
            p.galley(pos2(x, rect.top() + 21.0), sub, fg2);
        }
    }
    resp
}

fn truncate_galley(p: &egui::Painter, g: std::sync::Arc<egui::Galley>, text: &str, font: FontId, col: Color32, maxw: f32) -> std::sync::Arc<egui::Galley> {
    if g.size().x <= maxw || maxw <= 10.0 {
        return g;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, col);
    job.wrap = egui::text::TextWrapping::truncate_at_width(maxw);
    p.layout_job(job)
}

/// Color swatch (filled circle) of the given diameter.
pub fn swatch(ui: &mut Ui, color: Color32, d: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::hover());
    ui.painter().circle_filled(rect.center(), d / 2.0, color);
    resp
}
