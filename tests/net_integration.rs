//! End-to-end tests of the networking layer against small in-process fake servers
//! (IMAP, POP3 and SMTP over plain TCP on 127.0.0.1).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use purpamail::core::compose::OutgoingMessage;
use purpamail::core::config::{AccountConfig, AccountKind, AuthMethod, CheckFrequency, DownloadAttachments, Pop3Removal, Secrets, Security, ServerConfig, Settings};
use purpamail::core::model::*;
use purpamail::core::ops;
use purpamail::core::store::{MessageQuery, SharedStore, Store};
use purpamail::net::{Command, Engine, EngineHandle, Event, ServiceStatus};

// ====================================================================== fake IMAP server

mod fake_imap {
    use super::*;

    pub struct Message {
        pub uid: u32,
        pub flags: Vec<String>,
        pub raw: Vec<u8>,
    }

    pub struct Mailbox {
        pub attrs: String,
        pub uidvalidity: u32,
        pub uidnext: u32,
        pub messages: Vec<Message>,
    }

    impl Mailbox {
        pub fn new(attrs: &str, uidvalidity: u32) -> Self {
            Self { attrs: attrs.into(), uidvalidity, uidnext: 1, messages: Vec::new() }
        }
        pub fn by_uid(&self, uid: u32) -> Option<&Message> {
            self.messages.iter().find(|m| m.uid == uid)
        }
        pub fn subjects(&self) -> Vec<String> {
            self.messages
                .iter()
                .map(|m| {
                    let s = String::from_utf8_lossy(&m.raw).into_owned();
                    s.lines().find_map(|l| l.strip_prefix("Subject: ").map(|x| x.to_string())).unwrap_or_default()
                })
                .collect()
        }
    }

    #[derive(Default)]
    pub struct State {
        pub mailboxes: BTreeMap<String, Mailbox>,
        pub commands: Vec<String>,
        pub password: String,
        pub next_validity: u32,
    }

    impl State {
        pub fn add(&mut self, mailbox: &str, raw: Vec<u8>, flags: &[&str]) -> u32 {
            let mb = self.mailboxes.get_mut(mailbox).expect("mailbox");
            let uid = mb.uidnext;
            mb.uidnext += 1;
            mb.messages.push(Message { uid, flags: flags.iter().map(|s| s.to_string()).collect(), raw });
            uid
        }
        pub fn mb(&self, name: &str) -> &Mailbox {
            self.mailboxes.get(name).expect("mailbox")
        }
        pub fn mb_mut(&mut self, name: &str) -> &mut Mailbox {
            self.mailboxes.get_mut(name).expect("mailbox")
        }
        pub fn count(&self, prefix: &str) -> usize {
            self.commands.iter().filter(|c| c.to_ascii_uppercase().contains(&prefix.to_ascii_uppercase())).count()
        }
    }

    pub type Shared = Arc<Mutex<State>>;

