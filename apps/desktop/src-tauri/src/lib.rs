//! Tauri shell for the stock tuner.
//!
//! The window and the file dialogs live here. Pixel work lives on the
//! develop thread in [`worker`]. `rawstock-core` stays free of both.

mod api;
mod catalog;
mod frame;
mod paths;
mod worker;

use tauri::ipc::Response;
use tauri::{Manager, State};

use crate::api::{
    DevelopRequest, SaveRequest, SessionState, StockState, StockSummary, TurnDirection,
};
use crate::catalog::StockCatalog;
use crate::worker::{Bootstrap, WorkerHandle};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let worker = WorkerHandle::spawn(app.handle().clone());
            if let Some(open) = Bootstrap::from_env() {
                worker.bootstrap(open);
            }
            app.manage(worker);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_stocks,
            session,
            open_raw,
            load_stock,
            render_frame,
            rotate_image,
            export_jpeg,
            save_stock,
        ])
        .run(tauri::generate_context!())
        .expect("rawstock window failed");
}

#[tauri::command]
fn list_stocks() -> Result<Vec<StockSummary>, String> {
    StockCatalog::list().map_err(|err| format!("{err:#}"))
}

#[tauri::command]
async fn session(worker: State<'_, WorkerHandle>) -> Result<Option<SessionState>, String> {
    worker.snapshot().await
}

#[tauri::command]
async fn open_raw(worker: State<'_, WorkerHandle>, path: String) -> Result<SessionState, String> {
    worker.open_raw(path.into()).await
}

#[tauri::command]
async fn load_stock(worker: State<'_, WorkerHandle>, path: String) -> Result<SessionState, String> {
    worker.load_stock(path.into()).await
}

#[tauri::command]
async fn render_frame(
    worker: State<'_, WorkerHandle>,
    request: DevelopRequest,
) -> Result<Response, String> {
    let bytes = worker.develop(request).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn rotate_image(
    worker: State<'_, WorkerHandle>,
    direction: TurnDirection,
) -> Result<SessionState, String> {
    worker.rotate(direction).await
}

#[tauri::command]
async fn export_jpeg(worker: State<'_, WorkerHandle>, path: String) -> Result<(), String> {
    worker.export(path.into()).await
}

#[tauri::command]
async fn save_stock(
    worker: State<'_, WorkerHandle>,
    request: SaveRequest,
) -> Result<StockState, String> {
    worker.save(request).await
}
