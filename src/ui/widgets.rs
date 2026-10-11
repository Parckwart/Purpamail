//! Small reusable widgets and formatting helpers.

use egui::{Align2, Color32, FontId, Response, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::theme::Palette;

/// Short date as shown in the message list.
pub fn format_list_date(ts: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(d) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    let now = Local::now();
    let days = (now.date_naive() - d.date_naive()).num_days();
    if days == 0 {
        d.format("%H:%M").to_string()
    } else if days == 1 {
        "Yesterday".into()
    } else if (2..7).contains(&days) {
        d.format("%A").to_string()
    } else if d.year() == now.year() {
        d.format("%-d %b").to_string()
    } else {
        d.format("%d.%m.%y").to_string()
    }
}

/// Full date for message headers.
pub fn format_full_date(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(ts, 0).single().map(|d| d.format("%-d %B %Y at %H:%M").to_string()).unwrap_or_default()
}

/// Deterministic pleasant color for a name.
pub fn name_color(s: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in s.to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    const COLORS: [(u8, u8, u8); 10] = [
        (0x7b, 0x4f, 0xd6),
        (0x2f, 0x7c, 0xf6),
        (0x14, 0x9a, 0x8b),
        (0x3c, 0xa5, 0x4a),
        (0xe0, 0x8a, 0x1e),
        (0xd9, 0x48, 0x6b),
        (0x8e, 0x5c, 0x3c),
        (0x55, 0x6b, 0x8d),
        (0xb3, 0x4f, 0xc4),
        (0xd2, 0x5a, 0x32),
    ];
    let (r, g, b) = COLORS[(h % COLORS.len() as u32) as usize];
    Color32::from_rgb(r, g, b)
}

pub fn initials(name: &str, email: &str) -> String {
    let src = if name.trim().is_empty() { email } else { name };
    let words: Vec<&str> = src.split(|c: char| c.is_whitespace() || c == '.' || c == '@' || c == '_').filter(|w| !w.is_empty()).collect();
    let mut s = String::new();
    for w in words.iter().take(2) {
        if let Some(c) = w.chars().find(|c| c.is_alphanumeric()) {
            s.extend(c.to_uppercase());
        }
    }
    if s.is_empty() {
        s.push('?');
    }
    s
}

/// Circular avatar with initials. If `photo_uri` is given (bytes:// uri registered with egui) it is drawn instead.
pub fn avatar(ui: &mut Ui, name: &str, email: &str, size: f32, photo: Option<(&str, &[u8])>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    if let Some((uri, bytes)) = photo {
        let img = egui::Image::from_bytes(uri.to_string(), bytes.to_vec()).corner_radius(size / 2.0).fit_to_exact_size(Vec2::splat(size));
        img.paint_at(ui, rect);
        return resp;
    }
    let c = name_color(if email.is_empty() { name } else { email });
    let p = ui.painter();
    // subtle vertical gradient feel: two circles
    p.circle_filled(rect.center(), size / 2.0, c);
    p.circle_filled(rect.center() - Vec2::new(0.0, size * 0.04), size / 2.0 - size * 0.04, c.gamma_multiply(1.08));
    p.text(rect.center(), Align2::CENTER_CENTER, initials(name, email), FontId::new(size * 0.4, super::fonts::bold()), Color32::WHITE);
    resp
}

pub fn unread_dot(ui: &mut Ui, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.32, color);
}

/// Toolbar button: icon with optional label underneath / tooltip.
pub fn tool_button(ui: &mut Ui, icon: &str, tooltip: &str, enabled: bool) -> Response {
    let pal = Palette::of(ui.ctx());
    let size = Vec2::new(34.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        if enabled && (resp.hovered() || resp.is_pointer_button_down_on()) {
            let fill = if resp.is_pointer_button_down_on() { pal.accent_soft } else { pal.hover_bg };
            ui.painter().rect_filled(rect, 6.0, fill);
        }
        let col = if enabled { pal.text_secondary } else { pal.text_tertiary.gamma_multiply(0.6) };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(18.0), col);
    }
    resp
}

/// Small pill / chip.
pub fn chip(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), fg);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_filled(rect, size.y / 2.0, fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Section header in the sidebar style.
pub fn section_label(ui: &mut Ui, text: &str) {
    let pal = Palette::of(ui.ctx());
    ui.label(RichText::new(text).size(11.5).color(pal.text_tertiary).family(super::fonts::bold()));
}

pub fn flag_glyph(ui: &mut Ui, color: Color32, size: f32) {
    ui.label(RichText::new(ic::FLAG).color(color).size(size));
}

/// A thin horizontal separator line with the palette color.
pub fn hline(ui: &mut Ui) {
    let pal = Palette::of(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, pal.separator));
}

/// Primary (accent colored) button.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let pal = Palette::of(ui.ctx());
    ui.add(egui::Button::new(RichText::new(text).color(pal.accent_text)).fill(pal.accent).min_size(Vec2::new(72.0, 26.0)))
}

/// Open a URL or file with the desktop's default handler.
pub fn open_external(target: &str) {
    let t = target.to_string();
    std::thread::spawn(move || {
        let r = std::process::Command::new("xdg-open").arg(&t).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        if r.is_err() {
            let _ = std::process::Command::new("gio").arg("open").arg(&t).spawn();
        }
    });
}

/// Icon for an attachment by MIME type / filename.
pub fn file_icon(mime: &str, filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if mime.starts_with("image/") {
        ic::FILE_IMAGE
    } else if mime.starts_with("video/") {
        ic::FILE_VIDEO
    } else if mime.starts_with("audio/") {
        ic::FILE_AUDIO
    } else if mime == "application/pdf" || ext == "pdf" {
        ic::FILE_PDF
    } else if ["zip", "gz", "tar", "7z", "rar", "xz", "bz2"].contains(&ext.as_str()) {
        ic::FILE_ZIP
    } else if ["doc", "docx", "odt", "rtf"].contains(&ext.as_str()) {
        ic::FILE_DOC
    } else if ["xls", "xlsx", "ods", "csv"].contains(&ext.as_str()) {
        ic::FILE_XLS
    } else if ["ppt", "pptx", "odp"].contains(&ext.as_str()) {
        ic::FILE_PPT
    } else if ext == "ics" {
        ic::CALENDAR
    } else if ext == "vcf" {
        ic::ADDRESS_BOOK
    } else if mime.starts_with("text/") {
        ic::FILE_TEXT
    } else if mime == "message/rfc822" {
        ic::ENVELOPE
    } else {
        ic::FILE
    }
}
