//! POP3 client (RFC 1939) with CAPA (RFC 2449), STLS (RFC 2595), APOP and SASL XOAUTH2,
//! plus the download logic used by POP accounts.
//!
//! POP accounts download into the account's Inbox mailbox. Which server messages were
//! already downloaded is tracked by UIDL in the store's key/value table
//! (`pop3:uidl:<account>`), together with the local message id so the
//! "remove from server" policies can be applied later.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};

use super::imap_client::{MailStream, NetError, NetResult, Transcript, b64, open_stream, xoauth2_payload};
use crate::core::config::{AccountConfig, AuthMethod, Pop3Removal, Security, ServerConfig};
use crate::core::model::{MailboxId, MessageId, MessageSummary, flags, now_ts};
use crate::core::store::Store;
use crate::core::{categorize, mime};

/// Largest single response line accepted from a server.
const MAX_LINE: usize = 64 * 1024;
/// Largest message accepted from a server.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// A POP3 session.
pub struct Pop3Client {
    io: BufReader<MailStream>,
    host: String,
    insecure: bool,
    caps: Vec<String>,
    /// APOP timestamp from the greeting (`<...@...>`), if any.
    apop_stamp: Option<String>,
    pub greeting: String,
    pub transcript: Transcript,
}

impl Pop3Client {
    /// Connect, read the greeting and negotiate STLS when configured.
    pub fn connect(server: &ServerConfig, transcript: bool) -> NetResult<Self> {
        let mut tr = Transcript::new(transcript);
        tr.note(&format!("Connecting to {}:{} ({})", server.host, server.port, server.security.name()));
        let stream = open_stream(server)?;
        let mut c = Pop3Client {
            io: BufReader::new(stream),
            host: server.host.clone(),
            insecure: server.allow_insecure_certs,
            caps: Vec::new(),
            apop_stamp: None,
            greeting: String::new(),
            transcript: tr,
        };
        let greeting = c.read_status()?;
        c.apop_stamp = match (greeting.find('<'), greeting.rfind('>')) {
            (Some(a), Some(b)) if a < b && greeting[a..b].contains('@') => Some(greeting[a..=b].to_string()),
            _ => None,
        };
        c.greeting = greeting;
        c.caps = c.capa().unwrap_or_default();
        if server.security == Security::StartTls {
            c.stls()?;
        }
        Ok(c)
    }

    fn stls(&mut self) -> NetResult<()> {
        self.command("STLS", None).map_err(|e| match e {
            NetError::No(t) => NetError::Tls(format!("STLS rejected: {t}")),
            other => other,
        })?;
        let old = std::mem::replace(&mut self.io, BufReader::new(MailStream::Closed));
        self.io = BufReader::new(old.into_inner().upgrade(&self.host, self.insecure)?);
        self.transcript.note("TLS negotiated");
        self.caps = self.capa().unwrap_or_default();
        Ok(())
    }

    fn write_line(&mut self, line: &str, shown: Option<&str>) -> NetResult<()> {
        self.transcript.client(shown.unwrap_or(line));
        let w = self.io.get_mut();
        w.write_all(line.as_bytes())?;
        w.write_all(b"\r\n")?;
        w.flush()?;
        Ok(())
    }