    pub fn start(state: Shared) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let st = state.clone();
                std::thread::spawn(move || {
                    let _ = session(s, st);
                });
            }
        });
        port
    }

    fn tokens(s: &str) -> Vec<String> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b' ' => i += 1,
                b'"' => {
                    let mut t = String::new();
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        t.push(b[i] as char);
                        i += 1;
                    }
                    i += 1;
                    out.push(t);
                }
                b'(' => {
                    let start = i;
                    let mut depth = 0;
                    while i < b.len() {
                        if b[i] == b'(' {
                            depth += 1;
                        }
                        if b[i] == b')' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        i += 1;
                    }
                    out.push(s[start..=i.min(b.len() - 1)].to_string());
                    i += 1;
                }
                _ => {
                    let start = i;
                    let mut depth = 0;
                    while i < b.len() && (b[i] != b' ' || depth > 0) {
                        if b[i] == b'[' {
                            depth += 1;
                        }
                        if b[i] == b']' {
                            depth -= 1;
                        }
                        i += 1;
                    }
                    out.push(s[start..i].to_string());
                }
            }
        }
        out
    }

    fn in_set(set: &str, uid: u32, max: u32) -> bool {
        set.split(',').any(|part| {
            let p = |x: &str| if x == "*" { max } else { x.parse().unwrap_or(0) };
            match part.split_once(':') {
                Some((a, b)) => {
                    let (a, b) = (p(a), p(b));
                    uid >= a.min(b) && uid <= a.max(b)
                }
                None => p(part) == uid,
            }
        })
    }

    fn flag_list(s: &str) -> Vec<String> {
        s.trim_matches(['(', ')']).split_whitespace().map(String::from).collect()
    }

    fn quote(s: &str) -> String {
        format!("\"{s}\"")
    }

    fn header_of(raw: &[u8]) -> &[u8] {
        match raw.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(p) => &raw[..p + 4],
            None => raw,
        }
    }

    fn session(stream: TcpStream, st: Shared) -> std::io::Result<()> {
        let mut r = BufReader::new(stream.try_clone()?);
        let mut w = stream;
        w.write_all(b"* OK [CAPABILITY IMAP4rev1 IDLE MOVE UIDPLUS SPECIAL-USE LITERAL+] Fake IMAP ready\r\n")?;
        let mut selected: Option<String> = None;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let mut line = line.trim_end_matches(['\r', '\n']).to_string();
            let mut literal: Option<Vec<u8>> = None;
            if line.ends_with('}')
                && let Some(open) = line.rfind('{') {
                    let spec = &line[open + 1..line.len() - 1];
                    let plus = spec.ends_with('+');
                    let n: usize = spec.trim_end_matches('+').parse().unwrap_or(0);
                    if !plus {
                        w.write_all(b"+ go ahead\r\n")?;
                    }
                    let mut buf = vec![0; n];
                    r.read_exact(&mut buf)?;
                    let mut rest = String::new();
                    r.read_line(&mut rest)?;
                    line = format!("{}{}", &line[..open], rest.trim_end());
                    literal = Some(buf);
                }
            {
                let shown = if line.to_ascii_uppercase().contains(" LOGIN ") { line.split(' ').take(3).collect::<Vec<_>>().join(" ") + " ***" } else { line.clone() };
                st.lock().commands.push(shown);
            }
            let (tag, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let (cmd, args) = rest.split_once(' ').unwrap_or((rest, ""));
            let mut cmd = cmd.to_ascii_uppercase();
            let mut args = args.to_string();
            let uid_cmd = cmd == "UID";
            if uid_cmd {
                let (c, a) = args.split_once(' ').unwrap_or((args.as_str(), ""));
                cmd = c.to_ascii_uppercase();
                args = a.to_string();
            }
            let t = tokens(&args);
            let mut out: Vec<u8> = Vec::new();
            let ok = |out: &mut Vec<u8>, text: &str| out.extend_from_slice(format!("{tag} OK {text}\r\n").as_bytes());
            match cmd.as_str() {
                "CAPABILITY" => {
                    out.extend_from_slice(b"* CAPABILITY IMAP4rev1 IDLE MOVE UIDPLUS SPECIAL-USE LITERAL+\r\n");
                    ok(&mut out, "done");
                }
                "LOGIN" => {
                    let pw = st.lock().password.clone();
                    if t.get(1).map(|s| s.as_str()) == Some(pw.as_str()) {
                        ok(&mut out, "[CAPABILITY IMAP4rev1 IDLE MOVE UIDPLUS SPECIAL-USE LITERAL+] logged in");
                    } else {
                        out.extend_from_slice(format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n").as_bytes());
                    }
                }
                "NOOP" | "CHECK" | "SUBSCRIBE" => ok(&mut out, "done"),
                "LOGOUT" => {
                    w.write_all(format!("* BYE bye\r\n{tag} OK logged out\r\n").as_bytes())?;
                    return Ok(());
                }
                "LIST" => {
                    if t.get(1).map(|s| s.as_str()) == Some("") {
                        out.extend_from_slice(b"* LIST (\\Noselect) \"/\" \"\"\r\n");
                    } else {
                        let s = st.lock();
                        for (name, mb) in &s.mailboxes {
                            out.extend_from_slice(format!("* LIST ({}) \"/\" {}\r\n", mb.attrs, quote(name)).as_bytes());
                        }
                    }
                    ok(&mut out, "done");
                }
                "SELECT" | "EXAMINE" => {
                    let name = t.first().cloned().unwrap_or_default();
                    let s = st.lock();
                    match s.mailboxes.get(&name) {
                        Some(mb) => {
                            out.extend_from_slice(format!("* {} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n* FLAGS (\\Seen \\Flagged)\r\n", mb.messages.len(), mb.uidvalidity, mb.uidnext).as_bytes());
                            selected = Some(name);
                            ok(&mut out, if cmd == "SELECT" { "[READ-WRITE] selected" } else { "[READ-ONLY] examined" });
                        }
                        None => out.extend_from_slice(format!("{tag} NO no such mailbox\r\n").as_bytes()),
                    }
                }
                "STATUS" => {
                    let name = t.first().cloned().unwrap_or_default();
                    let s = st.lock();
                    match s.mailboxes.get(&name) {
                        Some(mb) => {
                            let unseen = mb.messages.iter().filter(|m| !m.flags.iter().any(|f| f == "\\Seen")).count();
                            out.extend_from_slice(format!("* STATUS {} (MESSAGES {} UIDNEXT {} UIDVALIDITY {} UNSEEN {unseen})\r\n", quote(&name), mb.messages.len(), mb.uidnext, mb.uidvalidity).as_bytes());
                            ok(&mut out, "done");
                        }
                        None => out.extend_from_slice(format!("{tag} NO no such mailbox\r\n").as_bytes()),
                    }
                }
                "FETCH" if uid_cmd => {
                    let set = t.first().cloned().unwrap_or_default();
                    let items = args.to_ascii_uppercase();
                    let s = st.lock();
                    let mb = s.mb(selected.as_deref().unwrap_or("INBOX"));
                    let max = mb.messages.iter().map(|m| m.uid).max().unwrap_or(0);
                    for (i, m) in mb.messages.iter().enumerate() {
                        if !in_set(&set, m.uid, max) {
                            continue;
                        }
                        let mut b: Vec<u8> = format!("* {} FETCH (UID {}", i + 1, m.uid).into_bytes();
                        if items.contains("FLAGS") {
                            b.extend_from_slice(format!(" FLAGS ({})", m.flags.join(" ")).as_bytes());
                        }
                        if items.contains("RFC822.SIZE") {
                            b.extend_from_slice(format!(" RFC822.SIZE {}", m.raw.len()).as_bytes());
                        }
                        if items.contains("INTERNALDATE") {
                            b.extend_from_slice(b" INTERNALDATE \"06-Oct-2025 10:00:00 +0000\"");
                        }
                        if items.contains("BODY.PEEK[HEADER]") {
                            let h = header_of(&m.raw);
                            b.extend_from_slice(format!(" BODY[HEADER] {{{}}}\r\n", h.len()).as_bytes());
                            b.extend_from_slice(h);
                        }
                        if items.contains("BODY.PEEK[]") {
                            b.extend_from_slice(format!(" BODY[] {{{}}}\r\n", m.raw.len()).as_bytes());
                            b.extend_from_slice(&m.raw);
                        }
                        b.extend_from_slice(b")\r\n");
                        out.extend_from_slice(&b);
                    }
                    ok(&mut out, "fetched");
                }
                "STORE" if uid_cmd => {
                    let set = t.first().cloned().unwrap_or_default();
                    let mode = t.get(1).cloned().unwrap_or_default();
                    let fl = flag_list(t.get(2).map(|s| s.as_str()).unwrap_or(""));
                    let mut s = st.lock();
                    let mb = s.mb_mut(selected.as_deref().unwrap_or("INBOX"));
                    let max = mb.messages.iter().map(|m| m.uid).max().unwrap_or(0);
                    for m in mb.messages.iter_mut().filter(|m| in_set(&set, m.uid, max)) {
                        for f in &fl {
                            if mode.starts_with('+') {
                                if !m.flags.contains(f) {
                                    m.flags.push(f.clone());
                                }
                            } else {
                                m.flags.retain(|x| x != f);
                            }
                        }
                    }
                    ok(&mut out, "stored");
                }
                "MOVE" | "COPY" if uid_cmd => {
                    let set = t.first().cloned().unwrap_or_default();
                    let dest = t.get(1).cloned().unwrap_or_default();
                    let mut s = st.lock();
                    if !s.mailboxes.contains_key(&dest) {
                        out.extend_from_slice(format!("{tag} NO [TRYCREATE] no such mailbox\r\n").as_bytes());
                    } else {
                        let src = selected.clone().unwrap_or_else(|| "INBOX".into());
                        let max = s.mb(&src).messages.iter().map(|m| m.uid).max().unwrap_or(0);
                        let picked: Vec<(usize, Vec<String>, Vec<u8>)> =
                            s.mb(&src).messages.iter().enumerate().filter(|(_, m)| in_set(&set, m.uid, max)).map(|(i, m)| (i, m.flags.clone(), m.raw.clone())).collect();
                        for (_, flags, raw) in &picked {
                            let fl: Vec<&str> = flags.iter().map(|s| s.as_str()).collect();
                            s.add(&dest, raw.clone(), &fl);
                        }
                        if cmd == "MOVE" {
                            for (i, _, _) in picked.iter().rev() {
                                s.mb_mut(&src).messages.remove(*i);
                                out.extend_from_slice(format!("* {} EXPUNGE\r\n", i + 1).as_bytes());
                            }
                        }
                        ok(&mut out, "done");
                    }
                }
                "EXPUNGE" => {
                    let set = if uid_cmd { t.first().cloned() } else { None };
                    let mut s = st.lock();
                    let mb = s.mb_mut(selected.as_deref().unwrap_or("INBOX"));
                    let max = mb.messages.iter().map(|m| m.uid).max().unwrap_or(0);
                    let mut i = mb.messages.len();
                    while i > 0 {
                        i -= 1;
                        let m = &mb.messages[i];
                        if m.flags.iter().any(|f| f == "\\Deleted") && set.as_deref().is_none_or(|s| in_set(s, m.uid, max)) {
                            mb.messages.remove(i);
                            out.extend_from_slice(format!("* {} EXPUNGE\r\n", i + 1).as_bytes());
                        }
                    }
                    ok(&mut out, "expunged");
                }
                "APPEND" => {
                    let name = t.first().cloned().unwrap_or_default();
                    let fl = t.get(1).filter(|x| x.starts_with('(')).map(|x| flag_list(x)).unwrap_or_default();
                    let mut s = st.lock();
                    if !s.mailboxes.contains_key(&name) {
                        out.extend_from_slice(format!("{tag} NO [TRYCREATE] no such mailbox\r\n").as_bytes());
                    } else {
                        let flr: Vec<&str> = fl.iter().map(|s| s.as_str()).collect();
                        let uid = s.add(&name, literal.clone().unwrap_or_default(), &flr);
                        let v = s.mb(&name).uidvalidity;
                        ok(&mut out, &format!("[APPENDUID {v} {uid}] appended"));
                    }
                }
                "CREATE" => {
                    let name = t.first().cloned().unwrap_or_default();
                    let mut s = st.lock();
                    if s.mailboxes.contains_key(&name) {
                        out.extend_from_slice(format!("{tag} NO [ALREADYEXISTS] exists\r\n").as_bytes());
                    } else {
                        s.next_validity += 1;
                        let v = 1000 + s.next_validity;
                        s.mailboxes.insert(name, Mailbox::new("\\HasNoChildren", v));
                        ok(&mut out, "created");
                    }
                }
                "DELETE" => {
                    let name = t.first().cloned().unwrap_or_default();
                    if st.lock().mailboxes.remove(&name).is_some() {
                        ok(&mut out, "deleted");
                    } else {
                        out.extend_from_slice(format!("{tag} NO [NONEXISTENT] no such mailbox\r\n").as_bytes());
                    }
                }
                "RENAME" => {
                    let (from, to) = (t.first().cloned().unwrap_or_default(), t.get(1).cloned().unwrap_or_default());
                    let mut s = st.lock();
                    match s.mailboxes.remove(&from) {
                        Some(mb) => {
                            s.mailboxes.insert(to, mb);
                            ok(&mut out, "renamed");
                        }
                        None => out.extend_from_slice(format!("{tag} NO [NONEXISTENT] no such mailbox\r\n").as_bytes()),
                    }
                }
                "CLOSE" => {
                    selected = None;
                    ok(&mut out, "closed");
                }
                "IDLE" => {
                    w.write_all(b"+ idling\r\n")?;
                    let name = selected.clone().unwrap_or_else(|| "INBOX".into());
                    let mut count = st.lock().mb(&name).messages.len();
                    w.set_read_timeout(Some(Duration::from_millis(50)))?;
                    loop {
                        let mut l = String::new();
                        match r.read_line(&mut l) {
                            Ok(0) => return Ok(()),
                            Ok(_) => {
                                if l.trim().eq_ignore_ascii_case("DONE") {
                                    break;
                                }
                            }
                            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                                let now = st.lock().mailboxes.get(&name).map(|m| m.messages.len()).unwrap_or(0);
                                if now != count {
                                    count = now;
                                    w.write_all(format!("* {now} EXISTS\r\n").as_bytes())?;
                                }
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    w.set_read_timeout(None)?;
                    ok(&mut out, "idle done");
                }
                _ => out.extend_from_slice(format!("{tag} BAD unknown command\r\n").as_bytes()),
            }
            w.write_all(&out)?;
        }
    }
}

// ====================================================================== fake POP3 server

mod fake_pop3 {
    use super::*;

    #[derive(Default)]
    pub struct State {
        pub messages: Vec<(String, Vec<u8>)>,
        pub deleted: Vec<String>,
        pub password: String,
        pub commands: Vec<String>,
    }

    pub type Shared = Arc<Mutex<State>>;

    pub fn start(state: Shared) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let st = state.clone();
                std::thread::spawn(move || {
                    let _ = session(s, st);
                });
            }
        });
        port
    }

    fn session(stream: TcpStream, st: Shared) -> std::io::Result<()> {
        const STAMP: &str = "<1896.697170952@fake.example>";
        let mut r = BufReader::new(stream.try_clone()?);
        let mut w = stream;
        w.write_all(format!("+OK fake POP3 ready {STAMP}\r\n").as_bytes())?;
        let mut marked: Vec<usize> = Vec::new();
        let mut authed = false;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let line = line.trim_end().to_string();
            st.lock().commands.push(if line.starts_with("PASS") { "PASS ***".into() } else { line.clone() });
            let mut parts = line.split_whitespace();
            let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
            let arg1 = parts.next().unwrap_or("").to_string();
            let arg2 = parts.next().unwrap_or("").to_string();
            let reply: String = match cmd.as_str() {
                "CAPA" => "+OK\r\nUSER\r\nUIDL\r\nTOP\r\n.\r\n".into(),
                "USER" => "+OK\r\n".into(),
                "PASS" => {
                    if arg1 == st.lock().password {
                        authed = true;
                        "+OK logged in\r\n".into()
                    } else {
                        "-ERR invalid password\r\n".into()
                    }
                }
                "APOP" => {
                    let expect = purpamail::net::pop3::md5_hex(format!("{STAMP}{}", st.lock().password).as_bytes());
                    if arg2 == expect {
                        authed = true;
                        "+OK logged in\r\n".into()
                    } else {
                        "-ERR bad digest\r\n".into()
                    }
                }
                _ if !authed && cmd != "QUIT" => "-ERR not authenticated\r\n".into(),
                "STAT" => {
                    let s = st.lock();
                    format!("+OK {} {}\r\n", s.messages.len(), s.messages.iter().map(|m| m.1.len()).sum::<usize>())
                }
                "UIDL" => {
                    let s = st.lock();
                    let mut o = String::from("+OK\r\n");
                    for (i, (u, _)) in s.messages.iter().enumerate() {
                        o.push_str(&format!("{} {u}\r\n", i + 1));
                    }
                    o + ".\r\n"
                }
                "LIST" => {
                    let s = st.lock();
                    let mut o = String::from("+OK\r\n");
                    for (i, (_, raw)) in s.messages.iter().enumerate() {
                        o.push_str(&format!("{} {}\r\n", i + 1, raw.len()));
                    }
                    o + ".\r\n"
                }
                "RETR" => {
                    let s = st.lock();
                    match arg1.parse::<usize>().ok().and_then(|n| s.messages.get(n.wrapping_sub(1))) {
                        Some((_, raw)) => {
                            let mut o = String::from("+OK\r\n");
                            for l in String::from_utf8_lossy(raw).split("\r\n") {
                                if l.starts_with('.') {
                                    o.push('.');
                                }
                                o.push_str(l);
                                o.push_str("\r\n");
                            }
                            o + ".\r\n"
                        }
                        None => "-ERR no such message\r\n".into(),
                    }
                }
                "DELE" => {
                    if let Ok(n) = arg1.parse::<usize>() {
                        marked.push(n - 1);
                    }
                    "+OK marked\r\n".into()
                }
                "NOOP" | "RSET" => "+OK\r\n".into(),
                "QUIT" => {
                    let mut s = st.lock();
                    marked.sort_unstable();
                    for i in marked.iter().rev() {
                        if *i < s.messages.len() {
                            let (u, _) = s.messages.remove(*i);
                            s.deleted.push(u);
                        }
                    }
                    w.write_all(b"+OK bye\r\n")?;
                    return Ok(());
                }
                _ => "-ERR unknown\r\n".into(),
            };
            w.write_all(reply.as_bytes())?;
        }
    }
}

