//! Native menus. Modifier shortcuts are menu accelerators (`CmdOrCtrl`),
//! so macOS shows Command and Windows shows Control. Unmodified keys are
//! handled in the webview, which can see whether a text field is focused.
//!
//! Muda does not report when a menu is about to open. Missing recent files
//! are pruned when the menu is built: at launch, when the window is focused,
//! and whenever the list changes.

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::contract::MenuAction;
use crate::state::{AppState, lock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuHit {
    Action(&'static str),
    Recent(usize),
    Ignore,
}

pub fn menu_hit(id: &str) -> MenuHit {
    match id {
        "open" => MenuHit::Action("open"),
        "open-folder" => MenuHit::Action("open-folder"),
        "export" => MenuHit::Action("export"),
        "close" => MenuHit::Action("close"),
        "undo" => MenuHit::Action("undo"),
        "redo" | "redo-y" => MenuHit::Action("redo"),
        "fit" => MenuHit::Action("fit"),
        "zoom-100" => MenuHit::Action("zoom-100"),
        "zoom-200" => MenuHit::Action("zoom-200"),
        "split" => MenuHit::Action("split"),
        "before-after" => MenuHit::Action("before-after"),
        "clipping" => MenuHit::Action("clipping"),
        "about" => MenuHit::Action("about"),
        "licenses" => MenuHit::Action("licenses"),
        "report-issue" => MenuHit::Action("report-issue"),
        "preferences" => MenuHit::Action("preferences"),
        "show-log" => MenuHit::Action("show-log"),
        "welcome" => MenuHit::Action("welcome"),
        // `?` in the webview. Routed here so suspended shortcuts stay off.
        "cheat-sheet" => MenuHit::Action("cheat-sheet"),
        "recent-empty" => MenuHit::Ignore,
        other => {
            if let Some(index) = other.strip_prefix("recent-")
                && let Ok(index) = index.parse()
            {
                MenuHit::Recent(index)
            } else {
                MenuHit::Ignore
            }
        }
    }
}

pub fn refresh_recent_menu(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let paths = {
        let mut recent = lock(&state.recent);
        if recent.prune_missing() > 0 {
            let _ = recent.save();
        }
        recent.list()
    };
    let ready = state
        .menu_ready
        .swap(true, std::sync::atomic::Ordering::Relaxed);
    let unchanged = {
        let current = lock(&state.menu_paths);
        ready && *current == paths
    };
    if unchanged {
        return Ok(());
    }
    *lock(&state.menu_paths) = paths.clone();
    install_menu(app, &paths)
}

pub fn on_menu_event(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    if state
        .shortcuts_suspended
        .load(std::sync::atomic::Ordering::Relaxed)
        && !matches!(menu_hit(id), MenuHit::Recent(_))
    {
        return;
    }
    match menu_hit(id) {
        MenuHit::Action(action) => {
            let _ = app.emit(
                "menu:action",
                MenuAction {
                    action: action.to_string(),
                },
            );
        }
        MenuHit::Recent(index) => {
            let path = lock(&state.menu_paths).get(index).cloned();
            if let Some(path) = path {
                crate::shell::forward_open(app, vec![path]);
            }
        }
        MenuHit::Ignore => {}
    }
}

fn install_menu(app: &AppHandle, paths: &[String]) -> tauri::Result<()> {
    let open = item(app, "open", "Open…", Some("CmdOrCtrl+O"))?;
    let open_folder = item(
        app,
        "open-folder",
        "Open Folder…",
        Some("CmdOrCtrl+Shift+O"),
    )?;
    let recent = recent_menu(app, paths)?;
    let export = item(app, "export", "Export…", Some("CmdOrCtrl+E"))?;
    let close = item(app, "close", "Close", None)?;
    let sep_a = PredefinedMenuItem::separator(app)?;
    let sep_b = PredefinedMenuItem::separator(app)?;
    #[cfg(not(target_os = "macos"))]
    let sep_c = PredefinedMenuItem::separator(app)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit"))?;

    let undo = item(app, "undo", "Undo", Some("CmdOrCtrl+Z"))?;
    let redo = item(app, "redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?;
    // macOS names it Settings in the app menu; Windows and Linux put
    // Preferences under Edit.
    #[cfg(target_os = "macos")]
    let preferences = item(app, "preferences", "Settings…", Some("CmdOrCtrl+,"))?;
    #[cfg(not(target_os = "macos"))]
    let preferences = item(app, "preferences", "Preferences…", Some("CmdOrCtrl+,"))?;
    let fit = item(app, "fit", "Fit", Some("CmdOrCtrl+0"))?;
    let zoom_100 = item(app, "zoom-100", "100%", Some("CmdOrCtrl+1"))?;
    let zoom_200 = item(app, "zoom-200", "200%", Some("CmdOrCtrl+2"))?;
    let split = item(app, "split", "Split", None)?;
    let before_after = item(app, "before-after", "Before/After", None)?;
    let clipping = item(app, "clipping", "Clipping", None)?;
    let sep_view = PredefinedMenuItem::separator(app)?;

    let welcome = item(app, "welcome", "Welcome…", None)?;
    let about = item(app, "about", "About", None)?;
    let licenses = item(app, "licenses", "Licenses", None)?;
    let report = item(app, "report-issue", "Report an issue", None)?;
    let show_log = item(app, "show-log", "Show Log File", None)?;
    let sep_help = PredefinedMenuItem::separator(app)?;

    let file = {
        #[cfg(target_os = "macos")]
        {
            Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &open,
                    &open_folder,
                    &recent,
                    &sep_a,
                    &export,
                    &sep_b,
                    &close,
                ],
            )?
        }
        #[cfg(not(target_os = "macos"))]
        {
            Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &open,
                    &open_folder,
                    &recent,
                    &sep_a,
                    &export,
                    &sep_b,
                    &close,
                    &sep_c,
                    &quit,
                ],
            )?
        }
    };
    #[cfg(target_os = "macos")]
    let edit = Submenu::with_items(app, "Edit", true, &[&undo, &redo])?;
    #[cfg(not(target_os = "macos"))]
    let edit = {
        let sep_edit = PredefinedMenuItem::separator(app)?;
        Submenu::with_items(app, "Edit", true, &[&undo, &redo, &sep_edit, &preferences])?
    };
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &fit,
            &zoom_100,
            &zoom_200,
            &sep_view,
            &split,
            &before_after,
            &clipping,
        ],
    )?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[&welcome, &about, &licenses, &sep_help, &report, &show_log],
    )?;

    #[cfg(target_os = "macos")]
    let menu = {
        let hide = PredefinedMenuItem::hide(app, None::<&str>)?;
        let hide_others = PredefinedMenuItem::hide_others(app, None::<&str>)?;
        let sep_app = PredefinedMenuItem::separator(app)?;
        let sep_settings = PredefinedMenuItem::separator(app)?;
        let app_menu = Submenu::with_items(
            app,
            "rawstock",
            true,
            &[
                &preferences,
                &sep_settings,
                &hide,
                &hide_others,
                &sep_app,
                &quit,
            ],
        )?;
        Menu::with_items(app, &[&app_menu, &file, &edit, &view, &help])?
    };
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::with_items(app, &[&file, &edit, &view, &help])?;

    app.set_menu(menu)?;
    Ok(())
}

