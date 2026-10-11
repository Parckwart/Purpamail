//! Editors for criteria and rule actions, shared by the rule dialog (in the
//! Rules preferences) and the smart mailbox dialog.
//!
//! Also hosts small pickers reused by other settings panes: mailbox, account,
//! flag color, category and sound pickers.

use std::collections::{HashMap, HashSet};
use std::thread::JoinHandle;

use egui::text::LayoutJob;
use egui::{Color32, RichText, TextFormat, Ui, WidgetText};
use egui_phosphor::regular as ic;
use once_cell::sync::Lazy;

use super::actions::{Action, SidebarSel, UiCx};
use super::prefs::form;
use super::theme::Palette;
use crate::core::config::{Rule, RuleAction, Settings, SmartMailbox};
use crate::core::criteria::{Condition, Criteria, EvalContext, Field, FieldKind, Op};
use crate::core::model::{Category, FlagColor, LOCAL_ACCOUNT, MailboxId, MailboxRole};
use crate::core::store::{MessageQuery, Store};

// ------------------------------------------------------------------ condition helpers

/// Fields offered in the condition editor, in menu order.
pub const FIELDS: [Field; 26] = [
    Field::From,
    Field::To,
    Field::Cc,
    Field::AnyRecipient,
    Field::Subject,
    Field::Body,
    Field::EntireMessage,
    Field::Header,
    Field::Account,
    Field::InMailbox,
    Field::SenderInContacts,
    Field::SenderNotInContacts,
    Field::SenderIsVip,
    Field::SenderIsBlocked,
    Field::DateReceived,
    Field::DateSent,
    Field::Priority,
    Field::HasAttachment,
    Field::IsUnread,
    Field::IsRead,
    Field::IsFlagged,
    Field::FlagColor,
    Field::IsJunk,
    Field::IsReplied,
    Field::Category,
    Field::Every,
];

/// Operators that make sense for a kind of field (empty: the field takes no operand).
pub fn ops_for(kind: FieldKind) -> &'static [Op] {
    match kind {
        FieldKind::Text => &Op::TEXT,
        FieldKind::Date => &Op::DATE,
        FieldKind::Account | FieldKind::Priority | FieldKind::Color | FieldKind::Category | FieldKind::Mailbox => &Op::EQ,
        FieldKind::None => &[],
    }
}

pub const PRIORITIES: [&str; 3] = ["High", "Normal", "Low"];

/// Make the operator and value of a condition consistent with its field
/// (called after the field changed). `accounts` are the valid account ids.
pub fn normalize_condition(c: &mut Condition, accounts: &[String], default_mailbox: Option<MailboxId>) {
    let kind = c.field.kind();
    let ops = ops_for(kind);
    if !ops.is_empty() && !ops.contains(&c.op) {
        c.op = ops[0];
    }
    match kind {
        FieldKind::Text => {}
        FieldKind::Date => {
            if c.value.trim().parse::<u32>().map(|d| d == 0).unwrap_or(true) {
                c.value = "7".into();
            }
        }
        FieldKind::Account => {
            if !accounts.contains(&c.value) {
                c.value = accounts.first().cloned().unwrap_or_default();
            }
        }
        FieldKind::Priority => {
            if !PRIORITIES.contains(&c.value.as_str()) {
                c.value = "High".into();
            }
        }
        FieldKind::Color => {
            if !FlagColor::ALL.iter().any(|f| f.name() == c.value) {
                c.value = FlagColor::Red.name().into();
            }
        }
        FieldKind::Category => {
            if !Category::ALL.iter().any(|f| f.name() == c.value) {
                c.value = Category::Primary.name().into();
            }
        }
        FieldKind::Mailbox => {
            if c.value.parse::<MailboxId>().map(|v| v <= 0).unwrap_or(true) {
                c.value = default_mailbox.map(|m| m.to_string()).unwrap_or_default();
            }
        }
        FieldKind::None => c.value.clear(),
    }
    if c.field != Field::Header {
        c.header.clear();
    }
}

/// Why a condition cannot be used as is, if anything is wrong with it.
pub fn condition_problem(c: &Condition) -> Option<String> {
    match c.field.kind() {
        FieldKind::Text => {
            if c.field == Field::Header && c.header.trim().is_empty() {
                return Some("Enter a header name".into());
            }
            if c.value.trim().is_empty() {
                return Some("Enter some text to match".into());
            }
            if c.op == Op::MatchesRegex
                && let Err(e) = regex::Regex::new(&c.value) {
                    let msg = e.to_string();
                    return Some(format!("Invalid pattern: {}", msg.lines().last().unwrap_or("").trim()));
                }
            None
        }
        FieldKind::Date if c.op != Op::IsToday => match c.value.trim().parse::<u32>() {
            Ok(d) if d > 0 => None,
            _ => Some("Enter a number of days".into()),
        },
        FieldKind::Mailbox => match c.value.parse::<MailboxId>() {
            Ok(v) if v > 0 => None,
            _ => Some("Choose a mailbox".into()),
        },
        FieldKind::Account if c.value.is_empty() => Some("Choose an account".into()),
        _ => None,
    }
}

