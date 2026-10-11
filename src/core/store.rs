//! Local mail store backed by SQLite.
//!
//! Every message known to Purpamail lives here. The networking layer keeps it
//! in sync with servers; the UI only ever reads from and writes to the store.
//! Changes that must be propagated to a server are queued as [`RemoteOp`]s.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::model::*;

pub type SharedStore = Arc<Store>;

pub struct Store {
    conn: Mutex<Connection>,
    pub path: PathBuf,
}

/// An operation that must be replayed against a server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RemoteOp {
    /// Add / remove flag bits (see [`flags`]) for server UIDs.
    SetFlags { mailbox: MailboxId, uids: Vec<u32>, add: u32, remove: u32 },
    /// Set (or clear) the flag color keywords.
    SetColor { mailbox: MailboxId, uids: Vec<u32>, color: Option<FlagColor> },
    /// Move messages between two mailboxes of the same account.
    Move { from: MailboxId, to: MailboxId, uids: Vec<u32> },
    /// Copy messages between two mailboxes of the same account.
    Copy { from: MailboxId, to: MailboxId, uids: Vec<u32> },
    /// Permanently remove messages from the server (flag \Deleted + expunge).
    Delete { mailbox: MailboxId, uids: Vec<u32> },
    /// Upload a locally stored message (by local id) to the server mailbox.
    Append { mailbox: MailboxId, message: MessageId },
    /// Expunge all messages flagged \Deleted.
    Expunge { mailbox: MailboxId },
    CreateMailbox { path: String },
    RenameMailbox { from: String, to: String },
    DeleteMailbox { path: String },
}

