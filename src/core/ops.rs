//! High level mail operations.
//!
//! These update the local store immediately and queue the matching
//! [`RemoteOp`]s so the sync engine can replay them against the server.
//! After calling any of these, send `Command::FlushOps` for the affected
//! accounts (the returned list) so changes reach the server promptly.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};

use super::compose::OutgoingMessage;
use super::config::Settings;
use super::model::*;
use super::store::{RemoteOp, Store};

pub type Accounts = BTreeSet<AccountId>;

fn group_by_mailbox(msgs: &[MessageSummary]) -> BTreeMap<MailboxId, Vec<&MessageSummary>> {
    let mut m: BTreeMap<MailboxId, Vec<&MessageSummary>> = BTreeMap::new();
    for x in msgs {
        m.entry(x.mailbox_id).or_default().push(x);
    }
    m
}

fn uids(v: &[&MessageSummary]) -> Vec<u32> {
    v.iter().filter(|m| m.uid != 0).map(|m| m.uid).collect()
}

/// Find a mailbox with `role` for the account, creating it (locally and on the server) if needed.
pub fn role_mailbox(store: &Store, account: &str, role: MailboxRole) -> Result<Mailbox> {
    if let Some(mb) = store.mailbox_by_role(account, role) {
        return Ok(mb);
    }
    let path = role.display_name().to_string();
    let id = store.ensure_mailbox(account, &path, role)?;
    store.set_mailbox_role(id, role)?;
    if account != LOCAL_ACCOUNT && role != MailboxRole::Outbox {
        store.push_op(account, &RemoteOp::CreateMailbox { path: path.clone() })?;
    }
    store.mailbox(id).ok_or_else(|| anyhow!("mailbox vanished"))
}

/// The local Outbox mailbox.
pub fn outbox(store: &Store) -> Result<Mailbox> {
    role_mailbox(store, LOCAL_ACCOUNT, MailboxRole::Outbox)
}

pub fn set_flags(store: &Store, ids: &[MessageId], add: u32, remove: u32) -> Result<Accounts> {
    let msgs = store.messages_by_ids(ids);
    store.update_flags(ids, add, remove)?;
    let mut accts = Accounts::new();
    for (mb, list) in group_by_mailbox(&msgs) {
        let u = uids(&list);
        let acct = &list[0].account_id;
        if !u.is_empty() && acct != LOCAL_ACCOUNT {
            store.push_op(acct, &RemoteOp::SetFlags { mailbox: mb, uids: u, add, remove })?;
            accts.insert(acct.clone());
        }
    }
    Ok(accts)
}

/// Flag messages with a color (or clear the flag with `None`).
pub fn set_flag_color(store: &Store, ids: &[MessageId], color: Option<FlagColor>) -> Result<Accounts> {
    let mut accts = match color {
        Some(_) => set_flags(store, ids, flags::FLAGGED, 0)?,
        None => set_flags(store, ids, 0, flags::FLAGGED)?,
    };
    let msgs = store.messages_by_ids(ids);
    store.set_color(ids, color)?;
    for (mb, list) in group_by_mailbox(&msgs) {
        let u = uids(&list);
        let acct = &list[0].account_id;
        if !u.is_empty() && acct != LOCAL_ACCOUNT {
            store.push_op(acct, &RemoteOp::SetColor { mailbox: mb, uids: u, color })?;
            accts.insert(acct.clone());
        }
    }
    Ok(accts)
}

pub fn toggle_read(store: &Store, ids: &[MessageId]) -> Result<Accounts> {
    let msgs = store.messages_by_ids(ids);
    let any_unread = msgs.iter().any(|m| m.is_unread());
    if any_unread { set_flags(store, ids, flags::SEEN, 0) } else { set_flags(store, ids, 0, flags::SEEN) }
}

