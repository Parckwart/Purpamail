//! Single-instance support.
//!
//! The first Purpamail process listens on a Unix socket. Later invocations
//! (for example when the desktop opens a `mailto:` link or an `.eml` file)
//! forward their arguments to it and exit, so everything ends up in one window.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use crossbeam_channel::Sender;

pub fn socket_path() -> PathBuf {
    if std::env::var("PURPAMAIL_DATA_DIR").is_ok() {
        return crate::core::config::data_dir().join("purpamail.sock");
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d).join("purpamail.sock"),
        _ => crate::core::config::data_dir().join("purpamail.sock"),
    }
}

/// Send `requests` to an already running instance. Returns true if one received them.
pub fn forward_to_running(requests: &[String]) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    let mut payload = String::new();
    for r in requests {
        payload.push_str(&r.replace('\n', " "));
        payload.push('\n');
    }
    if payload.is_empty() {
        payload.push_str("focus\n");
    }
    s.write_all(payload.as_bytes()).is_ok()
}

/// Start listening for requests from later invocations.
pub fn listen(tx: Sender<String>, wake: impl Fn() + Send + 'static) {
    let path = socket_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        log::warn!("could not listen on {}", path.display());
        return;
    };
    std::thread::Builder::new()
        .name("purpamail-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if !line.is_empty() && tx.send(line).is_err() {
                        return;
                    }
                }
                wake();
            }
        })
        .ok();
}

/// Turn command line arguments into requests (`mailto:` URLs and message files).
pub fn requests_from_args(args: &[String]) -> Vec<String> {
    args.iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(|a| {
            if a.starts_with("mailto:") {
                a.clone()
            } else {
                let p = std::path::Path::new(a);
                std::fs::canonicalize(p).map(|p| format!("open:{}", p.display())).unwrap_or_else(|_| format!("open:{a}"))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_to_requests() {
        let r = requests_from_args(&["purpamail".into(), "--demo".into(), "mailto:a@b.c?subject=x".into()]);
        assert_eq!(r, vec!["mailto:a@b.c?subject=x".to_string()]);
    }
}
