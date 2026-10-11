//! Signatures preferences: accounts | signatures | editor with live preview.
//!
//! Signatures are written in Purpamail's lightweight markup (`**bold**`,
//! `*italic*`, `[text](url)`, `- item`). The plain text editor can later be
//! replaced by the rich text editor (`ui::richtext::RichTextEditor`).

use egui::text::LayoutJob;
use egui::{FontFamily, FontId, RichText, Stroke, TextFormat, Ui};
use egui_phosphor::regular as ic;

use super::form;
use crate::core::config::{Settings, Signature};
use crate::ui::fonts;
use crate::ui::theme::Palette;

#[derive(Default)]
pub struct SignaturesState {
    /// Selected account filter (None = "All Signatures").
    account: Option<String>,
    selected: Option<String>,
}

/// An inline run of signature markup.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub link: Option<String>,
}

/// Parse one line of markup into styled spans.
pub fn parse_inline(line: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic) = (false, false);
    let mut cur = String::new();
    let chars: Vec<char> = line.chars().collect();
    let flush = |cur: &mut String, out: &mut Vec<Span>, bold: bool, italic: bool| {
        if !cur.is_empty() {
            out.push(Span { text: std::mem::take(cur), bold, italic, link: None });
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            flush(&mut cur, &mut out, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        }
        if c == '*' {
            flush(&mut cur, &mut out, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        if c == '[' {
            // [text](url)
            if let Some(close) = chars[i + 1..].iter().position(|&x| x == ']').map(|p| p + i + 1)
                && chars.get(close + 1) == Some(&'(')
                    && let Some(end) = chars[close + 2..].iter().position(|&x| x == ')').map(|p| p + close + 2) {
                        flush(&mut cur, &mut out, bold, italic);
                        let text: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..end].iter().collect();
                        out.push(Span { text: if text.is_empty() { url.clone() } else { text }, bold, italic, link: Some(url) });
                        i = end + 1;
                        continue;
                    }
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut out, bold, italic);
    out
}

/// Lay out signature markup for display.
pub fn markup_job(text: &str, size: f32, message_font: bool, pal: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let family = |b: bool, i: bool| -> FontFamily {
        if message_font {
            fonts::message_styled(b, i)
        } else {
            match (b, i) {
                (false, false) => FontFamily::Proportional,
                (true, false) => fonts::bold(),
                (false, true) => fonts::italic(),
                (true, true) => fonts::bold_italic(),
            }
        }
    };
    for (n, raw) in text.lines().enumerate() {
        if n > 0 {
            job.append("\n", 0.0, TextFormat { font_id: FontId::new(size, family(false, false)), ..Default::default() });
        }
        let (line, bullet) = match raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
            Some(rest) => (rest, true),
            None => (raw, false),
        };
        if bullet {
            job.append("•  ", 8.0, TextFormat { font_id: FontId::new(size, family(false, false)), color: pal.text_secondary, ..Default::default() });
        }
        for sp in parse_inline(line) {
            let mut f = TextFormat { font_id: FontId::new(size, family(sp.bold, sp.italic)), color: pal.text, ..Default::default() };
            if sp.link.is_some() {
                f.color = pal.accent;
                f.underline = Stroke::new(1.0, pal.accent);
            }
            job.append(&sp.text, 0.0, f);
        }
    }
    job
}

pub fn show(ui: &mut Ui, st: &mut SignaturesState, s: &mut Settings) {
    let pal = Palette::of(ui.ctx());
    if st.account.as_ref().is_some_and(|a| s.account(a).is_none()) {
        st.account = None;
    }
    let visible: Vec<usize> = s
        .signatures
        .iter()
        .enumerate()
        .filter(|(_, g)| match &st.account {
            None => true,
            Some(a) => g.accounts.is_empty() || g.accounts.contains(a),
        })
        .map(|(i, _)| i)
        .collect();
    if st.selected.as_ref().is_none_or(|id| !visible.iter().any(|i| &s.signatures[*i].id == id)) {
        st.selected = visible.first().map(|i| s.signatures[*i].id.clone());
    }
    let h = ui.available_height();
    ui.horizontal_top(|ui| {
        // ------------------------------------------------ accounts column
        ui.vertical(|ui| {
            ui.set_width(170.0);
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(h - 76.0);
                egui::ScrollArea::vertical().id_salt("sig_accounts").auto_shrink([false, false]).show(ui, |ui| {
                    let all_count = s.signatures.len();
                    if form::list_row(ui, st.account.is_none(), Some((ic::SIGNATURE, pal.accent)), "All Signatures", &count_label(all_count)).clicked() {
                        st.account = None;
                    }
                    for a in &s.accounts {
                        let n = s.signatures.iter().filter(|g| g.accounts.is_empty() || g.accounts.contains(&a.id)).count();
                        if form::list_row(ui, st.account.as_deref() == Some(a.id.as_str()), Some((ic::AT, pal.text_secondary)), a.display_name(), &count_label(n)).clicked() {
                            st.account = Some(a.id.clone());
                        }
                    }
                });
            });
            // default signature for the selected account
            if let Some(acct_id) = st.account.clone() {
                ui.label(RichText::new("Choose signature:").size(11.5).color(pal.text_secondary));
                let names: Vec<(Option<String>, String)> = std::iter::once((None, "None".to_string())).chain(visible.iter().map(|i| (Some(s.signatures[*i].id.clone()), s.signatures[*i].name.clone()))).collect();
                if let Some(acct) = s.account_mut(&acct_id) {
                    let refs: Vec<(Option<String>, &str)> = names.iter().map(|(i, n)| (i.clone(), n.as_str())).collect();
                    form::combo(ui, "sig_default", &mut acct.signature, &refs, 158.0);
                }
            }
        });
        // ------------------------------------------------ signatures column
        ui.vertical(|ui| {
            ui.set_width(180.0);
            form::list_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(h - 40.0);
                egui::ScrollArea::vertical().id_salt("sig_list").auto_shrink([false, false]).show(ui, |ui| {
                    if visible.is_empty() {
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("No signatures").color(pal.text_tertiary)));
                    }
                    for i in &visible {
                        let g = &s.signatures[*i];
                        let preview = g.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").replace(['*', '[', ']'], "");
                        if form::list_row(ui, st.selected.as_deref() == Some(g.id.as_str()), None, &g.name, &preview).clicked() {
                            st.selected = Some(g.id.clone());
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if form::icon_button(ui, ic::PLUS, "New signature", true).clicked() {
                    let n = s.signatures.len() + 1;
                    let mut g = Signature { name: format!("Signature #{n}"), ..Default::default() };
                    let me = s.accounts.first().map(|a| (a.full_name.clone(), a.primary_email().to_string())).unwrap_or_default();
                    g.body = if me.0.is_empty() { String::new() } else { format!("**{}**\n{}", me.0, me.1) };
                    if let Some(a) = &st.account {
                        g.accounts = vec![a.clone()];
                    }
                    st.selected = Some(g.id.clone());
                    s.signatures.push(g);
                }
                let sel_idx = st.selected.as_ref().and_then(|id| s.signatures.iter().position(|g| &g.id == id));
                if form::icon_button(ui, ic::MINUS, "Delete signature", sel_idx.is_some()).clicked()
                    && let Some(i) = sel_idx {
                        let removed = s.signatures.remove(i);
                        for a in &mut s.accounts {
                            if a.signature.as_deref() == Some(removed.id.as_str()) {
                                a.signature = None;
                            }
                        }
                        st.selected = None;
                    }
                if form::icon_button(ui, ic::COPY, "Duplicate signature", sel_idx.is_some()).clicked()
                    && let Some(i) = sel_idx {
                        let mut g = s.signatures[i].clone();
                        g.id = uuid::Uuid::new_v4().to_string();
                        g.name = format!("{} copy", g.name);
                        st.selected = Some(g.id.clone());
                        s.signatures.insert(i + 1, g);
                    }
                ui.add_space(6.0);
                let pos = sel_idx;
                if form::icon_button(ui, ic::ARROW_UP, "Move up", pos.is_some_and(|i| i > 0)).clicked()
                    && let Some(i) = pos {
                        s.signatures.swap(i, i - 1);
                    }
                if form::icon_button(ui, ic::ARROW_DOWN, "Move down", pos.is_some_and(|i| i + 1 < s.signatures.len())).clicked()
                    && let Some(i) = pos {
                        s.signatures.swap(i, i + 1);
                    }
            });
        });
        // ------------------------------------------------ editor column
        ui.vertical(|ui| {
            let Some(idx) = st.selected.as_ref().and_then(|id| s.signatures.iter().position(|g| &g.id == id)) else {
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(ic::SIGNATURE).size(40.0).color(pal.text_tertiary));
                    ui.label(RichText::new("Click + to create a signature.").color(pal.text_secondary));
                });
                return;
            };
            let accounts: Vec<(String, String)> = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
            let msg_size = s.fonts.message_font_size;
            let g = &mut s.signatures[idx];
            egui::ScrollArea::vertical().id_salt("sig_editor").auto_shrink([false, false]).show(ui, |ui| {
                ui.label(RichText::new("Name").size(11.5).color(pal.text_secondary));
                form::text_field(ui, &mut g.name, ui.available_width(), "Signature name");
                ui.add_space(6.0);
                ui.label(RichText::new("Signature").size(11.5).color(pal.text_secondary));
                ui.add(egui::TextEdit::multiline(&mut g.body).desired_width(f32::INFINITY).desired_rows(6).hint_text("Your signature…"));
                form::hint(ui, "**bold**   *italic*   [link text](https://…)   - list item");
                ui.add_space(6.0);
                ui.label(RichText::new("Preview").size(11.5).color(pal.text_secondary));
                egui::Frame::new().fill(pal.viewer_bg).stroke(Stroke::new(1.0, pal.separator)).corner_radius(6).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(60.0);
                    let size = if g.use_default_font { msg_size } else { 13.5 };
                    let mut job = markup_job(&g.body, size, g.use_default_font, &pal);
                    job.wrap.max_width = ui.available_width();
                    if g.body.trim().is_empty() {
                        ui.label(RichText::new("—").color(pal.text_tertiary));
                    } else {
                        ui.label(job);
                    }
                });
                ui.add_space(6.0);
                ui.checkbox(&mut g.use_default_font, "Always match my default message font");
                ui.add_space(6.0);
                ui.label(RichText::new("Offer for").size(11.5).color(pal.text_secondary));
                let mut all = g.accounts.is_empty();
                if ui.checkbox(&mut all, "All accounts").changed() {
                    g.accounts = if all { vec![] } else { accounts.first().map(|a| vec![a.0.clone()]).unwrap_or_default() };
                }
                if !all {
                    for (id, name) in &accounts {
                        let mut on = g.accounts.contains(id);
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                g.accounts.push(id.clone());
                            } else {
                                g.accounts.retain(|a| a != id);
                            }
                        }
                    }
                }
            });
        });
    });
}

