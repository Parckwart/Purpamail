//! "Add Account" assistant.
//!
//! Steps: choose a provider (with a welcome screen at first launch) → name,
//! email and password → automatic configuration and a connection test →
//! (on failure or on request) manual server settings → summary. On success
//! the account and its passwords are saved and mail is checked right away.

use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as ic;

use super::actions::{Action, SidebarSel, UiCx};
use super::prefs::form::{self, Form};
use super::theme::Palette;
use crate::core::config::{AccountConfig, AccountKind, AuthMethod, Secrets, Security, ServerConfig};
use crate::core::model::MailboxRole;
use crate::net::{Command, Event};

/// Seconds to wait for the connection test before giving up.
const TEST_TIMEOUT: f64 = 40.0;
const F: Form = Form::new(140.0);

// ------------------------------------------------------------------ providers

/// A provider tile on the first page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    pub name: &'static str,
    pub subtitle: &'static str,
    pub kind: AccountKind,
    /// An address at the provider's own domain, used to look up its servers
    /// even when the user's address has a custom domain.
    pub sample: Option<&'static str>,
}

pub const PROVIDERS: [Provider; 11] = [
    Provider { name: "IMAP Account", subtitle: "Keeps mail on the server", kind: AccountKind::Imap, sample: None },
    Provider { name: "POP Account", subtitle: "Downloads mail locally", kind: AccountKind::Pop3, sample: None },
    Provider { name: "Gmail-compatible", subtitle: "imap.gmail.com", kind: AccountKind::Imap, sample: Some("user@gmail.com") },
    Provider { name: "Outlook-compatible", subtitle: "outlook.office365.com", kind: AccountKind::Imap, sample: Some("user@outlook.com") },
    Provider { name: "Fastmail", subtitle: "imap.fastmail.com", kind: AccountKind::Imap, sample: Some("user@fastmail.com") },
    Provider { name: "Yahoo", subtitle: "imap.mail.yahoo.com", kind: AccountKind::Imap, sample: Some("user@yahoo.com") },
    Provider { name: "GMX", subtitle: "imap.gmx.net", kind: AccountKind::Imap, sample: Some("user@gmx.net") },
    Provider { name: "web.de", subtitle: "imap.web.de", kind: AccountKind::Imap, sample: Some("user@web.de") },
    Provider { name: "Posteo", subtitle: "posteo.de", kind: AccountKind::Imap, sample: Some("user@posteo.de") },
    Provider { name: "mailbox.org", subtitle: "imap.mailbox.org", kind: AccountKind::Imap, sample: Some("user@mailbox.org") },
    Provider { name: "Proton Bridge", subtitle: "Local bridge on 127.0.0.1", kind: AccountKind::Imap, sample: Some("user@proton.me") },
];

// ------------------------------------------------------------------ ports

/// The standard port for a server type and security.
pub fn default_port(incoming: bool, kind: AccountKind, security: Security) -> u16 {
    match (incoming, kind, security) {
        (true, AccountKind::Imap, Security::Tls) => 993,
        (true, AccountKind::Imap, _) => 143,
        (true, AccountKind::Pop3, Security::Tls) => 995,
        (true, AccountKind::Pop3, _) => 110,
        (false, _, Security::Tls) => 465,
        (false, _, _) => 587,
    }
}

/// After the server type or security changed: switch to the new standard port
/// unless the user entered a non-standard one.
pub fn adjust_port(port: u16, incoming: bool, kind: AccountKind, security: Security) -> u16 {
    const STD_IN: [u16; 4] = [993, 143, 995, 110];
    const STD_OUT: [u16; 3] = [465, 587, 25];
    let std: &[u16] = if incoming { &STD_IN } else { &STD_OUT };
    if port == 0 || std.contains(&port) { default_port(incoming, kind, security) } else { port }
}

