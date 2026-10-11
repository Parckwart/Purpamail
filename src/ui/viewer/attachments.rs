//! Attachments: inline image previews, file tiles and the in-app quick preview.

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::{AttInfo, MessageViewer, Preview};
use crate::core::mime::human_size;
use crate::core::model::*;
use crate::ui::actions::{Action, UiCx};
use crate::ui::theme::Palette;
use crate::ui::widgets;

const TILE: Vec2 = Vec2::new(236.0, 54.0);

/// Is this attachment previewable as text?
fn is_text(a: &AttInfo) -> bool {
    let ext = a.name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    a.mime.starts_with("text/") || a.mime == "message/rfc822" || a.mime == "application/json" || ["txt", "csv", "log", "json", "xml", "md", "ics", "vcf", "eml", "yaml", "yml", "toml", "ini"].contains(&ext.as_str())
}

fn att_menu(ui: &mut Ui, cx: &mut UiCx, previews: &mut Vec<Preview>, message: MessageId, a: &AttInfo, count: usize) {
    if ui.button(format!("{}  Open", ic::ARROW_SQUARE_OUT)).clicked() {
        cx.act(Action::OpenAttachment { message, index: a.index });
        ui.close();
    }
    if ui.button(format!("{}  Quick Look", ic::EYE)).clicked() {
        previews.retain(|p| !(p.message == message && p.att.index == a.index));
        previews.push(Preview { message, att: a.clone(), open: true });
        ui.close();
    }
    ui.separator();
    if ui.button(format!("{}  Save…", ic::DOWNLOAD_SIMPLE)).clicked() {
        cx.act(Action::SaveAttachment { message, index: a.index });
        ui.close();
    }
    if count > 1 && ui.button(format!("{}  Save All…", ic::DOWNLOAD)).clicked() {
        cx.act(Action::SaveAllAttachments(message));
        ui.close();
    }
}

impl MessageViewer {
    pub(super) fn attachments(&mut self, ui: &mut Ui, cx: &mut UiCx, m: &MessageSummary) {
        let pal = Palette::of(ui.ctx());
        let Some(atts) = self.cache.get(&m.id).map(|p| p.attachments.clone()) else { return };
        if atts.is_empty() {
            return;
        }
        let count = atts.len();
        let images: Vec<&AttInfo> = atts.iter().filter(|a| a.is_image()).collect();
        let files: Vec<&AttInfo> = atts.iter().filter(|a| !a.is_image()).collect();

        // Images are shown inline at the end of the message, scaled to fit.
        for a in &images {
            ui.add_space(12.0);
            let dims = a.dims.unwrap_or(Vec2::splat(200.0));
            let avail = ui.available_width();
            let mut size = dims;
            if size.x > avail {
                size = Vec2::new(avail, dims.y * avail / dims.x);
            }
            if size.y > 640.0 {
                size = Vec2::new(size.x * 640.0 / size.y, 640.0);
            }
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            if ui.is_rect_visible(rect) {
                egui::Image::from_bytes(a.uri.clone(), egui::load::Bytes::Shared(a.bytes.clone())).corner_radius(6).paint_at(ui, rect);
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, pal.separator.gamma_multiply(0.6)), egui::StrokeKind::Outside);
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let resp = resp.on_hover_text(format!("{} — {}", a.name, human_size(a.size as u64)));
            if resp.double_clicked() {
                cx.act(Action::OpenAttachment { message: m.id, index: a.index });
            } else if resp.clicked() {
                self.previews.retain(|p| !(p.message == m.id && p.att.index == a.index));
                self.previews.push(Preview { message: m.id, att: (*a).clone(), open: true });
            }
            let previews = &mut self.previews;
            resp.context_menu(|ui| att_menu(ui, cx, previews, m.id, a, count));
            ui.label(RichText::new(format!("{}  {}", ic::IMAGE, a.name)).size(11.5).color(pal.text_tertiary));
        }