pub fn move_to(store: &Store, ids: &[MessageId], dest: MailboxId) -> Result<Accounts> {
    let dest_mb = store.mailbox(dest).ok_or_else(|| anyhow!("no such mailbox"))?;
    let msgs = store.messages_by_ids(ids);
    let mut accts = Accounts::new();
    for (src, list) in group_by_mailbox(&msgs) {
        if src == dest {
            continue;
        }
        let src_acct = list[0].account_id.clone();
        let ids: Vec<MessageId> = list.iter().map(|m| m.id).collect();
        let u = uids(&list);
        if src_acct == dest_mb.account_id {
            if src_acct != LOCAL_ACCOUNT && !u.is_empty() {
                store.push_op(&src_acct, &RemoteOp::Move { from: src, to: dest, uids: u })?;
                accts.insert(src_acct.clone());
            }
        } else {
            if src_acct != LOCAL_ACCOUNT && !u.is_empty() {
                store.push_op(&src_acct, &RemoteOp::Delete { mailbox: src, uids: u })?;
                accts.insert(src_acct.clone());
            }
            if dest_mb.account_id != LOCAL_ACCOUNT {
                for id in &ids {
                    store.push_op(&dest_mb.account_id, &RemoteOp::Append { mailbox: dest, message: *id })?;
                }
                accts.insert(dest_mb.account_id.clone());
            }
        }
        store.move_messages(&ids, dest)?;
    }
    Ok(accts)
}

pub fn copy_to(store: &Store, ids: &[MessageId], dest: MailboxId) -> Result<Accounts> {
    let dest_mb = store.mailbox(dest).ok_or_else(|| anyhow!("no such mailbox"))?;
    let msgs = store.messages_by_ids(ids);
    let mut accts = Accounts::new();
    for (src, list) in group_by_mailbox(&msgs) {
        let src_acct = list[0].account_id.clone();
        let ids: Vec<MessageId> = list.iter().map(|m| m.id).collect();
        let u = uids(&list);
        let new_ids = store.copy_messages(&ids, dest)?;
        if dest_mb.account_id == LOCAL_ACCOUNT {
            continue;
        }
        if src_acct == dest_mb.account_id && !u.is_empty() {
            store.push_op(&src_acct, &RemoteOp::Copy { from: src, to: dest, uids: u })?;
        } else {
            for id in new_ids {
                store.push_op(&dest_mb.account_id, &RemoteOp::Append { mailbox: dest, message: id })?;
            }
        }
        accts.insert(dest_mb.account_id.clone());
    }
    Ok(accts)
}

/// Delete messages: move to the account's Trash, or erase if already in Trash
/// (or the account is configured not to use a Trash mailbox).
pub fn delete(store: &Store, settings: &Settings, ids: &[MessageId]) -> Result<Accounts> {
    if settings.general.archive_on_delete {
        return archive(store, settings, ids);
    }
    let msgs = store.messages_by_ids(ids);
    let mut accts = Accounts::new();
    let mut by_account: BTreeMap<AccountId, Vec<&MessageSummary>> = BTreeMap::new();
    for m in &msgs {
        by_account.entry(m.account_id.clone()).or_default().push(m);
    }
    for (acct, list) in by_account {
        let use_trash = settings.account(&acct).map(|a| a.move_deleted_to_trash).unwrap_or(true);
        let trash = if use_trash { Some(role_mailbox(store, &acct, MailboxRole::Trash)?) } else { None };
        let (erase, to_trash): (Vec<&MessageSummary>, Vec<&MessageSummary>) =
            list.into_iter().partition(|m| trash.as_ref().map(|t| t.id == m.mailbox_id).unwrap_or(true));
        if !erase.is_empty() {
            accts.extend(erase_messages(store, &erase.iter().map(|m| m.id).collect::<Vec<_>>())?);
        }
        if let (Some(t), false) = (&trash, to_trash.is_empty()) {
            let ids: Vec<MessageId> = to_trash.iter().map(|m| m.id).collect();
            store.update_flags(&ids, flags::SEEN, 0)?;
            accts.extend(move_to(store, &ids, t.id)?);
        }
    }
    Ok(accts)
}

/// Permanently remove messages locally and on the server.
pub fn erase_messages(store: &Store, ids: &[MessageId]) -> Result<Accounts> {
    let msgs = store.messages_by_ids(ids);
    let mut accts = Accounts::new();
    for (mb, list) in group_by_mailbox(&msgs) {
        let u = uids(&list);
        let acct = &list[0].account_id;
        if !u.is_empty() && acct != LOCAL_ACCOUNT {
            store.push_op(acct, &RemoteOp::Delete { mailbox: mb, uids: u })?;
            accts.insert(acct.clone());
        }
    }
    store.delete_messages(ids)?;
    Ok(accts)
}