/// Server settings rows (host, port & security, credentials, authentication,
/// certificates). Returns true when anything changed.
#[allow(clippy::too_many_arguments)]
pub fn server_fields(ui: &mut Ui, f: Form, id: &str, srv: &mut ServerConfig, incoming: bool, kind: AccountKind, password: Option<&mut String>, show_credentials: bool) -> bool {
    let mut changed = false;
    let pal = Palette::of(ui.ctx());
    f.row(ui, "Host Name:", |ui| {
        changed |= form::text_field(ui, &mut srv.host, 240.0, if incoming { "imap.example.com" } else { "smtp.example.com" }).changed();
    });
    f.row(ui, "Port:", |ui| {
        ui.horizontal(|ui| {
            changed |= ui.add(egui::DragValue::new(&mut srv.port).range(1..=65535).speed(0.0)).changed();
            let mut sec = srv.security;
            let opts = [(Security::Tls, Security::Tls.name()), (Security::StartTls, Security::StartTls.name()), (Security::None, Security::None.name())];
            if form::combo(ui, (id, "sec"), &mut sec, &opts, 110.0) {
                srv.security = sec;
                srv.port = adjust_port(srv.port, incoming, kind, sec);
                changed = true;
            }
        });
        if srv.security == Security::None {
            ui.label(RichText::new(format!("{} Your password and messages are sent unencrypted.", ic::WARNING)).size(11.5).color(pal.danger));
        }
    });
    if show_credentials {
        f.row(ui, "User Name:", |ui| changed |= form::text_field(ui, &mut srv.username, 240.0, "Usually your email address").changed());
        if let Some(pw) = password {
            f.row(ui, "Password:", |ui| {
                changed |= ui.add(egui::TextEdit::singleline(pw).password(true).desired_width(240.0).hint_text(if srv.auth == AuthMethod::OAuth2 { "OAuth 2.0 access token" } else { "Required" })).changed();
            });
        }
    }
    f.row(ui, "Authentication:", |ui| {
        let opts = [(AuthMethod::Password, AuthMethod::Password.name()), (AuthMethod::OAuth2, AuthMethod::OAuth2.name()), (AuthMethod::None, AuthMethod::None.name())];
        changed |= form::combo(ui, (id, "auth"), &mut srv.auth, &opts, 160.0);
    });
    f.indented(ui, |ui| {
        changed |= ui.checkbox(&mut srv.allow_insecure_certs, "Allow self-signed or invalid certificates").changed();
    });
    changed
}

// ------------------------------------------------------------------ form

/// Everything entered in the assistant.
#[derive(Clone, Debug)]
pub struct SetupForm {
    pub provider: Provider,
    pub full_name: String,
    pub email: String,
    pub password: String,
    pub description: String,
    /// Server settings (filled by [`SetupForm::autoconfigure`], editable manually).
    pub cfg: AccountConfig,
    /// SMTP password when it differs from the incoming one.
    pub out_password: String,
}

impl SetupForm {
    pub fn new(provider: Provider) -> Self {
        let full_name = std::env::var("USER").ok().and_then(|u| real_name(&u)).unwrap_or_default();
        Self { provider, full_name, email: String::new(), password: String::new(), description: String::new(), cfg: AccountConfig { kind: provider.kind, ..Default::default() }, out_password: String::new() }
    }

    /// Whether the address looks usable.
    pub fn email_valid(&self) -> bool {
        let e = self.email.trim();
        match e.split_once('@') {
            Some((local, domain)) => !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !e.contains(char::is_whitespace),
            None => false,
        }
    }

