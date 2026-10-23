//! Tauri shell for the rawstock 1.0 app.
//!
//! The window, menus, and command stubs live here. `rawstock-core` stays
//! free of windows and threads. Pixel work belongs to the develop service.

mod commands;
mod contract;
mod files;
mod frame;
mod help;
mod jobs;
mod menu;
mod onboarding;
mod recent;
mod settings;
mod shell;
mod sidecar;
mod state;
mod stocks;
mod thumbnails;
mod worker;

use std::sync::atomic::Ordering;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::files::paths_from_args;
use crate::state::{AppState, lock};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let paths = paths_from_args(&argv);
            shell::forward_open(app, paths);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?;
            std::fs::create_dir_all(&log_dir)?;
            state::log_info(
                &log_dir,
                &format!(
                    "rawstock {} ({}) started on {} {}",
                    env!("CARGO_PKG_VERSION"),
                    env!("RAWSTOCK_GIT_COMMIT"),
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            );
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let (settings, note) = settings::SettingsStore::load(config_dir.join("settings.json"));
            if let settings::LoadNote::Corrupt { reason, backup } = &note {
                state::log_detail(
                    &log_dir,
                    &format!(
                        "settings.json could not be read ({reason}); moved to {} and started on defaults",
                        backup.display()
                    ),
                );
            }
            let mut recent = recent::RecentFiles::load(config_dir.join("recent.json"));
            recent.set_limit(settings.get().recent.limit);
            recent.prune_missing();
            let _ = recent.save();
            app.manage(AppState::new(recent, settings, log_dir));
            let state = app.state::<AppState>();
            {
                let mut pending = lock(&state.pending_opens);
                pending.extend(shell::take_early_opens());
                #[cfg(not(target_os = "macos"))]
                pending.extend(paths_from_args(&std::env::args().collect::<Vec<_>>()));
            }
            menu::refresh_recent_menu(app.handle())?;
            worker::spawn(app.handle().clone());
            let thumbnailer = thumbnails::Thumbnailer::spawn(app.handle().clone());
            *lock(&state.thumbnailer) = Some(thumbnailer);
            Ok(())
        })
        .on_menu_event(|app, event| {
            menu::on_menu_event(app, event.id().as_ref());
        })
        .invoke_handler(commands::handlers())
        .build(tauri::generate_context!())
        .expect("rawstock window failed")
        .run(|app, event| match event {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            RunEvent::Opened { urls } => {
                let paths = urls
                    .into_iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>();
                shell::forward_open(app, paths);
            }
            RunEvent::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => {
                let _ = menu::refresh_recent_menu(app);
            }
            RunEvent::Exit => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.stopping.store(true, Ordering::Relaxed);
                    state::wake(&state);
                }
            }
            _ => {}
        });
}
