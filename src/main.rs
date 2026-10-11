//! Purpamail entry point.

use purpamail::core::config::{Settings, data_dir};
use purpamail::core::store::Store;

const HELP: &str = "Purpamail — a full-featured mail client

Usage: purpamail [OPTIONS] [mailto:URL | FILE.eml ...]

Options:
  --demo        Fill \"On This Computer\" with sample mail to explore Purpamail
  --offline     Start with all accounts offline
  --version     Print the version and exit
  --help        Show this help

Environment:
  PURPAMAIL_CONFIG_DIR   Override the configuration directory
  PURPAMAIL_DATA_DIR     Override the mail store directory
";

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Purpamail {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let requests = purpamail::ipc::requests_from_args(&args);
    if purpamail::ipc::forward_to_running(&requests) {
        return Ok(());
    }
    let store = match Store::open(&data_dir().join("mail.db")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Purpamail could not open its mail store: {e:#}");
            std::process::exit(1);
        }
    };
    let mut settings = Settings::load();
    if args.iter().any(|a| a == "--demo") {
        if let Err(e) = purpamail::core::demo::populate(&store) {
            eprintln!("could not create sample data: {e:#}");
        }
        settings.onboarded = true;
    }
    if args.iter().any(|a| a == "--offline") {
        settings.work_offline = true;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Purpamail")
            .with_app_id("org.purpamail.Purpamail")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(purpamail::ui::icon::app_icon()),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "Purpamail",
        options,
        Box::new(move |cc| {
            let mut app = purpamail::ui::app::PurpamailApp::new(cc, store, settings);
            for r in requests {
                app.handle_request(&r);
            }
            Ok(Box::new(app))
        }),
    )
}
