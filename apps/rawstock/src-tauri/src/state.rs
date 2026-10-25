//! Process-wide shell state. The develop board is the contract stub.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::contract::AsShotWhiteBalance;
use crate::develop::Developer;
use crate::jobs::ContractStub;
use crate::recent::RecentFiles;
use crate::settings::SettingsStore;
use crate::strip::Strip;
use crate::thumbnails::Thumbnailer;

pub struct AppState {
    pub stub: Mutex<ContractStub>,
    pub recent: Mutex<RecentFiles>,
    pub settings: Mutex<SettingsStore>,
    /// Paths that arrived before the webview was listening.
    pub pending_opens: Mutex<Vec<String>>,
    pub ui_ready: AtomicBool,
    pub shortcuts_suspended: AtomicBool,
    pub menu_ready: AtomicBool,
    pub stopping: AtomicBool,
    pub log_dir: PathBuf,
    /// Paths last painted into File > Open Recent, in menu order.
    pub menu_paths: Mutex<Vec<String>>,
    pub wake: Mutex<Option<mpsc::Sender<()>>>,
    /// Real stock thumbnails. Set once the app has started.
    pub thumbnailer: Mutex<Option<Thumbnailer>>,
    /// Filmstrip frames. Set once the app has started.
    pub strip: Mutex<Option<Strip>>,
    /// The real preview and full develop. Set once the app has started.
    pub developer: Mutex<Option<Developer>>,
    /// As-shot light per opened RAW, recorded when the develop thread
    /// decodes it. `None`: the file cannot take a custom white balance.
    pub as_shot: Mutex<HashMap<PathBuf, Option<AsShotWhiteBalance>>>,
}

impl AppState {
    pub fn new(recent: RecentFiles, settings: SettingsStore, log_dir: PathBuf) -> Self {
        Self {
            stub: Mutex::new(ContractStub::default()),
            recent: Mutex::new(recent),
            settings: Mutex::new(settings),
            pending_opens: Mutex::new(Vec::new()),
            ui_ready: AtomicBool::new(false),
            shortcuts_suspended: AtomicBool::new(false),
            menu_ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log_dir,
            menu_paths: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            thumbnailer: Mutex::new(None),
            strip: Mutex::new(None),
            developer: Mutex::new(None),
            as_shot: Mutex::new(HashMap::new()),
        }
    }
}

pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// Detail behind a user-facing error. The message the user saw is
/// friendlier; this is what goes in the log they can attach to a report.
pub fn log_detail(dir: &std::path::Path, detail: &str) {
    crate::help::Log::write(dir, crate::help::Level::Error, detail);
}

pub fn log_info(dir: &std::path::Path, detail: &str) {
    crate::help::Log::write(dir, crate::help::Level::Info, detail);
}

pub fn wake(state: &AppState) {
    if let Some(sender) = lock(&state.wake).as_ref() {
        let _ = sender.send(());
    }
}