pub fn archive(store: &Store, settings: &Settings, ids: &[MessageId]) -> Result<Accounts> {
    let msgs = store.messages_by_ids(ids);
    let mut accts = Accounts::new();
    let mut by_account: BTreeMap<AccountId, Vec<MessageId>> = BTreeMap::new();
    for m in &msgs {
        by_account.entry(m.account_id.clone()).or_default().push(m.id);
    }
    for (acct, ids) in by_account {
        let custom = settings.account(&acct).map(|a| a.archive_mailbox_path.clone()).unwrap_or_default();
        let dest = if !custom.is_empty() {
            match store.mailbox_by_path(&acct, &custom) {
                Some(m) => m,
                None => role_mailbox(store, &acct, MailboxRole::Archive)?,
            }
        } else {
            role_mailbox(store, &acct, MailboxRole::Archive)?
        };
        accts.extend(move_to(store, &ids, dest.id)?);
    }
    Ok(accts)
}

/// Mark as junk (moving to Junk) or not junk (moving back to the Inbox).
pub fn mark_junk(store: &Store, settings: &Settings, ids: &[MessageId], junk: bool) -> Result<Accounts> {
    let msgs = store.messages_by_ids(ids);
    for m in &msgs {
        if let Some(raw) = store.raw(m.id) {
            if let Some(p) = super::mime::parse(&raw) {
                super::junk::train(store, &p, junk);
            }
        }
    }
    let mut accts = if junk { set_flags(store, ids, flags::JUNK, flags::NOT_JUNK)? } else { set_flags(store, ids, flags::NOT_JUNK, flags::JUNK)? };
    let move_junk = settings.junk.action == super::config::JunkAction::MoveToJunk;
    let mut by_account: BTreeMap<AccountId, Vec<&MessageSummary>> = BTreeMap::new();
    for m in &msgs {
        by_account.entry(m.account_id.clone()).or_default().push(m);
    }
    for (acct, list) in by_account {
        if junk && move_junk {
            let jm = role_mailbox(store, &acct, MailboxRole::Junk)?;
            let ids: Vec<MessageId> = list.iter().filter(|m| m.mailbox_id != jm.id).map(|m| m.id).collect();
            accts.extend(move_to(store, &ids, jm.id)?);
        } else if !junk {
            if let Some(jm) = store.mailbox_by_role(&acct, MailboxRole::Junk) {
                let ids: Vec<MessageId> = list.iter().filter(|m| m.mailbox_id == jm.id).map(|m| m.id).collect();
                if !ids.is_empty() {
                    let inbox = role_mailbox(store, &acct, MailboxRole::Inbox)?;
                    accts.extend(move_to(store, &ids, inbox.id)?);
                }
            }
        }
    }
    Ok(accts)
}

/// Store a draft. Replaces `previous` (an earlier save of the same draft) if given.
pub fn save_draft(store: &Store, settings: &Settings, msg: &OutgoingMessage, previous: Option<MessageId>) -> Result<(MessageId, Accounts)> {
    let mut accts = Accounts::new();
    if let Some(p) = previous {
        accts.extend(erase_messages(store, &[p])?);
    }
    let acct = settings.account(&msg.account_id);
    let on_server = acct.map(|a| a.store_drafts_on_server).unwrap_or(false);
    let owner = if on_server { msg.account_id.as_str() } else { LOCAL_ACCOUNT };
    let drafts = role_mailbox(store, owner, MailboxRole::Drafts)?;
    let raw = msg.to_mime(true);
    let mut s = msg.summary();
    s.mailbox_id = drafts.id;
    s.account_id = drafts.account_id.clone();
    s.flags = flags::SEEN | flags::DRAFT;
    s.size = raw.len() as u32;
    let id = store.insert_message(&s, Some(&raw), &msg.text)?;
    if owner != LOCAL_ACCOUNT {
        store.push_op(owner, &RemoteOp::Append { mailbox: drafts.id, message: id })?;
        accts.insert(owner.to_string());
    }
    Ok((id, accts))
}

