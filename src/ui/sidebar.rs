//! The mailbox sidebar: favorites, aggregated mailboxes, smart mailboxes and
//! the mailbox tree of every account.

use std::collections::HashSet;

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Ui, Vec2};
use egui_phosphor::regular as ic;

use super::actions::*;
use super::app::{PurpamailApp, role_icon, role_order};
use super::theme::Palette;
use super::widgets;
use crate::core::config::FavoriteItem;
use crate::core::model::*;

const ROW_H: f32 = 26.0;

pub struct RowResult {
    pub clicked: bool,
    pub toggled: bool,
    pub response: egui::Response,
}

impl PurpamailApp {
    fn collapsed(&self, key: &str) -> bool {
        self.settings.read().collapsed_sidebar.iter().any(|k| k == key)
    }

    fn toggle_collapsed(&mut self, key: &str) {
        let mut s = self.settings.write();
        if let Some(i) = s.collapsed_sidebar.iter().position(|k| k == key) {
            s.collapsed_sidebar.remove(i);
        } else {
            s.collapsed_sidebar.push(key.to_string());
        }
    }

    /// Draw one sidebar row. `expand`: Some(expanded) shows a disclosure triangle.
    #[allow(clippy::too_many_arguments)]
    fn side_row(&mut self, ui: &mut Ui, depth: usize, icon: &str, icon_color: Option<Color32>, name: &str, count: u32, selected: bool, expand: Option<bool>, drop_target: Option<MailboxId>) -> RowResult {
        let pal = Palette::of(ui.ctx());
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
        let mut toggled = false;
        let hovered_drop = drop_target.is_some() && resp.dnd_hover_payload::<Vec<MessageId>>().is_some();
        if let (Some(dest), Some(ids)) = (drop_target, resp.dnd_release_payload::<Vec<MessageId>>()) {
            let copy = ui.input(|i| i.modifiers.alt || i.modifiers.ctrl);
            if copy {
                self.actions.push(Action::Copy { ids: (*ids).clone(), dest });
            } else {
                self.actions.push(Action::Move { ids: (*ids).clone(), dest });
            }
        }
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let bg = if selected {
                Some(pal.selection_inactive_bg)
            } else if hovered_drop {
                Some(pal.accent_soft)
            } else if resp.hovered() {
                Some(pal.hover_bg)
            } else {
                None
            };
            if let Some(bg) = bg {
                p.rect_filled(rect.shrink2(Vec2::new(0.0, 1.0)), 6.0, bg);
            }
            if hovered_drop {
                p.rect_stroke(rect.shrink2(Vec2::new(0.0, 1.0)), 6.0, egui::Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
            }
            let mut x = rect.left() + 4.0 + depth as f32 * 14.0;
            if let Some(expanded) = expand {
                let tri = Rect::from_center_size(egui::pos2(x + 6.0, rect.center().y), Vec2::splat(14.0));
                let tri_resp = ui.interact(tri, resp.id.with("tri"), Sense::click());
                if tri_resp.clicked() {
                    toggled = true;
                }
                let p = ui.painter();
                p.text(tri.center(), Align2::CENTER_CENTER, if expanded { ic::CARET_DOWN } else { ic::CARET_RIGHT }, FontId::proportional(10.0), pal.text_tertiary);
            }
            x += 14.0;
            let p = ui.painter();
            let text_col = pal.text;
            p.text(egui::pos2(x + 8.0, rect.center().y), Align2::CENTER_CENTER, icon, FontId::proportional(15.0), icon_color.unwrap_or(pal.accent));
            x += 22.0;
            let count_w = if count > 0 { 34.0 } else { 0.0 };
            let max_w = (rect.right() - 6.0 - count_w - x).max(10.0);
            let galley = p.layout(name.to_string(), FontId::proportional(13.5), text_col, f32::INFINITY);
            let galley = if galley.size().x > max_w {
                let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), FontId::proportional(13.5), text_col);
                job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                ui.painter().layout_job(job)
            } else {
                galley
            };
            let p = ui.painter();
            p.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_col);
            if count > 0 {
                let txt = if count > 9999 { "9999+".to_string() } else { count.to_string() };
                p.text(egui::pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, txt, FontId::new(12.0, super::fonts::bold()), pal.text_secondary);
            }
        }
        RowResult { clicked: resp.clicked(), toggled, response: resp }
    }

    fn section_header(&mut self, ui: &mut Ui, title: &str, key: &str, add: Option<&mut dyn FnMut(&mut Vec<Action>)>) -> bool {
        let pal = Palette::of(ui.ctx());
        let collapsed = self.collapsed(key);
        ui.add_space(6.0);
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 20.0), Sense::click());
        let p = ui.painter();
        p.text(egui::pos2(rect.left() + 6.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::new(11.5, super::fonts::bold()), pal.text_tertiary);
        if resp.hovered() {
            p.text(egui::pos2(rect.right() - 24.0, rect.center().y), Align2::RIGHT_CENTER, if collapsed { "Show" } else { "Hide" }, FontId::proportional(11.0), pal.text_tertiary);
        }
        if let Some(add) = add {
            let r = Rect::from_center_size(egui::pos2(rect.right() - 10.0, rect.center().y), Vec2::splat(16.0));
            let ar = ui.interact(r, resp.id.with("add"), Sense::click());
            ui.painter().text(r.center(), Align2::CENTER_CENTER, ic::PLUS, FontId::proportional(12.0), if ar.hovered() { pal.accent } else { pal.text_tertiary });
            if ar.clicked() {
                add(&mut self.actions);
                return !collapsed;
            }
        }
        if resp.clicked() {
            self.toggle_collapsed(key);
        }
        !collapsed
    }

    pub fn sidebar(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.sidebar_favorites(ui);
            self.sidebar_mailboxes(ui);
            self.sidebar_smart(ui);
            self.sidebar_accounts(ui);
            ui.add_space(20.0);
        });
    }

    fn sidebar_favorites(&mut self, ui: &mut Ui) {
        let favs = self.settings.read().favorites.clone();
        if favs.is_empty() {
            return;
        }
        if !self.section_header(ui, "Favorites", "sec:favorites", None) {
            return;
        }
        for f in favs {
            let sel = super::app::favorite_sel(&f);
            let (icon, name) = self.favorite_label(&f);
            let count = self.sel_unread(&sel);
            let drop = if let FavoriteItem::Mailbox(id) = f { Some(id) } else { None };
            let color = if matches!(f, FavoriteItem::Vips) { Some(Palette::of(ui.ctx()).vip) } else { None };
            let r = self.side_row(ui, 0, icon, color, &name, count, self.sel == sel && !self.search_active, None, drop);
            if r.clicked {
                self.actions.push(Action::SelectSidebar(sel.clone()));
            }
            r.response.context_menu(|ui| {
                if ui.button("Remove from Favorites").clicked() {
                    self.actions.push(Action::RemoveFavorite(f.clone()));
                    ui.close();
                }
            });
        }
    }

    /// Unread count for a sidebar selection.
    fn sel_unread(&self, sel: &SidebarSel) -> u32 {
        match sel {
            SidebarSel::Mailbox(id) => self.mailboxes.iter().find(|m| m.id == *id).map(|m| if matches!(m.role, MailboxRole::Drafts | MailboxRole::Outbox) { m.total } else if matches!(m.role, MailboxRole::Sent | MailboxRole::Trash | MailboxRole::Archive) { 0 } else { m.unread }).unwrap_or(0),
            SidebarSel::AllRole(role) => self
                .mailboxes
                .iter()
                .filter(|m| m.role == *role)
                .map(|m| match role {
                    MailboxRole::Drafts | MailboxRole::Outbox => m.total,
                    MailboxRole::Sent | MailboxRole::Trash | MailboxRole::Archive => 0,
                    _ => m.unread,
                })
                .sum(),
            SidebarSel::Flagged(None) => self.side.flagged_total,
            SidebarSel::Flagged(Some(c)) => self.side.per_color.iter().find(|(x, _)| x == c).map(|(_, n)| *n).unwrap_or(0),
            SidebarSel::Vip(None) => self.side.vip_unread,
            SidebarSel::Vip(Some(e)) => self.side.vips.iter().find(|(_, x, _)| x == e).map(|(_, _, n)| *n).unwrap_or(0),
            SidebarSel::Reminders => self.side.reminders,
            _ => 0,
        }
    }

    fn sidebar_mailboxes(&mut self, ui: &mut Ui) {
        if !self.section_header(ui, "Mailboxes", "sec:mailboxes", None) {
            return;
        }
        let pal = Palette::of(ui.ctx());
        let accounts = self.account_order();
        for role in [MailboxRole::Inbox, MailboxRole::Drafts, MailboxRole::Sent, MailboxRole::Junk, MailboxRole::Trash, MailboxRole::Archive, MailboxRole::Outbox] {
            // VIPs and Flagged sit right after the Inbox.
            if role == MailboxRole::Drafts {
                self.sidebar_vips(ui);
                self.sidebar_flagged(ui);
                if self.side.reminders > 0 {
                    let r = self.side_row(ui, 0, ic::ALARM, Some(pal.flag(FlagColor::Orange)), "Remind Me", self.side.reminders, self.sel == SidebarSel::Reminders, None, None);
                    if r.clicked {
                        self.actions.push(Action::SelectSidebar(SidebarSel::Reminders));
                    }
                }
                if self.settings.read().composing.follow_up_suggestions {
                    let r = self.side_row(ui, 0, ic::ARROW_U_UP_LEFT, Some(pal.text_secondary), "Follow Up", 0, self.sel == SidebarSel::FollowUp, None, None);
                    if r.clicked {
                        self.actions.push(Action::SelectSidebar(SidebarSel::FollowUp));
                    }
                }
            }
            let mbs: Vec<Mailbox> = accounts.iter().filter_map(|(aid, _)| self.mailboxes.iter().find(|m| &m.account_id == aid && m.role == role).cloned()).collect();
            if mbs.is_empty() {
                continue;
            }
            if role == MailboxRole::Outbox && self.side.outbox == 0 {
                continue;
            }
            let sel = SidebarSel::AllRole(role);
            let key = format!("role:{}", role.as_str());
            let expandable = mbs.len() > 1;
            let expanded = expandable && !self.collapsed(&key) && self.settings.read().collapsed_sidebar.iter().any(|k| k == &format!("open:{key}"));
            let count = self.sel_unread(&sel);
            let name = role.display_name();
            let drop = if mbs.len() == 1 { Some(mbs[0].id) } else { None };
            let r = self.side_row(ui, 0, role_icon(role), None, name, count, self.sel == sel && !self.search_active, if expandable { Some(expanded) } else { None }, drop);
            if r.toggled {
                self.toggle_collapsed(&format!("open:{key}"));
            }
            if r.clicked && !r.toggled {
                self.actions.push(Action::SelectSidebar(sel.clone()));
            }
            let fav = match role {
                MailboxRole::Inbox => Some(FavoriteItem::AllInboxes),
                MailboxRole::Drafts => Some(FavoriteItem::AllDrafts),
                MailboxRole::Sent => Some(FavoriteItem::AllSent),
                _ => None,
            };
            let mbs_for_menu = mbs.clone();
            r.response.context_menu(|ui| {
                if role == MailboxRole::Inbox && ui.button("Get New Mail").clicked() {
                    self.actions.push(Action::CheckMail(None));
                    ui.close();
                }
                if ui.button("Mark All as Read").clicked() {
                    self.actions.push(Action::MarkAllRead(mbs_for_menu.iter().map(|m| m.id).collect()));
                    ui.close();
                }
                if role == MailboxRole::Trash && ui.button("Erase Deleted Items").clicked() {
                    self.actions.push(Action::EraseDeleted(None));
                    ui.close();
                }
                if role == MailboxRole::Junk && ui.button("Erase Junk Mail").clicked() {
                    self.actions.push(Action::EraseJunk);
                    ui.close();
                }
                if let Some(f) = &fav {
                    if ui.button("Add to Favorites").clicked() {
                        self.actions.push(Action::AddFavorite(f.clone()));
                        ui.close();
                    }
                }
            });
            if expanded {
                for mb in mbs {
                    let acct = self.account_name(&mb.account_id);
                    let count = self.sel_unread(&SidebarSel::Mailbox(mb.id));
                    let selm = SidebarSel::Mailbox(mb.id);
                    let r = self.side_row(ui, 1, role_icon(role), Some(pal.text_secondary), &acct, count, self.sel == selm && !self.search_active, None, Some(mb.id));
                    if r.clicked {
                        self.actions.push(Action::SelectSidebar(selm));
                    }
                    let id = mb.id;
                    r.response.context_menu(|ui| self.mailbox_context_menu(ui, id));
                }
            }
        }
    }

    fn sidebar_vips(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        if self.vips.is_empty() {
            return;
        }
        let key = "open:vips";
        let expanded = self.settings.read().collapsed_sidebar.iter().any(|k| k == key);
        let r = self.side_row(ui, 0, ic::STAR, Some(pal.vip), "VIPs", self.side.vip_unread, self.sel == SidebarSel::Vip(None), Some(expanded), None);
        if r.toggled {
            self.toggle_collapsed(key);
        } else if r.clicked {
            self.actions.push(Action::SelectSidebar(SidebarSel::Vip(None)));
        }
        if expanded {
            for (name, email, n) in self.side.vips.clone() {
                let sel = SidebarSel::Vip(Some(email.clone()));
                let r = self.side_row(ui, 1, ic::STAR, Some(pal.vip), &name, n, self.sel == sel, None, None);
                if r.clicked {
                    self.actions.push(Action::SelectSidebar(sel));
                }
                r.response.context_menu(|ui| {
                    if ui.button("Remove from VIPs").clicked() {
                        self.actions.push(Action::SetVip { address: Address::new(name.clone(), email.clone()), vip: false });
                        ui.close();
                    }
                });
            }
        }
    }

    fn sidebar_flagged(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        if self.side.flagged_total == 0 && !matches!(self.sel, SidebarSel::Flagged(_)) {
            return;
        }
        let key = "open:flagged";
        let multi = self.side.per_color.len() > 1;
        let expanded = multi && self.settings.read().collapsed_sidebar.iter().any(|k| k == key);
        let r = self.side_row(ui, 0, ic::FLAG, Some(pal.flag(FlagColor::Orange)), "Flagged", self.side.flagged_total, self.sel == SidebarSel::Flagged(None), if multi { Some(expanded) } else { None }, None);
        if r.toggled {
            self.toggle_collapsed(key);
        } else if r.clicked {
            self.actions.push(Action::SelectSidebar(SidebarSel::Flagged(None)));
        }
        r.response.context_menu(|ui| {
            if ui.button("Add to Favorites").clicked() {
                self.actions.push(Action::AddFavorite(FavoriteItem::AllFlagged));
                ui.close();
            }
        });
        if expanded {
            for (c, n) in self.side.per_color.clone() {
                let name = self.settings.read().flag_name(c);
                let sel = SidebarSel::Flagged(Some(c));
                let r = self.side_row(ui, 1, ic::FLAG, Some(pal.flag(c)), &name, n, self.sel == sel, None, None);
                if r.clicked {
                    self.actions.push(Action::SelectSidebar(sel));
                }
            }
        }
    }

    fn sidebar_smart(&mut self, ui: &mut Ui) {
        let smart = self.settings.read().smart_mailboxes.clone();
        let mut add = |a: &mut Vec<Action>| a.push(Action::NewSmartMailbox(None));
        if !self.section_header(ui, "Smart Mailboxes", "sec:smart", Some(&mut add)) {
            return;
        }
        let pal = Palette::of(ui.ctx());
        for sm in smart {
            let sel = SidebarSel::Smart(sm.id.clone());
            let r = self.side_row(ui, 0, ic::GEAR_SIX, Some(pal.text_secondary), &sm.name, 0, self.sel == sel && !self.search_active, None, None);
            if r.clicked {
                self.actions.push(Action::SelectSidebar(sel));
            }
            if r.response.double_clicked() {
                self.actions.push(Action::EditSmartMailbox(sm.id.clone()));
            }
            r.response.context_menu(|ui| {
                if ui.button("Edit Smart Mailbox…").clicked() {
                    self.actions.push(Action::EditSmartMailbox(sm.id.clone()));
                    ui.close();
                }
                if ui.button("Duplicate Smart Mailbox").clicked() {
                    let mut d = sm.clone();
                    d.id = uuid::Uuid::new_v4().to_string();
                    d.name = format!("{} copy", sm.name);
                    self.actions.push(Action::NewSmartMailbox(Some(d)));
                    ui.close();
                }
                if ui.button("Add to Favorites").clicked() {
                    self.actions.push(Action::AddFavorite(FavoriteItem::Smart(sm.id.clone())));
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete Smart Mailbox").clicked() {
                    self.actions.push(Action::DeleteSmartMailbox(sm.id.clone()));
                    ui.close();
                }
            });
        }
    }

    /// Configured accounts in order, plus the local "On This Computer" pseudo account.
    pub fn account_order(&self) -> Vec<(AccountId, String)> {
        let s = self.settings.read();
        let mut v: Vec<(AccountId, String)> = s.accounts.iter().filter(|a| a.enabled).map(|a| (a.id.clone(), a.display_name().to_string())).collect();
        v.push((LOCAL_ACCOUNT.to_string(), "On This Computer".to_string()));
        v
    }

    fn sidebar_accounts(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        for (aid, name) in self.account_order() {
            let all: Vec<Mailbox> = self.mailboxes.iter().filter(|m| m.account_id == aid).cloned().collect();
            let custom: Vec<Mailbox> = all.iter().filter(|m| m.role == MailboxRole::Custom).cloned().collect();
            if aid == LOCAL_ACCOUNT && custom.is_empty() {
                continue;
            }
            let key = format!("acct:{aid}");
            let mut add = |a: &mut Vec<Action>| a.push(Action::NewMailbox { parent: None, account: aid.clone() });
            let status = self.account_status.get(&aid).cloned();
            let title = match &status {
                Some((false, Some(_))) => format!("{name}  {}", ic::WARNING),
                Some((false, None)) if aid != LOCAL_ACCOUNT => format!("{name}  {}", ic::WIFI_SLASH),
                _ => name.clone(),
            };
            let open = self.section_header(ui, &title, &key, Some(&mut add));
            if !open {
                continue;
            }
            // tree of custom mailboxes; roles live in "Mailboxes" above but nested children of role mailboxes appear here
            let ids: HashSet<MailboxId> = all.iter().map(|m| m.id).collect();
            let role_ids: HashSet<MailboxId> = all.iter().filter(|m| m.role != MailboxRole::Custom).map(|m| m.id).collect();
            let nodes: Vec<Mailbox> = custom
                .iter()
                .map(|m| {
                    let mut m = m.clone();
                    if m.parent.is_some_and(|p| !ids.contains(&p) || role_ids.contains(&p)) {
                        m.parent = None;
                    }
                    m
                })
                .collect();
            self.mailbox_tree(ui, &nodes, None, 0, &pal);
            if nodes.is_empty() {
                ui.label(RichText::new("    No other mailboxes").small().color(pal.text_tertiary));
            }
        }
    }

    fn mailbox_tree(&mut self, ui: &mut Ui, nodes: &[Mailbox], parent: Option<MailboxId>, depth: usize, pal: &Palette) {
        let mut kids: Vec<&Mailbox> = nodes.iter().filter(|m| m.parent == parent).collect();
        kids.sort_by_key(|m| (role_order(m.role), m.name.to_lowercase()));
        for mb in kids {
            let has_kids = nodes.iter().any(|n| n.parent == Some(mb.id));
            let key = format!("mb:{}", mb.id);
            let expanded = has_kids && !self.collapsed(&key);
            let sel = SidebarSel::Mailbox(mb.id);
            let r = self.side_row(ui, depth, if has_kids { ic::FOLDERS } else { ic::FOLDER_SIMPLE }, Some(pal.accent), &mb.name, mb.unread, self.sel == sel && !self.search_active, if has_kids { Some(expanded) } else { None }, if mb.selectable { Some(mb.id) } else { None });
            if r.toggled {
                self.toggle_collapsed(&key);
            } else if r.clicked && mb.selectable {
                self.actions.push(Action::SelectSidebar(sel));
            }
            let id = mb.id;
            r.response.context_menu(|ui| self.mailbox_context_menu(ui, id));
            if expanded {
                self.mailbox_tree(ui, nodes, Some(mb.id), depth + 1, pal);
            }
        }
    }

    pub fn mailbox_context_menu(&mut self, ui: &mut Ui, id: MailboxId) {
        let Some(mb) = self.mailboxes.iter().find(|m| m.id == id).cloned() else { return };
        if !mb.is_local() && ui.button("Get New Mail").clicked() {
            self.actions.push(Action::CheckMail(Some(mb.account_id.clone())));
            ui.close();
        }
        if ui.button("Mark All Messages as Read").clicked() {
            self.actions.push(Action::MarkAllRead(vec![id]));
            ui.close();
        }
        ui.separator();
        if ui.button("New Mailbox…").clicked() {
            self.actions.push(Action::NewMailbox { parent: Some(id), account: mb.account_id.clone() });
            ui.close();
        }
        if mb.role == MailboxRole::Custom {
            if ui.button("Rename Mailbox…").clicked() {
                self.actions.push(Action::RenameMailbox(id));
                ui.close();
            }
            if ui.button("Delete Mailbox…").clicked() {
                self.actions.push(Action::DeleteMailbox(id));
                ui.close();
            }
        }
        if ui.button("Export Mailbox…").clicked() {
            self.actions.push(Action::ExportMailbox(id));
            ui.close();
        }
        if !mb.is_local() && ui.button("Rebuild").clicked() {
            self.actions.push(Action::RebuildMailbox(id));
            ui.close();
        }
        ui.menu_button("Use This Mailbox For", |ui| {
            for role in [MailboxRole::Drafts, MailboxRole::Sent, MailboxRole::Junk, MailboxRole::Trash, MailboxRole::Archive] {
                if ui.radio(mb.role == role, role.display_name()).clicked() {
                    self.actions.push(Action::UseMailboxFor { mailbox: id, role });
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button("Add to Favorites").clicked() {
            self.actions.push(Action::AddFavorite(FavoriteItem::Mailbox(id)));
            ui.close();
        }
        if ui.button("New Smart Mailbox from This Mailbox…").clicked() {
            use crate::core::criteria::{Condition, Criteria, Field, Op};
            let sm = crate::core::config::SmartMailbox {
                name: format!("{} (Smart)", mb.name),
                criteria: Criteria { match_all: true, conditions: vec![Condition { field: Field::InMailbox, op: Op::Is, value: id.to_string(), header: String::new() }, Condition { field: Field::IsUnread, ..Default::default() }] },
                ..Default::default()
            };
            self.actions.push(Action::NewSmartMailbox(Some(sm)));
            ui.close();
        }
        if !mb.is_local() {
            ui.separator();
            if ui.button("Account Settings…").clicked() {
                self.actions.push(Action::ShowPreferences(Some(PrefsTab::Accounts)));
                ui.close();
            }
        }
        let _ = widgets::format_list_date;
    }
}
