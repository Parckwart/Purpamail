//! Background sync engine (stub — to be replaced by the full implementation).

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::RwLock;

use super::{Command, Event};
use crate::core::config::{Secrets, Settings};
use crate::core::store::SharedStore;

pub type SharedSettings = Arc<RwLock<Settings>>;
pub type SharedSecrets = Arc<RwLock<Secrets>>;
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Handle held by the UI.
#[derive(Clone)]
pub struct EngineHandle {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

pub struct Engine;

impl Engine {
    pub fn start(store: SharedStore, settings: SharedSettings, secrets: SharedSecrets, repaint: Repaint) -> EngineHandle {
        let (ctx, crx) = unbounded::<Command>();
        let (etx, erx) = unbounded::<Event>();
        let _ = (store, settings, secrets);
        std::thread::Builder::new()
            .name("purpamail-engine".into())
            .spawn(move || {
                while let Ok(cmd) = crx.recv() {
                    if let Command::Shutdown = cmd {
                        break;
                    }
                    let _ = etx.send(Event::Log(format!("{cmd:?}")));
                    repaint();
                }
            })
            .ok();
        EngineHandle { tx: ctx, rx: erx }
    }
}