/// Problems of all conditions, by row index.
pub fn criteria_problems(c: &Criteria) -> Vec<(usize, String)> {
    let mut v: Vec<(usize, String)> = c.conditions.iter().enumerate().filter_map(|(i, c)| condition_problem(c).map(|p| (i, p))).collect();
    if c.conditions.is_empty() {
        v.push((0, "Add at least one condition".into()));
    }
    v
}

/// Why an action cannot be performed as configured, if anything is wrong with it.
pub fn action_problem(a: &RuleAction) -> Option<String> {
    match a {
        RuleAction::MoveTo { mailbox } | RuleAction::CopyTo { mailbox } if *mailbox <= 0 => Some("Choose a mailbox".into()),
        RuleAction::ForwardTo { addresses, .. } | RuleAction::RedirectTo { addresses } => {
            let list = crate::core::model::Address::parse_list(addresses);
            if list.is_empty() || list.iter().any(|a| !a.is_valid()) { Some("Enter valid email addresses".into()) } else { None }
        }
        RuleAction::RunCommand { command } if command.trim().is_empty() => Some("Enter a command".into()),
        _ => None,
    }
}

fn same_kind(a: &RuleAction, b: &RuleAction) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

// ------------------------------------------------------------------ pickers

/// A mailbox as offered by [`mailbox_picker`].
#[derive(Clone, Debug)]
pub struct MailboxEntry {
    pub id: MailboxId,
    pub name: String,
    pub account: String,
    pub depth: usize,
    pub role: MailboxRole,
}

/// All mailboxes grouped by account (in settings order, local mailboxes last).
pub fn mailbox_catalog(store: &Store, settings: &Settings) -> Vec<MailboxEntry> {
    let mut all = store.mailboxes(None);
    all.retain(|m| m.role != MailboxRole::Outbox && m.selectable);
    let order = |acct: &str| -> usize {
        if acct == LOCAL_ACCOUNT {
            usize::MAX
        } else {
            settings.accounts.iter().position(|a| a.id == acct).unwrap_or(usize::MAX - 1)
        }
    };
    all.sort_by(|a, b| {
        order(&a.account_id)
            .cmp(&order(&b.account_id))
            .then_with(|| (a.role == MailboxRole::Custom).cmp(&(b.role == MailboxRole::Custom)))
            .then_with(|| crate::ui::app::role_order(a.role).cmp(&crate::ui::app::role_order(b.role)))
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    all.into_iter()
        .map(|m| {
            let depth = if m.delimiter.is_empty() { 0 } else { m.path.matches(m.delimiter.as_str()).count() };
            MailboxEntry { id: m.id, name: m.name.clone(), account: account_label(settings, &m.account_id), depth, role: m.role }
        })
        .collect()
}

/// Display name of an account id ("On This Computer" for local mailboxes).
pub fn account_label(settings: &Settings, id: &str) -> String {
    if id == LOCAL_ACCOUNT {
        "On This Computer".into()
    } else {
        settings.account(id).map(|a| a.display_name().to_string()).unwrap_or_else(|| id.to_string())
    }
}

/// Combo box choosing a mailbox. Returns true on change.
pub fn mailbox_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut MailboxId, catalog: &[MailboxEntry], width: f32) -> bool {
    let pal = Palette::of(ui.ctx());
    let selected = catalog.iter().find(|e| e.id == *value).map(|e| format!("{} — {}", e.name, e.account)).unwrap_or_else(|| "Choose a mailbox…".into());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt).width(width).height(320.0).truncate().selected_text(selected).show_ui(ui, |ui| {
        if catalog.is_empty() {
            ui.label(RichText::new("No mailboxes").color(pal.text_tertiary));
        }
        let mut last_account = String::new();
        for e in catalog {
            if e.account != last_account {
                if !last_account.is_empty() {
                    ui.separator();
                }
                ui.label(RichText::new(&e.account).size(11.0).color(pal.text_tertiary).family(crate::ui::fonts::bold()));
                last_account = e.account.clone();
            }
            let text = format!("{}{}  {}", "    ".repeat(e.depth), crate::ui::app::role_icon(e.role), e.name);
            if ui.selectable_label(*value == e.id, text).clicked() && *value != e.id {
                *value = e.id;
                changed = true;
            }
        }
    });
    changed
}

/// Combo box choosing an account by id (including "On This Computer" when `include_local`).
pub fn account_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, settings: &Settings, include_local: bool, width: f32) -> bool {
    let mut opts: Vec<(String, String)> = settings.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
    if include_local {
        opts.push((LOCAL_ACCOUNT.to_string(), "On This Computer".into()));
    }
    let refs: Vec<(String, &str)> = opts.iter().map(|(i, n)| (i.clone(), n.as_str())).collect();
    form::combo(ui, id_salt, value, &refs, width)
}

