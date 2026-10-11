//! A separate window showing the raw RFC 5322 source of a message.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{FontId, RichText};
use egui_phosphor::regular as ic;

use crate::core::model::MessageId;
use crate::ui::actions::{Action, UiCx};
use crate::ui::theme::Palette;

/// Longest source shown (larger messages are truncated in the window).
const MAX_SHOWN: usize = 2 * 1024 * 1024;

/// Prepared source text for one message.
#[derive(Clone)]
struct RawSource {
    text: Arc<String>,
    truncated: bool,
    size: usize,
    dark: bool,
    job: Arc<LayoutJob>,
}

/// Syntax-highlighted job: header names accented, header values and body plain.
fn highlight(text: &str, pal: &Palette) -> LayoutJob {
    let mono = FontId::new(12.5, crate::ui::fonts::fixed());
    let name_fmt = TextFormat::simple(mono.clone(), pal.accent);
    let value_fmt = TextFormat::simple(mono.clone(), pal.text);
    let cont_fmt = TextFormat::simple(mono.clone(), pal.text_secondary);
    let body_fmt = TextFormat::simple(mono, pal.text);
    let mut job = LayoutJob::default();
    let mut in_headers = true;
    for line in text.split_inclusive('\n') {
        if in_headers {
            let bare = line.trim_end_matches(['\r', '\n']);
            if bare.is_empty() {
                in_headers = false;
                job.append(line, 0.0, body_fmt.clone());
                continue;
            }
            if line.starts_with([' ', '\t']) {
                job.append(line, 0.0, cont_fmt.clone());
            } else if let Some(i) = line.find(':') {
                job.append(&line[..=i], 0.0, name_fmt.clone());
                job.append(&line[i + 1..], 0.0, value_fmt.clone());
            } else {
                job.append(line, 0.0, value_fmt.clone());
            }
        } else {
            job.append(line, 0.0, body_fmt.clone());
        }
    }
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Window showing the raw RFC 5322 source of a message. Returns false when closed.
pub fn raw_source_window(ctx: &egui::Context, cx: &mut UiCx, id: MessageId) -> bool {
    let pal = Palette::of(ctx);
    let key = egui::Id::new(("pm-raw-source", id));
    let cached: Option<RawSource> = ctx.data(|d| d.get_temp(key));
    let src = match cached.filter(|c| c.dark == pal.dark) {
        Some(c) => c,
        None => {
            let Some(raw) = cx.store.raw(id) else {
                cx.act(Action::Toast("The source of this message is not available yet.".into()));
                return false;
            };
            let size = raw.len();
            let truncated = size > MAX_SHOWN;
            let text = String::from_utf8_lossy(&raw[..size.min(MAX_SHOWN)]).replace("\r\n", "\n");
            let job = Arc::new(highlight(&text, &pal));
            let s = RawSource { text: Arc::new(text), truncated, size, dark: pal.dark, job };
            ctx.data_mut(|d| d.insert_temp(key, s.clone()));
            s
        }
    };
    let subject = cx.store.message(id).map(|m| m.subject).unwrap_or_default();
    let title = if subject.trim().is_empty() { "Raw Source".to_string() } else { format!("Raw Source — {subject}") };
    let mut open = true;
    let vid = egui::ViewportId::from_hash_of(("raw-source", id));
    ctx.show_viewport_immediate(vid, egui::ViewportBuilder::default().with_title(title).with_inner_size([860.0, 720.0]), |ui, _class| {
        let pal = Palette::of(ui.ctx());
        egui::Panel::top("raw_toolbar").frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(10, 6))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Copy", ic::COPY)).clicked() {
                    ui.ctx().copy_text(src.text.to_string());
                    cx.act(Action::Toast("Message source copied".into()));
                }
                if ui.button(format!("{}  Save…", ic::FLOPPY_DISK)).clicked() {
                    cx.act(Action::SaveMessageAs(id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let size = crate::core::mime::human_size(src.size as u64);
                    let note = if src.truncated { format!("{size} (showing the first {})", crate::core::mime::human_size(MAX_SHOWN as u64)) } else { size };
                    ui.label(RichText::new(note).size(12.0).color(pal.text_secondary));
                });
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.viewer_bg).inner_margin(12)).show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Label::new((*src.job).clone()).selectable(true).extend());
            });
        });
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
    });
    if !open {
        ctx.data_mut(|d| d.remove::<RawSource>(key));
    }
    open
}