// ====================================================================== fake SMTP server

mod fake_smtp {
    use super::*;

    #[derive(Default, Clone, Debug)]
    pub struct Delivery {
        pub from: String,
        pub rcpts: Vec<String>,
        pub data: String,
    }

    #[derive(Default)]
    pub struct State {
        pub deliveries: Vec<Delivery>,
        pub auths: usize,
    }

    pub type Shared = Arc<Mutex<State>>;

    pub fn start(state: Shared) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let st = state.clone();
                std::thread::spawn(move || {
                    let _ = session(s, st);
                });
            }
        });
        port
    }

    fn session(stream: TcpStream, st: Shared) -> std::io::Result<()> {
        let mut r = BufReader::new(stream.try_clone()?);
        let mut w = stream;
        w.write_all(b"220 fake ESMTP\r\n")?;
        let mut cur = Delivery::default();
        loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let line = line.trim_end().to_string();
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") {
                w.write_all(b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n")?;
            } else if upper.starts_with("HELO") {
                w.write_all(b"250 fake\r\n")?;
            } else if upper.starts_with("AUTH PLAIN") {
                st.lock().auths += 1;
                w.write_all(b"235 ok\r\n")?;
            } else if upper.starts_with("AUTH LOGIN") {
                w.write_all(b"334 VXNlcm5hbWU6\r\n")?;
                let mut u = String::new();
                r.read_line(&mut u)?;
                w.write_all(b"334 UGFzc3dvcmQ6\r\n")?;
                let mut p = String::new();
                r.read_line(&mut p)?;
                st.lock().auths += 1;
                w.write_all(b"235 ok\r\n")?;
            } else if upper.starts_with("MAIL FROM:") {
                cur = Delivery { from: line[10..].trim().trim_matches(['<', '>']).split_whitespace().next().unwrap_or("").trim_matches(['<', '>']).to_string(), ..Default::default() };
                w.write_all(b"250 ok\r\n")?;
            } else if upper.starts_with("RCPT TO:") {
                let a = line[8..].trim().trim_matches(['<', '>']).to_string();
                if a.contains("reject") {
                    w.write_all(b"550 5.1.1 no such user\r\n")?;
                } else {
                    cur.rcpts.push(a);
                    w.write_all(b"250 ok\r\n")?;
                }
            } else if upper == "DATA" {
                w.write_all(b"354 go\r\n")?;
                let mut data = String::new();
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l)? == 0 {
                        return Ok(());
                    }
                    if l == ".\r\n" {
                        break;
                    }
                    data.push_str(l.strip_prefix('.').unwrap_or(&l));
                }
                cur.data = data;
                st.lock().deliveries.push(std::mem::take(&mut cur));
                w.write_all(b"250 queued\r\n")?;
            } else if upper == "QUIT" {
                w.write_all(b"221 bye\r\n")?;
                return Ok(());
            } else {
                w.write_all(b"250 ok\r\n")?;
            }
        }
    }
}