fn flag_text(ui: &Ui, color: Color32, name: &str) -> WidgetText {
    let mut job = LayoutJob::default();
    let font = egui::TextStyle::Button.resolve(ui.style());
    job.append(ic::FLAG, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
    job.append(name, 6.0, TextFormat { font_id: font, color: ui.visuals().text_color(), ..Default::default() });
    job.into()
}

/// Combo box choosing a flag color (shown with the user's flag names).
pub fn color_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut FlagColor, settings: &Settings, width: f32) -> bool {
    let pal = Palette::of(ui.ctx());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt).width(width).selected_text(flag_text(ui, pal.flag(*value), &settings.flag_name(*value))).show_ui(ui, |ui| {
        for c in FlagColor::ALL {
            let t = flag_text(ui, pal.flag(c), &settings.flag_name(c));
            if ui.selectable_label(*value == c, t).clicked() && *value != c {
                *value = c;
                changed = true;
            }
        }
    });
    changed
}

/// Combo box choosing an optional flag color ("None" first).
pub fn opt_color_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut Option<FlagColor>, settings: &Settings, width: f32) -> bool {
    let pal = Palette::of(ui.ctx());
    let mut changed = false;
    let sel: WidgetText = match value {
        Some(c) => flag_text(ui, pal.flag(*c), &settings.flag_name(*c)),
        None => "None".into(),
    };
    egui::ComboBox::from_id_salt(id_salt).width(width).selected_text(sel).show_ui(ui, |ui| {
        if ui.selectable_label(value.is_none(), "None").clicked() && value.is_some() {
            *value = None;
            changed = true;
        }
        for c in FlagColor::ALL {
            let t = flag_text(ui, pal.flag(c), &settings.flag_name(c));
            if ui.selectable_label(*value == Some(c), t).clicked() && *value != Some(c) {
                *value = Some(c);
                changed = true;
            }
        }
    });
    changed
}

fn category_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut Category, width: f32) -> bool {
    let opts: Vec<(Category, &str)> = Category::ALL.iter().map(|c| (*c, c.name())).collect();
    form::combo(ui, id_salt, value, &opts, width)
}

/// Sounds found under `/usr/share/sounds`: (value, label). Sounds in the
/// directories searched by name are stored by name, others by full path.
pub fn available_sounds() -> &'static [(String, String)] {
    static SOUNDS: Lazy<Vec<(String, String)>> = Lazy::new(scan_sounds);
    &SOUNDS
}

fn scan_sounds() -> Vec<(String, String)> {
    let by_name = ["/usr/share/sounds/freedesktop/stereo", "/usr/share/sounds/gnome/default/alerts", "/usr/share/sounds"];
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![(std::path::PathBuf::from("/usr/share/sounds"), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 4 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !["oga", "ogg", "wav"].contains(&ext.as_str()) {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let in_named = p.parent().and_then(|d| d.to_str()).is_some_and(|d| by_name.contains(&d));
            let value = if in_named { stem.clone() } else { p.display().to_string() };
            if seen.insert(stem.clone()) {
                out.push((value, pretty_sound_name(&stem)));
            }
        }
    }
    out.sort_by_key(|a| a.1.to_lowercase());
    out
}

/// "message-new-instant" → "Message New Instant".
pub fn pretty_sound_name(stem: &str) -> String {
    stem.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sound_label(value: &str) -> String {
    match value {
        "" => "None".into(),
        "default" => "Default".into(),
        v => available_sounds()
            .iter()
            .find(|(val, _)| val == v)
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| pretty_sound_name(std::path::Path::new(v).file_stem().and_then(|s| s.to_str()).unwrap_or(v))),
    }
}

/// Sound chooser: None (optional), Default, system sounds and "Choose File…",
/// with a preview button. Returns true on change.
pub fn sound_picker(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut String, allow_none: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id_salt).width(200.0).height(300.0).selected_text(sound_label(value)).show_ui(ui, |ui| {
            let custom = (value.contains('/') && !available_sounds().iter().any(|(v, _)| v == value)).then(|| value.clone());
            let mut pick = |ui: &mut Ui, v: &str, label: &str| {
                if ui.selectable_label(value == v, label).clicked() && value != v {
                    *value = v.to_string();
                    changed = true;
                }
            };
            if allow_none {
                pick(ui, "", "None");
            }
            pick(ui, "default", "Default");
            if let Some(cur) = custom {
                pick(ui, &cur, &sound_label(&cur));
            }
            if !available_sounds().is_empty() {
                ui.separator();
            }
            for (v, l) in available_sounds() {
                pick(ui, v, l);
            }
            ui.separator();
            if ui.selectable_label(false, "Choose File…").clicked()
                && let Some(p) = rfd::FileDialog::new().set_title("Choose a Sound").add_filter("Sound", &["oga", "ogg", "wav", "mp3", "flac"]).pick_file() {
                    *value = p.display().to_string();
                    changed = true;
                }
        });
        if form::icon_button(ui, ic::PLAY, "Play sound", !value.is_empty()).clicked() {
            crate::ui::app::play_sound(value);
        }
    });
    changed
}

// ------------------------------------------------------------------ criteria editor

/// Data the row editors need, gathered once per frame.
struct EditorData {
    settings: Settings,
    accounts: Vec<String>,
    catalog: Vec<MailboxEntry>,
}

impl EditorData {
    fn gather(cx: &UiCx) -> Self {
        let settings = cx.settings.read().clone();
        let mut accounts: Vec<String> = settings.accounts.iter().map(|a| a.id.clone()).collect();
        accounts.push(LOCAL_ACCOUNT.into());
        let catalog = mailbox_catalog(cx.store, &settings);
        Self { settings, accounts, catalog }
    }
}

