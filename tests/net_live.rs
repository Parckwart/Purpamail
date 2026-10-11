//! Tests against a real IMAP / POP3 server (for example a local Dovecot).
//!
//! Ignored by default. Run with
//!
//! ```text
//! PURPAMAIL_LIVE_IMAP_PORT=143 PURPAMAIL_LIVE_IMAPS_PORT=993 PURPAMAIL_LIVE_POP3_PORT=110 \
//! PURPAMAIL_LIVE_USER=user PURPAMAIL_LIVE_POP3_USER=popuser PURPAMAIL_LIVE_PASSWORD=secret \
//! cargo test --test net_live -- --ignored --test-threads=1
//! ```
//!
//! The accounts' mailboxes are modified: use dedicated test users. The server must offer
//! STARTTLS on the plain port; self-signed certificates are accepted.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use purpamail::core::config::{AccountConfig, AccountKind, AuthMethod, CheckFrequency, Pop3Removal, Secrets, Security, ServerConfig, Settings};
use purpamail::core::model::*;
use purpamail::core::ops;
use purpamail::core::store::{MessageQuery, RemoteOp, SharedStore, Store};
use purpamail::net::imap_client::{ImapClient, NetError};
use purpamail::net::{Command, Engine, EngineHandle, Event};

struct Live {
    host: String,
    imap: u16,
    imaps: Option<u16>,
    pop3: Option<u16>,
    user: String,
    pop_user: String,
    password: String,
}

fn live() -> Option<Live> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(Live {
        host: var("PURPAMAIL_LIVE_HOST").unwrap_or_else(|| "127.0.0.1".into()),
        imap: var("PURPAMAIL_LIVE_IMAP_PORT")?.parse().ok()?,
        imaps: var("PURPAMAIL_LIVE_IMAPS_PORT").and_then(|p| p.parse().ok()),
        pop3: var("PURPAMAIL_LIVE_POP3_PORT").and_then(|p| p.parse().ok()),
        user: var("PURPAMAIL_LIVE_USER")?,
        pop_user: var("PURPAMAIL_LIVE_POP3_USER").unwrap_or_else(|| "popuser".into()),
        password: var("PURPAMAIL_LIVE_PASSWORD")?,
    })
}

fn server(l: &Live, port: u16, security: Security, user: &str) -> ServerConfig {
    ServerConfig { host: l.host.clone(), port, security, username: user.into(), auth: AuthMethod::Password, allow_insecure_certs: true }
}

fn msg(subject: &str, msgid: &str, body: &str) -> Vec<u8> {
    format!("From: Live Tester <tester@example.com>\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{msgid}>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n").into_bytes()
}

/// Connect directly (outside the engine) for setup and verification.
fn direct(l: &Live, user: &str) -> ImapClient {
    let cfg = server(l, l.imap, Security::StartTls, user);
    let mut c = ImapClient::connect(&cfg, false).expect("connect");
    c.login(&cfg, &l.password).expect("login");
    c
}

/// Empty INBOX and the special mailboxes, remove test mailboxes.
fn reset(c: &mut ImapClient) {
    for e in c.list().unwrap() {
        let keep = ["INBOX", "Drafts", "Sent", "Trash", "Junk"].contains(&e.path.as_str());
        if !keep {
            let _ = c.delete(&e.path);
            continue;
        }
        let info = c.select(&e.path).unwrap();
        if info.exists > 0 {
            c.run("STORE 1:* +FLAGS.SILENT (\\Deleted)").unwrap();
            c.expunge().unwrap();
        }
    }
}