        // Attachment area.
        ui.add_space(16.0);
        widgets::hline(ui);
        ui.add_space(8.0);
        let total: usize = atts.iter().map(|a| a.size).sum();
        ui.horizontal(|ui| {
            let label = format!("{}  {} attachment{}, {}", ic::PAPERCLIP, count, if count == 1 { "" } else { "s" }, human_size(total as u64));
            ui.label(RichText::new(label).size(12.5).color(pal.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if count > 1 { "Save All…" } else { "Save…" };
                if ui.add(egui::Button::new(RichText::new(format!("{}  {text}", ic::DOWNLOAD_SIMPLE)).size(12.5).color(pal.accent)).frame(false)).clicked() {
                    if count > 1 {
                        cx.act(Action::SaveAllAttachments(m.id));
                    } else {
                        cx.act(Action::SaveAttachment { message: m.id, index: atts[0].index });
                    }
                }
            });
        });
        if files.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
            for a in files {
                self.tile(ui, cx, m.id, a, count);
            }
        });
    }

    fn tile(&mut self, ui: &mut Ui, cx: &mut UiCx, message: MessageId, a: &AttInfo, count: usize) {
        let pal = Palette::of(ui.ctx());
        let w = TILE.x.min(ui.available_width().max(120.0));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, TILE.y), Sense::click());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let base = if pal.dark { Color32::from_rgb(0x26, 0x23, 0x2e) } else { Color32::from_rgb(0xf8, 0xf6, 0xfb) };
            let fill = if resp.hovered() { pal.hover_bg } else { base };
            p.rect_filled(rect, 10.0, fill);
            p.rect_stroke(rect, 10.0, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
            let icon_rect = Rect::from_min_size(rect.min + Vec2::new(8.0, 8.0), Vec2::splat(38.0));
            p.rect_filled(icon_rect, 8.0, pal.accent_soft);
            p.text(icon_rect.center(), Align2::CENTER_CENTER, widgets::file_icon(&a.mime, &a.name), FontId::proportional(22.0), pal.accent);
            let tx = icon_rect.max.x + 10.0;
            let max_w = (rect.max.x - tx - 8.0).max(20.0);
            let mut job = egui::text::LayoutJob::simple_singleline(a.name.clone(), FontId::new(13.0, crate::ui::fonts::bold()), pal.text);
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
            let g = p.layout_job(job);
            p.galley(egui::pos2(tx, rect.min.y + 9.0), g, pal.text);
            let kind = a.name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).unwrap_or_else(|| a.mime.clone());
            p.text(egui::pos2(tx, rect.min.y + 29.0), Align2::LEFT_TOP, format!("{kind} · {}", human_size(a.size as u64)), FontId::proportional(11.5), pal.text_tertiary);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let resp = resp.on_hover_text(&a.name);
        if resp.clicked() {
            cx.act(Action::OpenAttachment { message, index: a.index });
        }
        let previews = &mut self.previews;
        resp.context_menu(|ui| att_menu(ui, cx, previews, message, a, count));
    }

    /// In-app quick preview windows.
    pub(super) fn previews_ui(&mut self, ctx: &egui::Context, cx: &mut UiCx) {
        let pal = Palette::of(ctx);
        for p in &mut self.previews {
            let a = &p.att;
            let mut open = p.open;
            egui::Window::new(format!("{}  {}", ic::EYE, a.name))
                .id(egui::Id::new(("pm-quick-look", p.message, a.index)))
                .open(&mut open)
                .collapsible(false)
                .pivot(Align2::CENTER_CENTER)
                .default_pos(ctx.content_rect().center())
                .default_size([640.0, 520.0])
                .resizable(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} · {}", a.mime, human_size(a.size as u64))).size(12.0).color(pal.text_secondary));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(format!("{}  Save…", ic::DOWNLOAD_SIMPLE)).clicked() {
                                cx.act(Action::SaveAttachment { message: p.message, index: a.index });
                            }
                            if ui.button(format!("{}  Open", ic::ARROW_SQUARE_OUT)).clicked() {
                                cx.act(Action::OpenAttachment { message: p.message, index: a.index });
                            }
                        });
                    });
                    ui.separator();
                    if a.is_image() {
                        let avail = ui.available_size();
                        let dims = a.dims.unwrap_or(avail);
                        let scale = (avail.x / dims.x).min(avail.y / dims.y).min(1.0);
                        let size = dims * scale;
                        ui.centered_and_justified(|ui| {
                            ui.add(egui::Image::from_bytes(a.uri.clone(), egui::load::Bytes::Shared(a.bytes.clone())).fit_to_exact_size(size).corner_radius(4));
                        });
                    } else if is_text(a) {
                        let limit = a.bytes.len().min(256 * 1024);
                        let mut text = String::from_utf8_lossy(&a.bytes[..limit]).to_string();
                        if limit < a.bytes.len() {
                            text.push_str("\n…");
                        }
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(text).family(crate::ui::fonts::fixed()).size(12.5).color(pal.text)).selectable(true).extend());
                        });
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.label(RichText::new(widgets::file_icon(&a.mime, &a.name)).size(64.0).color(pal.accent));
                            ui.add_space(8.0);
                            ui.label(RichText::new(&a.name).strong());
                            ui.label(RichText::new("No preview available for this kind of file.").color(pal.text_tertiary));
                        });
                    }
                });
            p.open = open;
        }
        self.previews.retain(|p| p.open);
    }
}