    fn read_line_bytes(&mut self) -> NetResult<Vec<u8>> {
        let mut buf = Vec::new();
        let n = (&mut self.io).take(MAX_LINE as u64 + 2).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Err(NetError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the server closed the connection")));
        }
        if !buf.ends_with(b"\n") {
            return Err(NetError::Protocol("line too long".into()));
        }
        while buf.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
            buf.pop();
        }
        Ok(buf)
    }

    /// Read a status line; returns the text after `+OK`, or an error for `-ERR`.
    fn read_status(&mut self) -> NetResult<String> {
        let line = String::from_utf8_lossy(&self.read_line_bytes()?).into_owned();
        self.transcript.server(&line);
        if let Some(rest) = line.strip_prefix("+OK") {
            Ok(rest.trim().to_string())
        } else if let Some(rest) = line.strip_prefix("-ERR") {
            Err(NetError::No(rest.trim().to_string()))
        } else if line.starts_with('+') {
            // SASL continuation
            Ok(line)
        } else {
            Err(NetError::Protocol(format!("unexpected response: {line}")))
        }
    }

    fn command(&mut self, cmd: &str, shown: Option<&str>) -> NetResult<String> {
        self.write_line(cmd, shown)?;
        self.read_status()
    }

    /// Read a multi-line response body (after the status line), undoing dot-stuffing.
    fn read_multiline(&mut self) -> NetResult<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let mut buf = Vec::new();
            let n = (&mut self.io).take(MAX_LINE as u64 * 16).read_until(b'\n', &mut buf)?;
            if n == 0 {
                return Err(NetError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed during transfer")));
            }
            let content = buf.strip_suffix(b"\r\n").or_else(|| buf.strip_suffix(b"\n")).unwrap_or(&buf);
            if content == b"." {
                break;
            }
            let content = content.strip_prefix(b".").unwrap_or(content);
            out.extend_from_slice(content);
            out.extend_from_slice(b"\r\n");
            if out.len() > MAX_MESSAGE {
                return Err(NetError::Protocol("message too large".into()));
            }
        }
        Ok(out)
    }

    /// Capabilities (CAPA); empty when the server does not support CAPA.
    pub fn capa(&mut self) -> NetResult<Vec<String>> {
        match self.command("CAPA", None) {
            Ok(_) => {
                let body = self.read_multiline()?;
                let caps: Vec<String> = String::from_utf8_lossy(&body).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                for c in &caps {
                    self.transcript.server(c);
                }
                Ok(caps)
            }
            Err(NetError::No(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    pub fn has_cap(&self, name: &str) -> bool {
        self.caps.iter().any(|c| c.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case(name)))
    }

    /// Whether SASL mechanism `mech` is advertised.
    fn has_sasl(&self, mech: &str) -> bool {
        self.caps.iter().any(|c| {
            let mut w = c.split_whitespace();
            w.next().is_some_and(|x| x.eq_ignore_ascii_case("SASL")) && w.any(|m| m.eq_ignore_ascii_case(mech))
        })
    }

    pub fn is_tls(&self) -> bool {
        self.io.get_ref().is_tls()
    }

    /// Authenticate according to the server configuration.
    pub fn login(&mut self, server: &ServerConfig, password: &str) -> NetResult<()> {
        let res = match server.auth {
            AuthMethod::None => Ok(()),
            AuthMethod::OAuth2 => self.auth_xoauth2(&server.username, password),
            AuthMethod::Password => {
                // Prefer APOP on unencrypted connections so the password never travels in clear text.
                if !self.is_tls() && self.apop_stamp.is_some() {
                    match self.apop(&server.username, password) {
                        Ok(()) => return Ok(()),
                        Err(e) if e.is_connection() => return Err(e),
                        Err(_) => {}
                    }
                }
                self.user_pass(&server.username, password)
            }
        };
        res.map_err(|e| match e {
            NetError::No(t) => NetError::Auth(t),
            other => other,
        })
    }

    fn user_pass(&mut self, user: &str, password: &str) -> NetResult<()> {
        self.command(&format!("USER {user}"), None)?;
        self.command(&format!("PASS {password}"), Some("PASS ********"))?;
        Ok(())
    }

    fn apop(&mut self, user: &str, password: &str) -> NetResult<()> {
        let stamp = self.apop_stamp.clone().unwrap_or_default();
        let digest = md5_hex(format!("{stamp}{password}").as_bytes());
        self.command(&format!("APOP {user} {digest}"), Some(&format!("APOP {user} ********")))?;
        Ok(())
    }

    fn auth_xoauth2(&mut self, user: &str, token: &str) -> NetResult<()> {
        if !self.caps.is_empty() && !self.has_sasl("XOAUTH2") {
            log::debug!("pop3: XOAUTH2 not advertised, trying anyway");
        }
        let payload = b64(xoauth2_payload(user, token).as_bytes());
        let reply = self.command(&format!("AUTH XOAUTH2 {payload}"), Some("AUTH XOAUTH2 ********"))?;
        if reply.starts_with('+') {
            // Error details follow as a continuation; finish the exchange with an empty line.
            self.write_line("", None)?;
            self.read_status()?;
            return Err(NetError::Auth("token rejected".into()));
        }
        Ok(())
    }

    /// Number of messages and total size.
    pub fn stat(&mut self) -> NetResult<(u32, u64)> {
        let r = self.command("STAT", None)?;
        let mut w = r.split_whitespace();
        let n = w.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let size = w.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        Ok((n, size))
    }

    fn numbered_list(&mut self, cmd: &str) -> NetResult<Vec<(u32, String)>> {
        self.command(cmd, None)?;
        let body = self.read_multiline()?;
        Ok(String::from_utf8_lossy(&body)
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.parse().ok()?, w.next()?.to_string()))
            })
            .collect())
    }

    /// Unique ids of all messages (message number, uidl).
    pub fn uidl(&mut self) -> NetResult<Vec<(u32, String)>> {
        self.numbered_list("UIDL")
    }

    /// Sizes of all messages (message number, octets).
    pub fn list(&mut self) -> NetResult<Vec<(u32, u64)>> {
        Ok(self.numbered_list("LIST")?.into_iter().map(|(n, s)| (n, s.parse().unwrap_or(0))).collect())
    }

    /// Retrieve a full message.
    pub fn retr(&mut self, n: u32) -> NetResult<Vec<u8>> {
        self.command(&format!("RETR {n}"), None)?;
        self.read_multiline()
    }

    /// Retrieve the header and the first `lines` body lines of a message.
    pub fn top(&mut self, n: u32, lines: u32) -> NetResult<Vec<u8>> {
        self.command(&format!("TOP {n} {lines}"), None)?;
        self.read_multiline()
    }

    /// Mark a message for deletion (performed at QUIT).
    pub fn dele(&mut self, n: u32) -> NetResult<()> {
        self.command(&format!("DELE {n}"), None).map(|_| ())
    }

    pub fn noop(&mut self) -> NetResult<()> {
        self.command("NOOP", None).map(|_| ())
    }

    /// End the session, committing deletions.
    pub fn quit(&mut self) -> NetResult<()> {
        self.command("QUIT", None).map(|_| ())
    }
}

