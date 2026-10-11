//! Recipient token fields: typed addresses become rounded chips, with
//! address autocompletion.

use egui::{Align2, Color32, FontId, Id, Key, Modifiers, RichText, Sense, Stroke, Ui, Vec2};

use crate::core::model::Address;
use crate::ui::actions::{Action, UiCx};
use crate::ui::theme::Palette;

/// Split typed text into complete addresses and the unfinished remainder.
///
/// Separators are `,`, `;` and newlines outside quotes and angle brackets.
/// With `finish == true` the remainder is parsed as well (e.g. on blur).
pub fn parse_tokens(input: &str, finish: bool) -> (Vec<Address>, String) {
    let mut done = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut in_angle = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            '<' if !in_quote => {
                in_angle = true;
                cur.push(c);
            }
            '>' if !in_quote => {
                in_angle = false;
                cur.push(c);
            }
            ',' | ';' | '\n' | '\r' if !in_quote && !in_angle => {
                if let Some(a) = Address::parse_one(&cur) {
                    done.push(a);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if finish {
        if let Some(a) = Address::parse_one(&cur) {
            done.push(a);
        }
        cur.clear();
    }
    let rest = cur.trim_start().to_string();
    (done, rest)
}

/// Display settings for chips.
#[derive(Clone, Default)]
pub struct TokenStyle {
    /// Show addresses instead of names.
    pub show_addresses: bool,
    /// Highlight addresses not ending with this suffix (if set).
    pub mark_not_ending_with: Option<String>,
}

impl TokenStyle {
    fn is_marked(&self, a: &Address) -> bool {
        match &self.mark_not_ending_with {
            Some(suffixes) => {
                let e = a.email.to_lowercase();
                !suffixes.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).any(|s| e.ends_with(&s.to_lowercase()))
            }
            None => false,
        }
    }
}

/// An address token field.
pub struct TokenField {
    pub tokens: Vec<Address>,
    pub input: String,
    id: Id,
    suggestions: Vec<Address>,
    highlighted: usize,
    query: String,
    selected_chip: Option<usize>,
    focus_input: bool,
}

impl TokenField {
    pub fn new(id: Id, tokens: Vec<Address>) -> Self {
        TokenField {
            tokens,
            input: String::new(),
            id,
            suggestions: Vec::new(),
            highlighted: 0,
            query: String::new(),
            selected_chip: None,
            focus_input: false,
        }
    }

    /// All addresses including a not yet committed typed one.
    pub fn addresses(&self) -> Vec<Address> {
        let mut v = self.tokens.clone();
        v.extend(parse_tokens(&self.input, true).0);
        v
    }

    /// Turn the typed text into tokens.
    pub fn commit(&mut self) {
        let (done, _) = parse_tokens(&self.input, true);
        self.push_unique(done);
        self.input.clear();
        self.suggestions.clear();
    }

    fn push_unique(&mut self, addrs: Vec<Address>) {
        for a in addrs {
            if !self.tokens.iter().any(|t| t.email.eq_ignore_ascii_case(&a.email)) {
                self.tokens.push(a);
            }
        }
    }

    pub fn request_focus(&mut self) {
        self.focus_input = true;
    }

    fn input_id(&self) -> Id {
        self.id.with("input")
    }

    /// Show the field. Returns true when the token list changed.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut UiCx, style: &TokenStyle) -> bool {
        let pal = Palette::of(ui.ctx());
        let input_id = self.input_id();
        let mut changed = false;
        let focused = ui.memory(|m| m.has_focus(input_id));
        // keyboard handling before the text edit sees the keys
        let mut accept: Option<usize> = None;
        if focused {
            let n = self.suggestions.len();
            ui.input_mut(|i| {
                if n > 0 {
                    if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                        self.highlighted = (self.highlighted + 1) % n;
                    }
                    if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                        self.highlighted = (self.highlighted + n - 1) % n;
                    }
                    if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                        accept = Some(self.highlighted.min(n - 1));
                    }
                    if i.consume_key(Modifiers::NONE, Key::Escape) {
                        self.suggestions.clear();
                    }
                } else if !self.input.trim().is_empty() && i.consume_key(Modifiers::NONE, Key::Enter) {
                    accept = Some(usize::MAX);
                }
                if self.input.is_empty() && i.consume_key(Modifiers::NONE, Key::Backspace) {
                    if let Some(sel) = self.selected_chip.take() {
                        if sel < self.tokens.len() {
                            self.tokens.remove(sel);
                            changed = true;
                        }
                    } else if self.tokens.pop().is_some() {
                        changed = true;
                    }
                }
            });
        }
        if let Some(i) = accept {
            if i == usize::MAX {
                self.commit();
            } else if let Some(a) = self.suggestions.get(i).cloned() {
                self.push_unique(vec![a]);
                self.input.clear();
                self.suggestions.clear();
            }
            changed = true;
            self.focus_input = true;
        }
        let mut remove: Option<usize> = None;
        let mut edit: Option<usize> = None;
        let mut input_rect = egui::Rect::NOTHING;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for (i, a) in self.tokens.iter().enumerate() {
                let label = if style.show_addresses || a.name.trim().is_empty() { a.email.clone() } else { a.name.clone() };
                let valid = a.is_valid();
                let marked = style.is_marked(a);
                let selected = self.selected_chip == Some(i);
                let (fill, fg) = if selected {
                    (pal.accent, pal.accent_text)
                } else if !valid {
                    (pal.danger.gamma_multiply(if pal.dark { 0.35 } else { 0.14 }), pal.danger)
                } else if marked {
                    (
                        Color32::from_rgb(0xe8, 0x59, 0x0c).gamma_multiply(if pal.dark { 0.35 } else { 0.16 }),
                        if pal.dark { Color32::from_rgb(0xff, 0xa9, 0x4d) } else { Color32::from_rgb(0xb3, 0x45, 0x08) },
                    )
                } else {
                    (pal.accent_soft, if pal.dark { pal.text } else { Color32::from_rgb(0x4a, 0x2d, 0x96) })
                };
                let font = FontId::proportional(13.0);
                let galley = ui.painter().layout_no_wrap(label, font, fg);
                let size = Vec2::new(galley.size().x + 16.0, 22.0);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                ui.painter().rect_filled(rect, 11.0, fill);
                if !valid && !selected {
                    ui.painter().rect_stroke(rect, 11.0, Stroke::new(1.0, pal.danger.gamma_multiply(0.6)), egui::StrokeKind::Inside);
                }
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
                let tip = if valid { a.to_header() } else { format!("{} — invalid address", a.to_header()) };
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    self.selected_chip = Some(i);
                    self.focus_input = true;
                }
                if resp.double_clicked() {
                    edit = Some(i);
                }
                resp.context_menu(|ui| {
                    ui.label(RichText::new(a.to_header()).small().color(pal.text_secondary));
                    ui.separator();
                    if ui.button("Edit Address").clicked() {
                        edit = Some(i);
                        ui.close();
                    }
                    if ui.button("Remove Address").clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                    if ui.button("Copy Address").clicked() {
                        ui.ctx().copy_text(a.to_header());
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Add to VIPs").clicked() {
                        cx.act(Action::SetVip { address: a.clone(), vip: true });
                        ui.close();
                    }
                    if ui.button("Add to Contacts").clicked() {
                        cx.act(Action::AddToContacts(a.clone()));
                        ui.close();
                    }
                });
            }
            // remaining width on the current row (wrap to a new row if too narrow)
            let remaining = ui.max_rect().right() - ui.cursor().left() - 2.0;
            let w = if remaining < 120.0 { ui.max_rect().width() } else { remaining };
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .id(input_id)
                    .frame(egui::Frame::NONE)
                    .desired_width(w)
                    .margin(egui::Margin::symmetric(2, 3))
                    .font(FontId::proportional(13.5)),
            );
            input_rect = r.rect;
            if self.focus_input {
                r.request_focus();
                self.focus_input = false;
            }
            if r.changed() {
                self.selected_chip = None;
                if self.input.contains([',', ';', '\n']) {
                    let (done, rest) = parse_tokens(&self.input, false);
                    if !done.is_empty() {
                        self.push_unique(done);
                        changed = true;
                    }
                    self.input = rest;
                }
            }
            if r.lost_focus() && !self.input.trim().is_empty() && accept.is_none() {
                self.commit();
                changed = true;
            }
        });
        if let Some(i) = remove {
            self.tokens.remove(i);
            self.selected_chip = None;
            changed = true;
        }
        if let Some(i) = edit {
            let a = self.tokens.remove(i);
            self.commit();
            self.input = a.to_header();
            self.selected_chip = None;
            self.focus_input = true;
            changed = true;
        }
        if focused
            && self.selected_chip.is_some()
            && ui.input(|i| i.key_pressed(Key::Delete))
            && let Some(sel) = self.selected_chip.take()
            && sel < self.tokens.len()
        {
            self.tokens.remove(sel);
            changed = true;
        }
        // autocompletion
        let q = self.input.trim().to_string();
        let focused = ui.memory(|m| m.has_focus(input_id));
        if q != self.query {
            self.query = q.clone();
            self.highlighted = 0;
            self.suggestions = if q.is_empty() {
                vec![]
            } else {
                cx.store
                    .complete_address(&q, 8)
                    .into_iter()
                    .filter(|a| !self.tokens.iter().any(|t| t.email.eq_ignore_ascii_case(&a.email)))
                    .collect()
            };
        }
        if focused && !self.suggestions.is_empty() {
            self.suggestion_popup(ui, input_rect);
        }
        changed
    }

    fn suggestion_popup(&mut self, ui: &mut Ui, below: egui::Rect) {
        let pal = Palette::of(ui.ctx());
        let mut pick: Option<usize> = None;
        let pos = egui::pos2(below.left() - 4.0, below.bottom() + 4.0);
        egui::Area::new(self.id.with("completions")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                ui.set_min_width(300.0);
                for (i, a) in self.suggestions.iter().enumerate() {
                    let hl = i == self.highlighted;
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(300.0), 38.0), Sense::click());
                    if hl || resp.hovered() {
                        ui.painter().rect_filled(rect, 5.0, if hl { pal.accent } else { pal.hover_bg });
                    }
                    let (c1, c2) = if hl {
                        (pal.accent_text, pal.accent_text.gamma_multiply(0.85))
                    } else {
                        (pal.text, pal.text_secondary)
                    };
                    let av = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(24.0));
                    ui.painter().circle_filled(av.center(), 12.0, crate::ui::widgets::name_color(&a.email));
                    ui.painter().text(
                        av.center(),
                        Align2::CENTER_CENTER,
                        crate::ui::widgets::initials(&a.name, &a.email),
                        FontId::new(10.0, crate::ui::fonts::bold()),
                        Color32::WHITE,
                    );
                    let x = rect.left() + 38.0;
                    if a.name.trim().is_empty() {
                        ui.painter().text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, &a.email, FontId::proportional(13.0), c1);
                    } else {
                        ui.painter().text(egui::pos2(x, rect.top() + 11.0), Align2::LEFT_CENTER, &a.name, FontId::proportional(13.0), c1);
                        ui.painter().text(egui::pos2(x, rect.bottom() - 11.0), Align2::LEFT_CENTER, &a.email, FontId::proportional(11.5), c2);
                    }
                    if resp.clicked() {
                        pick = Some(i);
                    }
                }
            });
        });
        if let Some(i) = pick {
            let a = self.suggestions[i].clone();
            self.push_unique(vec![a]);
            self.input.clear();
            self.suggestions.clear();
            self.focus_input = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_on_separators() {
        let (done, rest) = parse_tokens("a@x.org, Bob <b@y.org>; \"Doe, Jane\" <j@z.org>, par", false);
        assert_eq!(done.len(), 3);
        assert_eq!(done[1], Address::new("Bob", "b@y.org"));
        assert_eq!(done[2], Address::new("Doe, Jane", "j@z.org"));
        assert_eq!(rest, "par");
        let (done, rest) = parse_tokens("par", true);
        assert_eq!(done, vec![Address::new("", "par")]);
        assert!(rest.is_empty());
        assert!(!done[0].is_valid());
        let (done, rest) = parse_tokens("", true);
        assert!(done.is_empty() && rest.is_empty());
        let (done, _) = parse_tokens("x@y.com\nz@w.com\n", false);
        assert_eq!(done.len(), 2);
    }

    #[test]
    fn marking_rule() {
        let st = TokenStyle {
            show_addresses: false,
            mark_not_ending_with: Some("example.com, corp.org".into()),
        };
        assert!(!st.is_marked(&Address::new("", "me@EXAMPLE.com")));
        assert!(!st.is_marked(&Address::new("", "a@corp.org")));
        assert!(st.is_marked(&Address::new("", "a@other.net")));
        assert!(!TokenStyle::default().is_marked(&Address::new("", "a@other.net")));
    }
}