const FIELD_W: f32 = 190.0;
const OP_W: f32 = 150.0;

/// Edits a list of conditions with an "any / all" selector. Returns true when
/// anything changed.
pub fn criteria_editor(ui: &mut Ui, id_salt: impl egui::AsIdSalt, criteria: &mut Criteria, cx: &UiCx) -> bool {
    let data = EditorData::gather(cx);
    let base = egui::Id::new(id_salt);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("If");
        let opts = [(false, "any"), (true, "all")];
        changed |= form::combo(ui, base.with("matchall"), &mut criteria.match_all, &opts, 60.0);
        ui.label("of the following conditions are met:");
    });
    ui.add_space(4.0);
    let mut remove: Option<usize> = None;
    let mut insert: Option<usize> = None;
    let n = criteria.conditions.len();
    for (i, c) in criteria.conditions.iter_mut().enumerate() {
        ui.push_id(base.with(i), |ui| {
            ui.horizontal(|ui| {
                changed |= condition_row(ui, c, &data);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if form::icon_button(ui, ic::PLUS, "Add a condition", true).clicked() {
                        insert = Some(i + 1);
                    }
                    if form::icon_button(ui, ic::MINUS, "Remove this condition", n > 1).clicked() {
                        remove = Some(i);
                    }
                    if let Some(p) = condition_problem(c) {
                        ui.label(RichText::new(ic::WARNING_CIRCLE).color(Palette::of(ui.ctx()).danger)).on_hover_text(p);
                    }
                });
            });
        });
    }
    if n == 0 && ui.button(format!("{} Add Condition", ic::PLUS)).clicked() {
        insert = Some(0);
    }
    if let Some(i) = remove {
        criteria.conditions.remove(i);
        changed = true;
    }
    if let Some(i) = insert {
        let mut c = Condition::default();
        normalize_condition(&mut c, &data.accounts, data.catalog.first().map(|e| e.id));
        criteria.conditions.insert(i.min(criteria.conditions.len()), c);
        changed = true;
    }
    changed
}

fn condition_row(ui: &mut Ui, c: &mut Condition, data: &EditorData) -> bool {
    let mut changed = false;
    // field
    let mut field = c.field;
    egui::ComboBox::from_id_salt("field").width(FIELD_W).height(360.0).selected_text(field.name()).show_ui(ui, |ui| {
        for (i, f) in FIELDS.iter().enumerate() {
            if matches!(i, 8 | 10 | 14 | 16) {
                ui.separator();
            }
            ui.selectable_value(&mut field, *f, f.name());
        }
    });
    if field != c.field {
        let was_text = c.field.kind() == FieldKind::Text;
        c.field = field;
        if !(was_text && field.kind() == FieldKind::Text) {
            c.value.clear();
        }
        normalize_condition(c, &data.accounts, data.catalog.first().map(|e| e.id));
        changed = true;
    }
    if c.field == Field::Header {
        changed |= form::text_field(ui, &mut c.header, 100.0, "Header name").changed();
    }
    // operator
    let ops = ops_for(c.field.kind());
    if !ops.is_empty() {
        let opts: Vec<(Op, &str)> = ops.iter().map(|o| (*o, o.name())).collect();
        changed |= form::combo(ui, "op", &mut c.op, &opts, if c.field.kind() == FieldKind::Text { OP_W } else { 70.0 });
    }
    // value
    let value_w = (ui.available_width() - 84.0).max(80.0);
    match c.field.kind() {
        FieldKind::Text => {
            changed |= form::text_field(ui, &mut c.value, value_w, if c.op == Op::MatchesRegex { "Regular expression" } else { "" }).changed();
        }
        FieldKind::Date => {
            if c.op != Op::IsToday {
                let mut days: u32 = c.value.trim().parse().unwrap_or(7);
                let suffix = if days == 1 { " day" } else { " days" };
                if ui.add(egui::DragValue::new(&mut days).range(1..=3650).suffix(suffix)).changed() {
                    c.value = days.to_string();
                    changed = true;
                }
            }
        }
        FieldKind::Account => {
            changed |= account_picker(ui, "acct", &mut c.value, &data.settings, true, value_w);
        }
        FieldKind::Priority => {
            let opts: Vec<(String, &str)> = PRIORITIES.iter().map(|p| (p.to_string(), *p)).collect();
            changed |= form::combo(ui, "prio", &mut c.value, &opts, value_w);
        }
        FieldKind::Color => {
            let mut col = FlagColor::ALL.iter().copied().find(|f| f.name() == c.value).unwrap_or_default();
            if color_picker(ui, "color", &mut col, &data.settings, value_w) {
                c.value = col.name().into();
                changed = true;
            }
        }
        FieldKind::Category => {
            let mut cat = Category::ALL.iter().copied().find(|f| f.name() == c.value).unwrap_or_default();
            if category_picker(ui, "cat", &mut cat, value_w) {
                c.value = cat.name().into();
                changed = true;
            }
        }
        FieldKind::Mailbox => {
            let mut id: MailboxId = c.value.parse().unwrap_or(0);
            if mailbox_picker(ui, "mbox", &mut id, &data.catalog, value_w) {
                c.value = id.to_string();
                changed = true;
            }
        }
        FieldKind::None => {}
    }
    changed
}

