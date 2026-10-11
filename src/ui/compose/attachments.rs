//! Attachment tiles shown in the compose window.

use std::path::Path;

use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use crate::core::compose::{OutgoingAttachment, guess_mime};
use crate::core::mime::human_size;
use crate::ui::theme::Palette;
use crate::ui::widgets;

/// Messages above this size may be rejected by some servers.
pub const LARGE_MESSAGE: u64 = 20 * 1024 * 1024;

/// An attachment in the composer.
pub struct ComposeAttachment {
    pub att: OutgoingAttachment,
    /// `bytes://` uri for the thumbnail of images.
    pub uri: String,
}

impl ComposeAttachment {
    pub fn new(att: OutgoingAttachment) -> Self {
        let uri = format!("bytes://compose-att-{}-{}", uuid::Uuid::new_v4().simple(), att.filename);
        ComposeAttachment { att, uri }
    }

    pub fn is_image(&self) -> bool {
        self.att.mime_type.starts_with("image/") && !self.att.mime_type.contains("heic") && !self.att.mime_type.contains("svg")
    }
}

/// Read a file from disk as an attachment.
pub fn read_file(path: &Path) -> std::io::Result<OutgoingAttachment> {
    let data = std::fs::read(path)?;
    let filename = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "attachment".into());
    Ok(OutgoingAttachment { mime_type: guess_mime(&filename).to_string(), filename, data, content_id: None })
}

/// MIME type for image bytes (falls back to the file name).
pub fn image_mime(bytes: &[u8], filename: &str) -> String {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "image/png".into(),
        Ok(image::ImageFormat::Jpeg) => "image/jpeg".into(),
        Ok(image::ImageFormat::Gif) => "image/gif".into(),
        Ok(image::ImageFormat::WebP) => "image/webp".into(),
        Ok(image::ImageFormat::Bmp) => "image/bmp".into(),
        _ => guess_mime(filename).to_string(),
    }
}

/// Total size of all attachments in bytes.
pub fn total_size(atts: &[ComposeAttachment]) -> u64 {
    atts.iter().map(|a| a.att.data.len() as u64).sum()
}

/// Show attachment tiles; returns the index of an attachment to remove.
pub fn tiles(ui: &mut Ui, atts: &[ComposeAttachment]) -> Option<usize> {
    let pal = Palette::of(ui.ctx());
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        for (i, a) in atts.iter().enumerate() {
            let size = Vec2::new(232.0, 52.0);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let hovered = resp.hovered();
            let p = ui.painter();
            p.rect_filled(rect, 8.0, if hovered { pal.hover_bg } else { pal.field_bg });
            p.rect_stroke(rect, 8.0, Stroke::new(1.0, pal.separator), egui::StrokeKind::Inside);
            let thumb = egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::splat(40.0));
            if a.is_image() {
                egui::Image::from_bytes(a.uri.clone(), a.att.data.clone()).corner_radius(5.0).maintain_aspect_ratio(true).fit_to_exact_size(thumb.size()).paint_at(ui, thumb);
            } else {
                ui.painter().rect_filled(thumb, 6.0, pal.accent_soft);
                ui.painter().text(thumb.center(), Align2::CENTER_CENTER, widgets::file_icon(&a.att.mime_type, &a.att.filename), FontId::proportional(22.0), pal.accent);
            }
            let text_x = thumb.right() + 10.0;
            let max_w = rect.right() - text_x - 30.0;
            let name = ui.painter().layout(
                a.att.filename.clone(),
                FontId::proportional(13.0),
                pal.text,
                max_w,
            );
            let name = if name.rows.len() > 1 {
                let mut job = egui::text::LayoutJob::simple_singleline(a.att.filename.clone(), FontId::proportional(13.0), pal.text);
                job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
                ui.painter().layout_job(job)
            } else {
                name
            };
            ui.painter().galley(egui::pos2(text_x, rect.top() + 9.0), name, pal.text);
            ui.painter().text(egui::pos2(text_x, rect.bottom() - 10.0), Align2::LEFT_BOTTOM, human_size(a.att.data.len() as u64), FontId::proportional(11.5), pal.text_secondary);
            // remove button
            let xr = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(20.0));
            let xresp = ui.interact(xr, ui.id().with(("att-remove", i)), Sense::click());
            if hovered || xresp.hovered() {
                ui.painter().circle_filled(xr.center(), 10.0, if xresp.hovered() { pal.danger } else { pal.separator });
                ui.painter().text(xr.center(), Align2::CENTER_CENTER, ic::X, FontId::proportional(11.0), if xresp.hovered() { Color32::WHITE } else { pal.text_secondary });
            }
            if xresp.on_hover_text("Remove Attachment").clicked() {
                remove = Some(i);
            }
            resp.on_hover_text(format!("{} — {}", a.att.filename, human_size(a.att.data.len() as u64))).context_menu(|ui| {
                if ui.button("Remove Attachment").clicked() {
                    remove = Some(i);
                    ui.close();
                }
            });
        }
    });
    remove
}

/// One-line size summary with a warning for very large messages.
pub fn size_summary(ui: &mut Ui, atts: &[ComposeAttachment]) {
    if atts.is_empty() {
        return;
    }
    let pal = Palette::of(ui.ctx());
    let total = total_size(atts);
    let n = atts.len();
    let text = format!("{} {} {}", ic::PAPERCLIP, n, if n == 1 { "attachment" } else { "attachments" });
    // laid out right to left: the summary ends up rightmost
    ui.label(RichText::new(format!("{text}, {}", human_size(total))).size(12.0).color(pal.text_secondary));
    if total > LARGE_MESSAGE {
        ui.add_space(8.0);
        ui.label(RichText::new(format!("{} This message may be too large for some servers", ic::WARNING)).size(12.0).color(Color32::from_rgb(0xe8, 0x59, 0x0c)));
    }
}