#[derive(Clone, Debug, Default)]
pub struct MessageQuery {
    /// Restrict to these mailboxes; empty means all mailboxes.
    pub mailboxes: Vec<MailboxId>,
    /// Restrict to these accounts; empty means all.
    pub accounts: Vec<AccountId>,
    /// Exclude messages in these mailboxes (e.g. trash / junk for global searches).
    pub exclude_mailboxes: Vec<MailboxId>,
    /// Only messages whose sender is one of these (lowercase) addresses.
    pub from_any: Vec<String>,
    pub only_flagged: bool,
    pub only_color: Option<FlagColor>,
    pub only_unread: bool,
    pub only_drafts: bool,
    /// FTS5 MATCH expression.
    pub fts: Option<String>,
    pub filter: ListFilter,
    /// Addresses of the user (for To:/Cc: me filters).
    pub my_addresses: Vec<String>,
    pub vip_addresses: Vec<String>,
    pub include_deleted: bool,
    pub sort: SortKey,
    pub ascending: bool,
    pub limit: Option<usize>,
    /// Only messages with a reminder that is due.
    pub reminders_due: bool,
    /// Only messages in a specific thread.
    pub thread: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Contact {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub nickname: String,
    pub organization: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub notes: String,
    pub vip: bool,
    pub photo: Option<Vec<u8>>,
}

impl Contact {
    pub fn display_name(&self) -> String {
        let n = format!("{} {}", self.first_name, self.last_name).trim().to_string();
        if !n.is_empty() {
            n
        } else if !self.organization.is_empty() {
            self.organization.clone()
        } else {
            self.emails.first().cloned().unwrap_or_default()
        }
    }
    pub fn primary_email(&self) -> String {
        self.emails.first().cloned().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviousRecipient {
    pub email: String,
    pub name: String,
    pub last_used: i64,
    pub count: i64,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS mailboxes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    parent INTEGER,
    role TEXT NOT NULL DEFAULT 'custom',
    delimiter TEXT NOT NULL DEFAULT '/',
    uidvalidity INTEGER NOT NULL DEFAULT 0,
    uidnext INTEGER NOT NULL DEFAULT 0,
    selectable INTEGER NOT NULL DEFAULT 1,
    subscribed INTEGER NOT NULL DEFAULT 1,
    UNIQUE(account_id, path)
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mailbox_id INTEGER NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL,
    uid INTEGER NOT NULL DEFAULT 0,
    message_id TEXT NOT NULL DEFAULT '',
    in_reply_to TEXT NOT NULL DEFAULT '',
    refs TEXT NOT NULL DEFAULT '[]',
    thread_id INTEGER NOT NULL DEFAULT 0,
    subject TEXT NOT NULL DEFAULT '',
    norm_subject TEXT NOT NULL DEFAULT '',
    from_name TEXT NOT NULL DEFAULT '',
    from_addr TEXT NOT NULL DEFAULT '',
    to_json TEXT NOT NULL DEFAULT '[]',
    cc_json TEXT NOT NULL DEFAULT '[]',
    bcc_json TEXT NOT NULL DEFAULT '[]',
    reply_to_json TEXT NOT NULL DEFAULT '[]',
    to_sort TEXT NOT NULL DEFAULT '',
    date INTEGER NOT NULL DEFAULT 0,
    received INTEGER NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0,
    flags INTEGER NOT NULL DEFAULT 0,
    color INTEGER,
    has_attachments INTEGER NOT NULL DEFAULT 0,
    snippet TEXT NOT NULL DEFAULT '',
    priority INTEGER NOT NULL DEFAULT 0,
    category INTEGER NOT NULL DEFAULT 0,
    remind_at INTEGER,
    list_unsubscribe TEXT NOT NULL DEFAULT '',
    send_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_msg_mailbox ON messages(mailbox_id, date);
CREATE INDEX IF NOT EXISTS idx_msg_uid ON messages(mailbox_id, uid);
CREATE INDEX IF NOT EXISTS idx_msg_msgid ON messages(message_id);
CREATE INDEX IF NOT EXISTS idx_msg_thread ON messages(thread_id);
CREATE INDEX IF NOT EXISTS idx_msg_from ON messages(from_addr);
CREATE INDEX IF NOT EXISTS idx_msg_irt ON messages(in_reply_to);
CREATE INDEX IF NOT EXISTS idx_msg_subject ON messages(account_id, norm_subject);
CREATE TABLE IF NOT EXISTS msg_refs (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ref TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_refs_ref ON msg_refs(ref);
CREATE INDEX IF NOT EXISTS idx_refs_msg ON msg_refs(message_id);
CREATE TABLE IF NOT EXISTS bodies (
    message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    raw BLOB NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(subject, sender, recipients, body, tokenize='unicode61 remove_diacritics 2');
CREATE TABLE IF NOT EXISTS pending_ops (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    op TEXT NOT NULL,
    created INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS contacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    first_name TEXT NOT NULL DEFAULT '',
    last_name TEXT NOT NULL DEFAULT '',
    nickname TEXT NOT NULL DEFAULT '',
    organization TEXT NOT NULL DEFAULT '',
    emails TEXT NOT NULL DEFAULT '[]',
    phones TEXT NOT NULL DEFAULT '[]',
    notes TEXT NOT NULL DEFAULT '',
    vip INTEGER NOT NULL DEFAULT 0,
    photo BLOB
);
CREATE TABLE IF NOT EXISTS previous_recipients (
    email TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    last_used INTEGER NOT NULL,
    count INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS blocked_senders (email TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS muted_threads (thread_id INTEGER PRIMARY KEY);
CREATE TABLE IF NOT EXISTS junk_tokens (token TEXT PRIMARY KEY, spam INTEGER NOT NULL DEFAULT 0, ham INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

const MSG_COLS: &str = "m.id, m.mailbox_id, m.account_id, m.uid, m.message_id, m.in_reply_to, m.refs, m.thread_id, m.subject, m.from_name, m.from_addr, m.to_json, m.cc_json, m.bcc_json, m.reply_to_json, m.date, m.received, m.size, m.flags, m.color, m.has_attachments, m.snippet, m.priority, m.category, m.remind_at, m.list_unsubscribe, m.send_at";

fn row_to_summary(r: &Row) -> rusqlite::Result<MessageSummary> {
    let js = |i: usize| -> rusqlite::Result<Vec<Address>> {
        let s: String = r.get(i)?;
        Ok(serde_json::from_str(&s).unwrap_or_default())
    };
    let refs: String = r.get(6)?;
    let color: Option<i64> = r.get(19)?;
    Ok(MessageSummary {
        id: r.get(0)?,
        mailbox_id: r.get(1)?,
        account_id: r.get(2)?,
        uid: r.get::<_, i64>(3)? as u32,
        message_id: r.get(4)?,
        in_reply_to: r.get(5)?,
        references: serde_json::from_str(&refs).unwrap_or_default(),
        thread_id: r.get(7)?,
        subject: r.get(8)?,
        from: Address { name: r.get(9)?, email: r.get(10)? },
        to: js(11)?,
        cc: js(12)?,
        bcc: js(13)?,
        reply_to: js(14)?,
        date: r.get(15)?,
        received: r.get(16)?,
        size: r.get::<_, i64>(17)? as u32,
        flags: r.get::<_, i64>(18)? as u32,
        color: color.map(|c| FlagColor::from_u8(c as u8)),
        has_attachments: r.get::<_, i64>(20)? != 0,
        snippet: r.get(21)?,
        priority: Priority::from_i32(r.get(22)?),
        category: Category::from_i32(r.get(23)?),
        remind_at: r.get(24)?,
        list_unsubscribe: r.get(25)?,
        send_at: r.get(26)?,
    })
}

fn row_to_mailbox(r: &Row) -> rusqlite::Result<Mailbox> {
    let role: String = r.get(5)?;
    Ok(Mailbox {
        id: r.get(0)?,
        account_id: r.get(1)?,
        path: r.get(2)?,
        name: r.get(3)?,
        parent: r.get(4)?,
        role: MailboxRole::parse(&role),
        delimiter: r.get(6)?,
        uidvalidity: r.get::<_, i64>(7)? as u32,
        uidnext: r.get::<_, i64>(8)? as u32,
        selectable: r.get::<_, i64>(9)? != 0,
        subscribed: r.get::<_, i64>(10)? != 0,
        unread: r.get::<_, i64>(11)? as u32,
        total: r.get::<_, i64>(12)? as u32,
    })
}

fn row_to_contact(r: &Row) -> rusqlite::Result<Contact> {
    let emails: String = r.get(5)?;
    let phones: String = r.get(6)?;
    Ok(Contact {
        id: r.get(0)?,
        first_name: r.get(1)?,
        last_name: r.get(2)?,
        nickname: r.get(3)?,
        organization: r.get(4)?,
        emails: serde_json::from_str(&emails).unwrap_or_default(),
        phones: serde_json::from_str(&phones).unwrap_or_default(),
        notes: r.get(7)?,
        vip: r.get::<_, i64>(8)? != 0,
        photo: r.get(9)?,
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

fn recipients_text(m: &MessageSummary) -> String {
    m.all_recipients().map(|a| format!("{} {}", a.name, a.email)).collect::<Vec<_>>().join(" ")
}

impl Store {
    pub fn open(path: &Path) -> Result<SharedStore> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Arc::new(Store { conn: Mutex::new(conn), path: path.to_path_buf() }))
    }

    pub fn open_in_memory() -> Result<SharedStore> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Arc::new(Store { conn: Mutex::new(conn), path: PathBuf::from(":memory:") }))
    }

    /// Run arbitrary code with the connection (for modules that keep their own tables).
    pub fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        let c = self.conn.lock();
        f(&c)
    }

    // ------------------------------------------------------------------ mailboxes

    pub fn mailboxes(&self, account: Option<&str>) -> Vec<Mailbox> {
        let c = self.conn.lock();
        let sql = "SELECT b.id, b.account_id, b.path, b.name, b.parent, b.role, b.delimiter, b.uidvalidity, b.uidnext, b.selectable, b.subscribed,
                (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id=b.id AND (m.flags & 1)=0 AND (m.flags & 8)=0),
                (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id=b.id AND (m.flags & 8)=0)
             FROM mailboxes b WHERE (?1 IS NULL OR b.account_id=?1) ORDER BY b.account_id, b.path COLLATE NOCASE";
        let mut st = c.prepare_cached(sql).unwrap();
        st.query_map(params![account], row_to_mailbox).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn mailbox(&self, id: MailboxId) -> Option<Mailbox> {
        let c = self.conn.lock();
        c.query_row(
            "SELECT b.id, b.account_id, b.path, b.name, b.parent, b.role, b.delimiter, b.uidvalidity, b.uidnext, b.selectable, b.subscribed,
                (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id=b.id AND (m.flags & 1)=0 AND (m.flags & 8)=0),
                (SELECT COUNT(*) FROM messages m WHERE m.mailbox_id=b.id AND (m.flags & 8)=0)
             FROM mailboxes b WHERE b.id=?1",
            params![id],
            row_to_mailbox,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn mailbox_by_path(&self, account: &str, path: &str) -> Option<Mailbox> {
        let id: Option<i64> = {
            let c = self.conn.lock();
            c.query_row("SELECT id FROM mailboxes WHERE account_id=?1 AND path=?2", params![account, path], |r| r.get(0)).optional().ok().flatten()
        };
        id.and_then(|id| self.mailbox(id))
    }

    pub fn mailbox_by_role(&self, account: &str, role: MailboxRole) -> Option<Mailbox> {
        let id: Option<i64> = {
            let c = self.conn.lock();
            c.query_row("SELECT id FROM mailboxes WHERE account_id=?1 AND role=?2 ORDER BY id LIMIT 1", params![account, role.as_str()], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
        };
        id.and_then(|id| self.mailbox(id))
    }

    /// Insert or update a mailbox identified by (account, path). Returns its id.
    pub fn upsert_mailbox(&self, mb: &Mailbox) -> Result<MailboxId> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO mailboxes (account_id, path, name, parent, role, delimiter, uidvalidity, uidnext, selectable, subscribed)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(account_id, path) DO UPDATE SET name=excluded.name, parent=excluded.parent, role=excluded.role,
                delimiter=excluded.delimiter, selectable=excluded.selectable, subscribed=excluded.subscribed",
            params![mb.account_id, mb.path, mb.name, mb.parent, mb.role.as_str(), mb.delimiter, mb.uidvalidity, mb.uidnext, mb.selectable, mb.subscribed],
        )?;
        let id: i64 = c.query_row("SELECT id FROM mailboxes WHERE account_id=?1 AND path=?2", params![mb.account_id, mb.path], |r| r.get(0))?;
        Ok(id)
    }

    /// Get or create a mailbox with the given role for an account.
    pub fn ensure_mailbox(&self, account: &str, path: &str, role: MailboxRole) -> Result<MailboxId> {
        if let Some(mb) = self.mailbox_by_path(account, path) {
            return Ok(mb.id);
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        self.upsert_mailbox(&Mailbox {
            account_id: account.to_string(),
            path: path.to_string(),
            name,
            role,
            delimiter: "/".into(),
            selectable: true,
            subscribed: true,
            ..Default::default()
        })
    }

    pub fn set_mailbox_role(&self, id: MailboxId, role: MailboxRole) -> Result<()> {
        let c = self.conn.lock();
        if role != MailboxRole::Custom {
            let acct: String = c.query_row("SELECT account_id FROM mailboxes WHERE id=?1", params![id], |r| r.get(0))?;
            c.execute("UPDATE mailboxes SET role='custom' WHERE account_id=?1 AND role=?2", params![acct, role.as_str()])?;
        }
        c.execute("UPDATE mailboxes SET role=?2 WHERE id=?1", params![id, role.as_str()])?;
        Ok(())
    }

    pub fn set_mailbox_sync_state(&self, id: MailboxId, uidvalidity: u32, uidnext: u32) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE mailboxes SET uidvalidity=?2, uidnext=?3 WHERE id=?1", params![id, uidvalidity, uidnext])?;
        Ok(())
    }

    pub fn rename_mailbox(&self, id: MailboxId, new_path: &str, new_name: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE mailboxes SET path=?2, name=?3 WHERE id=?1", params![id, new_path, new_name])?;
        Ok(())
    }

    pub fn delete_mailbox(&self, id: MailboxId) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE mailbox_id=?1)", params![id])?;
        c.execute("UPDATE mailboxes SET parent=NULL WHERE parent=?1", params![id])?;
        c.execute("DELETE FROM mailboxes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn delete_account_data(&self, account: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id=?1)", params![account])?;
        c.execute("DELETE FROM messages WHERE account_id=?1", params![account])?;
        c.execute("DELETE FROM mailboxes WHERE account_id=?1", params![account])?;
        c.execute("DELETE FROM pending_ops WHERE account_id=?1", params![account])?;
        Ok(())
    }

    /// Remove every message of a mailbox (used when UIDVALIDITY changes or "Rebuild").
    pub fn clear_mailbox(&self, id: MailboxId) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE mailbox_id=?1)", params![id])?;
        c.execute("DELETE FROM messages WHERE mailbox_id=?1", params![id])?;
        c.execute("UPDATE mailboxes SET uidnext=0 WHERE id=?1", params![id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ messages

    /// Insert a new message. `raw` is the full RFC 5322 source if downloaded.
    /// `body_text` is indexed for search. Returns the local id.
    pub fn insert_message(&self, m: &MessageSummary, raw: Option<&[u8]>, body_text: &str) -> Result<MessageId> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        let mut flags = m.flags;
        if raw.is_some() {
            flags |= flags::HAS_BODY;
        }
        let to_sort = m.to.first().map(|a| a.display().to_lowercase()).unwrap_or_default();
        tx.execute(
            "INSERT INTO messages (mailbox_id, account_id, uid, message_id, in_reply_to, refs, thread_id, subject, norm_subject, from_name, from_addr,
                to_json, cc_json, bcc_json, reply_to_json, to_sort, date, received, size, flags, color, has_attachments, snippet, priority, category,
                remind_at, list_unsubscribe, send_at)
             VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)",
            params![
                m.mailbox_id,
                m.account_id,
                m.uid,
                m.message_id,
                m.in_reply_to,
                serde_json::to_string(&m.references)?,
                m.subject,
                normalize_subject(&m.subject).to_lowercase(),
                m.from.name,
                m.from.email.to_lowercase(),
                serde_json::to_string(&m.to)?,
                serde_json::to_string(&m.cc)?,
                serde_json::to_string(&m.bcc)?,
                serde_json::to_string(&m.reply_to)?,
                to_sort,
                m.date,
                m.received,
                m.size,
                flags,
                m.color.map(|c| c as u8 as i64),
                m.has_attachments,
                m.snippet,
                m.priority.to_i32(),
                m.category.to_i32(),
                m.remind_at,
                m.list_unsubscribe,
                m.send_at,
            ],
        )?;
        let id = tx.last_insert_rowid();
        let thread = Self::compute_thread(&tx, id, m)?;
        tx.execute("UPDATE messages SET thread_id=?2 WHERE id=?1", params![id, thread])?;
        for r in m.references.iter().filter(|r| !r.is_empty()) {
            tx.execute("INSERT INTO msg_refs (message_id, ref) VALUES (?1, ?2)", params![id, r])?;
        }
        // Adopt children that arrived before this message.
        if !m.message_id.is_empty() {
            let orphans: Vec<i64> = {
                let mut st = tx.prepare_cached(
                    "SELECT thread_id FROM messages WHERE in_reply_to=?1 AND id<>?2 AND +account_id=?3
                     UNION SELECT m.thread_id FROM msg_refs r JOIN messages m ON m.id=r.message_id WHERE r.ref=?1 AND m.id<>?2 AND +m.account_id=?3",
                )?;
                st.query_map(params![m.message_id, id, m.account_id], |r| r.get(0))?.filter_map(|r| r.ok()).collect()
            };
            for t in orphans.into_iter().filter(|t| *t != thread) {
                tx.execute("UPDATE messages SET thread_id=?1 WHERE thread_id=?2", params![thread, t])?;
            }
        }
        if let Some(raw) = raw {
            tx.execute("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?1, ?2)", params![id, raw])?;
        }
        tx.execute(
            "INSERT INTO messages_fts (rowid, subject, sender, recipients, body) VALUES (?1,?2,?3,?4,?5)",
            params![id, m.subject, format!("{} {}", m.from.name, m.from.email), recipients_text(m), body_text],
        )?;
        tx.commit()?;
        Ok(id)
    }

    fn compute_thread(tx: &Connection, id: MessageId, m: &MessageSummary) -> Result<i64> {
        let mut ids: Vec<&str> = m.references.iter().map(|s| s.as_str()).collect();
        if !m.in_reply_to.is_empty() {
            ids.push(&m.in_reply_to);
        }
        ids.retain(|s| !s.is_empty());
        if !ids.is_empty() {
            let sql = format!("SELECT thread_id FROM messages WHERE message_id IN ({}) AND id<>? AND thread_id<>0 LIMIT 1", placeholders(ids.len()));
            let mut p: Vec<rusqlite::types::Value> = ids.iter().map(|s| rusqlite::types::Value::Text(s.to_string())).collect();
            p.push(rusqlite::types::Value::Integer(id));
            if let Some(t) = tx.query_row(&sql, params_from_iter(p), |r| r.get::<_, i64>(0)).optional()? {
                return Ok(t);
            }
        }
        // A message with the same Message-ID (e.g. copy in Sent and Inbox)
        if !m.message_id.is_empty() {
            if let Some(t) = tx
                .query_row("SELECT thread_id FROM messages WHERE message_id=?1 AND id<>?2 AND thread_id<>0 LIMIT 1", params![m.message_id, id], |r| r.get::<_, i64>(0))
                .optional()?
            {
                return Ok(t);
            }
        }
        let norm = normalize_subject(&m.subject).to_lowercase();
        if subject_is_reply(&m.subject) && norm.len() > 3 {
            if let Some(t) = tx
                .query_row(
                    "SELECT thread_id FROM messages WHERE norm_subject=?1 AND id<>?2 AND account_id=?3 AND date > ?4 AND thread_id<>0 ORDER BY date DESC LIMIT 1",
                    params![norm, id, m.account_id, m.date - 60 * 86400],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            {
                return Ok(t);
            }
        }
        Ok(id)
    }

    pub fn message(&self, id: MessageId) -> Option<MessageSummary> {
        let c = self.conn.lock();
        c.query_row(&format!("SELECT {MSG_COLS} FROM messages m WHERE m.id=?1"), params![id], row_to_summary).optional().ok().flatten()
    }

    pub fn messages_by_ids(&self, ids: &[MessageId]) -> Vec<MessageSummary> {
        if ids.is_empty() {
            return vec![];
        }
        let c = self.conn.lock();
        let sql = format!("SELECT {MSG_COLS} FROM messages m WHERE m.id IN ({})", placeholders(ids.len()));
        let mut st = c.prepare(&sql).unwrap();
        st.query_map(params_from_iter(ids.iter()), row_to_summary).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn message_by_uid(&self, mailbox: MailboxId, uid: u32) -> Option<MessageSummary> {
        let c = self.conn.lock();
        c.query_row(&format!("SELECT {MSG_COLS} FROM messages m WHERE m.mailbox_id=?1 AND m.uid=?2"), params![mailbox, uid], row_to_summary).optional().ok().flatten()
    }

    /// A message in the mailbox whose server uid is not yet known, matched by Message-ID.
    pub fn pending_message_by_msgid(&self, mailbox: MailboxId, msgid: &str) -> Option<MessageId> {
        if msgid.is_empty() {
            return None;
        }
        let c = self.conn.lock();
        c.query_row("SELECT id FROM messages WHERE mailbox_id=?1 AND uid=0 AND message_id=?2 LIMIT 1", params![mailbox, msgid], |r| r.get(0)).optional().ok().flatten()
    }

    /// Map of uid -> (local id, flags) for a mailbox.
    pub fn uid_map(&self, mailbox: MailboxId) -> Vec<(u32, MessageId, u32, Option<FlagColor>)> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT uid, id, flags, color FROM messages WHERE mailbox_id=?1 AND uid<>0").unwrap();
        st.query_map(params![mailbox], |r| {
            let col: Option<i64> = r.get(3)?;
            Ok((r.get::<_, i64>(0)? as u32, r.get(1)?, r.get::<_, i64>(2)? as u32, col.map(|c| FlagColor::from_u8(c as u8))))
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub fn set_uid(&self, id: MessageId, uid: u32) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE messages SET uid=?2 WHERE id=?1", params![id, uid])?;
        Ok(())
    }

    pub fn raw(&self, id: MessageId) -> Option<Vec<u8>> {
        let c = self.conn.lock();
        c.query_row("SELECT raw FROM bodies WHERE message_id=?1", params![id], |r| r.get(0)).optional().ok().flatten()
    }

    /// Store a downloaded body and update the summary fields derived from it.
    pub fn set_raw(&self, id: MessageId, raw: &[u8], body_text: &str, snippet: &str, has_attachments: bool) -> Result<()> {
        let c = self.conn.lock();
        c.execute("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?1, ?2)", params![id, raw])?;
        c.execute(
            "UPDATE messages SET flags = flags | ?2, snippet=?3, has_attachments=?4 WHERE id=?1",
            params![id, flags::HAS_BODY, snippet, has_attachments],
        )?;
        c.execute("UPDATE messages_fts SET body=?2 WHERE rowid=?1", params![id, body_text])?;
        Ok(())
    }

    /// Replace a message's raw source (e.g. "Remove Attachments").
    pub fn replace_raw(&self, id: MessageId, raw: &[u8], size: u32, has_attachments: bool) -> Result<()> {
        let c = self.conn.lock();
        c.execute("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?1, ?2)", params![id, raw])?;
        c.execute("UPDATE messages SET size=?2, has_attachments=?3 WHERE id=?1", params![id, size, has_attachments])?;
        Ok(())
    }

    pub fn messages_without_body(&self, mailbox: MailboxId, limit: usize) -> Vec<(MessageId, u32)> {
        let c = self.conn.lock();
        let mut st = c
            .prepare_cached("SELECT id, uid FROM messages WHERE mailbox_id=?1 AND (flags & ?2)=0 AND uid<>0 ORDER BY date DESC LIMIT ?3")
            .unwrap();
        st.query_map(params![mailbox, flags::HAS_BODY, limit as i64], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32)))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Overwrite the flags from the server (keeps local-only bits).
    pub fn set_server_flags(&self, id: MessageId, server_flags: u32, color: Option<FlagColor>) -> Result<()> {
        const SERVER_BITS: u32 = flags::SEEN | flags::ANSWERED | flags::FLAGGED | flags::DELETED | flags::DRAFT | flags::FORWARDED | flags::JUNK | flags::NOT_JUNK;
        let c = self.conn.lock();
        c.execute(
            "UPDATE messages SET flags = (flags & ?2) | ?3, color=?4 WHERE id=?1",
            params![id, !SERVER_BITS, server_flags & SERVER_BITS, color.map(|c| c as u8 as i64)],
        )?;
        Ok(())
    }

    pub fn update_flags(&self, ids: &[MessageId], add: u32, remove: u32) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let c = self.conn.lock();
        let sql = format!("UPDATE messages SET flags = (flags | ?) & ~? WHERE id IN ({})", placeholders(ids.len()));
        let mut p: Vec<i64> = vec![add as i64, remove as i64];
        p.extend(ids.iter().copied());
        c.execute(&sql, params_from_iter(p))?;
        Ok(())
    }

    pub fn set_color(&self, ids: &[MessageId], color: Option<FlagColor>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let c = self.conn.lock();
        let sql = format!("UPDATE messages SET color=? WHERE id IN ({})", placeholders(ids.len()));
        let mut p: Vec<Option<i64>> = vec![color.map(|c| c as u8 as i64)];
        p.extend(ids.iter().map(|i| Some(*i)));
        c.execute(&sql, params_from_iter(p))?;
        Ok(())
    }

    pub fn set_category(&self, id: MessageId, cat: Category) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE messages SET category=?2 WHERE id=?1", params![id, cat.to_i32()])?;
        Ok(())
    }

    pub fn set_remind_at(&self, ids: &[MessageId], at: Option<i64>) -> Result<()> {
        let c = self.conn.lock();
        for id in ids {
            c.execute("UPDATE messages SET remind_at=?2 WHERE id=?1", params![id, at])?;
        }
        Ok(())
    }

    pub fn set_send_at(&self, id: MessageId, at: Option<i64>) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE messages SET send_at=?2 WHERE id=?1", params![id, at])?;
        Ok(())
    }

    /// Move messages locally. The uid is reset to 0 until the server reports the new uid.
    pub fn move_messages(&self, ids: &[MessageId], dest: MailboxId) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let c = self.conn.lock();
        let acct: String = c.query_row("SELECT account_id FROM mailboxes WHERE id=?1", params![dest], |r| r.get(0))?;
        let sql = format!("UPDATE messages SET mailbox_id=?, account_id=?, uid=0 WHERE id IN ({})", placeholders(ids.len()));
        let mut p: Vec<rusqlite::types::Value> = vec![dest.into(), acct.into()];
        p.extend(ids.iter().map(|i| rusqlite::types::Value::Integer(*i)));
        c.execute(&sql, params_from_iter(p))?;
        Ok(())
    }

    /// Copy messages locally (including bodies). Returns the new ids.
    pub fn copy_messages(&self, ids: &[MessageId], dest: MailboxId) -> Result<Vec<MessageId>> {
        let mut out = Vec::new();
        let dest_acct = self.mailbox(dest).map(|m| m.account_id).unwrap_or_default();
        for m in self.messages_by_ids(ids) {
            let raw = self.raw(m.id);
            let body = self.fts_body(m.id);
            let mut n = m.clone();
            n.mailbox_id = dest;
            n.account_id = dest_acct.clone();
            n.uid = 0;
            out.push(self.insert_message(&n, raw.as_deref(), &body)?);
        }
        Ok(out)
    }

    fn fts_body(&self, id: MessageId) -> String {
        let c = self.conn.lock();
        c.query_row("SELECT body FROM messages_fts WHERE rowid=?1", params![id], |r| r.get(0)).optional().ok().flatten().unwrap_or_default()
    }

    /// Delete messages from the local store entirely.
    pub fn delete_messages(&self, ids: &[MessageId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let c = self.conn.lock();
        let ph = placeholders(ids.len());
        c.execute(&format!("DELETE FROM messages_fts WHERE rowid IN ({ph})"), params_from_iter(ids.iter()))?;
        c.execute(&format!("DELETE FROM messages WHERE id IN ({ph})"), params_from_iter(ids.iter()))?;
        Ok(())
    }

    pub fn delete_by_uids(&self, mailbox: MailboxId, uids: &[u32]) -> Result<()> {
        let ids: Vec<MessageId> = {
            let c = self.conn.lock();
            let mut st = c.prepare_cached("SELECT id FROM messages WHERE mailbox_id=?1 AND uid=?2").unwrap();
            uids.iter().filter_map(|u| st.query_row(params![mailbox, u], |r| r.get(0)).ok()).collect()
        };
        self.delete_messages(&ids)
    }

    /// Messages in the given mailbox flagged \Deleted.
    pub fn deleted_in(&self, mailbox: MailboxId) -> Vec<MessageId> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT id FROM messages WHERE mailbox_id=?1 AND (flags & 8)<>0").unwrap();
        st.query_map(params![mailbox], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn ids_in_mailbox(&self, mailbox: MailboxId) -> Vec<MessageId> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT id FROM messages WHERE mailbox_id=?1").unwrap();
        st.query_map(params![mailbox], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn older_than(&self, mailbox: MailboxId, ts: i64) -> Vec<MessageId> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT id FROM messages WHERE mailbox_id=?1 AND received < ?2").unwrap();
        st.query_map(params![mailbox, ts], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn thread_messages(&self, thread: i64) -> Vec<MessageSummary> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached(&format!("SELECT {MSG_COLS} FROM messages m WHERE m.thread_id=?1 AND (m.flags & 8)=0 ORDER BY m.date")).unwrap();
        st.query_map(params![thread], row_to_summary).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn list_messages(&self, q: &MessageQuery) -> Vec<MessageSummary> {
        let mut wh: Vec<String> = Vec::new();
        let mut p: Vec<rusqlite::types::Value> = Vec::new();
        use rusqlite::types::Value as V;
        if !q.mailboxes.is_empty() {
            wh.push(format!("m.mailbox_id IN ({})", placeholders(q.mailboxes.len())));
            p.extend(q.mailboxes.iter().map(|i| V::Integer(*i)));
        }
        if !q.accounts.is_empty() {
            wh.push(format!("m.account_id IN ({})", placeholders(q.accounts.len())));
            p.extend(q.accounts.iter().map(|i| V::Text(i.clone())));
        }
        if !q.exclude_mailboxes.is_empty() {
            wh.push(format!("m.mailbox_id NOT IN ({})", placeholders(q.exclude_mailboxes.len())));
            p.extend(q.exclude_mailboxes.iter().map(|i| V::Integer(*i)));
        }
        if !q.from_any.is_empty() {
            wh.push(format!("m.from_addr IN ({})", placeholders(q.from_any.len())));
            p.extend(q.from_any.iter().map(|i| V::Text(i.to_lowercase())));
        }
        if !q.include_deleted {
            wh.push("(m.flags & 8)=0".into());
        }
        if q.only_flagged || q.filter.flagged && q.filter.enabled {
            wh.push("(m.flags & 4)<>0".into());
        }
        if let Some(c) = q.only_color {
            wh.push(format!("m.color={}", c as u8));
        }
        if q.only_unread || q.filter.unread && q.filter.enabled {
            wh.push("(m.flags & 1)=0".into());
        }
        if q.only_drafts {
            wh.push("(m.flags & 16)<>0".into());
        }
        if let Some(t) = q.thread {
            wh.push("m.thread_id=?".into());
            p.push(V::Integer(t));
        }
        if q.reminders_due {
            wh.push("m.remind_at IS NOT NULL AND m.remind_at <= ?".into());
            p.push(V::Integer(now_ts()));
        }
        if q.filter.enabled {
            if q.filter.with_attachments {
                wh.push("m.has_attachments=1".into());
            }
            if q.filter.today {
                let start = chrono::Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap().and_local_timezone(chrono::Local).single().map(|d| d.timestamp()).unwrap_or(0);
                wh.push("m.date >= ?".into());
                p.push(V::Integer(start));
            }
            if q.filter.from_vip {
                if q.vip_addresses.is_empty() {
                    wh.push("0".into());
                } else {
                    wh.push(format!("m.from_addr IN ({})", placeholders(q.vip_addresses.len())));
                    p.extend(q.vip_addresses.iter().map(|i| V::Text(i.to_lowercase())));
                }
            }
            if q.filter.to_me || q.filter.cc_me {
                let col = if q.filter.to_me { "m.to_json" } else { "m.cc_json" };
                if q.my_addresses.is_empty() {
                    wh.push("0".into());
                } else {
                    let ors: Vec<String> = q.my_addresses.iter().map(|_| format!("lower({col}) LIKE ?")).collect();
                    wh.push(format!("({})", ors.join(" OR ")));
                    p.extend(q.my_addresses.iter().map(|a| V::Text(format!("%\"{}\"%", a.to_lowercase()))));
                }
            }
        }
        if let Some(cat) = q.filter.category {
            wh.push("m.category=?".into());
            p.push(V::Integer(cat.to_i32() as i64));
        }
        if let Some(f) = &q.fts {
            if !f.trim().is_empty() {
                wh.push("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into());
                p.push(V::Text(f.clone()));
            }
        }
        let order_col = match q.sort {
            SortKey::Attachments => "m.has_attachments",
            SortKey::Date => "m.date",
            SortKey::Flags => "((m.flags & 4)<>0)*10 + COALESCE(m.color, 0)",
            SortKey::From => "lower(CASE WHEN m.from_name<>'' THEN m.from_name ELSE m.from_addr END)",
            SortKey::Size => "m.size",
            SortKey::Subject => "m.norm_subject",
            SortKey::To => "m.to_sort",
            SortKey::Unread => "((m.flags & 1)=0)",
            SortKey::Mailbox => "m.mailbox_id",
            SortKey::Category => "m.category",
        };
        let dir = if q.ascending { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT {MSG_COLS} FROM messages m {} ORDER BY {order_col} {dir}, m.date DESC {}",
            if wh.is_empty() { String::new() } else { format!("WHERE {}", wh.join(" AND ")) },
            q.limit.map(|l| format!("LIMIT {l}")).unwrap_or_default()
        );
        let c = self.conn.lock();
        let mut st = match c.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("query failed: {e}: {sql}");
                return vec![];
            }
        };
        let res = st.query_map(params_from_iter(p), row_to_summary).map(|it| it.filter_map(|r| r.ok()).collect());
        match res {
            Ok(v) => v,
            Err(e) => {
                log::warn!("query failed: {e}");
                vec![]
            }
        }
    }

    pub fn unread_count(&self, mailboxes: &[MailboxId]) -> u32 {
        if mailboxes.is_empty() {
            return 0;
        }
        let c = self.conn.lock();
        let sql = format!("SELECT COUNT(*) FROM messages WHERE (flags & 1)=0 AND (flags & 8)=0 AND mailbox_id IN ({})", placeholders(mailboxes.len()));
        c.query_row(&sql, params_from_iter(mailboxes.iter()), |r| r.get::<_, i64>(0)).unwrap_or(0) as u32
    }

    pub fn total_size(&self, mailbox: MailboxId) -> u64 {
        let c = self.conn.lock();
        c.query_row("SELECT COALESCE(SUM(size),0) FROM messages WHERE mailbox_id=?1", params![mailbox], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64
    }

    // ------------------------------------------------------------------ pending ops

    pub fn push_op(&self, account: &str, op: &RemoteOp) -> Result<()> {
        if account == LOCAL_ACCOUNT {
            return Ok(());
        }
        let c = self.conn.lock();
        c.execute("INSERT INTO pending_ops (account_id, op, created) VALUES (?1,?2,?3)", params![account, serde_json::to_string(op)?, now_ts()])?;
        Ok(())
    }

    pub fn pending_ops(&self, account: &str) -> Vec<(i64, RemoteOp, i64)> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT id, op, attempts FROM pending_ops WHERE account_id=?1 ORDER BY id").unwrap();
        st.query_map(params![account], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .map(|it| it.filter_map(|r| r.ok()).filter_map(|(id, s, a)| serde_json::from_str(&s).ok().map(|op| (id, op, a))).collect())
            .unwrap_or_default()
    }

    pub fn pending_op_count(&self) -> usize {
        let c = self.conn.lock();
        c.query_row("SELECT COUNT(*) FROM pending_ops", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    pub fn remove_op(&self, id: i64) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM pending_ops WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn bump_op_attempts(&self, id: i64) -> Result<()> {
        let c = self.conn.lock();
        c.execute("UPDATE pending_ops SET attempts=attempts+1 WHERE id=?1", params![id])?;
        Ok(())
    }

    // ------------------------------------------------------------------ contacts

    pub fn contacts(&self) -> Vec<Contact> {
        let c = self.conn.lock();
        let mut st = c
            .prepare_cached("SELECT id, first_name, last_name, nickname, organization, emails, phones, notes, vip, photo FROM contacts ORDER BY lower(last_name), lower(first_name)")
            .unwrap();
        st.query_map([], row_to_contact).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn save_contact(&self, ct: &Contact) -> Result<i64> {
        let c = self.conn.lock();
        if ct.id > 0 {
            c.execute(
                "UPDATE contacts SET first_name=?2, last_name=?3, nickname=?4, organization=?5, emails=?6, phones=?7, notes=?8, vip=?9, photo=?10 WHERE id=?1",
                params![ct.id, ct.first_name, ct.last_name, ct.nickname, ct.organization, serde_json::to_string(&ct.emails)?, serde_json::to_string(&ct.phones)?, ct.notes, ct.vip, ct.photo],
            )?;
            Ok(ct.id)
        } else {
            c.execute(
                "INSERT INTO contacts (first_name, last_name, nickname, organization, emails, phones, notes, vip, photo) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![ct.first_name, ct.last_name, ct.nickname, ct.organization, serde_json::to_string(&ct.emails)?, serde_json::to_string(&ct.phones)?, ct.notes, ct.vip, ct.photo],
            )?;
            Ok(c.last_insert_rowid())
        }
    }

    pub fn delete_contact(&self, id: i64) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM contacts WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn contact_by_email(&self, email: &str) -> Option<Contact> {
        let e = email.to_lowercase();
        self.contacts().into_iter().find(|c| c.emails.iter().any(|x| x.to_lowercase() == e))
    }

    /// Lowercase emails of all VIP contacts.
    pub fn vip_emails(&self) -> Vec<String> {
        self.contacts().into_iter().filter(|c| c.vip).flat_map(|c| c.emails.into_iter().map(|e| e.to_lowercase())).collect()
    }

    pub fn contact_emails(&self) -> HashSet<String> {
        self.contacts().into_iter().flat_map(|c| c.emails.into_iter().map(|e| e.to_lowercase())).collect()
    }

    /// Mark a sender as VIP (creating a contact when needed).
    pub fn set_vip(&self, addr: &Address, vip: bool) -> Result<()> {
        let mut ct = self.contact_by_email(&addr.email).unwrap_or_else(|| {
            let mut parts = addr.name.splitn(2, ' ');
            Contact {
                first_name: parts.next().unwrap_or("").to_string(),
                last_name: parts.next().unwrap_or("").to_string(),
                emails: vec![addr.email.clone()],
                ..Default::default()
            }
        });
        ct.vip = vip;
        self.save_contact(&ct)?;
        Ok(())
    }

    pub fn record_recipient(&self, addr: &Address) -> Result<()> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO previous_recipients (email, name, last_used, count) VALUES (?1,?2,?3,1)
             ON CONFLICT(email) DO UPDATE SET last_used=excluded.last_used, count=count+1, name=CASE WHEN excluded.name<>'' THEN excluded.name ELSE name END",
            params![addr.email.to_lowercase(), addr.name, now_ts()],
        )?;
        Ok(())
    }

    pub fn previous_recipients(&self) -> Vec<PreviousRecipient> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT email, name, last_used, count FROM previous_recipients ORDER BY last_used DESC").unwrap();
        st.query_map([], |r| Ok(PreviousRecipient { email: r.get(0)?, name: r.get(1)?, last_used: r.get(2)?, count: r.get(3)? }))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    pub fn remove_previous_recipient(&self, email: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM previous_recipients WHERE email=?1", params![email.to_lowercase()])?;
        Ok(())
    }

    /// Address completion candidates from contacts and previous recipients.
    pub fn complete_address(&self, prefix: &str, limit: usize) -> Vec<Address> {
        let p = prefix.trim().to_lowercase();
        if p.is_empty() {
            return vec![];
        }
        let mut out: Vec<Address> = Vec::new();
        let mut seen = HashSet::new();
        for ct in self.contacts() {
            let name = ct.display_name();
            for e in &ct.emails {
                let hit = name.to_lowercase().split_whitespace().any(|w| w.starts_with(&p))
                    || name.to_lowercase().starts_with(&p)
                    || e.to_lowercase().starts_with(&p)
                    || ct.nickname.to_lowercase().starts_with(&p);
                if hit && seen.insert(e.to_lowercase()) {
                    out.push(Address::new(name.clone(), e.clone()));
                }
            }
        }
        for r in self.previous_recipients() {
            let hit = r.email.starts_with(&p) || r.name.to_lowercase().split_whitespace().any(|w| w.starts_with(&p));
            if hit && seen.insert(r.email.clone()) {
                out.push(Address::new(r.name, r.email));
            }
        }
        out.truncate(limit);
        out
    }

    // ------------------------------------------------------------------ blocked senders / muted threads

    pub fn blocked_senders(&self) -> Vec<String> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT email FROM blocked_senders ORDER BY email").unwrap();
        st.query_map([], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn is_blocked(&self, email: &str) -> bool {
        let c = self.conn.lock();
        c.query_row("SELECT 1 FROM blocked_senders WHERE email=?1", params![email.to_lowercase()], |_| Ok(())).optional().ok().flatten().is_some()
    }

    pub fn set_blocked(&self, email: &str, blocked: bool) -> Result<()> {
        let c = self.conn.lock();
        if blocked {
            c.execute("INSERT OR IGNORE INTO blocked_senders (email) VALUES (?1)", params![email.to_lowercase()])?;
        } else {
            c.execute("DELETE FROM blocked_senders WHERE email=?1", params![email.to_lowercase()])?;
        }
        Ok(())
    }

    pub fn is_muted(&self, thread: i64) -> bool {
        let c = self.conn.lock();
        c.query_row("SELECT 1 FROM muted_threads WHERE thread_id=?1", params![thread], |_| Ok(())).optional().ok().flatten().is_some()
    }

    pub fn muted_threads(&self) -> HashSet<i64> {
        let c = self.conn.lock();
        let mut st = c.prepare_cached("SELECT thread_id FROM muted_threads").unwrap();
        st.query_map([], |r| r.get(0)).map(|it| it.filter_map(|r| r.ok()).collect()).unwrap_or_default()
    }

    pub fn set_muted(&self, thread: i64, muted: bool) -> Result<()> {
        let c = self.conn.lock();
        if muted {
            c.execute("INSERT OR IGNORE INTO muted_threads (thread_id) VALUES (?1)", params![thread])?;
        } else {
            c.execute("DELETE FROM muted_threads WHERE thread_id=?1", params![thread])?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ key/value

    pub fn kv_get(&self, key: &str) -> Option<String> {
        let c = self.conn.lock();
        c.query_row("SELECT value FROM kv WHERE key=?1", params![key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("INSERT OR REPLACE INTO kv (key, value) VALUES (?1,?2)", params![key, value])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(mb: MailboxId, subject: &str, msgid: &str, irt: &str) -> MessageSummary {
        MessageSummary {
            mailbox_id: mb,
            account_id: "a".into(),
            subject: subject.into(),
            message_id: msgid.into(),
            in_reply_to: irt.into(),
            from: Address::new("Alice", "alice@example.com"),
            date: now_ts(),
            ..Default::default()
        }
    }

    #[test]
    fn threads_and_queries() {
        let s = Store::open_in_memory().unwrap();
        let mb = s.ensure_mailbox("a", "INBOX", MailboxRole::Inbox).unwrap();
        let a = s.insert_message(&msg(mb, "Lunch", "1@x", ""), None, "let's get lunch").unwrap();
        let b = s.insert_message(&msg(mb, "Re: Lunch", "2@x", "1@x"), None, "sure").unwrap();
        let c = s.insert_message(&msg(mb, "Other", "3@x", ""), None, "unrelated").unwrap();
        let ma = s.message(a).unwrap();
        let mbm = s.message(b).unwrap();
        assert_eq!(ma.thread_id, mbm.thread_id);
        assert_ne!(s.message(c).unwrap().thread_id, ma.thread_id);
        // child before parent
        let d = s.insert_message(&msg(mb, "Re: Party", "5@x", "4@x"), None, "").unwrap();
        let e = s.insert_message(&msg(mb, "Party", "4@x", ""), None, "").unwrap();
        assert_eq!(s.message(d).unwrap().thread_id, s.message(e).unwrap().thread_id);

        let all = s.list_messages(&MessageQuery { mailboxes: vec![mb], ..Default::default() });
        assert_eq!(all.len(), 5);
        let found = s.list_messages(&MessageQuery { fts: Some("lunch".into()), ..Default::default() });
        assert_eq!(found.len(), 2);
        s.update_flags(&[a], flags::SEEN, 0).unwrap();
        assert_eq!(s.mailbox(mb).unwrap().unread, 4);
        let unread = s.list_messages(&MessageQuery { only_unread: true, ..Default::default() });
        assert_eq!(unread.len(), 4);
    }

    #[test]
    fn contacts_and_completion() {
        let s = Store::open_in_memory().unwrap();
        s.set_vip(&Address::new("Jane Doe", "jane@example.com"), true).unwrap();
        s.record_recipient(&Address::new("Bob", "bob@example.com")).unwrap();
        assert_eq!(s.vip_emails(), vec!["jane@example.com".to_string()]);
        assert_eq!(s.complete_address("do", 10).len(), 1);
        assert_eq!(s.complete_address("bo", 10)[0].email, "bob@example.com");
    }
}