// ------------------------------------------------------------------ actions editor

/// Edits the list of actions of a rule. Returns true when anything changed.
pub fn actions_editor(ui: &mut Ui, id_salt: impl egui::AsIdSalt, actions: &mut Vec<RuleAction>, cx: &UiCx) -> bool {
    let data = EditorData::gather(cx);
    let base = egui::Id::new(id_salt);
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut insert: Option<usize> = None;
    let n = actions.len();
    for (i, a) in actions.iter_mut().enumerate() {
        ui.push_id(base.with(i), |ui| {
            ui.horizontal_top(|ui| {
                changed |= action_row(ui, a, &data);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if form::icon_button(ui, ic::PLUS, "Add an action", true).clicked() {
                        insert = Some(i + 1);
                    }
                    if form::icon_button(ui, ic::MINUS, "Remove this action", n > 1).clicked() {
                        remove = Some(i);
                    }
                    if let Some(p) = action_problem(a) {
                        ui.label(RichText::new(ic::WARNING_CIRCLE).color(Palette::of(ui.ctx()).danger)).on_hover_text(p);
                    }
                });
            });
        });
    }
    if n == 0 && ui.button(format!("{} Add Action", ic::PLUS)).clicked() {
        insert = Some(0);
    }
    if let Some(i) = remove {
        actions.remove(i);
        changed = true;
    }
    if let Some(i) = insert {
        actions.insert(i.min(actions.len()), RuleAction::MarkAsRead);
        changed = true;
    }
    changed
}

fn action_row(ui: &mut Ui, a: &mut RuleAction, data: &EditorData) -> bool {
    let mut changed = false;
    let mut chosen: Option<RuleAction> = None;
    egui::ComboBox::from_id_salt("kind").width(FIELD_W).height(400.0).selected_text(a.name()).show_ui(ui, |ui| {
        for t in RuleAction::templates() {
            if ui.selectable_label(same_kind(a, &t), t.name()).clicked() && !same_kind(a, &t) {
                chosen = Some(t);
            }
        }
    });
    if let Some(mut t) = chosen {
        if let RuleAction::MoveTo { mailbox } | RuleAction::CopyTo { mailbox } = &mut t
            && let RuleAction::MoveTo { mailbox: old } | RuleAction::CopyTo { mailbox: old } = a {
                *mailbox = *old;
            }
        *a = t;
        changed = true;
    }
    let w = (ui.available_width() - 84.0).max(120.0);
    match a {
        RuleAction::MoveTo { mailbox } | RuleAction::CopyTo { mailbox } => {
            ui.label("to mailbox:");
            let mw = (ui.available_width() - 84.0).max(120.0);
            changed |= mailbox_picker(ui, "mbox", mailbox, &data.catalog, mw);
        }
        RuleAction::SetColor { color } | RuleAction::SetHighlight { color } | RuleAction::MarkAsFlagged { color } => {
            changed |= color_picker(ui, "color", color, &data.settings, 140.0);
        }
        RuleAction::PlaySound { sound } => {
            changed |= sound_picker(ui, "sound", sound, false);
        }
        RuleAction::Notify { text } => {
            changed |= form::text_field(ui, text, w, "Notification text (optional)").changed();
        }
        RuleAction::ReplyWith { text } => {
            changed |= ui.add(egui::TextEdit::multiline(text).desired_width(w).desired_rows(2).hint_text("Reply message text")).changed();
        }
        RuleAction::ForwardTo { addresses, text } => {
            ui.vertical(|ui| {
                changed |= form::text_field(ui, addresses, w, "To: name@example.com, …").changed();
                changed |= ui.add(egui::TextEdit::multiline(text).desired_width(w).desired_rows(2).hint_text("Message text (optional)")).changed();
            });
        }
        RuleAction::RedirectTo { addresses } => {
            changed |= form::text_field(ui, addresses, w, "To: name@example.com, …").changed();
        }
        RuleAction::RunCommand { command } => {
            changed |= form::text_field(ui, command, w, "Shell command").changed();
        }
        RuleAction::SetCategory { category } => {
            changed |= category_picker(ui, "cat", category, 160.0);
        }
        _ => {}
    }
    changed
}

// ------------------------------------------------------------------ rule dialog

/// Outcome of a dialog frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogResult {
    Open,
    Ok,
    Cancel,
}

/// The dialog editing one mail rule (shown as a modal sheet).
pub struct RuleDialog {
    pub rule: Rule,
    pub is_new: bool,
}

impl RuleDialog {
    pub fn new(rule: Rule, is_new: bool) -> Self {
        Self { rule, is_new }
    }

