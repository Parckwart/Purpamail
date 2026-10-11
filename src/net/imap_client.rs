//! A small, defensive IMAP4rev1 client (RFC 3501).
//!
//! Besides the base protocol it understands the extensions Purpamail relies on:
//! IDLE (RFC 2177), MOVE (RFC 6851), UIDPLUS (RFC 4315), LITERAL+ (RFC 7888),
//! SASL-IR (RFC 4959), SPECIAL-USE (RFC 6154) and the XOAUTH2 SASL mechanism.
//!
//! Everything received from the server is treated as untrusted: malformed
//! responses produce [`NetError::Protocol`] instead of panics, literal sizes are
//! bounded and nesting depth is limited.
//!
//! This module also hosts the transport pieces shared with the POP3 client, the
//! SMTP probe and the connection doctor: [`MailStream`], [`open_stream`],
//! [`NetError`] and [`Transcript`].

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;

use crate::core::config::{AuthMethod, Security, ServerConfig};
use crate::core::model::{FLAG_COLOR_KEYWORDS, FlagColor, flags};

/// Timeout for establishing TCP connections.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Read / write timeout for regular protocol exchanges.
pub const IO_TIMEOUT: Duration = Duration::from_secs(90);
/// Largest literal we accept from a server (protects against memory exhaustion).
const MAX_LITERAL: usize = 256 * 1024 * 1024;
/// Maximum list nesting depth accepted by the response parser.
const MAX_DEPTH: usize = 64;

// ------------------------------------------------------------------ errors

/// Errors produced by the mail protocol clients.
#[derive(Debug)]
pub enum NetError {
    /// Transport failure (connection refused, reset, timeout, ...).
    Io(io::Error),
    /// TLS negotiation or certificate failure.
    Tls(String),
    /// The server rejected the credentials.
    Auth(String),
    /// The server refused a command (IMAP `NO`, POP3 `-ERR`, SMTP 5xx).
    No(String),
    /// The server did not understand a command (IMAP `BAD`).
    Bad(String),
    /// The server closed the session (`BYE`).
    Bye(String),
    /// The server sent something we could not make sense of.
    Protocol(String),
}

impl NetError {
    /// Whether the connection should be considered unusable after this error.
    pub fn is_connection(&self) -> bool {
        matches!(self, NetError::Io(_) | NetError::Tls(_) | NetError::Bye(_) | NetError::Protocol(_))
    }

    /// Whether the error was caused by a socket timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self, NetError::Io(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
    }

    /// Human readable explanation suitable for the UI.
    pub fn user_message(&self) -> String {
        match self {
            NetError::Io(e) => match e.kind() {
                io::ErrorKind::ConnectionRefused => "The server refused the connection.".into(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "The connection timed out.".into(),
                io::ErrorKind::NotFound => format!("The server could not be found ({e})."),
                _ => format!("Connection error: {e}"),
            },
            NetError::Tls(e) => format!("Secure connection failed: {e}"),
            NetError::Auth(e) => format!("The user name or password was rejected: {e}"),
            NetError::No(e) | NetError::Bad(e) => format!("The server reported an error: {e}"),
            NetError::Bye(e) => format!("The server closed the connection: {e}"),
            NetError::Protocol(e) => format!("Unexpected response from the server: {e}"),
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::Io(e) => write!(f, "I/O error: {e}"),
            NetError::Tls(e) => write!(f, "TLS error: {e}"),
            NetError::Auth(e) => write!(f, "authentication failed: {e}"),
            NetError::No(e) => write!(f, "server refused: {e}"),
            NetError::Bad(e) => write!(f, "bad command: {e}"),
            NetError::Bye(e) => write!(f, "server closed connection: {e}"),
            NetError::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        NetError::Io(e)
    }
}

pub type NetResult<T> = Result<T, NetError>;

// ------------------------------------------------------------------ transcript

/// Optional protocol log used by the connection doctor. Secrets are never recorded:
/// callers mark sensitive lines and they are replaced by a placeholder.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Option<Vec<String>>,
}

impl Transcript {
    const MAX_LINES: usize = 400;
    const MAX_LINE_LEN: usize = 240;

    pub fn new(enabled: bool) -> Self {
        Self { lines: enabled.then(Vec::new) }
    }

    pub fn enabled(&self) -> bool {
        self.lines.is_some()
    }

    fn push(&mut self, prefix: &str, text: &str) {
        if let Some(lines) = &mut self.lines {
            if lines.len() >= Self::MAX_LINES {
                return;
            }
            let mut t: String = text.trim_end_matches(['\r', '\n']).chars().take(Self::MAX_LINE_LEN).collect();
            if text.chars().count() > Self::MAX_LINE_LEN {
                t.push('…');
            }
            lines.push(format!("{prefix}{t}"));
        }
    }

    /// Record a line sent by the client.
    pub fn client(&mut self, text: &str) {
        self.push("C: ", text);
    }

    /// Record a line received from the server.
    pub fn server(&mut self, text: &str) {
        self.push("S: ", text);
    }

    /// Record a free-form note (connection attempts, TLS state...).
    pub fn note(&mut self, text: &str) {
        self.push("-- ", text);
    }

    /// Take the recorded lines.
    pub fn take(&mut self) -> Vec<String> {
        match &mut self.lines {
            Some(l) => std::mem::take(l),
            None => Vec::new(),
        }
    }
}

// ------------------------------------------------------------------ transport

/// A plain or TLS protected TCP stream.
pub enum MailStream {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
    /// Placeholder used while a stream is being upgraded or after it was shut down.
    Closed,
}

impl MailStream {
    /// The underlying TCP socket.
    pub fn tcp(&self) -> Option<&TcpStream> {
        match self {
            MailStream::Plain(s) => Some(s),
            MailStream::Tls(s) => Some(s.get_ref()),
            MailStream::Closed => None,
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, MailStream::Tls(_))
    }

    pub fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        match self.tcp() {
            Some(s) => s.set_read_timeout(t),
            None => Ok(()),
        }
    }

    /// Upgrade a plain stream to TLS (STARTTLS / STLS).
    pub fn upgrade(self, host: &str, insecure: bool) -> NetResult<MailStream> {
        match self {
            MailStream::Plain(tcp) => wrap_tls(tcp, host, insecure),
            other => Ok(other),
        }
    }
}

fn closed_err() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "connection closed")
}

impl Read for MailStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MailStream::Plain(s) => s.read(buf),
            MailStream::Tls(s) => s.read(buf),
            MailStream::Closed => Err(closed_err()),
        }
    }
}

impl Write for MailStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MailStream::Plain(s) => s.write(buf),
            MailStream::Tls(s) => s.write(buf),
            MailStream::Closed => Err(closed_err()),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            MailStream::Plain(s) => s.flush(),
            MailStream::Tls(s) => s.flush(),
            MailStream::Closed => Ok(()),
        }
    }
}

