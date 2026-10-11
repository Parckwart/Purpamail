//! Import and export of mailboxes in mbox format (and folders of .eml files).
//!
//! Supported sources:
//! * mbox files in the mboxo and mboxrd variants (`From ` separator lines,
//!   `>From ` un-escaping),
//! * directories of `.eml` files,
//! * Maildir directories (`cur` / `new`, including Maildir++ sub folders),
//! * mailbox bundles (`Name.mbox/` directories containing an `mbox` file),
//! * whole archives of the above via [`import_mailbox_tree`].
//!
//! Export always writes mboxrd.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// Does a line look like an mbox `From ` separator ("From sender date")?
fn is_separator(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"From ") else { return false };
    let rest = String::from_utf8_lossy(rest);
    let mut parts = rest.split_whitespace();
    let Some(_sender) = parts.next() else { return false };
    // Something date-like must follow: a 4 digit year or a time.
    let tail: Vec<&str> = parts.collect();
    !tail.is_empty()
        && tail.iter().any(|p| (p.len() == 4 && p.chars().all(|c| c.is_ascii_digit())) || p.matches(':').count() >= 1 && p.chars().any(|c| c.is_ascii_digit()))
}

/// Split a byte buffer into lines, keeping the line terminators.
fn lines_with_endings(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= data.len() {
            return None;
        }
        let end = data[start..].iter().position(|&b| b == b'\n').map(|p| start + p + 1).unwrap_or(data.len());
        let line = &data[start..end];
        start = end;
        Some(line)
    })
}

fn trim_eol(line: &[u8]) -> &[u8] {
    let l = line.strip_suffix(b"\n").unwrap_or(line);
    l.strip_suffix(b"\r").unwrap_or(l)
}

fn is_blank(line: &[u8]) -> bool {
    trim_eol(line).is_empty()
}

/// Parse an mbox buffer into individual raw messages.
pub fn parse_mbox(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut cur: Option<Vec<u8>> = None;
    let mut prev_blank = true;
    for line in lines_with_endings(data) {
        let content = trim_eol(line);
        if prev_blank && is_separator(content) {
            if let Some(m) = cur.take() {
                out.push(finish_message(m));
            }
            cur = Some(Vec::new());
            prev_blank = false;
            continue;
        }
        prev_blank = is_blank(line);
        let Some(buf) = cur.as_mut() else {
            // Data before the first separator: not an mbox (or garbage); ignore.
            continue;
        };
        // mboxrd / mboxo un-escaping: ">From " -> "From ", ">>From " -> ">From "
        let gt = content.iter().take_while(|&&b| b == b'>').count();
        if gt > 0 && content[gt..].starts_with(b"From ") {
            buf.extend_from_slice(&line[1..]);
        } else {
            buf.extend_from_slice(line);
        }
    }
    if let Some(m) = cur.take() {
        out.push(finish_message(m));
    }
    out.retain(|m| !m.iter().all(|b| b.is_ascii_whitespace()));
    out
}

/// Remove the blank separator line that precedes the next `From ` line.
fn finish_message(mut m: Vec<u8>) -> Vec<u8> {
    if m.ends_with(b"\r\n\r\n") {
        m.truncate(m.len() - 2);
    } else if m.ends_with(b"\n\n") {
        m.truncate(m.len() - 1);
    }
    m
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    v.sort();
    v
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(true)
}

fn is_maildir(dir: &Path) -> bool {
    dir.join("cur").is_dir() || dir.join("new").is_dir()
}

fn read_maildir(dir: &Path) -> Vec<Vec<u8>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for sub in ["cur", "new"] {
        files.extend(sorted_entries(&dir.join(sub)).into_iter().filter(|p| p.is_file() && !is_hidden(p)));
    }
    // Maildir file names start with the delivery timestamp: sort chronologically.
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    files.into_iter().filter_map(|p| fs::read(p).ok()).filter(|d| !d.is_empty()).collect()
}

fn is_eml(p: &Path) -> bool {
    p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("eml"))
}

fn read_eml_dir(dir: &Path) -> Vec<Vec<u8>> {
    sorted_entries(dir).into_iter().filter(|p| is_eml(p)).filter_map(|p| fs::read(p).ok()).filter(|d| !d.is_empty()).collect()
}

fn looks_like_mbox_file(p: &Path) -> bool {
    if !p.is_file() || is_hidden(p) {
        return false;
    }
    let mut head = [0u8; 512];
    let n = fs::File::open(p).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    let first_line = head[..n].split(|&b| b == b'\n').next().unwrap_or(&[]);
    is_separator(trim_eol(first_line))
}

