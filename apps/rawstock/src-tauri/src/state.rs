//! Process-wide shell state. The develop board is the contract stub.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use crate::jobs::ContractStub;
use crate::recent::RecentFiles;

pub struct AppState {
    pub stub: Mutex<ContractStub>,
    pub recent: Mutex<RecentFiles>,
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
}

impl AppState {
    pub fn new(recent: RecentFiles, log_dir: PathBuf) -> Self {
        Self {
            stub: Mutex::new(ContractStub::default()),
            recent: Mutex::new(recent),
            pending_opens: Mutex::new(Vec::new()),
            ui_ready: AtomicBool::new(false),
            shortcuts_suspended: AtomicBool::new(false),
            menu_ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            log_dir,
            menu_paths: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
        }
    }
}

pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

pub fn log_detail(dir: &std::path::Path, detail: &str) {
    let path = dir.join("rawstock.log");
    let _ = std::fs::create_dir_all(dir);
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => return,
    };
    use std::io::Write;
    let _ = writeln!(file, "{detail}");
}

pub fn wake(state: &AppState) {
    if let Some(sender) = lock(&state.wake).as_ref() {
        let _ = sender.send(());
    }
}