/// Build a TLS connector, optionally accepting invalid certificates.
pub fn tls_connector(insecure: bool) -> NetResult<native_tls::TlsConnector> {
    native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(insecure)
        .danger_accept_invalid_hostnames(insecure)
        .build()
        .map_err(|e| NetError::Tls(e.to_string()))
}

fn wrap_tls(tcp: TcpStream, host: &str, insecure: bool) -> NetResult<MailStream> {
    let connector = tls_connector(insecure)?;
    match connector.connect(host, tcp) {
        Ok(s) => Ok(MailStream::Tls(Box::new(s))),
        Err(native_tls::HandshakeError::Failure(e)) => Err(NetError::Tls(e.to_string())),
        Err(native_tls::HandshakeError::WouldBlock(_)) => Err(NetError::Tls("TLS handshake timed out".into())),
    }
}

/// Open a TCP connection (trying every resolved address) with sane timeouts.
pub fn open_tcp(host: &str, port: u16) -> NetResult<TcpStream> {
    if host.trim().is_empty() {
        return Err(NetError::Io(io::Error::new(io::ErrorKind::InvalidInput, "no server name configured")));
    }
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| NetError::Io(io::Error::new(io::ErrorKind::NotFound, format!("{host}: {e}"))))?
        .collect();
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host}: no addresses"));
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(NetError::Io(last))
}

/// Connect to `server`; with [`Security::Tls`] the TLS handshake happens immediately.
/// STARTTLS upgrades are protocol specific and done by the callers.
pub fn open_stream(server: &ServerConfig) -> NetResult<MailStream> {
    let tcp = open_tcp(&server.host, server.port)?;
    match server.security {
        Security::Tls => wrap_tls(tcp, &server.host, server.allow_insecure_certs),
        Security::StartTls | Security::None => Ok(MailStream::Plain(tcp)),
    }
}

/// SASL XOAUTH2 initial client response (not yet base64 encoded).
pub fn xoauth2_payload(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// Base64 (standard alphabet, padded).
pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ------------------------------------------------------------------ response values

/// A parsed IMAP data item.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Atom(String),
    /// Quoted string or literal.
    Str(Vec<u8>),
    List(Vec<Value>),
    Nil,
}

impl Value {
    /// Atom or string content as text.
    pub fn text(&self) -> Option<String> {
        match self {
            Value::Atom(a) => Some(a.clone()),
            Value::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
            _ => None,
        }
    }
    pub fn number(&self) -> Option<u64> {
        match self {
            Value::Atom(a) => a.parse().ok(),
            Value::Str(s) => std::str::from_utf8(s).ok()?.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s),
            Value::Atom(a) => Some(a.as_bytes()),
            _ => None,
        }
    }
}

struct Tokenizer<'a> {
    b: &'a [u8],
    i: usize,
}

impl Tokenizer<'_> {
    fn skip_spaces(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] == b' ' || self.b[self.i] == b'\r' || self.b[self.i] == b'\n') {
            self.i += 1;
        }
    }

    fn values(&mut self, close: Option<u8>, depth: usize) -> Result<Vec<Value>, String> {
        if depth > MAX_DEPTH {
            return Err("nesting too deep".into());
        }
        let mut out = Vec::new();
        loop {
            self.skip_spaces();
            let Some(&c) = self.b.get(self.i) else {
                return match close {
                    Some(_) => Err("unterminated list".into()),
                    None => Ok(out),
                };
            };
            if Some(c) == close {
                self.i += 1;
                return Ok(out);
            }
            match c {
                b'(' => {
                    self.i += 1;
                    out.push(Value::List(self.values(Some(b')'), depth + 1)?));
                }
                b')' => return Err("unexpected ')'".into()),
                b'"' => out.push(self.quoted()?),
                b'{' => out.push(self.literal()?),
                _ => out.push(self.atom()),
            }
        }
    }

    fn quoted(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut s = Vec::new();
        while let Some(&c) = self.b.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => {
                    if let Some(&n) = self.b.get(self.i) {
                        s.push(n);
                        self.i += 1;
                    }
                }
                b'"' => return Ok(Value::Str(s)),
                _ => s.push(c),
            }
        }
        Err("unterminated quoted string".into())
    }

    fn literal(&mut self) -> Result<Value, String> {
        let start = self.i + 1;
        let end = self.b[start..].iter().position(|&c| c == b'}').map(|p| start + p).ok_or("bad literal")?;
        let digits = std::str::from_utf8(&self.b[start..end]).map_err(|_| "bad literal")?.trim_end_matches('+');
        let n: usize = digits.parse().map_err(|_| "bad literal length")?;
        let mut p = end + 1;
        if self.b.get(p) == Some(&b'\r') {
            p += 1;
        }
        if self.b.get(p) == Some(&b'\n') {
            p += 1;
        }
        if p + n > self.b.len() {
            return Err("truncated literal".into());
        }
        self.i = p + n;
        Ok(Value::Str(self.b[p..p + n].to_vec()))
    }

    fn atom(&mut self) -> Value {
        let start = self.i;
        let mut depth = 0usize;
        while let Some(&c) = self.b.get(self.i) {
            if depth > 0 {
                match c {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                self.i += 1;
                continue;
            }
            match c {
                b' ' | b'(' | b')' | b'"' | b'\r' | b'\n' => break,
                b'[' => depth += 1,
                _ => {}
            }
            self.i += 1;
        }
        if self.i == start {
            // Unknown single character: consume it so parsing always advances.
            self.i += 1;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
        if s.eq_ignore_ascii_case("NIL") { Value::Nil } else { Value::Atom(s) }
    }
}

/// Parse a sequence of IMAP data items (atoms, strings, literals, lists).
pub fn parse_values(input: &[u8]) -> Result<Vec<Value>, String> {
    Tokenizer { b: input, i: 0 }.values(None, 0)
}

/// Split status text into an optional response code and the human readable rest:
/// `"[UIDNEXT 5] Predicted"` → `(Some("UIDNEXT 5"), "Predicted")`.
pub fn split_code(text: &str) -> (Option<String>, String) {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix('[') {
        // Response codes may contain nested brackets (e.g. BADCHARSET lists); match them.
        let mut depth = 1usize;
        for (i, c) in rest.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return (Some(rest[..i].to_string()), rest[i + 1..].trim().to_string());
                    }
                }
                _ => {}
            }
        }
    }
    (None, t.to_string())
}

// ------------------------------------------------------------------ public result types