/// Read all messages from an mbox file (mboxrd/mboxo) or a directory of `.eml` files / Maildir.
pub fn import(path: &Path) -> anyhow::Result<Vec<Vec<u8>>> {
    if path.is_dir() {
        if is_maildir(path) {
            return Ok(read_maildir(path));
        }
        let inner = path.join("mbox");
        if inner.is_file() {
            return import(&inner);
        }
        return Ok(read_eml_dir(path));
    }
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let first_line = data.split(|&b| b == b'\n').next().unwrap_or(&[]);
    if is_separator(trim_eol(first_line)) {
        Ok(parse_mbox(&data))
    } else if data.iter().all(|b| b.is_ascii_whitespace()) {
        Ok(Vec::new())
    } else {
        // A single message file (.eml)
        Ok(vec![data])
    }
}

/// Find every mailbox below `path` (mbox files, `.mbox` bundles, Maildirs and folders
/// of `.eml` files) and return them with hierarchical names ("Projects/Website").
pub fn import_mailbox_tree(path: &Path) -> Vec<(String, Vec<Vec<u8>>)> {
    let mut out: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    let root_name = mailbox_name_of(path);
    if path.is_file() {
        if let Ok(msgs) = import(path) {
            out.push((root_name, msgs));
        }
        return out;
    }
    walk(path, "", &root_name, &mut out, 0);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Display name of a mailbox file or directory: file stem without `.mbox` / `.mbx`.
fn mailbox_name_of(p: &Path) -> String {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("Imported").to_string();
    for ext in [".mbox", ".mbx", ".mbs", ".sbd"] {
        if name.len() > ext.len() && name.to_ascii_lowercase().ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name
}

fn join_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") }
}

fn walk(dir: &Path, prefix: &str, root_name: &str, out: &mut Vec<(String, Vec<Vec<u8>>)>, depth: usize) {
    if depth > 32 {
        return;
    }
    // The name this directory itself would have as a mailbox.
    let own_name = if prefix.is_empty() && depth == 0 { root_name.to_string() } else { prefix.to_string() };

    if is_maildir(dir) {
        out.push((own_name.clone(), read_maildir(dir)));
        // Maildir++ sub folders: ".Sub.Folder"
        for e in sorted_entries(dir) {
            let Some(n) = e.file_name().and_then(|n| n.to_str()) else { continue };
            if e.is_dir() && n.starts_with('.') && n.len() > 1 && is_maildir(&e) {
                let sub = n[1..].replace('.', "/");
                let name = if depth == 0 { sub } else { join_name(&own_name, &sub) };
                out.push((name, read_maildir(&e)));
            }
        }
        return;
    }

    let mut emls = Vec::new();
    for e in sorted_entries(dir) {
        if is_hidden(&e) {
            continue;
        }
        let fname = e.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if e.is_dir() {
            let child_prefix = join_name(prefix, &mailbox_name_of(&e));
            walk(&e, &child_prefix, root_name, out, depth + 1);
        } else if fname == "mbox" {
            if let Ok(data) = fs::read(&e) {
                out.push((own_name.clone(), parse_mbox(&data)));
            }
        } else if is_eml(&e) {
            if let Ok(d) = fs::read(&e)
                && !d.is_empty()
            {
                emls.push(d);
            }
        } else if looks_like_mbox_file(&e)
            && let Ok(data) = fs::read(&e)
        {
            out.push((join_name(prefix, &mailbox_name_of(&e)), parse_mbox(&data)));
        }
    }
    if !emls.is_empty() {
        out.push((own_name, emls));
    }
}

/// Envelope sender and date for the `From ` line of a message.
fn envelope(raw: &[u8]) -> (String, String) {
    let parsed = super::mime::parse(raw);
    let sender = parsed
        .as_ref()
        .and_then(|p| {
            p.header("Return-Path")
                .map(|r| r.trim().trim_matches(['<', '>']).to_string())
                .filter(|s| !s.is_empty() && !s.contains(' '))
                .or_else(|| Some(p.from.email.clone()))
        })
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .unwrap_or_else(|| "MAILER-DAEMON".to_string());
    let ts = parsed.map(|p| p.date).filter(|d| *d != 0).unwrap_or_else(super::model::now_ts);
    let date = chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default().format("%a %b %e %H:%M:%S %Y").to_string();
    (sender, date)
}

/// Serialize messages as mboxrd.
pub fn to_mboxrd(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for raw in messages {
        let (sender, date) = envelope(raw);
        out.extend_from_slice(format!("From {sender} {date}\n").as_bytes());
        let mut last_nl = true;
        for line in lines_with_endings(raw) {
            let content = trim_eol(line);
            let gt = content.iter().take_while(|&&b| b == b'>').count();
            if content[gt..].starts_with(b"From ") {
                out.push(b'>');
            }
            out.extend_from_slice(line);
            last_nl = line.ends_with(b"\n");
        }
        if !last_nl {
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    out
}

/// Write messages to an mbox file.
pub fn export(path: &Path, messages: &[Vec<u8>]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir).ok();
    }
    let mut f = fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(&to_mboxrd(messages))?;
    f.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("purpamail-mbox-test-{}-{}", name, uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    const M1: &[u8] =
        b"From: Anna <anna@example.com>\nTo: me@example.org\nSubject: One\nDate: Mon, 6 Oct 2025 10:00:00 +0000\n\nHello\nFrom the body\n>From quoted\n";
    const M2: &[u8] = b"From: Bob <bob@example.com>\r\nSubject: Two\r\n\r\nSecond\r\n";

    #[test]
    fn mboxrd_roundtrip() {
        let data = to_mboxrd(&[M1.to_vec(), M2.to_vec()]);
        let s = String::from_utf8_lossy(&data);
        assert!(s.starts_with("From anna@example.com Mon Oct  6 10:00:00 2025\n"));
        assert!(s.contains("\n>From the body\n"));
        assert!(s.contains("\n>>From quoted\n"));
        let back = parse_mbox(&data);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], M1.to_vec());
        assert_eq!(back[1], M2.to_vec());
    }

    #[test]
    fn mboxo_without_blank_line_escape() {
        let data =
            b"From MAILER-DAEMON Thu Jan  1 00:00:00 2025\nSubject: a\n\nFrom here on, text.\n\nFrom someone Fri Jan  2 00:00:00 2025\nSubject: b\n\nbody\n";
        let msgs = parse_mbox(data);
        assert_eq!(msgs.len(), 2);
        // "From here on" has no date, so it is body text, not a separator.
        assert!(String::from_utf8_lossy(&msgs[0]).contains("From here on, text."));
        assert!(parse_mbox(b"").is_empty());
        assert!(parse_mbox(b"no separator here\n").is_empty());
    }

    #[test]
    fn file_export_import_and_eml_dir() {
        let d = tmpdir("files");
        let p = d.join("out/test.mbox");
        export(&p, &[M1.to_vec(), M2.to_vec()]).unwrap();
        assert_eq!(import(&p).unwrap().len(), 2);
        let emls = d.join("emls");
        fs::create_dir_all(&emls).unwrap();
        fs::write(emls.join("a.eml"), M1).unwrap();
        fs::write(emls.join("b.eml"), M2).unwrap();
        fs::write(emls.join("notes.txt"), b"ignore").unwrap();
        assert_eq!(import(&emls).unwrap().len(), 2);
        assert_eq!(import(&emls.join("a.eml")).unwrap(), vec![M1.to_vec()]);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn maildir_import() {
        let d = tmpdir("maildir");
        fs::create_dir_all(d.join("cur")).unwrap();
        fs::create_dir_all(d.join("new")).unwrap();
        fs::create_dir_all(d.join("tmp")).unwrap();
        fs::write(d.join("cur/1700000000.1.host:2,S"), M1).unwrap();
        fs::write(d.join("new/1700000001.2.host"), M2).unwrap();
        assert_eq!(import(&d).unwrap().len(), 2);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn mailbox_tree() {
        let d = tmpdir("tree");
        let root = d.join("Archive");
        fs::create_dir_all(root.join("Projects.mbox/Website.mbox")).unwrap();
        fs::write(root.join("Projects.mbox/mbox"), to_mboxrd(&[M1.to_vec()])).unwrap();
        fs::write(root.join("Projects.mbox/Website.mbox/mbox"), to_mboxrd(&[M1.to_vec(), M2.to_vec()])).unwrap();
        fs::write(root.join("Receipts"), to_mboxrd(&[M2.to_vec()])).unwrap();
        fs::write(root.join("README.txt"), b"not a mailbox").unwrap();
        fs::create_dir_all(root.join("Old/Maildir/cur")).unwrap();
        fs::write(root.join("Old/Maildir/cur/1.x:2,S"), M1).unwrap();
        fs::create_dir_all(root.join("Old/Maildir/.Sub.Deep/cur")).unwrap();
        fs::write(root.join("Old/Maildir/.Sub.Deep/cur/2.x:2,S"), M2).unwrap();
        fs::create_dir_all(root.join("Loose")).unwrap();
        fs::write(root.join("Loose/x.eml"), M2).unwrap();
        let tree = import_mailbox_tree(&root);
        let names: Vec<(&str, usize)> = tree.iter().map(|(n, m)| (n.as_str(), m.len())).collect();
        assert_eq!(names, vec![("Loose", 1), ("Old/Maildir", 1), ("Old/Maildir/Sub/Deep", 1), ("Projects", 1), ("Projects/Website", 2), ("Receipts", 1)]);
        // A single mbox file as root
        let single = import_mailbox_tree(&root.join("Receipts"));
        assert_eq!(single, vec![("Receipts".to_string(), vec![M2.to_vec()])]);
        fs::remove_dir_all(&d).ok();
    }
}
