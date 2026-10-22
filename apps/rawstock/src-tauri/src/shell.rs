//! File-open forwarding. Cold start on Windows uses argv. macOS uses
//! `RunEvent::Opened`. A second instance is forwarded by the single-instance
//! plugin. All of them become `open:requested` once the webview is listening,
//! or stay queued until `take_startup_opens`.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::contract::OpenRequested;
use crate::state::{AppState, lock};

static EARLY_OPENS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn forward_open(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        focus_main(app);
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        lock(&EARLY_OPENS).extend(paths);
        return;
    };
    if state.ui_ready.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = app.emit("open:requested", OpenRequested { paths });
    } else {
        lock(&state.pending_opens).extend(paths);
    }
    focus_main(app);
}

pub fn take_early_opens() -> Vec<String> {
    std::mem::take(&mut *lock(&EARLY_OPENS))
}

fn focus_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