// ------------------------------------------------------------------ download state

/// A server message that has been downloaded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SeenEntry {
    /// When it was first downloaded (unix seconds).
    pub first_seen: i64,
    /// Local message created for it.
    pub local_id: Option<MessageId>,
}

/// Downloaded UIDLs of an account, persisted in the store's kv table.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SeenUidls {
    pub entries: BTreeMap<String, SeenEntry>,
}

/// kv key holding the downloaded UIDLs of an account.
pub fn kv_key(account: &str) -> String {
    format!("pop3:uidl:{account}")
}

impl SeenUidls {
    /// Load the state; `None` when the account was never downloaded.
    pub fn load(store: &Store, account: &str) -> Option<Self> {
        store.kv_get(&kv_key(account)).and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn save(&self, store: &Store, account: &str) {
        match serde_json::to_string(self) {
            Ok(s) => {
                if let Err(e) = store.kv_set(&kv_key(account), &s) {
                    log::warn!("pop3: could not save state: {e}");
                }
            }
            Err(e) => log::warn!("pop3: could not encode state: {e}"),
        }
    }
}

/// Whether a downloaded message should now be removed from the server.
pub fn should_remove(policy: Pop3Removal, entry: &SeenEntry, now: i64, in_inbox: bool) -> bool {
    let age = now - entry.first_seen;
    match policy {
        Pop3Removal::Never => false,
        Pop3Removal::OneDay => age >= 86400,
        Pop3Removal::OneWeek => age >= 7 * 86400,
        Pop3Removal::OneMonth => age >= 30 * 86400,
        Pop3Removal::WhenMovedFromInbox => !in_inbox,
    }
}

/// Outcome of a POP download.
#[derive(Debug, Default)]
pub struct DownloadResult {
    /// Local ids of the newly downloaded messages.
    pub new_ids: Vec<MessageId>,
    /// Whether this was the first download for the account.
    pub initial: bool,
    /// Messages removed from the server.
    pub removed: usize,
}

/// Download new messages into `inbox` and apply the removal policy.
/// `progress(done, total)` is called while downloading. The caller sends QUIT.
pub fn download(c: &mut Pop3Client, store: &Store, acct: &AccountConfig, inbox: MailboxId, progress: &mut dyn FnMut(usize, usize)) -> NetResult<DownloadResult> {
    let loaded = SeenUidls::load(store, &acct.id);
    let initial = loaded.is_none();
    let mut seen = loaded.unwrap_or_default();
    let sizes: BTreeMap<u32, u64> = c.list()?.into_iter().collect();
    let uidls: Vec<(u32, String)> = match c.uidl() {
        Ok(v) => v,
        Err(NetError::No(_)) => fallback_uidls(c, &sizes)?,
        Err(e) => return Err(e),
    };
    let mut result = DownloadResult { initial, ..Default::default() };
    let on_server: HashSet<&str> = uidls.iter().map(|(_, u)| u.as_str()).collect();
    // Forget messages that disappeared from the server.
    seen.entries.retain(|k, _| on_server.contains(k.as_str()));

    let new: Vec<&(u32, String)> = uidls.iter().filter(|(_, u)| !seen.entries.contains_key(u)).collect();
    let total = new.len();
    let warn_kb = acct.pop3_size_warning_kb as u64;
    for (i, (n, uidl)) in new.iter().enumerate() {
        progress(i, total);
        if warn_kb > 0 && sizes.get(n).copied().unwrap_or(0) > warn_kb * 1024 {
            log::info!("pop3: message {uidl} is larger than {warn_kb} KB");
        }
        let raw = c.retr(*n)?;
        let local_id = store_message(store, acct, inbox, &raw);
        if let Some(id) = local_id {
            result.new_ids.push(id);
        }
        seen.entries.insert(uidl.clone(), SeenEntry { first_seen: now_ts(), local_id });
        // Persist incrementally so an interrupted download does not duplicate messages.
        if i % 10 == 9 {
            seen.save(store, &acct.id);
        }
    }
    progress(total, total);

    // Removal policy.
    let now = now_ts();
    let numbers: BTreeMap<&str, u32> = uidls.iter().map(|(n, u)| (u.as_str(), *n)).collect();
    for (uidl, entry) in &seen.entries {
        let in_inbox = entry.local_id.and_then(|id| store.message(id)).map(|m| m.mailbox_id == inbox).unwrap_or(false);
        if should_remove(acct.pop3_remove, entry, now, in_inbox)
            && let Some(n) = numbers.get(uidl.as_str()) {
                c.dele(*n)?;
                result.removed += 1;
            }
    }
    seen.save(store, &acct.id);
    Ok(result)
}

/// Servers without UIDL: identify messages by Message-ID (from TOP) or size.
fn fallback_uidls(c: &mut Pop3Client, sizes: &BTreeMap<u32, u64>) -> NetResult<Vec<(u32, String)>> {
    let mut out = Vec::new();
    for (n, size) in sizes {
        let head = c.top(*n, 0).unwrap_or_default();
        let id = mime::parse(&head).map(|p| p.message_id).filter(|m| !m.is_empty()).unwrap_or_else(|| format!("size-{size}-{n}"));
        out.push((*n, id));
    }
    Ok(out)
}

/// Insert a downloaded message into the store.
fn store_message(store: &Store, acct: &AccountConfig, inbox: MailboxId, raw: &[u8]) -> Option<MessageId> {
    let p = mime::parse(raw)?;
    let mut s: MessageSummary = mime::summarize(&p, raw.len());
    s.mailbox_id = inbox;
    s.account_id = acct.id.clone();
    s.uid = 0;
    s.flags = flags::HAS_BODY;
    s.category = categorize::categorize(&p);
    match store.insert_message(&s, Some(raw), &p.plain_text()) {
        Ok(id) => Some(id),
        Err(e) => {
            log::warn!("pop3: could not store message: {e}");
            None
        }
    }
}

// ------------------------------------------------------------------ MD5 (for APOP)

/// MD5 digest as lowercase hex (RFC 1321). Only used for APOP.
pub fn md5_hex(data: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16,
        23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0].iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
        // RFC 1939 APOP example
        assert_eq!(md5_hex(b"<1896.697170952@dbc.mtview.ca.us>tanstaaf"), "c4c9334bac560ecc979e58001b3e22fb");
    }

    #[test]
    fn removal_policy() {
        let e = SeenEntry { first_seen: 0, local_id: None };
        assert!(!should_remove(Pop3Removal::Never, &e, 10 * 86400, false));
        assert!(should_remove(Pop3Removal::OneWeek, &e, 8 * 86400, true));
        assert!(!should_remove(Pop3Removal::OneMonth, &e, 8 * 86400, true));
        assert!(should_remove(Pop3Removal::WhenMovedFromInbox, &e, 0, false));
        assert!(!should_remove(Pop3Removal::WhenMovedFromInbox, &e, 0, true));
    }
}
