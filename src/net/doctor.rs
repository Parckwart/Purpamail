//! Connection doctor: tests connectivity and authentication of incoming and outgoing
//! servers and records a protocol log with secrets redacted.

use std::io::{BufRead, BufReader, Read, Write};

use super::imap_client::{ImapClient, MailStream, NetError, NetResult, Transcript, b64, open_stream, xoauth2_payload};
use super::pop3::Pop3Client;
use super::smtp::smtp_username;
use super::{DoctorResult, ServiceStatus};
use crate::core::config::{AccountConfig, AccountKind, AuthMethod, Secrets, Security, ServerConfig};

/// Test the incoming server: connect, authenticate and log out.
pub fn test_incoming(acct: &AccountConfig, password: &str, log: &mut Vec<String>) -> Result<(), String> {
    let res = match acct.kind {
        AccountKind::Imap => test_imap(&acct.incoming, password, log),
        AccountKind::Pop3 => test_pop3(&acct.incoming, password, log),
    };
    res.map_err(|e| {
        log.push(format!("!! {e}"));
        e.user_message()
    })
}

fn test_imap(server: &ServerConfig, password: &str, log: &mut Vec<String>) -> NetResult<()> {
    let mut c = match ImapClient::connect(server, true) {
        Ok(c) => c,
        Err(e) => {
            log.push(format!("-- Connecting to {}:{} ({}) failed", server.host, server.port, server.security.name()));
            return Err(e);
        }
    };
    let r = c.login(server, password).and_then(|_| {
        c.transcript.note(&format!("Capabilities: {}", c.capabilities().join(" ")));
        c.list().map(|l| c.transcript.note(&format!("{} mailboxes", l.len())))
    });
    if r.is_ok() {
        c.logout();
    }
    log.extend(c.transcript.take());
    r
}

fn test_pop3(server: &ServerConfig, password: &str, log: &mut Vec<String>) -> NetResult<()> {
    let mut c = match Pop3Client::connect(server, true) {
        Ok(c) => c,
        Err(e) => {
            log.push(format!("-- Connecting to {}:{} ({}) failed", server.host, server.port, server.security.name()));
            return Err(e);
        }
    };
    let r = c.login(server, password).and_then(|_| c.stat()).map(|(n, size)| c.transcript.note(&format!("{n} messages, {size} bytes")));
    let _ = c.quit();
    log.extend(c.transcript.take());
    r
}

/// Test the outgoing server: connect, (STARTTLS), authenticate and quit.
pub fn test_outgoing(acct: &AccountConfig, password: &str, log: &mut Vec<String>) -> Result<(), String> {
    let mut probe = SmtpProbe { io: None, transcript: Transcript::new(true) };
    let r = probe.run(&acct.outgoing, &smtp_username(acct), password);
    log.extend(probe.transcript.take());
    r.map_err(|e| {
        log.push(format!("!! {e}"));
        e.user_message()
    })
}

/// Diagnose one configured account.
pub fn diagnose(acct: &AccountConfig, secrets: &Secrets) -> DoctorResult {
    let mut log = vec![format!("== {} ({})", acct.display_name(), acct.primary_email())];
    log.push(format!("== Incoming: {} server {}:{}", if acct.kind == AccountKind::Imap { "IMAP" } else { "POP" }, acct.incoming.host, acct.incoming.port));
    let incoming = match test_incoming(acct, &secrets.incoming(&acct.id), &mut log) {
        Ok(()) => ServiceStatus::Ok,
        Err(e) => ServiceStatus::Failed(e),
    };
    log.push(format!("== Outgoing: SMTP server {}:{}", acct.outgoing.host, acct.outgoing.port));
    let outgoing = match test_outgoing(acct, &secrets.outgoing(acct), &mut log) {
        Ok(()) => ServiceStatus::Ok,
        Err(e) => ServiceStatus::Failed(e),
    };
    DoctorResult { account: acct.id.clone(), account_name: acct.display_name().to_string(), incoming, outgoing, log: log.join("\n") }
}

/// Test an account configuration before it is saved. The password is used for both servers.
pub fn test_account(acct: &AccountConfig, password: &str) -> (Result<(), String>, Result<(), String>) {
    let mut log = Vec::new();
    let incoming = test_incoming(acct, password, &mut log);
    let outgoing = test_outgoing(acct, password, &mut log);
    log::debug!("account test log:\n{}", log.join("\n"));
    (incoming, outgoing)
}

/// A minimal SMTP client used only for diagnostics, so that each step can be logged.
struct SmtpProbe {
    io: Option<BufReader<MailStream>>,
    transcript: Transcript,
}

