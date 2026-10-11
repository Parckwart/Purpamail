//! Background loading of remote images (only for messages whose remote
//! content the user allowed), with a size limit, a timeout and a shared cache.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use egui::Vec2;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

/// Largest image accepted.
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const WORKERS: usize = 4;

/// Loading state of a remote image.
#[derive(Clone, Debug)]
pub enum RemoteImage {
    Loading,
    Ready { uri: String, bytes: Arc<[u8]>, size: Option<Vec2> },
    Failed,
}

static CACHE: Lazy<Mutex<HashMap<String, RemoteImage>>> = Lazy::new(Default::default);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static QUEUE: Lazy<crossbeam_channel::Sender<(String, egui::Context)>> = Lazy::new(|| {
    let (tx, rx) = crossbeam_channel::unbounded::<(String, egui::Context)>();
    for i in 0..WORKERS {
        let rx = rx.clone();
        let _ = std::thread::Builder::new().name(format!("pm-images-{i}")).spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(20)))
                .max_redirects(5)
                .https_only(false)
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) Purpamail")
                .build()
                .into();
            for (url, ctx) in rx {
                let state = match fetch(&agent, &url) {
                    Some(bytes) => {
                        let size = super::tree::image_dims(&bytes);
                        let uri = super::tree::bytes_uri(&bytes);
                        RemoteImage::Ready { uri, bytes: Arc::from(bytes), size }
                    }
                    None => RemoteImage::Failed,
                };
                CACHE.lock().insert(url, state);
                GENERATION.fetch_add(1, Ordering::Relaxed);
                ctx.request_repaint();
            }
        });
    }
    tx
});

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let resp = agent.get(url).call().ok()?;
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if !ct.is_empty() && !ct.starts_with("image/") && !ct.starts_with("application/octet-stream") {
        return None;
    }
    resp.into_body().with_config().limit(MAX_BYTES).read_to_vec().ok()
}

/// Current state of `url`, starting a download when it was never requested.
pub fn get(url: &str, ctx: &egui::Context) -> RemoteImage {
    let mut cache = CACHE.lock();
    if let Some(s) = cache.get(url) {
        return s.clone();
    }
    cache.insert(url.to_string(), RemoteImage::Loading);
    drop(cache);
    let _ = QUEUE.send((url.to_string(), ctx.clone()));
    RemoteImage::Loading
}

/// State of `url` without starting a download.
pub fn peek(url: &str) -> Option<RemoteImage> {
    CACHE.lock().get(url).cloned()
}

/// Incremented whenever an image finished loading (layouts depending on
/// remote image sizes must be redone).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}
