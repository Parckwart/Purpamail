//! General preferences: default mail reader, checking, sounds, notifications,
//! downloads, deleting, search scope, undo send and launch at login.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use egui::{RichText, Ui};
use egui_phosphor::regular as ic;

use super::form::{self, Form};
use super::{Edit, page};
use crate::core::config::{CheckFrequency, NotifyFor, RemoveDownloads, Settings};
use crate::ui::actions::UiCx;
use crate::ui::rule_editor::sound_picker;
use crate::ui::theme::Palette;

/// Desktop entry name of Purpamail (matches the app id).
pub const DESKTOP_FILE: &str = "org.purpamail.Purpamail.desktop";

const F: Form = Form::new(230.0);

#[derive(Default)]
pub struct GeneralState {
    /// Current default handler for `mailto:` (queried lazily).
    current_handler: Option<String>,
    query: Option<JoinHandle<String>>,
    /// Output of the last "make default" command.
    set_result: Option<(bool, String)>,
    set_job: Option<JoinHandle<(bool, String)>>,
    autostart_error: Option<String>,
}

pub fn show(ui: &mut Ui, st: &mut GeneralState, s: &mut Settings, _cx: &mut UiCx, _edit: &mut Edit) {
    poll_jobs(ui, st);
    page(ui, "prefs_general", 620.0, |ui| {
        let pal = Palette::of(ui.ctx());
        F.row(ui, "Default mail reader:", |ui| {
            let cur = st.current_handler.clone().unwrap_or_else(|| "…".into());
            let is_us = cur.trim() == DESKTOP_FILE;
            ui.horizontal(|ui| {
                if is_us {
                    ui.label(RichText::new(format!("{} Purpamail", ic::CHECK_CIRCLE)).color(pal.accent));
                } else {
                    ui.label(if cur.trim().is_empty() { "Not set".to_string() } else { pretty_desktop_name(&cur) });
                }
                if !is_us && ui.add_enabled(st.set_job.is_none(), egui::Button::new("Make Purpamail the Default")).clicked() {
                    st.set_job = Some(std::thread::spawn(set_default_mail_reader));
                }
                if st.set_job.is_some() {
                    ui.add(egui::Spinner::new().size(14.0));
                }
            });
            match &st.set_result {
                Some((true, out)) => form::hint(ui, &format!("Done. {out}")),
                Some((false, out)) => form::error_text(ui, out),
                None => form::hint(ui, "Opens “mailto:” links in Purpamail (uses xdg-mime)."),
            }
        });
        form::section(ui);

        F.row(ui, "Check for new messages:", |ui| {
            let opts: Vec<(CheckFrequency, &str)> = CheckFrequency::ALL.iter().map(|c| (*c, c.name())).collect();
            form::combo(ui, "check_freq", &mut s.general.check_frequency, &opts, 200.0);
        });
        F.row(ui, "New messages sound:", |ui| {
            sound_picker(ui, "new_mail_sound", &mut s.general.new_mail_sound, true);
            ui.checkbox(&mut s.general.play_sounds_for_other_actions, "Play sounds for other mail actions");
        });
        F.row(ui, "New message notifications:", |ui| {
            let opts: Vec<(NotifyFor, &str)> = NotifyFor::ALL.iter().map(|c| (*c, c.name())).collect();
            form::combo(ui, "notify_for", &mut s.general.notify_for, &opts, 200.0);
        });
        F.row(ui, "Unread count:", |ui| {
            ui.checkbox(&mut s.general.show_unread_in_title, "Show unread count in the window title");
        });
        form::section(ui);

        F.row(ui, "Downloads folder:", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::FOLDER).color(pal.accent));
                let shown = shorten_home(&s.general.downloads_folder);
                ui.add(egui::Label::new(shown).truncate()).on_hover_text(&s.general.downloads_folder);
            });
            ui.horizontal(|ui| {
                if ui.button("Choose…").clicked() {
                    let mut dlg = rfd::FileDialog::new().set_title("Choose Downloads Folder");
                    if Path::new(&s.general.downloads_folder).is_dir() {
                        dlg = dlg.set_directory(&s.general.downloads_folder);
                    }
                    if let Some(dir) = dlg.pick_folder() {
                        s.general.downloads_folder = dir.display().to_string();
                    }
                }
                if ui.button("Reset").clicked() {
                    s.general.downloads_folder = crate::core::config::downloads_dir().display().to_string();
                }
            });
        });
        F.row(ui, "Remove unedited downloads:", |ui| {
            let opts = [
                (RemoveDownloads::Never, "Never"),
                (RemoveDownloads::WhenMessageDeleted, "When message is deleted"),
                (RemoveDownloads::AfterQuitting, "After quitting Purpamail"),
            ];
            form::combo(ui, "remove_downloads", &mut s.general.remove_unedited_downloads, &opts, 200.0);
        });
        form::section(ui);

        F.row(ui, "Deleting:", |ui| {
            ui.checkbox(&mut s.general.archive_on_delete, "When deleting, archive messages instead");
            form::hint(ui, "Delete moves messages to the account's Archive mailbox instead of the Trash.");
        });
        F.row(ui, "Opening messages:", |ui| {
            ui.checkbox(&mut s.general.prefer_opening_in_split_view, "Prefer opening messages in split view");
        });
        F.row(ui, "When searching all mailboxes,", |ui| {
            ui.label("include results from:");
            ui.checkbox(&mut s.general.search_include_trash, "Trash");
            ui.checkbox(&mut s.general.search_include_junk, "Junk");
            ui.checkbox(&mut s.general.search_include_encrypted, "Encrypted messages");
        });
        form::section(ui);

        F.row(ui, "Undo send delay:", |ui| {
            let opts = [(0u32, "Off"), (5, "5 seconds"), (10, "10 seconds"), (20, "20 seconds"), (30, "30 seconds")];
            form::combo(ui, "undo_send", &mut s.general.undo_send_delay_secs, &opts, 200.0);
            form::hint(ui, "Messages wait this long in the Outbox so you can take them back.");
        });
        F.row(ui, "Startup:", |ui| {
            let mut v = s.general.launch_at_login;
            if ui.checkbox(&mut v, "Open Purpamail when you log in").changed() {
                match set_launch_at_login(v) {
                    Ok(()) => {
                        s.general.launch_at_login = v;
                        st.autostart_error = None;
                    }
                    Err(e) => st.autostart_error = Some(e.to_string()),
                }
            }
            if let Some(e) = &st.autostart_error {
                form::error_text(ui, e);
            } else {
                form::hint(ui, &format!("Adds an entry to {}.", shorten_home(&autostart_path().display().to_string())));
            }
        });
    });
}