    pub fn show(&mut self, ctx: &egui::Context, cx: &UiCx) -> DialogResult {
        let mut result = DialogResult::Open;
        let pal = Palette::of(ctx);
        let max_h = ctx.content_rect().height() - 60.0;
        egui::Modal::new(egui::Id::new("rule_dialog")).show(ctx, |ui| {
            form::window_style(ui);
            ui.set_width(ctx.content_rect().width().min(760.0) - 60.0);
            ui.label(RichText::new(if self.is_new { "New Rule" } else { "Edit Rule" }).family(crate::ui::fonts::bold()).size(15.0));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Description:");
                form::text_field(ui, &mut self.rule.name, ui.available_width(), "Rule name");
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical().id_salt("rule_body").max_height((max_h - 150.0).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
                egui::Frame::new().fill(pal.toolbar_bg).corner_radius(8).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    criteria_editor(ui, "rule_criteria", &mut self.rule.criteria, cx);
                });
                ui.add_space(8.0);
                egui::Frame::new().fill(pal.toolbar_bg).corner_radius(8).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label("Perform the following actions:");
                    ui.add_space(4.0);
                    actions_editor(ui, "rule_actions", &mut self.rule.actions, cx);
                });
            });
            ui.add_space(10.0);
            let problems = criteria_problems(&self.rule.criteria).len() + self.rule.actions.iter().filter(|a| action_problem(a).is_some()).count();
            let valid = problems == 0 && !self.rule.actions.is_empty() && !self.rule.name.trim().is_empty();
            ui.horizontal(|ui| {
                if !valid {
                    let msg = if self.rule.name.trim().is_empty() { "Enter a description".to_string() } else { "Complete the highlighted fields".to_string() };
                    ui.label(RichText::new(format!("{} {msg}", ic::WARNING_CIRCLE)).size(12.0).color(pal.text_secondary));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(valid, egui::Button::new(RichText::new("OK").color(pal.accent_text)).fill(pal.accent).min_size(egui::vec2(72.0, 26.0))).clicked() {
                        result = DialogResult::Ok;
                    }
                    if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(72.0, 26.0))).clicked() {
                        result = DialogResult::Cancel;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                result = DialogResult::Cancel;
            }
        });
        result
    }
}

// ------------------------------------------------------------------ smart mailbox dialog

/// Most messages scanned when counting matches (keeps the count fast on big stores).
const COUNT_CAP: usize = 20_000;
/// Most messages whose body is parsed when the criteria need message content.
const BODY_CAP: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MatchCount {
    matched: usize,
    /// Not every message was examined.
    capped: bool,
}

/// Dialog for creating / editing a smart mailbox.
pub struct SmartMailboxDialog {
    pub mailbox: SmartMailbox,
    pub is_new: bool,
    /// Time the criteria last changed (the count is recomputed after a short pause).
    dirty_at: Option<f64>,
    generation: u64,
    job: Option<(u64, JoinHandle<MatchCount>)>,
    count: Option<MatchCount>,
    focus_name: bool,
}

impl SmartMailboxDialog {
    pub fn new(mailbox: SmartMailbox, is_new: bool) -> Self {
        Self { mailbox, is_new, dirty_at: Some(f64::NEG_INFINITY), generation: 0, job: None, count: None, focus_name: true }
    }