fn count_label(n: usize) -> String {
    match n {
        0 => "No signatures".into(),
        1 => "1 signature".into(),
        n => format!("{n} signatures"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(text: &str, bold: bool, italic: bool, link: Option<&str>) -> Span {
        Span { text: text.into(), bold, italic, link: link.map(|s| s.into()) }
    }

    #[test]
    fn inline_markup() {
        assert_eq!(parse_inline("plain"), vec![sp("plain", false, false, None)]);
        assert_eq!(parse_inline("**Jane** Doe"), vec![sp("Jane", true, false, None), sp(" Doe", false, false, None)]);
        assert_eq!(parse_inline("*CEO*, ACME"), vec![sp("CEO", false, true, None), sp(", ACME", false, false, None)]);
        assert_eq!(parse_inline("Web: [acme](https://acme.test)!"), vec![sp("Web: ", false, false, None), sp("acme", false, false, Some("https://acme.test")), sp("!", false, false, None)]);
        assert_eq!(parse_inline("a \\*b\\*"), vec![sp("a *b*", false, false, None)]);
        assert_eq!(parse_inline("[unclosed link"), vec![sp("[unclosed link", false, false, None)]);
    }

    #[test]
    fn job_has_bullets() {
        let pal = Palette::light();
        let job = markup_job("- one\n- two", 13.0, false, &pal);
        assert!(job.text.contains("•  one\n•  two"));
    }
}