/// Put a message into the local Outbox. `send_at` schedules it for later.
pub fn queue_outgoing(store: &Store, msg: &OutgoingMessage, send_at: Option<i64>) -> Result<MessageId> {
    let ob = outbox(store)?;
    let raw = msg.to_mime(true);
    let mut s = msg.summary();
    s.mailbox_id = ob.id;
    s.account_id = LOCAL_ACCOUNT.to_string();
    s.flags = flags::SEEN;
    s.size = raw.len() as u32;
    s.send_at = send_at;
    let id = store.insert_message(&s, Some(&raw), &msg.text)?;
    Ok(id)
}

/// Bookkeeping after successful delivery: move from Outbox to Sent and mark the
/// replied / forwarded original. Returns the id of the Sent copy.
pub fn after_sent(store: &Store, settings: &Settings, outbox_id: MessageId, msg: &OutgoingMessage) -> Result<(Option<MessageId>, Accounts)> {
    let mut accts = Accounts::new();
    for a in msg.all_recipients() {
        store.record_recipient(&a)?;
    }
    // Mark original as answered / forwarded.
    if !msg.in_reply_to.is_empty() {
        let originals = store.list_messages(&super::store::MessageQuery { fts: None, ..Default::default() });
        let fwd = msg.subject.to_lowercase().starts_with("fwd:") || msg.subject.to_lowercase().starts_with("fw:");
        let ids: Vec<MessageId> = originals.iter().filter(|m| m.message_id == msg.in_reply_to).map(|m| m.id).collect();
        if !ids.is_empty() {
            accts.extend(set_flags(store, &ids, if fwd { flags::FORWARDED } else { flags::ANSWERED }, 0)?);
        }
    }
    let raw = store.raw(outbox_id).unwrap_or_else(|| msg.to_mime(true));
    store.delete_messages(&[outbox_id])?;
    if msg.redirect_raw.is_some() {
        return Ok((None, accts));
    }
    let acct = settings.account(&msg.account_id);
    let on_server = acct.map(|a| a.store_sent_on_server).unwrap_or(false);
    let owner = if on_server { msg.account_id.as_str() } else { LOCAL_ACCOUNT };
    let sent = role_mailbox(store, owner, MailboxRole::Sent)?;
    let mut s = msg.summary();
    s.mailbox_id = sent.id;
    s.account_id = sent.account_id.clone();
    s.flags = flags::SEEN;
    s.size = raw.len() as u32;
    let id = store.insert_message(&s, Some(&raw), &msg.text)?;
    if owner != LOCAL_ACCOUNT {
        store.push_op(owner, &RemoteOp::Append { mailbox: sent.id, message: id })?;
        accts.insert(owner.to_string());
    }
    Ok((Some(id), accts))
}

/// Erase all messages in the Trash of an account (or every account with `None`).
pub fn erase_deleted(store: &Store, account: Option<&str>) -> Result<Accounts> {
    let mut accts = Accounts::new();
    for mb in store.mailboxes(account) {
        if mb.role == MailboxRole::Trash {
            let ids = store.ids_in_mailbox(mb.id);
            accts.extend(erase_messages(store, &ids)?);
        } else {
            // messages flagged \Deleted (IMAP model)
            let ids = store.deleted_in(mb.id);
            if !ids.is_empty() {
                accts.extend(erase_messages(store, &ids)?);
            }
        }
    }
    Ok(accts)
}

pub fn erase_junk(store: &Store, account: Option<&str>) -> Result<Accounts> {
    let mut accts = Accounts::new();
    for mb in store.mailboxes(account) {
        if mb.role == MailboxRole::Junk {
            let ids = store.ids_in_mailbox(mb.id);
            accts.extend(erase_messages(store, &ids)?);
        }
    }
    Ok(accts)
}