/// Result of a completed (tagged OK) command.
#[derive(Debug, Default, Clone)]
pub struct Response {
    /// Untagged response lines (without the leading `* `), literals inlined.
    pub untagged: Vec<Vec<u8>>,
    /// Response code of the tagged completion, e.g. `APPENDUID 1 42`.
    pub code: Option<String>,
    pub text: String,
}

/// One entry of a LIST response.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    /// Mailbox name decoded from modified UTF-7.
    pub path: String,
    pub delimiter: Option<String>,
    /// Name attributes such as `\Noselect` or `\Sent`.
    pub attributes: Vec<String>,
}

impl ListEntry {
    pub fn has_attr(&self, a: &str) -> bool {
        self.attributes.iter().any(|x| x.eq_ignore_ascii_case(a))
    }
    /// Whether the mailbox can be selected.
    pub fn selectable(&self) -> bool {
        !self.has_attr("\\Noselect") && !self.has_attr("\\NonExistent")
    }
    /// Last path component.
    pub fn name(&self) -> String {
        match &self.delimiter {
            Some(d) if !d.is_empty() => self.path.rsplit(d.as_str()).next().unwrap_or(&self.path).to_string(),
            _ => self.path.clone(),
        }
    }
}

/// State reported by SELECT / EXAMINE.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectInfo {
    pub exists: u32,
    pub uidvalidity: u32,
    /// 0 when the server did not announce it.
    pub uidnext: u32,
    pub read_only: bool,
    pub permanent_flags: Vec<String>,
}

/// Result of a STATUS command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusInfo {
    pub messages: u32,
    pub uidnext: u32,
    pub uidvalidity: u32,
    pub unseen: u32,
}

/// One message from a FETCH response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FetchItem {
    pub seq: u32,
    pub uid: u32,
    pub flags: Option<Vec<String>>,
    pub size: Option<u32>,
    /// INTERNALDATE as unix seconds.
    pub internal_date: Option<i64>,
    pub header: Option<Vec<u8>>,
    pub body: Option<Vec<u8>>,
}

/// What happened while idling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// The mailbox changed (new, expunged or re-flagged messages).
    Changed,
    /// Nothing happened before the timeout.
    Timeout,
}

/// A piece of a command line.
enum Part<'a> {
    Text(String),
    /// Text that must not appear in the transcript.
    Secret(String),
    /// A literal (`{n}` + data); the flag hides the content from the transcript.
    Literal(Cow<'a, [u8]>, bool),
}

// ------------------------------------------------------------------ client

/// An IMAP connection.
pub struct ImapClient {
    io: BufReader<MailStream>,
    host: String,
    insecure: bool,
    tag: u32,
    caps: HashSet<String>,
    /// Bytes of a partially received line (kept across read timeouts while idling).
    partial: Vec<u8>,
    selected: Option<String>,
    idle_tag: Option<String>,
    logging_out: bool,
    pub transcript: Transcript,
    pub greeting: String,
}

impl ImapClient {
    /// Connect and read the greeting. STARTTLS is negotiated when configured.
    pub fn connect(server: &ServerConfig, transcript: bool) -> NetResult<Self> {
        let mut tr = Transcript::new(transcript);
        tr.note(&format!("Connecting to {}:{} ({})", server.host, server.port, server.security.name()));
        let stream = open_stream(server)?;
        let mut c = ImapClient {
            io: BufReader::new(stream),
            host: server.host.clone(),
            insecure: server.allow_insecure_certs,
            tag: 0,
            caps: HashSet::new(),
            partial: Vec::new(),
            selected: None,
            idle_tag: None,
            logging_out: false,
            transcript: tr,
            greeting: String::new(),
        };
        let line = c.read_line()?;
        let text = String::from_utf8_lossy(&line).into_owned();
        c.greeting = text.clone();
        let rest = text.strip_prefix("* ").ok_or_else(|| NetError::Protocol(format!("unexpected greeting: {text}")))?;
        let (word, status) = rest.split_once(' ').unwrap_or((rest, ""));
        match word.to_ascii_uppercase().as_str() {
            "OK" | "PREAUTH" => {}
            "BYE" => return Err(NetError::Bye(status.to_string())),
            _ => return Err(NetError::Protocol(format!("unexpected greeting: {text}"))),
        }
        c.absorb_code(split_code(status).0.as_deref());
        if server.security == Security::StartTls {
            c.starttls()?;
        }
        if c.caps.is_empty() {
            c.capability()?;
        }
        Ok(c)
    }

    fn starttls(&mut self) -> NetResult<()> {
        if !self.caps.is_empty() && !self.has_cap("STARTTLS") {
            return Err(NetError::Tls("the server does not offer STARTTLS".into()));
        }
        self.run("STARTTLS").map_err(|e| match e {
            NetError::No(t) | NetError::Bad(t) => NetError::Tls(format!("STARTTLS rejected: {t}")),
            other => other,
        })?;
        let old = std::mem::replace(&mut self.io, BufReader::new(MailStream::Closed));
        let stream = old.into_inner().upgrade(&self.host, self.insecure)?;
        self.io = BufReader::new(stream);
        self.transcript.note("TLS negotiated");
        // Capabilities must be discarded after STARTTLS (RFC 3501 6.2.1).
        self.caps.clear();
        self.capability()?;
        Ok(())
    }

    /// Authenticate according to the server configuration.
    pub fn login(&mut self, server: &ServerConfig, password: &str) -> NetResult<()> {
        let result = match server.auth {
            AuthMethod::None => return Ok(()),
            AuthMethod::Password => {
                if self.has_cap("LOGINDISABLED") {
                    return Err(NetError::Auth("the server does not allow plain-text login on this connection; use SSL/TLS or STARTTLS".into()));
                }
                let parts = [Part::Text("LOGIN ".into()), Self::astring(&server.username, false), Part::Text(" ".into()), Self::astring(password, true)];
                self.run_parts(&parts)
            }
            AuthMethod::OAuth2 => self.authenticate_xoauth2(&server.username, password),
        };
        match result {
            Ok(r) => {
                // Capabilities may change after login; servers often include them in the reply.
                match r.code.as_deref() {
                    Some(c) if strip_prefix_ci(c, "CAPABILITY ").is_some() => self.absorb_code(Some(c)),
                    _ => self.capability()?,
                }
                Ok(())
            }
            Err(NetError::No(t)) | Err(NetError::Bad(t)) => Err(NetError::Auth(t)),
            Err(e) => Err(e),
        }
    }

