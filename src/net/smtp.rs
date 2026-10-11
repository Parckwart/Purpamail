//! SMTP delivery (via `lettre`) and helpers to reconstruct Outbox messages.

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{SmtpTransport, Transport};

use crate::core::compose::OutgoingMessage;
use crate::core::config::{AccountConfig, AuthMethod, Security, ServerConfig, Settings};
use crate::core::mime;
use crate::core::model::Address;

/// Timeout for SMTP operations.
const SMTP_TIMEOUT: Duration = Duration::from_secs(90);

/// Why a delivery failed.
#[derive(Debug, Clone, PartialEq)]
pub struct SendError {
    pub message: String,
    /// The server rejected the message permanently (5xx); retrying will not help.
    pub permanent: bool,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl SendError {
    fn new(message: impl Into<String>, permanent: bool) -> Self {
        Self { message: message.into(), permanent }
    }
}

/// User name for the SMTP server (falls back to the incoming user name when shared credentials are used).
pub fn smtp_username(acct: &AccountConfig) -> String {
    if acct.outgoing.username.is_empty() { acct.incoming.username.clone() } else { acct.outgoing.username.clone() }
}

/// Build a lettre transport for an SMTP server.
fn transport(server: &ServerConfig, username: &str, password: &str) -> Result<SmtpTransport, SendError> {
    let params = TlsParameters::builder(server.host.clone())
        .dangerous_accept_invalid_certs(server.allow_insecure_certs)
        .dangerous_accept_invalid_hostnames(server.allow_insecure_certs)
        .build()
        .map_err(|e| SendError::new(format!("TLS setup failed: {e}"), false))?;
    let tls = match server.security {
        Security::Tls => Tls::Wrapper(params),
        Security::StartTls => Tls::Required(params),
        Security::None => Tls::None,
    };
    let mut b = SmtpTransport::builder_dangerous(server.host.clone()).port(server.port).tls(tls).timeout(Some(SMTP_TIMEOUT));
    match server.auth {
        AuthMethod::Password => {
            b = b.credentials(Credentials::new(username.to_string(), password.to_string())).authentication(vec![Mechanism::Plain, Mechanism::Login]);
        }
        AuthMethod::OAuth2 => {
            b = b.credentials(Credentials::new(username.to_string(), password.to_string())).authentication(vec![Mechanism::Xoauth2]);
        }
        AuthMethod::None => {}
    }
    Ok(b.build())
}

/// Deliver raw message bytes through the account's SMTP server.
pub fn deliver(acct: &AccountConfig, password: &str, from: &str, recipients: &[String], data: &[u8]) -> Result<(), SendError> {
    if acct.outgoing.host.trim().is_empty() {
        return Err(SendError::new("No outgoing mail server is configured for this account.", true));
    }
    if recipients.is_empty() {
        return Err(SendError::new("The message has no recipients.", true));
    }
    let from_addr = if from.is_empty() {
        None
    } else {
        Some(from.parse::<lettre::Address>().map_err(|e| SendError::new(format!("Invalid sender address “{from}”: {e}"), true))?)
    };
    let mut to = Vec::new();
    for r in recipients {
        to.push(r.parse::<lettre::Address>().map_err(|e| SendError::new(format!("Invalid recipient address “{r}”: {e}"), true))?);
    }
    let envelope = Envelope::new(from_addr, to).map_err(|e| SendError::new(e.to_string(), true))?;
    let t = transport(&acct.outgoing, &smtp_username(acct), password)?;
    match t.send_raw(&envelope, data) {
        Ok(_) => Ok(()),
        Err(e) => Err(SendError::new(describe_error(&e), e.is_permanent())),
    }
}

fn describe_error(e: &lettre::transport::smtp::Error) -> String {
    let s = e.to_string();
    if e.is_permanent() {
        format!("The server rejected the message: {s}")
    } else if e.is_transient() {
        format!("The server temporarily refused the message: {s}")
    } else {
        format!("Could not send the message: {s}")
    }
}

/// Envelope sender and recipients for an outgoing message.
///
/// Recipients are To + Cc + Bcc, plus the sender itself when "automatically Bcc myself"
/// is enabled for the account or globally. Redirected messages use the account address
/// as envelope sender.
pub fn envelope(msg: &OutgoingMessage, acct: &AccountConfig, settings: &Settings) -> (String, Vec<String>) {
    let from = if msg.redirect_raw.is_some() || msg.from.email.is_empty() { acct.primary_email().to_string() } else { msg.from.email.clone() };
    let mut rcpts: Vec<String> = Vec::new();
    let mut push = |e: &str| {
        let e = e.trim();
        if !e.is_empty() && !rcpts.iter().any(|x| x.eq_ignore_ascii_case(e)) {
            rcpts.push(e.to_string());
        }
    };
    for a in msg.all_recipients() {
        push(&a.email);
    }
    if msg.redirect_raw.is_none() && (acct.auto_bcc_myself || settings.composing.auto_bcc_myself) {
        let me = if msg.from.email.is_empty() { acct.primary_email().to_string() } else { msg.from.email.clone() };
        push(&me);
    }
    (from, rcpts)
}

/// Remove `Bcc:` (and `Resent-Bcc:`) header fields, including folded continuation lines,
/// from a raw RFC 5322 message. The body is left untouched.
pub fn strip_bcc(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut pos = 0;
    let mut skipping = false;
    while pos < raw.len() {
        let end = raw[pos..].iter().position(|&c| c == b'\n').map(|p| pos + p + 1).unwrap_or(raw.len());
        let line = &raw[pos..end];
        let content = line.strip_suffix(b"\n").map(|l| l.strip_suffix(b"\r").unwrap_or(l)).unwrap_or(line);
        if content.is_empty() {
            // End of the header block: copy the rest verbatim.
            out.extend_from_slice(&raw[pos..]);
            return out;
        }
        let continuation = matches!(content.first(), Some(b' ') | Some(b'\t'));
        if !continuation {
            let lower = content.to_ascii_lowercase();
            skipping = lower.starts_with(b"bcc:") || lower.starts_with(b"resent-bcc:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
        pos = end;
    }
    out
}

/// A message reconstructed from the raw source of an Outbox row.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub message: OutgoingMessage,
    /// Bytes to transmit (Bcc removed).
    pub data: Vec<u8>,
}

/// Rebuild an [`OutgoingMessage`] from raw Outbox source so it can be delivered.
/// The sending account is determined from the From address (`Resent-From` for redirects).
pub fn queued_from_raw(raw: &[u8], settings: &Settings) -> Option<QueuedMessage> {
    let p = mime::parse(raw)?;
    let resent_to = p.header("Resent-To").map(Address::parse_list).unwrap_or_default();
    let mut msg = OutgoingMessage {
        from: p.from.clone(),
        to: p.to.clone(),
        cc: p.cc.clone(),
        bcc: p.bcc.clone(),
        reply_to: p.reply_to.clone(),
        subject: p.subject.clone(),
        text: p.plain_text(),
        html: p.html.clone(),
        in_reply_to: p.in_reply_to.clone(),
        references: p.references.clone(),
        priority: p.priority,
        message_id: p.message_id.clone(),
        ..Default::default()
    };
    let sender = if resent_to.is_empty() {
        p.from.email.clone()
    } else {
        p.header("Resent-From").and_then(Address::parse_one).map(|a| a.email).unwrap_or_else(|| p.from.email.clone())
    };
    msg.account_id = settings.account_for_address(&sender).or_else(|| settings.account_for_address(&p.from.email)).map(|a| a.id.clone()).unwrap_or_default();
    if !resent_to.is_empty() {
        // Redirect: deliver the stored source unchanged to the Resent-To recipients.
        let resent_cc = p.header("Resent-Cc").map(Address::parse_list).unwrap_or_default();
        let resent_bcc = p.header("Resent-Bcc").map(Address::parse_list).unwrap_or_default();
        msg.to = resent_to;
        msg.cc = resent_cc;
        msg.bcc = resent_bcc;
        msg.redirect_raw = Some(raw.to_vec());
    }
    Some(QueuedMessage { message: msg, data: strip_bcc(raw) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AccountConfig;

    #[test]
    fn strips_bcc_headers() {
        let raw = b"From: a@x.com\r\nBcc: secret@x.com,\r\n other@x.com\r\nTo: b@x.com\r\nSubject: hi\r\n\r\nBcc: in body stays\r\n";
        let s = String::from_utf8(strip_bcc(raw)).unwrap();
        assert!(!s.contains("secret@"));
        assert!(!s.contains("other@"));
        assert!(s.contains("To: b@x.com\r\n"));
        assert!(s.contains("Bcc: in body stays"));
    }

    #[test]
    fn envelope_includes_bcc_and_self() {
        let mut settings = Settings::default();
        let acct = AccountConfig { id: "a".into(), emails: vec!["me@x.com".into()], auto_bcc_myself: true, ..Default::default() };
        settings.accounts.push(acct.clone());
        let msg = OutgoingMessage {
            from: Address::new("Me", "me@x.com"),
            to: vec![Address::new("", "you@x.com")],
            bcc: vec![Address::new("", "hidden@x.com")],
            ..Default::default()
        };
        let (from, rcpts) = envelope(&msg, &acct, &settings);
        assert_eq!(from, "me@x.com");
        assert_eq!(rcpts, vec!["you@x.com", "hidden@x.com", "me@x.com"]);
    }

    #[test]
    fn rebuilds_queued_message() {
        let mut settings = Settings::default();
        settings.accounts.push(AccountConfig { id: "a".into(), emails: vec!["me@x.com".into()], ..Default::default() });
        let msg = OutgoingMessage {
            from: Address::new("Me", "me@x.com"),
            to: vec![Address::new("You", "you@x.com")],
            bcc: vec![Address::new("", "hidden@x.com")],
            subject: "Hello".into(),
            text: "Body".into(),
            message_id: "id1@x.com".into(),
            ..Default::default()
        };
        let raw = msg.to_mime(true);
        let q = queued_from_raw(&raw, &settings).unwrap();
        assert_eq!(q.message.account_id, "a");
        assert_eq!(q.message.bcc[0].email, "hidden@x.com");
        assert_eq!(q.message.message_id, "id1@x.com");
        assert!(!String::from_utf8_lossy(&q.data).contains("hidden@x.com"));
    }
}