    /// Returns false once the dialog is closed (saved or cancelled).
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        let now = ctx.input(|i| i.time);
        self.poll_count(ctx, cx, now);
        let pal = Palette::of(ctx);
        let mut result = DialogResult::Open;
        let max_h = ctx.content_rect().height() - 80.0;
        egui::Modal::new(egui::Id::new("smart_mailbox_dialog")).show(ctx, |ui| {
            form::window_style(ui);
            ui.set_width(ctx.content_rect().width().min(780.0) - 80.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::GEAR_SIX).size(26.0).color(pal.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new(if self.is_new { "New Smart Mailbox" } else { "Edit Smart Mailbox" }).family(crate::ui::fonts::bold()).size(15.0));
                    ui.label(RichText::new("Smart mailboxes always show the messages that match their conditions.").size(12.0).color(pal.text_secondary));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Smart Mailbox Name:");
                let r = form::text_field(ui, &mut self.mailbox.name, ui.available_width(), "Name");
                if self.focus_name {
                    r.request_focus();
                    self.focus_name = false;
                }
            });
            ui.add_space(8.0);
            let mut changed = false;
            egui::ScrollArea::vertical().id_salt("smart_body").max_height((max_h - 200.0).max(140.0)).auto_shrink([false, true]).show(ui, |ui| {
                egui::Frame::new().fill(pal.toolbar_bg).corner_radius(8).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    changed |= criteria_editor(ui, "smart_criteria", &mut self.mailbox.criteria, cx);
                });
            });
            ui.add_space(6.0);
            changed |= ui.checkbox(&mut self.mailbox.include_trash, "Include messages from Trash").changed();
            changed |= ui.checkbox(&mut self.mailbox.include_sent, "Include messages from Sent").changed();
            if changed {
                self.dirty_at = Some(now);
            }
            ui.add_space(10.0);
            let valid = criteria_problems(&self.mailbox.criteria).is_empty() && !self.mailbox.name.trim().is_empty();
            ui.horizontal(|ui| {
                let busy = self.dirty_at.is_some() || self.job.is_some();
                if !criteria_problems(&self.mailbox.criteria).is_empty() {
                    ui.label(RichText::new(format!("{} Complete the highlighted conditions", ic::WARNING_CIRCLE)).size(12.0).color(pal.text_secondary));
                } else if busy {
                    ui.add(egui::Spinner::new().size(13.0));
                    ui.label(RichText::new("Counting messages…").size(12.0).color(pal.text_secondary));
                } else if let Some(c) = self.count {
                    ui.label(RichText::new(count_text(c)).size(12.0).color(pal.text_secondary));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(valid, egui::Button::new(RichText::new("OK").color(pal.accent_text)).fill(pal.accent).min_size(egui::vec2(72.0, 26.0))).clicked() {
                        result = DialogResult::Ok;
                    }
                    if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(72.0, 26.0))).clicked() {
                        result = DialogResult::Cancel;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                result = DialogResult::Cancel;
            }
        });
        match result {
            DialogResult::Open => true,
            DialogResult::Cancel => false,
            DialogResult::Ok => {
                self.mailbox.name = self.mailbox.name.trim().to_string();
                {
                    let mut s = cx.settings.write();
                    match s.smart_mailboxes.iter_mut().find(|m| m.id == self.mailbox.id) {
                        Some(m) => *m = self.mailbox.clone(),
                        None => s.smart_mailboxes.push(self.mailbox.clone()),
                    }
                }
                cx.act(Action::SettingsChanged);
                cx.act(Action::SelectSidebar(SidebarSel::Smart(self.mailbox.id.clone())));
                false
            }
        }
    }

    fn poll_count(&mut self, ctx: &egui::Context, cx: &UiCx, now: f64) {
        if let Some((generation, job)) = self.job.take_if(|(_, j)| j.is_finished())
            && let Ok(c) = job.join()
                && generation == self.generation {
                    self.count = Some(c);
                }
        if let Some(t) = self.dirty_at
            && self.job.is_none() && now - t >= 0.35 && criteria_problems(&self.mailbox.criteria).is_empty() {
                self.dirty_at = None;
                self.generation += 1;
                let store = cx.store.clone();
                let settings = cx.settings.read().clone();
                let sm = self.mailbox.clone();
                let generation = self.generation;
                if let Ok(h) = std::thread::Builder::new().name("smart-count".into()).spawn(move || count_matches(&store, &settings, &sm)) {
                    self.job = Some((generation, h));
                }
            }
        if self.dirty_at.is_some() || self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn count_text(c: MatchCount) -> String {
    let n = c.matched;
    let num = group_thousands(n);
    match (n, c.capped) {
        (_, true) => format!("At least {num} messages match"),
        (0, false) => "No messages match".into(),
        (1, false) => "1 message matches".into(),
        _ => format!("{num} messages match"),
    }
}

fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Count messages matching a smart mailbox, the same way the message list evaluates it.
fn count_matches(store: &Store, s: &Settings, sm: &SmartMailbox) -> MatchCount {
    let mailboxes = store.mailboxes(None);
    let exclude: Vec<MailboxId> = mailboxes
        .iter()
        .filter(|m| (m.role == MailboxRole::Trash && !sm.include_trash) || (m.role == MailboxRole::Junk && !s.general.search_include_junk) || (m.role == MailboxRole::Sent && !sm.include_sent))
        .map(|m| m.id)
        .collect();
    let needs_body = sm.criteria.needs_body();
    let cap = if needs_body { BODY_CAP } else { COUNT_CAP };
    let q = MessageQuery { exclude_mailboxes: exclude, include_deleted: s.viewing.show_deleted_messages, limit: Some(cap + 1), ..Default::default() };
    let msgs = store.list_messages(&q);
    let capped = msgs.len() > cap;
    let contacts = store.contact_emails();
    let vips: HashSet<String> = store.vip_emails().into_iter().collect();
    let blocked: HashSet<String> = store.blocked_senders().into_iter().collect();
    let names: HashMap<String, String> = s.accounts.iter().map(|a| (a.id.clone(), a.display_name().to_string())).collect();
    let matched = msgs
        .iter()
        .take(cap)
        .filter(|m| {
            let parsed = if needs_body { store.raw(m.id).and_then(|r| crate::core::mime::parse(&r)) } else { None };
            let body = parsed.as_ref().map(|p| p.plain_text());
            let from = m.from.email.to_lowercase();
            let ecx = EvalContext {
                msg: m,
                body: body.as_deref(),
                headers: parsed.as_ref().map(|p| p.headers.as_slice()),
                account_name: names.get(&m.account_id).map(|s| s.as_str()).unwrap_or(""),
                sender_in_contacts: contacts.contains(&from),
                sender_is_vip: vips.contains(&from),
                sender_is_blocked: blocked.contains(&from),
            };
            sm.criteria.matches(&ecx)
        })
        .count();
    MatchCount { matched, capped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(field: Field, op: Op, value: &str) -> Condition {
        Condition { field, op, value: value.into(), header: String::new() }
    }

    #[test]
    fn ops_match_field_kinds() {
        assert_eq!(ops_for(Field::Subject.kind()), &Op::TEXT);
        assert_eq!(ops_for(Field::DateSent.kind()), &Op::DATE);
        assert_eq!(ops_for(Field::Priority.kind()), &Op::EQ);
        assert!(ops_for(Field::IsUnread.kind()).is_empty());
        // every field in the menu is in the catalog once
        let set: HashSet<_> = FIELDS.iter().map(|f| *f as u8).collect();
        assert_eq!(set.len(), FIELDS.len());
    }

    #[test]
    fn normalize_after_field_change() {
        let accts = vec!["a1".to_string(), LOCAL_ACCOUNT.to_string()];
        let mut c = cond(Field::DateReceived, Op::Contains, "hello");
        normalize_condition(&mut c, &accts, Some(5));
        assert_eq!((c.op, c.value.as_str()), (Op::IsInTheLast, "7"));

        let mut c = cond(Field::Account, Op::Contains, "nope");
        normalize_condition(&mut c, &accts, Some(5));
        assert_eq!((c.op, c.value.as_str()), (Op::Is, "a1"));

        let mut c = cond(Field::InMailbox, Op::Is, "");
        normalize_condition(&mut c, &accts, Some(5));
        assert_eq!(c.value, "5");

        let mut c = cond(Field::FlagColor, Op::IsToday, "Mauve");
        normalize_condition(&mut c, &accts, None);
        assert_eq!((c.op, c.value.as_str()), (Op::Is, "Red"));

        let mut c = Condition { field: Field::IsFlagged, op: Op::Contains, value: "x".into(), header: "X-Foo".into() };
        normalize_condition(&mut c, &accts, None);
        assert!(c.value.is_empty() && c.header.is_empty());

        // text values survive switching between text fields
        let mut c = cond(Field::Subject, Op::BeginsWith, "Invoice");
        c.field = Field::Body;
        normalize_condition(&mut c, &accts, None);
        assert_eq!((c.op, c.value.as_str()), (Op::BeginsWith, "Invoice"));
    }

    #[test]
    fn condition_validation() {
        assert!(condition_problem(&cond(Field::Subject, Op::Contains, "")).is_some());
        assert!(condition_problem(&cond(Field::Subject, Op::Contains, "x")).is_none());
        assert!(condition_problem(&cond(Field::Subject, Op::MatchesRegex, "(unclosed")).is_some());
        assert!(condition_problem(&cond(Field::Subject, Op::MatchesRegex, "^re:")).is_none());
        assert!(condition_problem(&Condition { field: Field::Header, op: Op::Contains, value: "x".into(), header: " ".into() }).is_some());
        assert!(condition_problem(&cond(Field::DateSent, Op::IsInTheLast, "0")).is_some());
        assert!(condition_problem(&cond(Field::DateSent, Op::IsInTheLast, "3")).is_none());
        assert!(condition_problem(&cond(Field::DateSent, Op::IsToday, "")).is_none());
        assert!(condition_problem(&cond(Field::InMailbox, Op::Is, "")).is_some());
        assert!(condition_problem(&cond(Field::IsUnread, Op::Contains, "")).is_none());
        let empty = Criteria { match_all: true, conditions: vec![] };
        assert_eq!(criteria_problems(&empty).len(), 1);
    }

    #[test]
    fn action_validation() {
        assert!(action_problem(&RuleAction::MoveTo { mailbox: 0 }).is_some());
        assert!(action_problem(&RuleAction::MoveTo { mailbox: 3 }).is_none());
        assert!(action_problem(&RuleAction::RedirectTo { addresses: "nobody".into() }).is_some());
        assert!(action_problem(&RuleAction::ForwardTo { addresses: "a@b.org, C <c@d.org>".into(), text: String::new() }).is_none());
        assert!(action_problem(&RuleAction::RunCommand { command: " ".into() }).is_some());
        assert!(action_problem(&RuleAction::MarkAsRead).is_none());
    }

    #[test]
    fn counting_text() {
        assert_eq!(count_text(MatchCount { matched: 0, capped: false }), "No messages match");
        assert_eq!(count_text(MatchCount { matched: 1, capped: false }), "1 message matches");
        assert_eq!(count_text(MatchCount { matched: 23, capped: false }), "23 messages match");
        assert_eq!(count_text(MatchCount { matched: 20000, capped: true }), "At least 20,000 messages match");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn sound_names() {
        assert_eq!(pretty_sound_name("message-new-instant"), "Message New Instant");
        assert_eq!(sound_label(""), "None");
        assert_eq!(sound_label("/home/x/ding_dong.wav"), "Ding Dong");
    }

    #[test]
    fn counts_matches_in_store() {
        let store = Store::open_in_memory().unwrap();
        let inbox = store.ensure_mailbox(LOCAL_ACCOUNT, "Inbox", MailboxRole::Inbox).unwrap();
        for (i, subj) in ["Invoice 1", "Hello", "invoice 2"].iter().enumerate() {
            let m = crate::core::model::MessageSummary { mailbox_id: inbox, account_id: LOCAL_ACCOUNT.into(), uid: i as u32 + 1, subject: subj.to_string(), date: 1, received: 1, ..Default::default() };
            store.insert_message(&m, None, "").unwrap();
        }
        let sm = SmartMailbox { criteria: Criteria { match_all: true, conditions: vec![cond(Field::Subject, Op::Contains, "invoice")] }, ..Default::default() };
        let c = count_matches(&store, &Settings::default(), &sm);
        assert_eq!(c, MatchCount { matched: 2, capped: false });
    }
}