    fn authenticate_xoauth2(&mut self, user: &str, token: &str) -> NetResult<Response> {
        let payload = b64(xoauth2_payload(user, token).as_bytes());
        let tag = self.next_tag();
        if self.has_cap("SASL-IR") {
            self.write_line(&format!("{tag} AUTHENTICATE XOAUTH2 {payload}"), Some(&format!("{tag} AUTHENTICATE XOAUTH2 ********")))?;
        } else {
            self.write_line(&format!("{tag} AUTHENTICATE XOAUTH2"), None)?;
            self.wait_continuation(&tag)?;
            self.write_line(&payload, Some("********"))?;
        }
        loop {
            let line = self.read_line()?;
            if line.starts_with(b"+") {
                // Error details (base64 JSON); answer with an empty response to finish.
                self.write_line("", None)?;
                continue;
            }
            if let Some(done) = self.handle_line(&tag, line, &mut Vec::new())? {
                return done;
            }
        }
    }

    /// Request and store the capability list.
    pub fn capability(&mut self) -> NetResult<()> {
        let r = self.run("CAPABILITY")?;
        for l in &r.untagged {
            let s = String::from_utf8_lossy(l);
            if let Some(rest) = strip_prefix_ci(&s, "CAPABILITY ") {
                self.set_caps(rest);
            }
        }
        self.absorb_code(r.code.as_deref());
        Ok(())
    }

    fn set_caps(&mut self, list: &str) {
        self.caps = list.split_whitespace().map(|c| c.to_ascii_uppercase()).collect();
    }