/// Import raw messages into a mailbox (used by mbox import and "Open Message" + drag & drop).
pub fn import_raw(store: &Store, mailbox: MailboxId, raws: &[Vec<u8>]) -> Result<Vec<MessageId>> {
    let mb = store.mailbox(mailbox).ok_or_else(|| anyhow!("no such mailbox"))?;
    let mut out = Vec::new();
    for raw in raws {
        let Some(p) = super::mime::parse(raw) else { continue };
        let mut s = super::mime::summarize(&p, raw.len());
        s.mailbox_id = mailbox;
        s.account_id = mb.account_id.clone();
        s.flags |= flags::SEEN;
        s.category = super::categorize::categorize(&p);
        let id = store.insert_message(&s, Some(raw), &p.plain_text())?;
        if mb.account_id != LOCAL_ACCOUNT {
            store.push_op(&mb.account_id, &RemoteOp::Append { mailbox, message: id })?;
        }
        out.push(id);
    }
    Ok(out)
}

/// Remove attachments from stored messages (local copy only; IMAP has no in-place edit,
/// so for server mailboxes the stripped message is re-appended and the original deleted).
pub fn remove_attachments(store: &Store, ids: &[MessageId]) -> Result<Accounts> {
    let mut accts = Accounts::new();
    for m in store.messages_by_ids(ids) {
        let Some(raw) = store.raw(m.id) else { continue };
        let Some(stripped) = super::compose::strip_attachments(&raw) else { continue };
        store.replace_raw(m.id, &stripped, stripped.len() as u32, false)?;
        if m.account_id != LOCAL_ACCOUNT && m.uid != 0 {
            store.push_op(&m.account_id, &RemoteOp::Delete { mailbox: m.mailbox_id, uids: vec![m.uid] })?;
            store.push_op(&m.account_id, &RemoteOp::Append { mailbox: m.mailbox_id, message: m.id })?;
            store.set_uid(m.id, 0)?;
            accts.insert(m.account_id.clone());
        }
    }
    Ok(accts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AccountConfig;

    #[test]
    fn delete_moves_to_trash_then_erases() {
        let store = Store::open_in_memory().unwrap();
        let mut settings = Settings::default();
        settings.accounts.push(AccountConfig { id: "a".into(), ..Default::default() });
        let inbox = store.ensure_mailbox("a", "INBOX", MailboxRole::Inbox).unwrap();
        let m = MessageSummary { mailbox_id: inbox, account_id: "a".into(), uid: 7, subject: "x".into(), ..Default::default() };
        let id = store.insert_message(&m, None, "").unwrap();
        delete(&store, &settings, &[id]).unwrap();
        let moved = store.message(id).unwrap();
        let trash = store.mailbox_by_role("a", MailboxRole::Trash).unwrap();
        assert_eq!(moved.mailbox_id, trash.id);
        let ops = store.pending_ops("a");
        assert!(ops.iter().any(|(_, o, _)| matches!(o, RemoteOp::CreateMailbox { .. })));
        assert!(ops.iter().any(|(_, o, _)| matches!(o, RemoteOp::Move { uids, .. } if uids == &vec![7])));
        delete(&store, &settings, &[id]).unwrap();
        assert!(store.message(id).is_none());
    }

    #[test]
    fn outbox_to_sent() {
        let store = Store::open_in_memory().unwrap();
        let mut settings = Settings::default();
        settings.accounts.push(AccountConfig { id: "a".into(), emails: vec!["me@x.com".into()], ..Default::default() });
        let msg = OutgoingMessage {
            account_id: "a".into(),
            from: Address::new("Me", "me@x.com"),
            to: vec![Address::new("You", "you@x.com")],
            subject: "Hi".into(),
            text: "Body".into(),
            message_id: "m1@x.com".into(),
            ..Default::default()
        };
        let ob = queue_outgoing(&store, &msg, None).unwrap();
        let (sent, accts) = after_sent(&store, &settings, ob, &msg).unwrap();
        assert!(store.message(ob).is_none());
        let s = store.message(sent.unwrap()).unwrap();
        assert_eq!(store.mailbox(s.mailbox_id).unwrap().role, MailboxRole::Sent);
        assert!(accts.contains("a"));
        assert_eq!(store.previous_recipients().len(), 1);
    }
}