    /// Fill in the server settings for the chosen provider and address.
    pub fn autoconfigure(&mut self) {
        let email = self.email.trim().to_string();
        self.cfg.kind = self.provider.kind;
        let user_domain = email.rsplit_once('@').map(|(_, d)| d.to_lowercase()).unwrap_or_default();
        let target = match self.provider.sample {
            Some(sample) => {
                let provider_domain = sample.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
                // keep the user's domain when it is one the provider table knows itself
                let mut probe = AccountConfig { kind: self.cfg.kind, ..Default::default() };
                probe.autoconfigure(&email);
                let mut reference = AccountConfig { kind: self.cfg.kind, ..Default::default() };
                reference.autoconfigure(sample);
                if user_domain == provider_domain || probe.incoming.host == reference.incoming.host { email.clone() } else { sample.to_string() }
            }
            None => email.clone(),
        };
        self.cfg.autoconfigure(&target);
        self.cfg.incoming.username = email.clone();
        self.cfg.outgoing.username = email;
    }

    /// The account configuration to save.
    pub fn to_config(&self) -> AccountConfig {
        let mut c = self.cfg.clone();
        let email = self.email.trim().to_string();
        c.full_name = self.full_name.trim().to_string();
        c.name = if self.description.trim().is_empty() { email.clone() } else { self.description.trim().to_string() };
        match c.emails.first_mut() {
            Some(first) => *first = email.clone(),
            None => c.emails = vec![email.clone()],
        }
        c.incoming.host = c.incoming.host.trim().to_string();
        c.outgoing.host = c.outgoing.host.trim().to_string();
        if c.incoming.username.trim().is_empty() {
            c.incoming.username = email.clone();
        }
        c.incoming.username = c.incoming.username.trim().to_string();
        if c.smtp_same_credentials {
            c.outgoing.username = c.incoming.username.clone();
            c.outgoing.auth = if c.outgoing.auth == AuthMethod::None { AuthMethod::None } else { c.incoming.auth };
        } else if c.outgoing.username.trim().is_empty() {
            c.outgoing.username = email;
        }
        c
    }

    /// Passwords to store: (key, value).
    pub fn secrets(&self, account: &str) -> Vec<(String, String)> {
        let mut v = vec![(Secrets::incoming_key(account), self.password.clone())];
        if !self.cfg.smtp_same_credentials {
            v.push((Secrets::outgoing_key(account), self.out_password.clone()));
        }
        v
    }

    fn servers_complete(&self) -> bool {
        !self.cfg.incoming.host.trim().is_empty() && !self.cfg.outgoing.host.trim().is_empty() && self.cfg.incoming.port > 0 && self.cfg.outgoing.port > 0
    }
}

/// The user's full name from the password database (GECOS field).
fn real_name(user: &str) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let parts: Vec<&str> = l.split(':').collect();
        (parts.first() == Some(&user)).then(|| parts.get(4).map(|g| g.split(',').next().unwrap_or("").trim().to_string())).flatten().filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(user))
    })
}

// ------------------------------------------------------------------ assistant

type TestResult = (Result<(), String>, Result<(), String>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Choose,
    Credentials,
    Manual,
    Done,
}

pub struct AccountSetup {
    step: Step,
    form: SetupForm,
    /// Connection test in progress (start time; None = not started yet this frame).
    verifying: Option<f64>,
    start_verify: bool,
    received: Option<TestResult>,
    /// Result of the last failed test, shown on the manual page.
    last_error: Option<TestResult>,
    icon: Option<egui::TextureHandle>,
    first_launch: Option<bool>,
    /// Move keyboard focus to the first empty field of the credentials page.
    focus_pending: bool,
}

impl Default for AccountSetup {
    fn default() -> Self {
        Self { step: Step::Choose, form: SetupForm::new(PROVIDERS[0]), verifying: None, start_verify: false, received: None, last_error: None, icon: None, first_launch: None, focus_pending: false }
    }
}