fn wait<T>(engine: &EngineHandle, events: &mut Vec<Event>, secs: u64, what: &str, mut f: impl FnMut(&[Event]) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        events.extend(engine.poll());
        if let Some(v) = f(events) {
            return v;
        }
        if Instant::now() > deadline {
            panic!("timed out: {what}\n{:#?}", events.iter().rev().take(20).collect::<Vec<_>>());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start(store: &SharedStore, acct: AccountConfig, password: &str) -> EngineHandle {
    let mut settings = Settings::default();
    settings.general.check_frequency = CheckFrequency::Manually;
    settings.junk.enabled = false;
    let id = acct.id.clone();
    settings.accounts.push(acct);
    let mut secrets = Secrets::default();
    secrets.set(Secrets::incoming_key(&id), password.into());
    Engine::start(store.clone(), Arc::new(RwLock::new(settings)), Arc::new(RwLock::new(secrets)), Arc::new(|| {}))
}

fn count(store: &Store, mb: MailboxId) -> usize {
    store.list_messages(&MessageQuery { mailboxes: vec![mb], include_deleted: true, ..Default::default() }).len()
}

#[test]
#[ignore]
fn live_tls_modes() {
    let Some(l) = live() else { return };
    // STARTTLS with a self-signed certificate.
    let mut c = direct(&l, &l.user);
    assert!(c.is_tls());
    c.logout();
    // Plain.
    let cfg = server(&l, l.imap, Security::None, &l.user);
    let mut c = ImapClient::connect(&cfg, false).unwrap();
    c.login(&cfg, &l.password).unwrap();
    assert!(!c.is_tls());
    // Wrong password.
    let mut c = ImapClient::connect(&cfg, false).unwrap();
    assert!(matches!(c.login(&cfg, "wrong"), Err(NetError::Auth(_))));
    if let Some(p) = l.imaps {
        let cfg = server(&l, p, Security::Tls, &l.user);
        let mut c = ImapClient::connect(&cfg, true).unwrap();
        c.login(&cfg, &l.password).unwrap();
        assert!(c.is_tls());
        // Certificate validation is enforced unless explicitly disabled.
        let strict = ServerConfig { allow_insecure_certs: false, ..cfg };
        assert!(matches!(ImapClient::connect(&strict, false), Err(NetError::Tls(_))));
    }
}

#[test]
#[ignore]
fn live_imap_engine() {
    let Some(l) = live() else { return };
    let mut c = direct(&l, &l.user);
    reset(&mut c);
    c.append("INBOX", &["\\Seen".into()], None, &msg("Live one", "live1@example.com", "first")).unwrap();
    c.append("INBOX", &["\\Flagged".into(), "$MailFlagBit1".into()], None, &msg("Live two", "live2@example.com", "second")).unwrap();
    c.append("INBOX", &[], None, &msg("Live three", "live3@example.com", "third")).unwrap();

    let store = Store::open_in_memory().unwrap();
    let acct = AccountConfig {
        id: "live".into(),
        kind: AccountKind::Imap,
        emails: vec!["me@example.com".into()],
        incoming: server(&l, l.imap, Security::StartTls, &l.user),
        outgoing: server(&l, 1, Security::None, &l.user),
        use_idle: true,
        ..Default::default()
    };
    let engine = start(&store, acct, &l.password);
    let mut ev = Vec::new();
    let inbox = wait(&engine, &mut ev, 20, "inbox synced", |_| store.mailbox_by_role("live", MailboxRole::Inbox).filter(|i| count(&store, i.id) == 3));
    for (role, path) in [(MailboxRole::Drafts, "Drafts"), (MailboxRole::Sent, "Sent"), (MailboxRole::Trash, "Trash"), (MailboxRole::Junk, "Junk")] {
        assert_eq!(store.mailbox_by_role("live", role).map(|m| m.path), Some(path.to_string()), "{role:?}");
    }
    let msgs = store.list_messages(&MessageQuery { mailboxes: vec![inbox.id], ..Default::default() });
    let two = msgs.iter().find(|m| m.subject == "Live two").unwrap().clone();
    assert_eq!(two.color, Some(FlagColor::Yellow));
    let three = msgs.iter().find(|m| m.subject == "Live three").unwrap().clone();
    assert!(three.has(flags::HAS_BODY) && three.is_unread());

    // Flags.
    ops::set_flags(&store, &[three.id], flags::SEEN | flags::ANSWERED, 0).unwrap();
    engine.send(Command::FlushOps("live".into()));
    wait(&engine, &mut ev, 10, "flags on server", |_| store.pending_ops("live").is_empty().then_some(()));
    c.select("INBOX").unwrap();
    let fl = c.fetch_flags(&three.uid.to_string()).unwrap();
    assert!(fl[0].1.iter().any(|f| f == "\\Seen") && fl[0].1.iter().any(|f| f == "\\Answered"), "{fl:?}");

    // Archive mailbox created on demand, move, append.
    let archive = ops::role_mailbox(&store, "live", MailboxRole::Archive).unwrap();
    let one = msgs.iter().find(|m| m.subject == "Live one").unwrap().clone();
    ops::move_to(&store, &[one.id], archive.id).unwrap();
    let imported = ops::import_raw(&store, archive.id, &[msg("Live imported", "live4@example.com", "imported")]).unwrap();
    engine.send(Command::FlushOps("live".into()));
    wait(&engine, &mut ev, 10, "ops replayed", |_| store.pending_ops("live").is_empty().then_some(()));
    assert!(store.message(imported[0]).unwrap().uid != 0, "APPENDUID adopted");
    engine.send(Command::SyncMailbox(archive.id));
    wait(&engine, &mut ev, 10, "move adopted", |_| store.message(one.id).filter(|m| m.uid != 0).map(|_| ()));
    assert_eq!(count(&store, archive.id), 2);
    assert_eq!(c.status("Archive").unwrap().messages, 2);

    // Unicode mailbox names round-trip through modified UTF-7.
    store.ensure_mailbox("live", "Entwürfe 2025", MailboxRole::Custom).unwrap();
    store.push_op("live", &RemoteOp::CreateMailbox { path: "Entwürfe 2025".into() }).unwrap();
    engine.send(Command::FlushOps("live".into()));
    wait(&engine, &mut ev, 10, "unicode mailbox", |_| store.pending_ops("live").is_empty().then_some(()));
    assert!(c.list().unwrap().iter().any(|e| e.path == "Entwürfe 2025"));

    // IDLE push.
    std::thread::sleep(Duration::from_secs(1));
    let mut other = direct(&l, &l.user);
    other.append("INBOX", &[], None, &msg("Live pushed", "live5@example.com", "pushed")).unwrap();
    let pushed = wait(&engine, &mut ev, 15, "idle new mail", |evs| {
        evs.iter().find_map(|e| match e {
            Event::NewMail { messages, .. } => Some(messages.clone()),
            _ => None,
        })
    });
    assert_eq!(store.message(pushed[0]).unwrap().subject, "Live pushed");

    // Server side deletion is mirrored.
    c.select("INBOX").unwrap();
    c.uid_delete(&[two.uid]).unwrap();
    engine.send(Command::CheckMail(Some("live".into())));
    wait(&engine, &mut ev, 10, "deletion mirrored", |_| store.message(two.id).is_none().then_some(()));
    engine.send(Command::Shutdown);
}

#[test]
#[ignore]
fn live_pop3_engine() {
    let Some(l) = live() else { return };
    let Some(pop_port) = l.pop3 else { return };
    let mut c = direct(&l, &l.pop_user);
    reset(&mut c);
    c.append("INBOX", &[], None, &msg("Pop live one", "pl1@example.com", "one\r\n.leading dot")).unwrap();
    c.append("INBOX", &[], None, &msg("Pop live two", "pl2@example.com", "two")).unwrap();

    let store = Store::open_in_memory().unwrap();
    let acct = AccountConfig {
        id: "pop".into(),
        kind: AccountKind::Pop3,
        emails: vec!["me@example.com".into()],
        incoming: server(&l, pop_port, Security::StartTls, &l.pop_user),
        outgoing: server(&l, 1, Security::None, &l.pop_user),
        pop3_remove: Pop3Removal::WhenMovedFromInbox,
        ..Default::default()
    };
    let engine = start(&store, acct, &l.password);
    let mut ev = Vec::new();
    let inbox = wait(&engine, &mut ev, 20, "pop download", |_| store.mailbox_by_role("pop", MailboxRole::Inbox).filter(|i| count(&store, i.id) == 2));
    let msgs = store.list_messages(&MessageQuery { mailboxes: vec![inbox.id], ..Default::default() });
    let one = msgs.iter().find(|m| m.subject == "Pop live one").unwrap().clone();
    assert!(String::from_utf8_lossy(&store.raw(one.id).unwrap()).contains("\r\n.leading dot"));

    c.append("INBOX", &[], None, &msg("Pop live three", "pl3@example.com", "three")).unwrap();
    engine.send(Command::CheckMail(Some("pop".into())));
    wait(&engine, &mut ev, 10, "pop new mail", |evs| evs.iter().any(|e| matches!(e, Event::NewMail { .. })).then_some(()));

    let archive = ops::role_mailbox(&store, "pop", MailboxRole::Archive).unwrap();
    ops::move_to(&store, &[one.id], archive.id).unwrap();
    engine.send(Command::CheckMail(Some("pop".into())));
    wait(&engine, &mut ev, 10, "removed from server", |_| (direct(&l, &l.pop_user).status("INBOX").ok()?.messages == 2).then_some(()));
    engine.send(Command::Shutdown);
}