    fn absorb_code(&mut self, code: Option<&str>) {
        if let Some(c) = code
            && let Some(rest) = strip_prefix_ci(c, "CAPABILITY ") {
                self.set_caps(rest);
            }
    }

    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.contains(&cap.to_ascii_uppercase())
    }

    pub fn capabilities(&self) -> Vec<String> {
        let mut v: Vec<String> = self.caps.iter().cloned().collect();
        v.sort();
        v
    }

    /// The currently selected mailbox (decoded path).
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Whether the stream is TLS protected.
    pub fn is_tls(&self) -> bool {
        self.io.get_ref().is_tls()
    }

    /// A clone of the socket that can be used to abort a blocking read from another thread.
    pub fn shutdown_handle(&self) -> Option<TcpStream> {
        self.io.get_ref().tcp().and_then(|t| t.try_clone().ok())
    }

    // -------------------------------------------------------------- low level I/O

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("P{:04}", self.tag)
    }

    fn write_line(&mut self, line: &str, shown: Option<&str>) -> NetResult<()> {
        self.transcript.client(shown.unwrap_or(line));
        let w = self.io.get_mut();
        w.write_all(line.as_bytes())?;
        w.write_all(b"\r\n")?;
        w.flush()?;
        Ok(())
    }

    /// Read one physical line including the trailing CRLF.
    fn read_raw_line(&mut self) -> NetResult<Vec<u8>> {
        loop {
            match self.io.read_until(b'\n', &mut self.partial) {
                Ok(0) => return Err(NetError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "the server closed the connection"))),
                Ok(_) => {
                    if self.partial.ends_with(b"\n") {
                        return Ok(std::mem::take(&mut self.partial));
                    }
                    if self.partial.len() > MAX_LITERAL {
                        return Err(NetError::Protocol("line too long".into()));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(NetError::Io(e)),
            }
        }
    }

    /// Read one logical response line: literals (`{n}\r\n<n bytes>`) are read and
    /// kept inline so that [`parse_values`] can decode them. The final CRLF is removed.
    fn read_line(&mut self) -> NetResult<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let line = self.read_raw_line()?;
            let trimmed_len = line.len() - if line.ends_with(b"\r\n") { 2 } else { 1 };
            let body = &line[..trimmed_len];
            match literal_len(body) {
                Some(n) => {
                    if n > MAX_LITERAL || out.len() + n > MAX_LITERAL {
                        return Err(NetError::Protocol(format!("literal of {n} bytes is too large")));
                    }
                    if self.transcript.enabled() {
                        self.transcript.server(&String::from_utf8_lossy(body));
                    }
                    out.extend_from_slice(body);
                    out.extend_from_slice(b"\r\n");
                    let start = out.len();
                    out.resize(start + n, 0);
                    self.io.read_exact(&mut out[start..])?;
                    if self.transcript.enabled() {
                        self.transcript.server(&format!("<{n} bytes>"));
                    }
                }
                None => {
                    if self.transcript.enabled() {
                        self.transcript.server(&String::from_utf8_lossy(body));
                    }
                    out.extend_from_slice(body);
                    return Ok(out);
                }
            }
        }
    }

    /// Process one received line while waiting for the completion of `tag`.
    /// Returns `Some(result)` when the command completed.
    fn handle_line(&mut self, tag: &str, line: Vec<u8>, untagged: &mut Vec<Vec<u8>>) -> NetResult<Option<NetResult<Response>>> {
        if let Some(rest) = line.strip_prefix(b"* ") {
            if starts_with_ci(rest, b"BYE") {
                let text = String::from_utf8_lossy(rest).into_owned();
                // A BYE in response to LOGOUT is followed by the tagged OK.
                if self.logging_out {
                    untagged.push(rest.to_vec());
                    return Ok(None);
                }
                return Err(NetError::Bye(text.get(4..).unwrap_or("").trim().to_string()));
            }
            untagged.push(rest.to_vec());
            return Ok(None);
        }
        if line.starts_with(b"+") {
            // Unexpected continuation request: ignore.
            return Ok(None);
        }
        let s = String::from_utf8_lossy(&line).into_owned();
        let Some((t, rest)) = s.split_once(' ') else {
            return Err(NetError::Protocol(format!("unexpected line: {s}")));
        };
        if t != tag {
            log::debug!("imap: ignoring response for unknown tag {t}");
            return Ok(None);
        }
        let (status, text) = rest.split_once(' ').unwrap_or((rest, ""));
        let (code, text) = split_code(text);
        let full = match &code {
            Some(c) => format!("[{c}] {text}"),
            None => text.clone(),
        };
        let res = match status.to_ascii_uppercase().as_str() {
            "OK" => Ok(Response { untagged: std::mem::take(untagged), code, text }),
            "NO" => Err(NetError::No(full)),
            "BAD" => Err(NetError::Bad(full)),
            _ => return Err(NetError::Protocol(format!("unexpected status: {s}"))),
        };
        Ok(Some(res))
    }

    fn read_until_tagged(&mut self, tag: &str, mut untagged: Vec<Vec<u8>>) -> NetResult<Response> {
        loop {
            let line = self.read_line()?;
            if let Some(done) = self.handle_line(tag, line, &mut untagged)? {
                return done;
            }
        }
    }

    /// Wait for a `+` continuation request; untagged lines are collected.
    fn wait_continuation(&mut self, tag: &str) -> NetResult<Vec<Vec<u8>>> {
        let mut untagged = Vec::new();
        loop {
            let line = self.read_line()?;
            if line.starts_with(b"+") {
                return Ok(untagged);
            }
            if let Some(done) = self.handle_line(tag, line, &mut untagged)? {
                return Err(match done {
                    Ok(_) => NetError::Protocol("command completed before continuation".into()),
                    Err(e) => e,
                });
            }
        }
    }

    /// Encode a string argument as quoted string, or as literal when needed.
    fn astring(s: &str, secret: bool) -> Part<'static> {
        if s.bytes().any(|b| b == b'\r' || b == b'\n' || b >= 0x80 || b == 0) {
            Part::Literal(Cow::Owned(s.as_bytes().to_vec()), secret)
        } else if secret {
            Part::Secret(quote(s))
        } else {
            Part::Text(quote(s))
        }
    }

    fn run_parts(&mut self, parts: &[Part]) -> NetResult<Response> {
        let tag = self.next_tag();
        let mut line: Vec<u8> = format!("{tag} ").into_bytes();
        let mut shown = line.clone();
        let mut untagged = Vec::new();
        let literal_plus = self.has_cap("LITERAL+");
        let mut i = 0;
        while i < parts.len() {
            match &parts[i] {
                Part::Text(t) => {
                    line.extend_from_slice(t.as_bytes());
                    shown.extend_from_slice(t.as_bytes());
                }
                Part::Secret(t) => {
                    line.extend_from_slice(t.as_bytes());
                    shown.extend_from_slice(b"********");
                }
                Part::Literal(data, secret) => {
                    let header = if literal_plus { format!("{{{}+}}", data.len()) } else { format!("{{{}}}", data.len()) };
                    line.extend_from_slice(header.as_bytes());
                    shown.extend_from_slice(header.as_bytes());
                    self.write_line(&String::from_utf8_lossy(&line), Some(&String::from_utf8_lossy(&shown)))?;
                    if !literal_plus {
                        untagged.extend(self.wait_continuation(&tag)?);
                    }
                    self.io.get_mut().write_all(data)?;
                    self.transcript.client(&if *secret { "********".to_string() } else { format!("<{} bytes>", data.len()) });
                    line.clear();
                    shown.clear();
                }
            }
            i += 1;
        }
        self.write_line(&String::from_utf8_lossy(&line), Some(&String::from_utf8_lossy(&shown)))?;
        self.read_until_tagged(&tag, untagged)
    }

    /// Run a simple command (no literals) and wait for its completion.
    pub fn run(&mut self, cmd: &str) -> NetResult<Response> {
        let tag = self.next_tag();
        self.write_line(&format!("{tag} {cmd}"), None)?;
        self.read_until_tagged(&tag, Vec::new())
    }

    // -------------------------------------------------------------- commands

    pub fn noop(&mut self) -> NetResult<()> {
        self.run("NOOP").map(|_| ())
    }

    pub fn logout(&mut self) {
        self.logging_out = true;
        let _ = self.run("LOGOUT");
        self.selected = None;
    }

    /// List all mailboxes.
    pub fn list(&mut self) -> NetResult<Vec<ListEntry>> {
        let r = self.run("LIST \"\" \"*\"")?;
        let mut out = Vec::new();
        for l in &r.untagged {
            if !starts_with_ci(l, b"LIST ") {
                continue;
            }
            match parse_list(&l[5..]) {
                Some(e) => out.push(e),
                None => log::debug!("imap: unparsable LIST line: {}", String::from_utf8_lossy(l)),
            }
        }
        Ok(out)
    }

    /// The server's hierarchy delimiter (`LIST "" ""`), if it has one.
    pub fn hierarchy_delimiter(&mut self) -> NetResult<Option<String>> {
        let r = self.run("LIST \"\" \"\"")?;
        for l in &r.untagged {
            if starts_with_ci(l, b"LIST ")
                && let Ok(vals) = parse_values(&l[5..]) {
                    return Ok(vals.get(1).and_then(|v| v.text()).filter(|d| !d.is_empty()));
                }
        }
        Ok(None)
    }

    fn open_mailbox(&mut self, verb: &str, path: &str) -> NetResult<SelectInfo> {
        self.selected = None;
        let r = self.run(&format!("{verb} {}", quote(&encode_mailbox(path))))?;
        let mut info = SelectInfo { read_only: verb == "EXAMINE", ..Default::default() };
        for l in &r.untagged {
            let s = String::from_utf8_lossy(l);
            let mut words = s.split_whitespace();
            let (Some(a), b) = (words.next(), words.next()) else { continue };
            if let (Ok(n), Some(kw)) = (a.parse::<u32>(), b) {
                if kw.eq_ignore_ascii_case("EXISTS") {
                    info.exists = n;
                }
                continue;
            }
            if a.eq_ignore_ascii_case("OK") {
                let rest = s[2..].trim_start();
                if let (Some(code), _) = split_code(rest) {
                    let mut cw = code.split_whitespace();
                    let name = cw.next().unwrap_or("").to_ascii_uppercase();
                    let arg = cw.next().unwrap_or("");
                    match name.as_str() {
                        "UIDVALIDITY" => info.uidvalidity = arg.parse().unwrap_or(0),
                        "UIDNEXT" => info.uidnext = arg.parse().unwrap_or(0),
                        "PERMANENTFLAGS" => {
                            info.permanent_flags = code[name.len()..].trim().trim_matches(['(', ')']).split_whitespace().map(String::from).collect();
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(c) = &r.code
            && c.eq_ignore_ascii_case("READ-ONLY") {
                info.read_only = true;
            }
        self.selected = Some(path.to_string());
        Ok(info)
    }

    /// SELECT a mailbox (read-write).
    pub fn select(&mut self, path: &str) -> NetResult<SelectInfo> {
        self.open_mailbox("SELECT", path)
    }

    /// EXAMINE a mailbox (read-only).
    pub fn examine(&mut self, path: &str) -> NetResult<SelectInfo> {
        self.open_mailbox("EXAMINE", path)
    }

    /// Mailbox status without selecting it.
    pub fn status(&mut self, path: &str) -> NetResult<StatusInfo> {
        let r = self.run(&format!("STATUS {} (MESSAGES UIDNEXT UIDVALIDITY UNSEEN)", quote(&encode_mailbox(path))))?;
        let mut info = StatusInfo::default();
        for l in &r.untagged {
            if !starts_with_ci(l, b"STATUS ") {
                continue;
            }
            let Ok(vals) = parse_values(&l[7..]) else { continue };
            let Some(items) = vals.get(1).and_then(|v| v.list()) else { continue };
            for pair in items.chunks(2) {
                let (Some(k), Some(v)) = (pair.first().and_then(|v| v.text()), pair.get(1).and_then(|v| v.number())) else { continue };
                let v = v.min(u32::MAX as u64) as u32;
                match k.to_ascii_uppercase().as_str() {
                    "MESSAGES" => info.messages = v,
                    "UIDNEXT" => info.uidnext = v,
                    "UIDVALIDITY" => info.uidvalidity = v,
                    "UNSEEN" => info.unseen = v,
                    _ => {}
                }
            }
        }
        Ok(info)
    }

    /// `UID FETCH <set> <items>`; only items carrying a UID are returned.
    pub fn uid_fetch(&mut self, set: &str, items: &str) -> NetResult<Vec<FetchItem>> {
        let r = self.run(&format!("UID FETCH {set} {items}"))?;
        Ok(r.untagged.iter().filter_map(|l| parse_fetch(l)).filter(|f| f.uid != 0).collect())
    }

    /// Flags of all messages with UID in `set` (e.g. `1:*`).
    pub fn fetch_flags(&mut self, set: &str) -> NetResult<Vec<(u32, Vec<String>)>> {
        Ok(self.uid_fetch(set, "(UID FLAGS)")?.into_iter().map(|f| (f.uid, f.flags.unwrap_or_default())).collect())
    }

    /// UID, flags and size of all messages with UID in `set`.
    pub fn fetch_sizes(&mut self, set: &str) -> NetResult<Vec<FetchItem>> {
        self.uid_fetch(set, "(UID FLAGS RFC822.SIZE)")
    }

    /// Header block, flags, size and internal date for the given UIDs.
    pub fn fetch_headers(&mut self, uids: &[u32]) -> NetResult<Vec<FetchItem>> {
        let mut out = Vec::new();
        for set in uid_set_chunks(uids) {
            out.extend(self.uid_fetch(&set, "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])")?);
        }
        Ok(out)
    }

    /// Full message source for the given UIDs (does not set `\Seen`).
    pub fn fetch_bodies(&mut self, uids: &[u32]) -> NetResult<Vec<FetchItem>> {
        let mut out = Vec::new();
        for set in uid_set_chunks(uids) {
            out.extend(self.uid_fetch(&set, "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[])")?);
        }
        Ok(out)
    }

    /// Add (`add = true`) or remove flags / keywords on the given UIDs.
    pub fn uid_store(&mut self, uids: &[u32], add: bool, flags: &[String]) -> NetResult<()> {
        if uids.is_empty() || flags.is_empty() {
            return Ok(());
        }
        let op = if add { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
        for set in uid_set_chunks(uids) {
            match self.run(&format!("UID STORE {set} {op} ({})", flags.join(" "))) {
                Ok(_) => {}
                Err(NetError::No(t)) | Err(NetError::Bad(t)) if flags.iter().any(|f| !f.starts_with('\\')) => {
                    // The server may not allow keywords; retry with system flags only.
                    let sys: Vec<&String> = flags.iter().filter(|f| f.starts_with('\\')).collect();
                    log::debug!("imap: keyword STORE refused ({t}); retrying with system flags");
                    if !sys.is_empty() {
                        self.run(&format!("UID STORE {set} {op} ({})", sys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")))?;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Copy messages to another mailbox.
    pub fn uid_copy(&mut self, uids: &[u32], dest: &str) -> NetResult<()> {
        for set in uid_set_chunks(uids) {
            self.run(&format!("UID COPY {set} {}", quote(&encode_mailbox(dest))))?;
        }
        Ok(())
    }

    /// Move messages to another mailbox (MOVE, or COPY + `\Deleted` + EXPUNGE).
    pub fn uid_move(&mut self, uids: &[u32], dest: &str) -> NetResult<()> {
        if uids.is_empty() {
            return Ok(());
        }
        if self.has_cap("MOVE") {
            for set in uid_set_chunks(uids) {
                self.run(&format!("UID MOVE {set} {}", quote(&encode_mailbox(dest))))?;
            }
            return Ok(());
        }
        self.uid_copy(uids, dest)?;
        self.uid_store(uids, true, &["\\Deleted".to_string()])?;
        self.expunge_uids(uids)
    }

    /// Flag messages `\Deleted` and expunge them.
    pub fn uid_delete(&mut self, uids: &[u32]) -> NetResult<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.uid_store(uids, true, &["\\Deleted".to_string()])?;
        self.expunge_uids(uids)
    }

    /// Expunge specific UIDs (UIDPLUS) or, without UIDPLUS, everything flagged `\Deleted`.
    pub fn expunge_uids(&mut self, uids: &[u32]) -> NetResult<()> {
        if self.has_cap("UIDPLUS") {
            for set in uid_set_chunks(uids) {
                self.run(&format!("UID EXPUNGE {set}"))?;
            }
            Ok(())
        } else {
            self.expunge()
        }
    }

    pub fn expunge(&mut self) -> NetResult<()> {
        self.run("EXPUNGE").map(|_| ())
    }

    /// Upload a message. Returns `(uidvalidity, uid)` when the server supports UIDPLUS.
    pub fn append(&mut self, path: &str, flags: &[String], date: Option<i64>, data: &[u8]) -> NetResult<Option<(u32, u32)>> {
        let mut head = format!("APPEND {} ({})", quote(&encode_mailbox(path)), flags.iter().filter(|f| !f.eq_ignore_ascii_case("\\Recent")).cloned().collect::<Vec<_>>().join(" "));
        if let Some(d) = date.and_then(|d| chrono::DateTime::<chrono::Utc>::from_timestamp(d, 0)) {
            head.push_str(&format!(" \"{}\"", d.format("%d-%b-%Y %H:%M:%S +0000")));
        }
        head.push(' ');
        let r = self.run_parts(&[Part::Text(head), Part::Literal(Cow::Borrowed(data), false)])?;
        Ok(r.code.as_deref().and_then(parse_appenduid))
    }

    pub fn create(&mut self, path: &str) -> NetResult<()> {
        self.run(&format!("CREATE {}", quote(&encode_mailbox(path)))).map(|_| ())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> NetResult<()> {
        self.run(&format!("RENAME {} {}", quote(&encode_mailbox(from)), quote(&encode_mailbox(to)))).map(|_| ())
    }

    pub fn delete(&mut self, path: &str) -> NetResult<()> {
        if self.selected.as_deref() == Some(path) {
            self.selected = None;
            let _ = self.run("CLOSE");
        }
        self.run(&format!("DELETE {}", quote(&encode_mailbox(path)))).map(|_| ())
    }

    pub fn subscribe(&mut self, path: &str) -> NetResult<()> {
        self.run(&format!("SUBSCRIBE {}", quote(&encode_mailbox(path)))).map(|_| ())
    }

    // -------------------------------------------------------------- IDLE

    /// Enter IDLE. The selected mailbox is watched until [`idle_done`](Self::idle_done).
    pub fn idle_start(&mut self) -> NetResult<()> {
        if !self.has_cap("IDLE") {
            return Err(NetError::No("the server does not support IDLE".into()));
        }
        let tag = self.next_tag();
        self.write_line(&format!("{tag} IDLE"), None)?;
        self.wait_continuation(&tag)?;
        self.idle_tag = Some(tag);
        Ok(())
    }

    /// Wait for a mailbox change while idling.
    pub fn idle_wait(&mut self, timeout: Duration) -> NetResult<IdleEvent> {
        self.io.get_ref().set_read_timeout(Some(timeout))?;
        let res = loop {
            match self.read_line() {
                Ok(line) => {
                    let Some(rest) = line.strip_prefix(b"* ") else { continue };
                    let s = String::from_utf8_lossy(rest).to_ascii_uppercase();
                    if s.starts_with("BYE") {
                        break Err(NetError::Bye(s));
                    }
                    let mut w = s.split_whitespace();
                    if let (Some(n), Some(kw)) = (w.next(), w.next())
                        && n.parse::<u32>().is_ok() && matches!(kw, "EXISTS" | "EXPUNGE" | "FETCH" | "RECENT") {
                            break Ok(IdleEvent::Changed);
                        }
                    // "* OK Still here" and friends.
                }
                Err(e) if e.is_timeout() => break Ok(IdleEvent::Timeout),
                Err(e) => break Err(e),
            }
        };
        let _ = self.io.get_ref().set_read_timeout(Some(IO_TIMEOUT));
        res
    }

    /// Leave IDLE.
    pub fn idle_done(&mut self) -> NetResult<()> {
        let Some(tag) = self.idle_tag.take() else { return Ok(()) };
        self.write_line("DONE", None)?;
        self.read_until_tagged(&tag, Vec::new()).map(|_| ())
    }
}

impl Drop for ImapClient {
    fn drop(&mut self) {
        if let Some(t) = self.io.get_ref().tcp() {
            let _ = t.shutdown(std::net::Shutdown::Both);
        }
    }
}

// ------------------------------------------------------------------ parsing helpers

fn starts_with_ci(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix) { Some(&s[prefix.len()..]) } else { None }
}

/// If a line ends with a literal announcement `{n}` / `{n+}`, return `n`.
fn literal_len(line: &[u8]) -> Option<usize> {
    if !line.ends_with(b"}") {
        return None;
    }
    let open = line.iter().rposition(|&c| c == b'{')?;
    let inner = std::str::from_utf8(&line[open + 1..line.len() - 1]).ok()?;
    inner.trim_end_matches('+').parse().ok()
}

/// Quote a string for use as an IMAP quoted string.
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            o.push('\\');
        }
        if c != '\r' && c != '\n' {
            o.push(c);
        }
    }
    o.push('"');
    o
}

fn parse_list(rest: &[u8]) -> Option<ListEntry> {
    let vals = parse_values(rest).ok()?;
    let attrs = vals.first()?.list()?.iter().filter_map(|v| v.text()).collect();
    let delimiter = match vals.get(1)? {
        Value::Nil => None,
        v => v.text(),
    };
    let raw = vals.get(2)?.text()?;
    Some(ListEntry { path: decode_mailbox(&raw), delimiter, attributes: attrs })
}

/// Parse one untagged `n FETCH (...)` line.
pub fn parse_fetch(line: &[u8]) -> Option<FetchItem> {
    let vals = parse_values(line).ok()?;
    let seq = vals.first()?.number()? as u32;
    if !vals.get(1)?.text()?.eq_ignore_ascii_case("FETCH") {
        return None;
    }
    let items = vals.get(2)?.list()?;
    let mut f = FetchItem { seq, ..Default::default() };
    for pair in items.chunks(2) {
        let Some(key) = pair.first().and_then(|k| k.text()) else { continue };
        let Some(val) = pair.get(1) else { continue };
        let mut key = key.to_ascii_uppercase();
        if key.ends_with('>')
            && let Some(p) = key.rfind('<') {
                key.truncate(p);
            }
        match key.as_str() {
            "UID" => f.uid = val.number().unwrap_or(0).min(u32::MAX as u64) as u32,
            "FLAGS" => f.flags = Some(val.list().map(|l| l.iter().filter_map(|v| v.text()).collect()).unwrap_or_default()),
            "RFC822.SIZE" => f.size = val.number().map(|n| n.min(u32::MAX as u64) as u32),
            "INTERNALDATE" => f.internal_date = val.text().and_then(|t| parse_internal_date(&t)),
            "BODY[HEADER]" | "RFC822.HEADER" => f.header = val.bytes().map(|b| b.to_vec()),
            "BODY[]" | "RFC822" => f.body = val.bytes().map(|b| b.to_vec()),
            _ => {}
        }
    }
    Some(f)
}

/// Parse an IMAP date-time such as `17-Jul-1996 02:44:25 -0700`.
pub fn parse_internal_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(s.trim(), "%d-%b-%Y %H:%M:%S %z").ok().map(|d| d.timestamp())
}

fn parse_appenduid(code: &str) -> Option<(u32, u32)> {
    let mut w = code.split_whitespace();
    if !w.next()?.eq_ignore_ascii_case("APPENDUID") {
        return None;
    }
    let validity = w.next()?.parse().ok()?;
    let uid = w.next()?.parse().ok()?;
    Some((validity, uid))
}

// ------------------------------------------------------------------ UID sets

/// Compress UIDs into IMAP sequence-set syntax (`1:3,7,9:10`).
pub fn uid_set(uids: &[u32]) -> String {
    uid_ranges(uids).join(",")
}

fn uid_ranges(uids: &[u32]) -> Vec<String> {
    let mut v: Vec<u32> = uids.iter().copied().filter(|&u| u != 0).collect();
    v.sort_unstable();
    v.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        out.push(if start == end { start.to_string() } else { format!("{start}:{end}") });
        i += 1;
    }
    out
}

/// Sequence sets limited in length, so commands stay below common server line limits.
pub fn uid_set_chunks(uids: &[u32]) -> Vec<String> {
    const MAX: usize = 900;
    let mut out = Vec::new();
    let mut cur = String::new();
    for r in uid_ranges(uids) {
        if !cur.is_empty() && cur.len() + r.len() + 1 > MAX {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(',');
        }
        cur.push_str(&r);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ------------------------------------------------------------------ flags

/// Convert IMAP flags and keywords into our flag bits and flag color.
pub fn flags_from_imap(list: &[String]) -> (u32, Option<FlagColor>) {
    let mut bits = 0;
    let mut color = [false; 3];
    for f in list {
        let l = f.to_ascii_lowercase();
        match l.as_str() {
            "\\seen" => bits |= flags::SEEN,
            "\\answered" => bits |= flags::ANSWERED,
            "\\flagged" => bits |= flags::FLAGGED,
            "\\deleted" => bits |= flags::DELETED,
            "\\draft" => bits |= flags::DRAFT,
            "$forwarded" | "forwarded" => bits |= flags::FORWARDED,
            "$junk" | "junk" => bits |= flags::JUNK,
            "$notjunk" | "notjunk" | "nonjunk" | "$nonjunk" => bits |= flags::NOT_JUNK,
            _ => {
                for (i, kw) in FLAG_COLOR_KEYWORDS.iter().enumerate() {
                    if l == kw.to_ascii_lowercase() {
                        color[i] = true;
                    }
                }
            }
        }
    }
    let color = (bits & flags::FLAGGED != 0).then(|| FlagColor::from_keyword_bits(color));
    (bits, color)
}

/// IMAP flags / keywords corresponding to our flag bits (local-only bits are ignored).
pub fn flags_to_imap(bits: u32) -> Vec<String> {
    let map: [(u32, &str); 8] = [
        (flags::SEEN, "\\Seen"),
        (flags::ANSWERED, "\\Answered"),
        (flags::FLAGGED, "\\Flagged"),
        (flags::DELETED, "\\Deleted"),
        (flags::DRAFT, "\\Draft"),
        (flags::FORWARDED, "$Forwarded"),
        (flags::JUNK, "$Junk"),
        (flags::NOT_JUNK, "$NotJunk"),
    ];
    map.iter().filter(|(b, _)| bits & b != 0).map(|(_, s)| s.to_string()).collect()
}

/// Keywords to add and to remove to represent `color` on the server.
pub fn color_keywords(color: Option<FlagColor>) -> (Vec<String>, Vec<String>) {
    let bits = color.map(|c| c.to_keyword_bits()).unwrap_or([false; 3]);
    let mut add = Vec::new();
    let mut remove = Vec::new();
    for (i, kw) in FLAG_COLOR_KEYWORDS.iter().enumerate() {
        if bits[i] { add.push(kw.to_string()) } else { remove.push(kw.to_string()) }
    }
    (add, remove)
}

// ------------------------------------------------------------------ modified UTF-7

fn mutf7_engine() -> base64::engine::GeneralPurpose {
    let cfg = base64::engine::GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true);
    base64::engine::GeneralPurpose::new(&base64::alphabet::IMAP_MUTF7, cfg)
}

/// Encode a mailbox name with IMAP's modified UTF-7 (RFC 3501 5.1.3).
pub fn encode_mailbox(name: &str) -> String {
    let engine = mutf7_engine();
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if !pending.is_empty() {
            let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
            out.push('&');
            out.push_str(&engine.encode(bytes));
            out.push('-');
            pending.clear();
        }
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' { out.push_str("&-") } else { out.push(c) }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Decode a modified UTF-7 mailbox name; invalid input is returned unchanged.
pub fn decode_mailbox(name: &str) -> String {
    if !name.contains('&') {
        return name.to_string();
    }
    let engine = mutf7_engine();
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let Some(end) = after.find('-') else { return name.to_string() };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else {
            let Ok(bytes) = engine.decode(chunk) else { return name.to_string() };
            if bytes.len() % 2 != 0 {
                return name.to_string();
            }
            let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            match String::from_utf16(&units) {
                Ok(s) => out.push_str(&s),
                Err(_) => return name.to_string(),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutf7_roundtrip() {
        for s in ["INBOX", "Entwürfe", "Gelöschte Elemente", "A&B", "日本語/テスト", "~peter/mail/台北/日本語"] {
            assert_eq!(decode_mailbox(&encode_mailbox(s)), s);
        }
        assert_eq!(encode_mailbox("Entwürfe"), "Entw&APw-rfe");
        assert_eq!(decode_mailbox("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(encode_mailbox("A&B"), "A&-B");
    }

    #[test]
    fn parse_fetch_line() {
        let raw = b"12 FETCH (UID 345 FLAGS (\\Seen $MailFlagBit1 \\Flagged) RFC822.SIZE 1024 INTERNALDATE \" 7-Oct-2025 10:00:00 +0200\" BODY[HEADER] {15}\r\nSubject: hi\r\n\r\n)";
        let f = parse_fetch(raw).unwrap();
        assert_eq!(f.seq, 12);
        assert_eq!(f.uid, 345);
        assert_eq!(f.size, Some(1024));
        assert_eq!(f.header.as_deref(), Some(&b"Subject: hi\r\n\r\n"[..]));
        assert!(f.internal_date.is_some());
        let (bits, color) = flags_from_imap(f.flags.as_ref().unwrap());
        assert_eq!(bits, flags::SEEN | flags::FLAGGED);
        assert_eq!(color, Some(FlagColor::Yellow));
    }

    #[test]
    fn parse_list_line() {
        let e = parse_list(b"(\\HasNoChildren \\Sent) \"/\" \"Sent Items\"").unwrap();
        assert_eq!(e.path, "Sent Items");
        assert_eq!(e.delimiter.as_deref(), Some("/"));
        assert!(e.has_attr("\\sent"));
        let e = parse_list(b"(\\Noselect) NIL Entw&APw-rfe").unwrap();
        assert_eq!(e.path, "Entwürfe");
        assert!(!e.selectable());
    }

    #[test]
    fn sets_and_codes() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 10, 7]), "1:3,5,7,9:10");
        let many: Vec<u32> = (1..2000).map(|i| i * 2).collect();
        let chunks = uid_set_chunks(&many);
        assert!(chunks.len() > 1 && chunks.iter().all(|c| c.len() <= 900));
        assert_eq!(split_code("[UIDNEXT 5] Predicted"), (Some("UIDNEXT 5".into()), "Predicted".into()));
        assert_eq!(parse_appenduid("APPENDUID 38505 3955"), Some((38505, 3955)));
        assert_eq!(literal_len(b"* 1 FETCH (BODY[] {123}"), Some(123));
        assert_eq!(literal_len(b"A1 LOGIN {5+}"), Some(5));
        // Malformed input must not panic.
        assert!(parse_values(b"(((").is_err());
        assert!(parse_values(b"{99999}\r\nabc").is_err());
        assert!(parse_fetch(b"x FETCH").is_none());
    }

    #[test]
    fn flag_mapping() {
        let f = flags_to_imap(flags::SEEN | flags::JUNK | flags::HAS_BODY);
        assert_eq!(f, vec!["\\Seen".to_string(), "$Junk".to_string()]);
        let (add, remove) = color_keywords(Some(FlagColor::Purple));
        assert_eq!(add, vec!["$MailFlagBit0", "$MailFlagBit2"]);
        assert_eq!(remove, vec!["$MailFlagBit1"]);
    }
}