impl AccountSetup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed engine events (test results) to the assistant.
    pub fn on_event(&mut self, ev: &Event) {
        if let Event::TestAccountResult { incoming, outgoing } = ev
            && (self.verifying.is_some() || self.start_verify) {
                self.received = Some((incoming.clone(), outgoing.clone()));
            }
    }

    /// Returns false once finished or cancelled.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut UiCx) -> bool {
        let first = *self.first_launch.get_or_insert_with(|| cx.settings.read().accounts.is_empty());
        let mut keep = true;
        let vid = egui::ViewportId::from_hash_of("purpamail-add-account");
        let title = if first { "Welcome to Purpamail" } else { "Add Account" };
        let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([560.0, 500.0]).with_min_inner_size([520.0, 440.0]).with_resizable(true);
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            let now = ui.input(|i| i.time);
            form::window_style(ui);
            self.tick(cx, now);
            if self.verifying.is_some() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            let pal = Palette::of(ui.ctx());
            egui::Panel::bottom("setup_buttons")
                .frame(egui::Frame::new().fill(pal.toolbar_bg).inner_margin(egui::Margin::symmetric(20, 12)))
                .show_separator_line(true)
                .show(ui, |ui| {
                    if !self.buttons(ui, cx, first) {
                        keep = false;
                    }
                });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.window_bg).inner_margin(egui::Margin::symmetric(26, 20))).show(ui, |ui| match self.step {
                Step::Choose => {
                    if !self.choose_page(ui, cx, first) {
                        keep = false;
                    }
                }
                Step::Credentials => self.credentials_page(ui),
                Step::Manual => self.manual_page(ui),
                Step::Done => self.done_page(ui),
            });
            if ui.input(|i| i.viewport().close_requested()) {
                keep = false;
            }
        });
        keep
    }

    /// Advance the connection test state machine.
    fn tick(&mut self, cx: &mut UiCx, now: f64) {
        if self.start_verify {
            self.start_verify = false;
            self.received = None;
            self.verifying = Some(now);
            let cfg = self.form.to_config();
            cx.engine.send(Command::TestAccount(Box::new(cfg), self.form.password.clone()));
        }
        let Some(t0) = self.verifying else { return };
        let result = match self.received.take() {
            Some(r) => r,
            None if now - t0 > TEST_TIMEOUT => {
                let msg = "No response from the server (timed out)".to_string();
                (Err(msg.clone()), Err(msg))
            }
            None => return,
        };
        self.verifying = None;
        if result.0.is_ok() && result.1.is_ok() {
            self.last_error = None;
            self.step = Step::Done;
        } else {
            self.last_error = Some(result);
            self.step = Step::Manual;
        }
    }

    fn header(ui: &mut Ui, title: &str, subtitle: &str) {
        let pal = Palette::of(ui.ctx());
        ui.label(RichText::new(title).family(crate::ui::fonts::bold()).size(19.0));
        if !subtitle.is_empty() {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(subtitle).color(pal.text_secondary)).wrap());
        }
        ui.add_space(14.0);
    }

    /// Returns false when the assistant finished (sample mail).
    fn choose_page(&mut self, ui: &mut Ui, cx: &mut UiCx, first: bool) -> bool {
        let pal = Palette::of(ui.ctx());
        if first {
            let tex = self
                .icon
                .get_or_insert_with(|| {
                    let icon = super::icon::app_icon();
                    let img = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
                    ui.ctx().load_texture("pm-setup-icon", img, egui::TextureOptions::LINEAR)
                })
                .clone();
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(64.0, 64.0)));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Welcome to Purpamail").family(crate::ui::fonts::bold()).size(22.0));
                    ui.add(egui::Label::new(RichText::new("All your email accounts in one fast, private mail app. Choose your mail provider to get started.").color(pal.text_secondary)).wrap());
                });
            });
            ui.add_space(14.0);
        } else {
            Self::header(ui, "Choose a mail account provider", "Purpamail works with any provider that offers IMAP or POP access.");
        }
        let mut picked = None;
        egui::ScrollArea::vertical().id_salt("providers").auto_shrink([false, true]).max_height(ui.available_height() - 34.0).show(ui, |ui| {
            let cols = 3;
            let gap = 10.0;
            let w = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
            for chunk in PROVIDERS.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for p in chunk {
                        if provider_tile(ui, p, w).clicked() {
                            picked = Some(*p);
                        }
                    }
                });
                ui.add_space(gap - ui.spacing().item_spacing.y);
            }
        });
        if let Some(p) = picked {
            self.form.provider = p;
            self.form.cfg.kind = p.kind;
            self.step = Step::Credentials;
            self.focus_pending = true;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic::SPARKLE).color(pal.accent));
            let r = ui.add(egui::Label::new(RichText::new("Try Purpamail with sample mail").color(pal.accent)).sense(Sense::click()));
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked() {
                match crate::core::demo::populate(cx.store) {
                    Ok(()) => {
                        cx.act(Action::SettingsChanged);
                        cx.act(Action::SelectSidebar(SidebarSel::AllRole(MailboxRole::Inbox)));
                        return false;
                    }
                    Err(e) => cx.act(Action::Toast(format!("Could not create sample mail: {e}"))),
                }
            }
            true
        })
        .inner
    }

    fn credentials_page(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        let p = self.form.provider;
        let generic = p.sample.is_none();
        Self::header(
            ui,
            &if generic { format!("Add an {} account", if p.kind == AccountKind::Pop3 { "POP" } else { "IMAP" }) } else { format!("Add a {} account", p.name) },
            "Enter your name, email address and password. Purpamail looks up the server settings for you.",
        );
        let busy = self.verifying.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let focus = std::mem::take(&mut self.focus_pending);
            let name_empty = self.form.full_name.trim().is_empty();
            F.row(ui, "Name:", |ui| {
                let r = form::text_field(ui, &mut self.form.full_name, 260.0, "Jane Appleseed");
                if focus && name_empty {
                    r.request_focus();
                }
            });
            F.row(ui, "Email Address:", |ui| {
                let r = form::text_field(ui, &mut self.form.email, 260.0, "name@example.com");
                if focus && !name_empty {
                    r.request_focus();
                }
                if !self.form.email.trim().is_empty() && !self.form.email_valid() {
                    form::error_text(ui, "Enter a complete email address, like name@example.com.");
                }
            });
            F.row(ui, "Password:", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.form.password).password(true).desired_width(260.0).hint_text("Required"));
                if p.name == "Gmail-compatible" || p.name == "Yahoo" || p.name == "Outlook-compatible" {
                    form::hint(ui, "Some providers require an app-specific password for mail apps.");
                }
                if p.name == "Proton Bridge" {
                    form::hint(ui, "Use the password shown in the bridge application.");
                }
            });
            F.row(ui, "Description:", |ui| {
                form::text_field(ui, &mut self.form.description, 260.0, "Optional, e.g. Work");
            });
        });
        if busy {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(F.label_w + 8.0);
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new("Verifying account information…").color(pal.text_secondary));
            });
        }
    }

    fn manual_page(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        Self::header(ui, "Server Settings", "Enter the settings from your mail provider. Purpamail tests them before adding the account.");
        if let Some((inc, out)) = &self.last_error {
            egui::Frame::new().fill(pal.warning_bg).corner_radius(8).inner_margin(10).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{} Unable to verify account name or password.", ic::WARNING)).family(crate::ui::fonts::bold()));
                crate::ui::prefs::form::hint(ui, "Check the settings below and try again.");
                result_lines(ui, inc, out);
            });
            ui.add_space(8.0);
        }
        let busy = self.verifying.is_some();
        egui::ScrollArea::vertical().id_salt("manual").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                F.row(ui, "Email Address:", |ui| form::text_field(ui, &mut self.form.email, 240.0, "name@example.com"));
                ui.add_space(8.0);
                form::title(ui, "Incoming Mail Server");
                ui.add_space(2.0);
                F.row(ui, "Account Type:", |ui| {
                    let mut kind = self.form.cfg.kind;
                    if form::segmented(ui, &mut kind, &[(AccountKind::Imap, "IMAP"), (AccountKind::Pop3, "POP")]) {
                        self.form.cfg.kind = kind;
                        self.form.cfg.incoming.port = adjust_port(self.form.cfg.incoming.port, true, kind, self.form.cfg.incoming.security);
                    }
                });
                let kind = self.form.cfg.kind;
                server_fields(ui, F, "setup_in", &mut self.form.cfg.incoming, true, kind, Some(&mut self.form.password), true);
                form::section(ui);
                form::title(ui, "Outgoing Mail Server (SMTP)");
                ui.add_space(2.0);
                F.indented(ui, |ui| ui.checkbox(&mut self.form.cfg.smtp_same_credentials, "Use the same user name and password"));
                let same = self.form.cfg.smtp_same_credentials;
                server_fields(ui, F, "setup_out", &mut self.form.cfg.outgoing, false, kind, Some(&mut self.form.out_password), !same);
            });
        });
        if busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0));
                ui.label(RichText::new("Verifying account information…").color(pal.text_secondary));
            });
        }
    }

    fn done_page(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().id_salt("done").auto_shrink([false, false]).show(ui, |ui| self.done_contents(ui));
    }

    fn done_contents(&mut self, ui: &mut Ui) {
        let pal = Palette::of(ui.ctx());
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(ic::CHECK_CIRCLE).size(44.0).color(pal.accent));
            ui.label(RichText::new("Your account is ready").family(crate::ui::fonts::bold()).size(19.0));
            ui.label(RichText::new("Purpamail connected to both servers successfully.").color(pal.text_secondary));
        });
        ui.add_space(14.0);
        let cfg = self.form.to_config();
        egui::Frame::new().fill(pal.toolbar_bg).corner_radius(8).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let f = Form::new(130.0);
            f.row(ui, "Account:", |ui| ui.label(RichText::new(cfg.display_name()).family(crate::ui::fonts::bold())));
            f.row(ui, "Email Address:", |ui| ui.label(cfg.primary_email()));
            let kind = if cfg.kind == AccountKind::Pop3 { "POP" } else { "IMAP" };
            f.row(ui, "Incoming:", |ui| ui.label(format!("{kind} · {}:{} · {}", cfg.incoming.host, cfg.incoming.port, cfg.incoming.security.name())));
            f.row(ui, "Outgoing:", |ui| ui.label(format!("SMTP · {}:{} · {}", cfg.outgoing.host, cfg.outgoing.port, cfg.outgoing.security.name())));
        });
        ui.add_space(12.0);
        form::title(ui, "Mailbox Behaviors");
        ui.add_space(4.0);
        let c = &mut self.form.cfg;
        ui.checkbox(&mut c.store_drafts_on_server, "Store draft messages on the server");
        ui.checkbox(&mut c.store_sent_on_server, "Store sent messages on the server");
        ui.checkbox(&mut c.store_junk_on_server, "Store junk messages on the server");
        ui.checkbox(&mut c.move_deleted_to_trash, "Move deleted messages to the Trash mailbox");
        form::hint(ui, "You can change these later in Preferences → Accounts.");
    }

    /// Bottom button row. Returns false when the assistant should close.
    fn buttons(&mut self, ui: &mut Ui, cx: &mut UiCx, first: bool) -> bool {
        let mut keep = true;
        let busy = self.verifying.is_some();
        ui.horizontal(|ui| {
            match self.step {
                Step::Choose => {
                    if ui.add(egui::Button::new(if first { "Not Now" } else { "Cancel" }).min_size(vec2(80.0, 28.0))).clicked() {
                        keep = false;
                    }
                }
                Step::Credentials | Step::Manual => {
                    if busy {
                        if ui.add(egui::Button::new("Stop").min_size(vec2(80.0, 28.0))).clicked() {
                            self.verifying = None;
                            self.received = None;
                        }
                    } else if ui.add(egui::Button::new("Back").min_size(vec2(80.0, 28.0))).clicked() {
                        self.step = if self.step == Step::Manual { Step::Credentials } else { Step::Choose };
                        self.last_error = None;
                    }
                }
                Step::Done => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match self.step {
                Step::Choose => {}
                Step::Credentials => {
                    let ok = self.form.email_valid() && (!self.form.password.is_empty() || self.form.cfg.incoming.auth == AuthMethod::None);
                    if primary(ui, "Sign In", ok && !busy).clicked() || (ok && !busy && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        self.form.autoconfigure();
                        self.start_verify = true;
                    }
                    if ui.add_enabled(!busy && self.form.email_valid(), egui::Button::new("Configure Manually…").min_size(vec2(80.0, 28.0))).clicked() {
                        self.form.autoconfigure();
                        self.step = Step::Manual;
                    }
                }
                Step::Manual => {
                    let ok = self.form.email_valid() && self.form.servers_complete();
                    if primary(ui, "Sign In", ok && !busy).clicked() {
                        self.start_verify = true;
                    }
                    if self.last_error.is_some() && ui.add_enabled(ok && !busy, egui::Button::new("Add Without Verifying").min_size(vec2(80.0, 28.0))).clicked() {
                        self.finish(cx);
                        keep = false;
                    }
                }
                Step::Done => {
                    if primary(ui, "Done", true).clicked() {
                        self.finish(cx);
                        keep = false;
                    }
                }
            });
        });
        keep
    }

    /// Save the account and its passwords and start checking mail.
    fn finish(&mut self, cx: &mut UiCx) {
        let cfg = self.form.to_config();
        let id = cfg.id.clone();
        {
            let mut sec = cx.secrets.write();
            for (k, v) in self.form.secrets(&id) {
                sec.set(k, v);
            }
        }
        if let Err(e) = cx.secrets.read().save() {
            cx.act(Action::Toast(format!("Could not save the password: {e}")));
        }
        {
            let mut s = cx.settings.write();
            s.accounts.push(cfg);
            s.onboarded = true;
        }
        cx.act(Action::SettingsChanged);
        cx.act(Action::CheckMail(Some(id)));
    }
}