// ====================================================================== helpers

fn msg(from: &str, subject: &str, msgid: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nMessage-ID: <{msgid}>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

fn server(port: u16) -> ServerConfig {
    ServerConfig { host: "127.0.0.1".into(), port, security: Security::None, username: "user".into(), auth: AuthMethod::Password, allow_insecure_certs: false }
}

fn account(kind: AccountKind, incoming: u16, outgoing: u16) -> AccountConfig {
    AccountConfig {
        id: "acct".into(),
        kind,
        name: "Test".into(),
        full_name: "Me Myself".into(),
        emails: vec!["me@example.com".into()],
        incoming: server(incoming),
        outgoing: server(outgoing),
        use_idle: false,
        download_attachments: DownloadAttachments::All,
        ..Default::default()
    }
}

struct Harness {
    store: SharedStore,
    settings: Arc<RwLock<Settings>>,
    engine: EngineHandle,
    events: Vec<Event>,
}

impl Harness {
    fn new(acct: AccountConfig, configure: impl FnOnce(&mut Settings)) -> Self {
        let store = Store::open_in_memory().unwrap();
        let mut settings = Settings::default();
        settings.general.check_frequency = CheckFrequency::Manually;
        settings.general.undo_send_delay_secs = 0;
        settings.junk.enabled = false;
        settings.accounts.push(acct);
        configure(&mut settings);
        let mut secrets = Secrets::default();
        secrets.set(Secrets::incoming_key("acct"), "secret".into());
        let settings = Arc::new(RwLock::new(settings));
        let engine = Engine::start(store.clone(), settings.clone(), Arc::new(RwLock::new(secrets)), Arc::new(|| {}));
        Harness { store, settings, engine, events: Vec::new() }
    }

    /// Wait until an event matches; returns it.
    fn wait_event(&mut self, secs: u64, pred: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            for ev in self.engine.poll() {
                let hit = pred(&ev);
                self.events.push(ev.clone());
                if hit {
                    return ev;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for event; seen: {:#?}", self.events.iter().rev().take(30).collect::<Vec<_>>());
    }

    /// Like [`wait_event`](Self::wait_event) but also matches events seen earlier.
    fn wait_seen(&mut self, secs: u64, pred: impl Fn(&Event) -> bool) -> Event {
        if let Some(e) = self.events.iter().find(|e| pred(e)) {
            return e.clone();
        }
        self.wait_event(secs, pred)
    }

    /// Wait until a condition holds.
    fn wait_until(&mut self, secs: u64, what: &str, mut cond: impl FnMut(&Harness) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            self.events.extend(self.engine.poll());
            if cond(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}; events: {:#?}", self.events.iter().rev().take(30).collect::<Vec<_>>());
    }

    fn inbox(&self) -> Mailbox {
        self.store.mailbox_by_role("acct", MailboxRole::Inbox).expect("inbox")
    }

    fn messages(&self, mb: MailboxId) -> Vec<MessageSummary> {
        self.store.list_messages(&MessageQuery { mailboxes: vec![mb], include_deleted: true, ..Default::default() })
    }

    fn by_subject(&self, mb: MailboxId, subject: &str) -> Option<MessageSummary> {
        self.messages(mb).into_iter().find(|m| m.subject == subject)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.engine.send(Command::Shutdown);
    }
}

fn imap_state() -> fake_imap::Shared {
    let mut st = fake_imap::State { password: "secret".into(), ..Default::default() };
    st.mailboxes.insert("INBOX".into(), fake_imap::Mailbox::new("\\HasNoChildren", 100));
    st.mailboxes.insert("Sent Items".into(), fake_imap::Mailbox::new("\\HasNoChildren \\Sent", 200));
    st.mailboxes.insert("Trash".into(), fake_imap::Mailbox::new("\\HasNoChildren \\Trash", 300));
    st.mailboxes.insert("Archive".into(), fake_imap::Mailbox::new("\\HasNoChildren", 400));
    st.mailboxes.insert("Entw&APw-rfe".into(), fake_imap::Mailbox::new("\\HasNoChildren \\Drafts", 500));
    st.add("INBOX", msg("Alice <alice@example.com>", "Hello", "m1@example.com", "First message"), &["\\Seen"]);
    st.add("INBOX", msg("Bob <bob@example.com>", "Flagged one", "m2@example.com", "Second message"), &["\\Flagged", "$MailFlagBit2"]);
    st.add("INBOX", msg("Carol <carol@example.com>", "Unread", "m3@example.com", "Third message"), &[]);
    st.add("Sent Items", msg("Me <me@example.com>", "Sent thing", "s1@example.com", "Sent body"), &["\\Seen"]);
    Arc::new(Mutex::new(st))
}

// ====================================================================== tests

#[test]
fn imap_full_sync_and_remote_operations() {
    let st = imap_state();
    let port = fake_imap::start(st.clone());
    let mut h = Harness::new(account(AccountKind::Imap, port, 1), |_| {});
    h.wait_until(15, "initial inbox sync", |h| h.store.mailbox_by_role("acct", MailboxRole::Inbox).is_some_and(|i| h.messages(i.id).len() == 3));
    h.wait_seen(10, |e| matches!(e, Event::AccountStatus { online: true, error: None, .. }));

    // Mailbox roles from special-use attributes and names; modified UTF-7 decoded.
    let sent = h.store.mailbox_by_role("acct", MailboxRole::Sent).unwrap();
    assert_eq!(sent.path, "Sent Items");
    assert_eq!(h.store.mailbox_by_role("acct", MailboxRole::Trash).unwrap().path, "Trash");
    assert_eq!(h.store.mailbox_by_role("acct", MailboxRole::Archive).unwrap().path, "Archive");
    assert_eq!(h.store.mailbox_by_role("acct", MailboxRole::Drafts).unwrap().path, "Entwürfe");
    h.wait_until(10, "sent synced", |h| h.messages(sent.id).len() == 1);

    // Flags and colors.
    let inbox = h.inbox();
    let hello = h.by_subject(inbox.id, "Hello").unwrap();
    assert!(hello.has(flags::SEEN));
    let flagged = h.by_subject(inbox.id, "Flagged one").unwrap();
    assert!(flagged.has(flags::FLAGGED));
    assert_eq!(flagged.color, Some(FlagColor::Blue));
    let unread = h.by_subject(inbox.id, "Unread").unwrap();
    assert!(unread.is_unread());
    // Bodies prefetched for small inbox messages.
    assert!(unread.has(flags::HAS_BODY));
    assert!(h.store.raw(unread.id).is_some());
    assert_eq!(unread.snippet, "Third message");
    assert_eq!(unread.from.email, "carol@example.com");

    // Push a flag change.
    ops::set_flags(&h.store, &[unread.id], flags::SEEN, 0).unwrap();
    ops::set_flag_color(&h.store, &[unread.id], Some(FlagColor::Green)).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.wait_until(10, "flags pushed", |_| {
        let s = st.lock();
        let m = s.mb("INBOX").by_uid(unread.uid).unwrap();
        ["\\Seen", "\\Flagged", "$MailFlagBit0", "$MailFlagBit1"].iter().all(|f| m.flags.iter().any(|x| x == f))
    });
    h.wait_until(5, "ops queue drained", |h| h.store.pending_ops("acct").is_empty());

    // Move to Archive: the local copy is adopted by Message-ID, not duplicated.
    let archive = h.store.mailbox_by_role("acct", MailboxRole::Archive).unwrap();
    ops::move_to(&h.store, &[hello.id], archive.id).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.wait_until(10, "moved on server", |_| st.lock().mb("Archive").messages.len() == 1 && st.lock().mb("INBOX").messages.len() == 2);
    h.engine.send(Command::SyncMailbox(archive.id));
    h.wait_until(10, "move adopted", |h| h.messages(archive.id).first().is_some_and(|m| m.uid != 0));
    assert_eq!(h.messages(archive.id).len(), 1);
    assert_eq!(h.messages(archive.id)[0].id, hello.id);

    // Append (import into a server mailbox) gets its UID from APPENDUID.
    let ids = ops::import_raw(&h.store, archive.id, &[msg("Dave <dave@example.com>", "Imported", "imp@example.com", "Imported body")]).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.wait_until(10, "appended", |h| h.store.message(ids[0]).is_some_and(|m| m.uid != 0));
    assert_eq!(st.lock().mb("Archive").messages.len(), 2);
    h.engine.send(Command::SyncMailbox(archive.id));
    h.wait_until(10, "archive synced", |h| h.store.mailbox(archive.id).is_some_and(|m| m.uidnext == 3));
    assert_eq!(h.messages(archive.id).len(), 2, "no duplicate after sync");

    // Creating a role mailbox locally creates it on the server.
    ops::role_mailbox(&h.store, "acct", MailboxRole::Junk).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.wait_until(10, "junk created", |_| st.lock().mailboxes.contains_key("Junk"));

    // Server side changes: new message, deletion, flag change.
    {
        let mut s = st.lock();
        s.add("INBOX", msg("Erin <erin@example.com>", "Brand new", "m4@example.com", "Fresh"), &[]);
        let fl = s.mb("INBOX").messages.iter().position(|m| m.uid == flagged.uid).unwrap();
        s.mb_mut("INBOX").messages.remove(fl);
        let un = s.mb_mut("INBOX").messages.iter_mut().find(|m| m.uid == unread.uid).unwrap();
        un.flags.retain(|f| f != "\\Seen");
    }
    h.engine.send(Command::CheckMail(Some("acct".into())));
    let ev = h.wait_event(10, |e| matches!(e, Event::NewMail { .. }));
    let Event::NewMail { account, messages } = ev else { unreachable!() };
    assert_eq!(account, "acct");
    assert_eq!(messages.len(), 1);
    assert_eq!(h.store.message(messages[0]).unwrap().subject, "Brand new");
    h.wait_until(5, "deletion and flags reconciled", |h| h.store.message(flagged.id).is_none() && h.store.message(unread.id).is_some_and(|m| m.is_unread()));

    // A message that was not prefetched gets its body on demand.
    let new_id = messages[0];
    h.store.with_conn(|c| c.execute("DELETE FROM bodies WHERE message_id=?1", [new_id]).unwrap());
    h.store.update_flags(&[new_id], 0, flags::HAS_BODY).unwrap();
    h.engine.send(Command::FetchBody(new_id));
    h.wait_event(10, |e| matches!(e, Event::BodyFetched(id) if *id == new_id));
    assert!(String::from_utf8_lossy(&h.store.raw(new_id).unwrap()).contains("Fresh"));

    // Deleting messages on the client removes them from the server (via Trash).
    let trash = h.store.mailbox_by_role("acct", MailboxRole::Trash).unwrap();
    let settings = h.settings.read().clone();
    ops::delete(&h.store, &settings, &[new_id]).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.wait_until(10, "moved to trash on server", |_| st.lock().mb("Trash").subjects().contains(&"Brand new".to_string()));
    let in_trash = h.store.message(new_id).unwrap();
    assert_eq!(in_trash.mailbox_id, trash.id);

    // UIDVALIDITY change reloads the mailbox.
    st.lock().mb_mut("Archive").uidvalidity = 999;
    h.engine.send(Command::SyncMailbox(archive.id));
    h.wait_until(10, "archive reloaded", |h| h.store.mailbox(archive.id).is_some_and(|m| m.uidvalidity == 999) && h.messages(archive.id).len() == 2);
    assert!(h.store.message(hello.id).is_none(), "old rows replaced");

    // Mailboxes removed on the server disappear locally.
    st.lock().mailboxes.remove("Junk");
    h.engine.send(Command::Synchronize("acct".into()));
    h.wait_until(10, "junk mailbox removed", |h| h.store.mailbox_by_role("acct", MailboxRole::Junk).is_none());
}

#[test]
fn imap_blocked_sender_goes_to_trash() {
    let st = imap_state();
    let port = fake_imap::start(st.clone());
    let mut h = Harness::new(account(AccountKind::Imap, port, 1), |_| {});
    h.wait_until(15, "initial sync", |h| h.store.mailbox_by_role("acct", MailboxRole::Inbox).is_some_and(|i| h.messages(i.id).len() == 3));
    h.store.set_blocked("spammer@example.com", true).unwrap();
    st.lock().add("INBOX", msg("Spammer <spammer@example.com>", "Buy now", "spam1@example.com", "cheap"), &[]);
    st.lock().add("INBOX", msg("Friend <friend@example.com>", "Hi there", "ok1@example.com", "hello"), &[]);
    h.engine.send(Command::CheckMail(None));
    let Event::NewMail { messages, .. } = h.wait_event(10, |e| matches!(e, Event::NewMail { .. })) else { unreachable!() };
    assert_eq!(messages.len(), 1);
    assert_eq!(h.store.message(messages[0]).unwrap().subject, "Hi there");
    let trash = h.store.mailbox_by_role("acct", MailboxRole::Trash).unwrap();
    h.wait_until(10, "blocked message moved on server", |_| st.lock().mb("Trash").subjects().contains(&"Buy now".to_string()));
    assert!(h.by_subject(trash.id, "Buy now").is_some());
}

#[test]
fn imap_idle_delivers_new_mail() {
    let st = imap_state();
    let port = fake_imap::start(st.clone());
    let mut acct = account(AccountKind::Imap, port, 1);
    acct.use_idle = true;
    let mut h = Harness::new(acct, |_| {});
    h.wait_until(15, "initial sync", |h| h.store.mailbox_by_role("acct", MailboxRole::Inbox).is_some_and(|i| h.messages(i.id).len() == 3));
    h.wait_until(10, "idle started", |_| st.lock().count(" IDLE") > 0);
    st.lock().add("INBOX", msg("Zed <zed@example.com>", "Pushed", "push@example.com", "via idle"), &[]);
    let Event::NewMail { messages, .. } = h.wait_event(10, |e| matches!(e, Event::NewMail { .. })) else { unreachable!() };
    assert_eq!(h.store.message(messages[0]).unwrap().subject, "Pushed");
}

#[test]
fn imap_offline_and_bad_password() {
    let st = imap_state();
    let port = fake_imap::start(st.clone());
    let mut h = Harness::new(account(AccountKind::Imap, port, 1), |s| s.work_offline = true);
    h.wait_event(5, |e| matches!(e, Event::AccountStatus { online: false, .. }));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(st.lock().count("LOGIN"), 0, "no connection while offline");
    h.engine.send(Command::SetOnline { account: None, online: true });
    h.wait_until(15, "synced after going online", |h| h.store.mailbox_by_role("acct", MailboxRole::Inbox).is_some_and(|i| h.messages(i.id).len() == 3));

    // Wrong password: an error status, no panic.
    st.lock().password = "other".into();
    h.engine.send(Command::SetOnline { account: None, online: false });
    h.wait_event(5, |e| matches!(e, Event::AccountStatus { online: false, .. }));
    h.engine.send(Command::SetOnline { account: None, online: true });
    let ev = h.wait_event(10, |e| matches!(e, Event::AccountStatus { error: Some(_), .. }));
    let Event::AccountStatus { error: Some(err), .. } = ev else { unreachable!() };
    assert!(err.contains("rejected"), "{err}");
    // A body request while the server is unreachable fails cleanly.
    h.engine.send(Command::FetchBody(987_654));
    h.wait_event(5, |e| matches!(e, Event::BodyFetchFailed(987_654, _)));
}

#[test]
fn pop3_download_and_removal_policy() {
    let st: fake_pop3::Shared = Arc::new(Mutex::new(fake_pop3::State { password: "secret".into(), ..Default::default() }));
    {
        let mut s = st.lock();
        s.messages.push(("u1".into(), msg("A <a@example.com>", "Pop one", "p1@example.com", "one\r\n.dotted line")));
        s.messages.push(("u2".into(), msg("B <b@example.com>", "Pop two", "p2@example.com", "two")));
    }
    let port = fake_pop3::start(st.clone());
    let mut acct = account(AccountKind::Pop3, port, 1);
    acct.pop3_remove = Pop3Removal::WhenMovedFromInbox;
    let mut h = Harness::new(acct, |_| {});
    h.wait_until(15, "initial pop download", |h| h.store.mailbox_by_role("acct", MailboxRole::Inbox).is_some_and(|i| h.messages(i.id).len() == 2));
    let inbox = h.inbox();
    let one = h.by_subject(inbox.id, "Pop one").unwrap();
    assert!(String::from_utf8_lossy(&h.store.raw(one.id).unwrap()).contains("\r\n.dotted line"), "dot-stuffing undone");
    assert!(st.lock().commands.iter().any(|c| c.starts_with("APOP")), "APOP used on plain connection");
    assert!(h.store.kv_get("pop3:uidl:acct").unwrap().contains("u1"));

    // New message → NewMail; already downloaded ones are not fetched again.
    st.lock().messages.push(("u3".into(), msg("C <c@example.com>", "Pop three", "p3@example.com", "three")));
    h.engine.send(Command::CheckMail(Some("acct".into())));
    let Event::NewMail { messages, .. } = h.wait_event(10, |e| matches!(e, Event::NewMail { .. })) else { unreachable!() };
    assert_eq!(messages.len(), 1);
    assert_eq!(h.messages(inbox.id).len(), 3);

    // Moving a message out of the Inbox removes it from the server.
    let archive = ops::role_mailbox(&h.store, "acct", MailboxRole::Archive).unwrap();
    ops::move_to(&h.store, &[one.id], archive.id).unwrap();
    h.engine.send(Command::FlushOps("acct".into()));
    h.engine.send(Command::CheckMail(Some("acct".into())));
    h.wait_until(10, "removed from server", |_| st.lock().deleted.contains(&"u1".to_string()));
    h.wait_until(5, "ops dropped for POP", |h| h.store.pending_ops("acct").is_empty());
    assert_eq!(h.messages(inbox.id).len(), 2);
}

fn smtp_account(smtp_port: u16) -> AccountConfig {
    let mut a = account(AccountKind::Imap, 1, smtp_port);
    a.store_sent_on_server = false;
    a
}

#[test]
fn smtp_send_flush_and_failures() {
    let st: fake_smtp::Shared = Arc::new(Mutex::new(fake_smtp::State::default()));
    let port = fake_smtp::start(st.clone());
    let mut h = Harness::new(smtp_account(port), |s| s.accounts[0].enabled = true);
    let message = OutgoingMessage {
        account_id: "acct".into(),
        from: Address::new("Me Myself", "me@example.com"),
        to: vec![Address::new("You", "you@example.com")],
        bcc: vec![Address::new("", "hidden@example.com")],
        subject: "Greetings".into(),
        text: "Hello there".into(),
        message_id: "out1@example.com".into(),
        ..Default::default()
    };
    let ob = ops::queue_outgoing(&h.store, &message, None).unwrap();
    h.engine.send(Command::Send { outbox_id: ob, message: Box::new(message.clone()) });
    let Event::SendSucceeded { sent_id, .. } = h.wait_event(15, |e| matches!(e, Event::SendSucceeded { outbox_id, .. } if *outbox_id == ob)) else { unreachable!() };
    {
        let s = st.lock();
        let d = &s.deliveries[0];
        assert_eq!(d.from, "me@example.com");
        assert_eq!(d.rcpts, vec!["you@example.com", "hidden@example.com"]);
        assert!(d.data.contains("Subject: Greetings"));
        assert!(!d.data.contains("hidden@example.com"), "Bcc header must not be transmitted");
        assert!(s.auths >= 1);
    }
    assert!(h.store.message(ob).is_none(), "removed from Outbox");
    let sent = h.store.message(sent_id.unwrap()).unwrap();
    assert_eq!(h.store.mailbox(sent.mailbox_id).unwrap().role, MailboxRole::Sent);
    h.wait_seen(5, |e| matches!(e, Event::Activity(a) if a.title.contains("Greetings") && a.state == purpamail::net::ActivityState::Done));

    // FlushOutbox reconstructs queued messages from their source.
    let queued = OutgoingMessage { message_id: "out2@example.com".into(), subject: "Queued".into(), bcc: vec![Address::new("", "bcc2@example.com")], ..message.clone() };
    let ob2 = ops::queue_outgoing(&h.store, &queued, None).unwrap();
    h.engine.send(Command::FlushOutbox);
    h.wait_event(15, |e| matches!(e, Event::SendSucceeded { outbox_id, .. } if *outbox_id == ob2));
    {
        let s = st.lock();
        let d = s.deliveries.iter().find(|d| d.data.contains("Subject: Queued")).unwrap();
        assert!(d.rcpts.contains(&"bcc2@example.com".to_string()));
        assert!(!d.data.contains("bcc2@example.com"));
    }

    // Scheduled messages wait.
    let later = OutgoingMessage { message_id: "out3@example.com".into(), subject: "Later".into(), ..message.clone() };
    let ob3 = ops::queue_outgoing(&h.store, &later, Some(now_ts() + 3600)).unwrap();
    h.engine.send(Command::Send { outbox_id: ob3, message: Box::new(later) });
    std::thread::sleep(Duration::from_millis(500));
    assert!(h.store.message(ob3).is_some());

    // Rejected recipient: SendFailed and the message stays in the Outbox.
    let bad = OutgoingMessage { message_id: "out4@example.com".into(), subject: "Bad".into(), to: vec![Address::new("", "reject@example.com")], bcc: vec![], ..message };
    let ob4 = ops::queue_outgoing(&h.store, &bad, None).unwrap();
    h.engine.send(Command::Send { outbox_id: ob4, message: Box::new(bad) });
    let Event::SendFailed { error, .. } = h.wait_event(15, |e| matches!(e, Event::SendFailed { outbox_id, .. } if *outbox_id == ob4)) else { unreachable!() };
    assert!(!error.is_empty());
    assert!(h.store.message(ob4).is_some());
}

#[test]
fn doctor_and_account_test() {
    let ist = imap_state();
    let iport = fake_imap::start(ist.clone());
    let sst: fake_smtp::Shared = Arc::new(Mutex::new(fake_smtp::State::default()));
    let sport = fake_smtp::start(sst);
    let acct = account(AccountKind::Imap, iport, sport);
    let mut h = Harness::new(acct.clone(), |s| s.work_offline = true);

    h.engine.send(Command::TestAccount(Box::new(acct.clone()), "secret".into()));
    let Event::TestAccountResult { incoming, outgoing } = h.wait_event(10, |e| matches!(e, Event::TestAccountResult { .. })) else { unreachable!() };
    assert_eq!(incoming, Ok(()));
    assert_eq!(outgoing, Ok(()));

    h.engine.send(Command::TestAccount(Box::new(acct.clone()), "wrong".into()));
    let Event::TestAccountResult { incoming, .. } = h.wait_event(10, |e| matches!(e, Event::TestAccountResult { .. })) else { unreachable!() };
    assert!(incoming.is_err());

    h.engine.send(Command::ConnectionDoctor);
    let Event::Doctor(r) = h.wait_event(10, |e| matches!(e, Event::Doctor(_))) else { unreachable!() };
    assert_eq!(r.incoming, ServiceStatus::Ok);
    assert_eq!(r.outgoing, ServiceStatus::Ok);
    assert!(r.log.contains("LOGIN"));
    assert!(!r.log.contains("secret"), "passwords are redacted:\n{}", r.log);

    let bad = AccountConfig { incoming: server(1), ..acct };
    let r = purpamail::net::doctor::diagnose(&bad, &Secrets::default());
    assert!(matches!(r.incoming, ServiceStatus::Failed(_)));
}