fn recent_menu<R: Runtime>(app: &AppHandle<R>, paths: &[String]) -> tauri::Result<Submenu<R>> {
    if paths.is_empty() {
        let empty = item_r(app, "recent-empty", "No recent files", None)?;
        let _ = empty.set_enabled(false);
        return Submenu::with_id_and_items(app, "recent", "Open Recent", true, &[&empty]);
    }
    let mut items = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let label = recent_label(path, paths);
        items.push(item_r(app, &format!("recent-{index}"), &label, None)?);
    }
    let refs: Vec<&dyn IsMenuItem<R>> = items
        .iter()
        .map(|item| item as &dyn IsMenuItem<R>)
        .collect();
    Submenu::with_id_and_items(app, "recent", "Open Recent", true, &refs)
}

fn recent_label(path: &str, all: &[String]) -> String {
    let file = std::path::Path::new(path);
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    let duplicated = all
        .iter()
        .filter(|other| {
            std::path::Path::new(other)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(name)
        })
        .count()
        > 1;
    if duplicated
        && let Some(parent) = file.parent().and_then(|parent| parent.file_name())
        && let Some(parent) = parent.to_str()
    {
        format!("{name} ({parent})")
    } else {
        name.to_string()
    }
}

fn item(
    app: &AppHandle,
    id: &str,
    text: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    item_r(app, id, text, accelerator)
}

fn item_r<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    text: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(app, id, text, true, accelerator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(menu_hit("open"), MenuHit::Action("open"));
        assert_eq!(menu_hit("redo-y"), MenuHit::Action("redo"));
        assert_eq!(menu_hit("zoom-100"), MenuHit::Action("zoom-100"));
        assert_eq!(menu_hit("split"), MenuHit::Action("split"));
        assert_eq!(menu_hit("preferences"), MenuHit::Action("preferences"));
        assert_eq!(menu_hit("show-log"), MenuHit::Action("show-log"));
        assert_eq!(menu_hit("welcome"), MenuHit::Action("welcome"));
        assert_eq!(menu_hit("cheat-sheet"), MenuHit::Action("cheat-sheet"));
        assert_eq!(menu_hit("recent-2"), MenuHit::Recent(2));
        assert_eq!(menu_hit("recent-empty"), MenuHit::Ignore);
        assert_eq!(menu_hit("quit"), MenuHit::Ignore);
    }
}