fn poll_jobs(ui: &Ui, st: &mut GeneralState) {
    if st.current_handler.is_none() && st.query.is_none() {
        st.query = Some(std::thread::spawn(query_default_mail_reader));
    }
    if st.query.as_ref().is_some_and(|j| j.is_finished()) {
        st.current_handler = st.query.take().and_then(|j| j.join().ok());
    }
    if st.set_job.as_ref().is_some_and(|j| j.is_finished()) {
        st.set_result = st.set_job.take().and_then(|j| j.join().ok());
        st.current_handler = None; // re-query
    }
    if st.query.is_some() || st.set_job.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn query_default_mail_reader() -> String {
    std::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/mailto"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn set_default_mail_reader() -> (bool, String) {
    match std::process::Command::new("xdg-mime").args(["default", DESKTOP_FILE, "x-scheme-handler/mailto"]).output() {
        Ok(o) if o.status.success() => (true, format!("xdg-mime default {DESKTOP_FILE} x-scheme-handler/mailto")),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            (false, if err.is_empty() { format!("xdg-mime failed ({})", o.status) } else { err })
        }
        Err(e) => (false, format!("Could not run xdg-mime: {e}")),
    }
}

/// "thunderbird.desktop" → "thunderbird".
fn pretty_desktop_name(s: &str) -> String {
    s.trim().trim_end_matches(".desktop").rsplit('.').next().unwrap_or(s).to_string()
}

fn shorten_home(p: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let h = home.to_string_lossy().to_string();
        if !h.is_empty() && p.starts_with(&h) {
            return format!("~{}", &p[h.len()..]);
        }
    }
    p.to_string()
}

/// `$XDG_CONFIG_HOME/autostart/purpamail.desktop`.
pub fn autostart_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("autostart").join("purpamail.desktop")
}

/// Contents of the autostart desktop entry for the given executable.
pub fn autostart_entry(exec: &Path) -> String {
    let exec = exec.display().to_string();
    let quoted = if exec.contains(' ') { format!("\"{exec}\"") } else { exec };
    format!(
        "[Desktop Entry]\nType=Application\nName=Purpamail\nComment=Read and send email\nExec={quoted}\nIcon=org.purpamail.Purpamail\nTerminal=false\nCategories=Network;Email;\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// Create or remove the autostart entry.
pub fn set_launch_at_login(enabled: bool) -> anyhow::Result<()> {
    write_autostart(&autostart_path(), enabled, &std::env::current_exe()?)
}

fn write_autostart(path: &Path, enabled: bool, exec: &Path) -> anyhow::Result<()> {
    if enabled {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, autostart_entry(exec))?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_contents() {
        let e = autostart_entry(Path::new("/usr/bin/purpamail"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=/usr/bin/purpamail\n"));
        assert!(e.contains("Type=Application"));
        let q = autostart_entry(Path::new("/opt/my apps/purpamail"));
        assert!(q.contains("Exec=\"/opt/my apps/purpamail\""));
    }

    #[test]
    fn autostart_write_and_remove() {
        let dir = std::env::temp_dir().join(format!("pm-autostart-{}", std::process::id()));
        let p = dir.join("autostart").join("purpamail.desktop");
        write_autostart(&p, true, Path::new("/bin/purpamail")).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("Exec=/bin/purpamail"));
        write_autostart(&p, false, Path::new("/bin/purpamail")).unwrap();
        assert!(!p.exists());
        // removing twice is fine
        write_autostart(&p, false, Path::new("/bin/purpamail")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn desktop_names() {
        assert_eq!(pretty_desktop_name("org.mozilla.Thunderbird.desktop"), "Thunderbird");
        assert_eq!(pretty_desktop_name("evolution.desktop\n"), "evolution");
    }
}