impl SmtpProbe {
    fn io(&mut self) -> NetResult<&mut BufReader<MailStream>> {
        self.io.as_mut().ok_or_else(|| NetError::Protocol("not connected".into()))
    }

    fn send(&mut self, line: &str, shown: Option<&str>) -> NetResult<()> {
        self.transcript.client(shown.unwrap_or(line));
        let w = self.io()?.get_mut();
        w.write_all(line.as_bytes())?;
        w.write_all(b"\r\n")?;
        w.flush()?;
        Ok(())
    }

    /// Read a (possibly multi-line) reply; returns the code and the text lines.
    fn reply(&mut self) -> NetResult<(u16, Vec<String>)> {
        let mut lines = Vec::new();
        loop {
            let mut buf = Vec::new();
            let n = (&mut *self.io()?).take(8192).read_until(b'\n', &mut buf)?;
            if n == 0 {
                return Err(NetError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the server closed the connection")));
            }
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            self.transcript.server(&line);
            let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| NetError::Protocol(format!("unexpected reply: {line}")))?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_string());
            if !more {
                return Ok((code, lines));
            }
        }
    }

    fn expect(&mut self, ok: &[u16]) -> NetResult<Vec<String>> {
        let (code, lines) = self.reply()?;
        if ok.contains(&code) { Ok(lines) } else { Err(NetError::No(format!("{code} {}", lines.join(" ")))) }
    }

    fn ehlo(&mut self) -> NetResult<Vec<String>> {
        let name = hello_name();
        self.send(&format!("EHLO {name}"), None)?;
        match self.reply()? {
            (250, lines) => Ok(lines),
            _ => {
                self.send(&format!("HELO {name}"), None)?;
                self.expect(&[250])
            }
        }
    }

    fn run(&mut self, server: &ServerConfig, user: &str, password: &str) -> NetResult<()> {
        self.transcript.note(&format!("Connecting to {}:{} ({})", server.host, server.port, server.security.name()));
        self.io = Some(BufReader::new(open_stream(server)?));
        self.expect(&[220])?;
        let mut caps = self.ehlo()?;
        if server.security == Security::StartTls {
            self.send("STARTTLS", None)?;
            self.expect(&[220]).map_err(|e| NetError::Tls(format!("STARTTLS rejected: {e}")))?;
            let old = self.io.take().ok_or_else(|| NetError::Protocol("not connected".into()))?;
            self.io = Some(BufReader::new(old.into_inner().upgrade(&server.host, server.allow_insecure_certs)?));
            self.transcript.note("TLS negotiated");
            caps = self.ehlo()?;
        }
        let mechs: Vec<String> = caps
            .iter()
            .filter_map(|l| {
                let u = l.to_ascii_uppercase();
                u.strip_prefix("AUTH ").or_else(|| u.strip_prefix("AUTH=")).map(|s| s.to_string())
            })
            .flat_map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>())
            .collect();
        match server.auth {
            AuthMethod::None => {}
            AuthMethod::Password => {
                if mechs.iter().any(|m| m == "PLAIN") || mechs.is_empty() {
                    let token = b64(format!("\0{user}\0{password}").as_bytes());
                    self.send(&format!("AUTH PLAIN {token}"), Some("AUTH PLAIN ********"))?;
                    self.expect(&[235]).map_err(auth_err)?;
                } else if mechs.iter().any(|m| m == "LOGIN") {
                    self.send("AUTH LOGIN", None)?;
                    self.expect(&[334]).map_err(auth_err)?;
                    self.send(&b64(user.as_bytes()), Some("********"))?;
                    self.expect(&[334]).map_err(auth_err)?;
                    self.send(&b64(password.as_bytes()), Some("********"))?;
                    self.expect(&[235]).map_err(auth_err)?;
                } else {
                    return Err(NetError::Auth(format!("no supported authentication method (server offers {})", mechs.join(" "))));
                }
            }
            AuthMethod::OAuth2 => {
                let token = b64(xoauth2_payload(user, password).as_bytes());
                self.send(&format!("AUTH XOAUTH2 {token}"), Some("AUTH XOAUTH2 ********"))?;
                let (code, lines) = self.reply()?;
                if code == 334 {
                    self.send("", None)?;
                    let _ = self.reply();
                    return Err(NetError::Auth("token rejected".into()));
                }
                if code != 235 {
                    return Err(NetError::Auth(format!("{code} {}", lines.join(" "))));
                }
            }
        }
        self.send("QUIT", None)?;
        let _ = self.reply();
        Ok(())
    }
}

fn auth_err(e: NetError) -> NetError {
    match e {
        NetError::No(t) => NetError::Auth(t),
        other => other,
    }
}

/// Name used in EHLO.
fn hello_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .unwrap_or_else(|| "localhost".into())
}