fn primary(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let pal = Palette::of(ui.ctx());
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(pal.accent_text)).fill(pal.accent).min_size(vec2(88.0, 28.0)))
}

fn result_lines(ui: &mut Ui, inc: &Result<(), String>, out: &Result<(), String>) {
    let pal = Palette::of(ui.ctx());
    for (what, r) in [("Incoming server", inc), ("Outgoing server", out)] {
        ui.horizontal_top(|ui| match r {
            Ok(()) => {
                ui.label(RichText::new(ic::CHECK_CIRCLE).color(Color32::from_rgb(0x3c, 0xb8, 0x4a)));
                ui.label(format!("{what}: connected"));
            }
            Err(e) => {
                ui.label(RichText::new(ic::X_CIRCLE).color(pal.danger));
                ui.add(egui::Label::new(format!("{what}: {e}")).wrap());
            }
        });
    }
}

fn provider_tile(ui: &mut Ui, p: &Provider, w: f32) -> egui::Response {
    let pal = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 54.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let fill = if hovered { pal.accent_soft } else { pal.toolbar_bg };
        let stroke = Stroke::new(1.0, if hovered { pal.accent } else { pal.separator });
        ui.painter().rect(rect, 10.0, fill, stroke, egui::StrokeKind::Inside);
        ui.painter().text(rect.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, p.name, FontId::new(14.0, crate::ui::fonts::bold()), pal.text);
        let mut job = egui::text::LayoutJob::simple_singleline(p.subtitle.to_string(), FontId::proportional(11.0), pal.text_secondary);
        job.wrap = egui::text::TextWrapping::truncate_at_width(w - 24.0);
        let g = ui.painter().layout_job(job);
        ui.painter().galley(rect.left_top() + vec2(12.0, 30.0), g, pal.text_secondary);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str) -> Provider {
        *PROVIDERS.iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn ports_follow_security() {
        assert_eq!(default_port(true, AccountKind::Imap, Security::Tls), 993);
        assert_eq!(default_port(true, AccountKind::Pop3, Security::StartTls), 110);
        assert_eq!(adjust_port(993, true, AccountKind::Imap, Security::StartTls), 143);
        assert_eq!(adjust_port(993, true, AccountKind::Pop3, Security::Tls), 995);
        assert_eq!(adjust_port(587, false, AccountKind::Imap, Security::Tls), 465);
        // custom ports are kept
        assert_eq!(adjust_port(1143, true, AccountKind::Imap, Security::Tls), 1143);
    }

    #[test]
    fn email_validation() {
        let mut f = SetupForm::new(provider("IMAP Account"));
        for (e, ok) in [("me@example.com", true), ("me@example", false), ("@example.com", false), ("me example@x.org", false), ("me@.com", false), (" me@example.org ", true)] {
            f.email = e.into();
            assert_eq!(f.email_valid(), ok, "{e}");
        }
    }

    #[test]
    fn autoconfigure_generic_and_providers() {
        let mut f = SetupForm::new(provider("IMAP Account"));
        f.email = "jane@example.org".into();
        f.autoconfigure();
        assert_eq!(f.cfg.incoming.host, "imap.example.org");
        assert_eq!((f.cfg.incoming.port, f.cfg.outgoing.port), (993, 587));

        let mut f = SetupForm::new(provider("POP Account"));
        f.email = "jane@example.org".into();
        f.autoconfigure();
        assert_eq!(f.cfg.kind, AccountKind::Pop3);
        assert_eq!((f.cfg.incoming.host.as_str(), f.cfg.incoming.port), ("pop.example.org", 995));

        // a custom domain hosted by a known provider uses the provider's servers
        let mut f = SetupForm::new(provider("Fastmail"));
        f.email = "jane@janedoe.net".into();
        f.autoconfigure();
        assert_eq!(f.cfg.incoming.host, "imap.fastmail.com");
        assert_eq!(f.cfg.incoming.username, "jane@janedoe.net");

        // addresses at domains the provider table knows keep their own lookup
        let mut f = SetupForm::new(provider("Gmail-compatible"));
        f.email = "jane@googlemail.com".into();
        f.autoconfigure();
        assert_eq!(f.cfg.incoming.host, "imap.gmail.com");

        let mut f = SetupForm::new(provider("Proton Bridge"));
        f.email = "jane@pm.me".into();
        f.autoconfigure();
        assert_eq!((f.cfg.incoming.host.as_str(), f.cfg.incoming.port), ("127.0.0.1", 1143));
        assert!(f.cfg.incoming.allow_insecure_certs);
    }

    #[test]
    fn form_to_config() {
        let mut f = SetupForm::new(provider("IMAP Account"));
        f.full_name = "  Jane Doe ".into();
        f.email = " jane@example.org ".into();
        f.password = "secret".into();
        f.autoconfigure();
        f.cfg.incoming.host = " imap.example.org ".into();
        let c = f.to_config();
        assert_eq!(c.full_name, "Jane Doe");
        assert_eq!(c.emails, vec!["jane@example.org".to_string()]);
        assert_eq!(c.name, "jane@example.org");
        assert_eq!(c.incoming.host, "imap.example.org");
        assert_eq!(c.outgoing.username, "jane@example.org");
        assert_eq!(f.secrets(&c.id), vec![(Secrets::incoming_key(&c.id), "secret".to_string())]);

        f.description = "Work".into();
        f.cfg.smtp_same_credentials = false;
        f.cfg.outgoing.username = String::new();
        f.out_password = "smtp-pw".into();
        let c = f.to_config();
        assert_eq!(c.name, "Work");
        assert_eq!(c.outgoing.username, "jane@example.org");
        assert_eq!(f.secrets(&c.id).len(), 2);
        assert_eq!(f.secrets(&c.id)[1], (Secrets::outgoing_key(&c.id), "smtp-pw".to_string()));
    }
}
