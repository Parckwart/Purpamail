//! The "Send Later…" date and time picker.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use egui::{Align2, FontId, RichText, Sense, Ui, Vec2};
use egui_phosphor::regular as ic;

use crate::ui::theme::Palette;

/// Today at `hour:00` local time, if still in the future.
pub fn tonight_at(hour: u32) -> Option<i64> {
    let now = Local::now();
    let t = Local.with_ymd_and_hms(now.year(), now.month(), now.day(), hour, 0, 0).single()?;
    (t > now).then(|| t.timestamp())
}

/// Tomorrow at `hour:00` local time.
pub fn tomorrow_at(hour: u32) -> Option<i64> {
    let d = Local::now().date_naive() + Duration::days(1);
    Local.with_ymd_and_hms(d.year(), d.month(), d.day(), hour, 0, 0).single().map(|t| t.timestamp())
}

/// Date + time picker state.
pub struct SendLaterPicker {
    pub date: NaiveDate,
    pub hour: u32,
    pub minute: u32,
    date_text: String,
    view: NaiveDate,
}

impl Default for SendLaterPicker {
    fn default() -> Self {
        let next = Local::now() + Duration::hours(1);
        let date = next.date_naive();
        SendLaterPicker { date, hour: next.hour(), minute: 0, date_text: date.format("%Y-%m-%d").to_string(), view: date.with_day(1).unwrap_or(date) }
    }
}

impl SendLaterPicker {
    /// The chosen moment as a Unix timestamp.
    pub fn timestamp(&self) -> Option<i64> {
        Local.with_ymd_and_hms(self.date.year(), self.date.month(), self.date.day(), self.hour, self.minute, 0).earliest().map(|t| t.timestamp())
    }

    fn set_date(&mut self, d: NaiveDate) {
        self.date = d;
        self.date_text = d.format("%Y-%m-%d").to_string();
        self.view = d.with_day(1).unwrap_or(d);
    }

    /// Draw the picker. Returns `Some(true)` to schedule, `Some(false)` to cancel.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<bool> {
        let pal = Palette::of(ui.ctx());
        let mut result = None;
        ui.set_width(330.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic::CLOCK_COUNTDOWN).size(26.0).color(pal.accent));
            ui.vertical(|ui| {
                ui.label(RichText::new("Send Later").size(15.0).family(crate::ui::fonts::bold()));
                ui.label(RichText::new("Choose when this message should be sent.").size(12.0).color(pal.text_secondary));
            });
        });
        ui.add_space(10.0);
        // month header
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(ic::CARET_LEFT).frame(false)).clicked() {
                self.view = shift_month(self.view, -1);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(ic::CARET_RIGHT).frame(false)).clicked() {
                    self.view = shift_month(self.view, 1);
                }
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(self.view.format("%B %Y").to_string()).family(crate::ui::fonts::bold()));
                });
            });
        });
        // calendar grid
        let today = Local::now().date_naive();
        let cell = Vec2::new(44.0, 28.0);
        let (grid_rect, _) = ui.allocate_exact_size(Vec2::new(cell.x * 7.0, cell.y * 7.0), Sense::hover());
        for (i, wd) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
            let c = grid_rect.min + Vec2::new(cell.x * (i as f32 + 0.5), cell.y * 0.5);
            ui.painter().text(c, Align2::CENTER_CENTER, *wd, FontId::proportional(11.0), pal.text_tertiary);
        }
        let first = self.view;
        let offset = first.weekday().num_days_from_monday() as i64;
        for k in 0..42i64 {
            let d = first - Duration::days(offset) + Duration::days(k);
            let row = k / 7 + 1;
            let col = k % 7;
            let r = egui::Rect::from_min_size(grid_rect.min + Vec2::new(cell.x * col as f32, cell.y * row as f32), cell).shrink2(Vec2::new(5.0, 1.0));
            let in_month = d.month() == first.month();
            let past = d < today;
            let id = ui.id().with(("cal", d));
            let resp = ui.interact(r, id, if past { Sense::hover() } else { Sense::click() });
            let selected = d == self.date;
            if selected {
                ui.painter().rect_filled(r, 13.0, pal.accent);
            } else if resp.hovered() && !past {
                ui.painter().rect_filled(r, 13.0, pal.hover_bg);
            }
            let col = if selected {
                pal.accent_text
            } else if past || !in_month {
                pal.text_tertiary.gamma_multiply(if past { 0.6 } else { 1.0 })
            } else if d == today {
                pal.accent
            } else {
                pal.text
            };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, d.day().to_string(), FontId::proportional(13.0), col);
            if resp.clicked() {
                self.set_date(d);
            }
        }
        ui.add_space(8.0);
        egui::Grid::new("send-later-fields").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Date:").color(pal.text_secondary));
            let r = ui.add(egui::TextEdit::singleline(&mut self.date_text).desired_width(110.0).hint_text("YYYY-MM-DD"));
            if r.changed() {
                if let Ok(d) = NaiveDate::parse_from_str(self.date_text.trim(), "%Y-%m-%d") {
                    self.date = d;
                    self.view = d.with_day(1).unwrap_or(d);
                }
            }
            ui.end_row();
            ui.label(RichText::new("Time:").color(pal.text_secondary));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add(egui::DragValue::new(&mut self.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)).speed(0.1));
                ui.label(":");
                ui.add(egui::DragValue::new(&mut self.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)).speed(0.2));
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        let ts = self.timestamp();
        let valid = ts.is_some_and(|t| t > Local::now().timestamp());
        let summary = match ts {
            Some(t) if valid => format!("Will be sent {}", crate::ui::widgets::format_full_date(t)),
            _ => "Choose a time in the future.".to_string(),
        };
        ui.label(RichText::new(summary).size(12.0).color(if valid { pal.text_secondary } else { pal.danger }));
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let sched = ui.add_enabled(valid, egui::Button::new(RichText::new("Schedule").color(pal.accent_text)).fill(pal.accent).min_size(Vec2::new(84.0, 26.0)));
            if sched.clicked() {
                result = Some(true);
            }
            if ui.button("Cancel").clicked() {
                result = Some(false);
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            result = Some(false);
        }
        result
    }
}

fn shift_month(d: NaiveDate, delta: i32) -> NaiveDate {
    let m0 = d.year() * 12 + d.month0() as i32 + delta;
    NaiveDate::from_ymd_opt(m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_shifting_and_presets() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        assert_eq!(shift_month(d, -1), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        assert_eq!(shift_month(d, 12), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert!(tomorrow_at(8).unwrap() > Local::now().timestamp());
        let p = SendLaterPicker::default();
        assert!(p.timestamp().is_some());
    }
}
