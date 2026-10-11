//! Purpamail entry point.

use purpamail::core::config::{Settings, data_dir};
use purpamail::core::store::Store;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().collect();
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
    let mailto = args.iter().skip(1).find(|a| a.starts_with("mailto:")).cloned();
    let icon = purpamail::ui::icon::app_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Purpamail")
            .with_app_id("org.purpamail.Purpamail")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(icon),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "Purpamail",
        options,
        Box::new(move |cc| {
            let mut app = purpamail::ui::app::PurpamailApp::new(cc, store, settings);
            if let Some(m) = mailto {
                app.actions.push(purpamail::ui::actions::Action::Compose(purpamail::ui::actions::ComposeInit::Mailto(m)));
            }
            Ok(Box::new(app))
        }),
    )
}
