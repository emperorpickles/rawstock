//! Tauri commands. Develop commands return a job id and emit the contract
//! events. Previews and full frames run in [`crate::develop`], stock
//! thumbnails in [`crate::thumbnails`]. Export is still the stub.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};

use rawstock_core::decode::RawImage;
use rawstock_core::white_balance::WhiteBalanceSolver;

use crate::contract::{
    AppError, AsShotWhiteBalance, EditSettings, ExportOptions, JobKind, JobStarted, OpenOutcome,
    SaveResult, StockSummary,
};
use crate::develop::{DevelopRequest, ExportTarget};
use crate::files::classify_inputs;
use crate::menu::{self, on_menu_event};
use crate::settings::{AppSettings, SettingsChanged};
use crate::shell;
use crate::state::{self, AppState, lock};
use crate::stocks;
use crate::thumbnails::ThumbnailRequest;
use crate::worker;

#[tauri::command]
fn open_paths(app: AppHandle, paths: Vec<String>) -> Result<OpenOutcome, AppError> {
    let (outcome, logs) = classify_inputs(&paths);
    let app_state = app.state::<AppState>();
    for line in &logs {
        state::log_detail(&app_state.log_dir, &line.detail);
    }
    {
        let mut stub = lock(&app_state.stub);
        for image in &outcome.images {
            stub.insert_image(image.clone());
        }
    }
    if outcome.images.iter().any(|image| image.error.is_none()) {
        let mut recent = lock(&app_state.recent);
        for image in &outcome.images {
            if image.error.is_none() {
                recent.remember(std::path::Path::new(&image.path));
            }
        }
        let _ = recent.save();
    }
    if let Err(error) = menu::refresh_recent_menu(&app) {
        state::log_detail(&app_state.log_dir, &error.to_string());
    }
    Ok(outcome)
}

#[tauri::command]
fn select_image(app: AppHandle, image_id: String) -> Result<crate::contract::ImageInfo, AppError> {
    let (info, events) = {
        let app_state = app.state::<AppState>();
        let mut stub = lock(&app_state.stub);
        stub.select_image(&image_id)?
    };
    worker::emit(&app, &events);
    let held = app.state::<AppState>();
    state::wake(&held);
    Ok(info)
}

#[tauri::command]
fn list_stocks(app: AppHandle) -> Result<Vec<StockSummary>, AppError> {
    stocks::list().inspect_err(|error| {
        state::log_detail(
            &app.state::<AppState>().log_dir,
            &format!("{} {}", error.code_name(), stocks::directory().display()),
        );
    })
}

#[tauri::command]
fn develop_preview(
    app: AppHandle,
    image_id: String,
    settings: EditSettings,
    long_edge: u32,
    compare: bool,
) -> Result<JobStarted, AppError> {
    let started = dispatch(&app, |stub| {
        stub.develop_preview(&image_id, &settings, long_edge, compare)
    })?;
    log_develop(&app, "develop_preview", started.job_id, &image_id);
    submit_develop(
        &app,
        &image_id,
        started.job_id,
        JobKind::Preview,
        settings,
        long_edge,
    );
    Ok(started)
}

#[tauri::command]
fn develop_full(
    app: AppHandle,
    image_id: String,
    settings: EditSettings,
) -> Result<JobStarted, AppError> {
    let started = dispatch(&app, |stub| stub.develop_full(&image_id, &settings))?;
    log_develop(&app, "develop_full", started.job_id, &image_id);
    submit_develop(&app, &image_id, started.job_id, JobKind::Full, settings, 0);
    Ok(started)
}

/// Hand an accepted job to the real develop thread.
fn submit_develop(
    app: &AppHandle,
    image_id: &str,
    job_id: u64,
    kind: JobKind,
    settings: EditSettings,
    long_edge: u32,
) {
    let state = app.state::<AppState>();
    let Some(path) = lock(&state.stub).image_path(image_id) else {
        return;
    };
    if let Some(developer) = lock(&state.developer).as_ref() {
        developer.submit(DevelopRequest {
            job_id,
            kind,
            image_id: image_id.to_string(),
            path: PathBuf::from(path),
            settings,
            long_edge,
            export: None,
        });
    }
}

/// One log line per develop job, so a pan can be checked for zero develops.
fn log_develop(app: &AppHandle, command: &str, job_id: u64, image_id: &str) {
    state::log_info(
        &app.state::<AppState>().log_dir,
        &format!("{command} job {job_id} image {image_id}"),
    );
}

#[tauri::command]
fn get_region(
    app: AppHandle,
    image_id: String,
    settings_hash: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<Response, AppError> {
    let bytes = lock(&app.state::<AppState>().stub).get_region(
        &image_id,
        &settings_hash,
        x,
        y,
        width,
        height,
    )?;
    Ok(Response::new(bytes))
}

#[tauri::command]
fn render_thumbnails(
    app: AppHandle,
    image_id: String,
    settings: EditSettings,
    stock_ids: Vec<String>,
    long_edge: u32,
) -> Result<JobStarted, AppError> {
    let path = lock(&app.state::<AppState>().stub).image_path(&image_id);
    let started = dispatch(&app, |stub| {
        stub.render_thumbnails(&image_id, &settings, stock_ids.clone(), long_edge)
    })?;
    if let (Some(path), Some(thumbnailer)) =
        (path, lock(&app.state::<AppState>().thumbnailer).as_ref())
    {
        thumbnailer.submit(ThumbnailRequest {
            job_id: started.job_id,
            path: PathBuf::from(path),
            settings,
            stock_ids,
            long_edge,
        });
    }
    Ok(started)
}

/// The photo's as-shot light, for the custom white balance sliders.
/// `null` when the file cannot take a custom balance. Read from the develop
/// thread's decode when it has opened the photo, else decoded here.
#[tauri::command]
async fn as_shot_white_balance(
    app: AppHandle,
    image_id: String,
) -> Result<Option<AsShotWhiteBalance>, AppError> {
    let path = lock(&app.state::<AppState>().stub)
        .image_path(&image_id)
        .map(PathBuf::from)
        .ok_or_else(|| AppError::internal("This photo is no longer open."))?;
    if let Some(known) = lock(&app.state::<AppState>().as_shot).get(&path) {
        return Ok(*known);
    }
    let read = path.clone();
    let as_shot = tauri::async_runtime::spawn_blocking(move || {
        RawImage::open(&read).ok().and_then(|raw| {
            WhiteBalanceSolver::new(raw.camera_matrix(), raw.as_shot_wb())
                .and_then(|solver| solver.as_shot())
                .map(AsShotWhiteBalance::from)
        })
    })
    .await
    .map_err(|_| AppError::internal("The white balance of this photo could not be read."))?;
    lock(&app.state::<AppState>().as_shot).insert(path, as_shot);
    Ok(as_shot)
}

#[tauri::command]
fn take_frame(app: AppHandle, job_id: u64, stock_id: Option<String>) -> Result<Response, AppError> {
    let bytes = lock(&app.state::<AppState>().stub).take_frame(job_id, stock_id.as_deref())?;
    Ok(Response::new(bytes))
}

#[tauri::command]
fn cancel(app: AppHandle, job_id: u64) -> Cancelled {
    let (cancelled, events) = lock(&app.state::<AppState>().stub).cancel(job_id);
    worker::emit(&app, &events);
    let held = app.state::<AppState>();
    state::wake(&held);
    Cancelled { cancelled }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Cancelled {
    cancelled: bool,
}

#[tauri::command]
fn export_image(
    app: AppHandle,
    image_id: String,
    settings: EditSettings,
    options: ExportOptions,
) -> Result<JobStarted, AppError> {
    let started = dispatch(&app, |stub| {
        stub.export_image(&image_id, &settings, &options)
    })?;
    let state = app.state::<AppState>();
    let (path, destination) = {
        let stub = lock(&state.stub);
        (stub.image_path(&image_id), stub.output_path(started.job_id))
    };
    if let (Some(path), Some(destination), Some(developer)) =
        (path, destination, lock(&state.developer).as_ref())
    {
        developer.submit(DevelopRequest {
            job_id: started.job_id,
            kind: JobKind::Export,
            image_id: image_id.clone(),
            path: PathBuf::from(path),
            settings,
            long_edge: 0,
            export: Some(ExportTarget {
                destination: PathBuf::from(destination),
                quality: u8::try_from(options.quality).unwrap_or(92),
            }),
        });
    }
    Ok(started)
}

#[tauri::command]
fn save_sidecar(
    app: AppHandle,
    image_id: String,
    settings: EditSettings,
) -> Result<SaveResult, AppError> {
    let state = app.state::<AppState>();
    match lock(&state.stub).save_sidecar(&image_id, &settings) {
        Ok(saved) => Ok(saved),
        Err(failure) => {
            state::log_detail(&state.log_dir, &failure.detail);
            Err(failure.error)
        }
    }
}

#[tauri::command]
fn reset_sidecar(app: AppHandle, image_id: String) -> Result<crate::contract::ImageInfo, AppError> {
    let state = app.state::<AppState>();
    match lock(&state.stub).reset_sidecar(&image_id) {
        Ok(info) => {
            state::log_info(
                &state.log_dir,
                &format!("renamed the edit file beside {} to .bak", info.file_name),
            );
            Ok(info)
        }
        Err(failure) => {
            state::log_detail(&state.log_dir, &failure.detail);
            Err(failure.error)
        }
    }
}

#[tauri::command]
fn recent_files(app: AppHandle) -> Vec<String> {
    let _ = menu::refresh_recent_menu(&app);
    lock(&app.state::<AppState>().recent).list()
}

#[tauri::command]
fn clear_recent(app: AppHandle) -> Result<(), AppError> {
    {
        let state = app.state::<AppState>();
        let mut recent = lock(&state.recent);
        recent.clear();
        let _ = recent.save();
    }
    let _ = menu::refresh_recent_menu(&app);
    Ok(())
}

#[tauri::command]
fn get_app_settings(state: State<'_, AppState>) -> AppSettings {
    lock(&state.settings).get().clone()
}

#[tauri::command]
fn set_app_settings(app: AppHandle, settings: AppSettings) -> Result<AppSettings, AppError> {
    let saved = {
        let state = app.state::<AppState>();
        let mut store = lock(&state.settings);
        store.set(settings).inspect_err(|error| {
            state::log_detail(&state.log_dir, &format!("set_app_settings {error}"));
        })?;
        store.get().clone()
    };
    apply_settings(&app, &saved, false);
    Ok(saved)
}

/// Restore every default. Photos, sidecars, and exports are never touched.
/// Recent files are cleared only when the user asked for that too.
#[tauri::command]
fn reset_app_settings(app: AppHandle, clear_recent: bool) -> Result<AppSettings, AppError> {
    let saved = {
        let state = app.state::<AppState>();
        let mut store = lock(&state.settings);
        store.reset().inspect_err(|error| {
            state::log_detail(&state.log_dir, &format!("reset_app_settings {error}"));
        })?;
        store.get().clone()
    };
    apply_settings(&app, &saved, clear_recent);
    Ok(saved)
}

fn apply_settings(app: &AppHandle, settings: &AppSettings, clear_recent: bool) {
    {
        let state = app.state::<AppState>();
        let mut recent = lock(&state.recent);
        recent.set_limit(settings.recent.limit);
        if clear_recent {
            recent.clear();
        }
        let _ = recent.save();
    }
    let _ = menu::refresh_recent_menu(app);
    let _ = app.emit(
        "settings:changed",
        SettingsChanged {
            settings: settings.clone(),
        },
    );
}

/// Path of the bundled onboarding sample, or `None` when the build has none.
#[tauri::command]
fn onboarding_sample(app: AppHandle) -> Option<String> {
    let dir = app.path().resource_dir().ok()?;
    crate::onboarding::OnboardingSample::find(&dir).map(|path| path.display().to_string())
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> crate::help::AppInfo {
    crate::help::AppInfo::current(&state.log_dir)
}

#[tauri::command]
fn dispatch_menu_action(app: AppHandle, action: String) -> bool {
    let state = app.state::<AppState>();
    if state.shortcuts_suspended.load(Ordering::Relaxed) {
        return false;
    }
    on_menu_event(&app, &action);
    true
}

#[tauri::command]
fn set_shortcuts_suspended(state: State<'_, AppState>, suspended: bool) {
    state
        .shortcuts_suspended
        .store(suspended, Ordering::Relaxed);
}

#[tauri::command]
fn request_open(app: AppHandle, paths: Vec<String>) {
    shell::forward_open(&app, paths);
}

#[tauri::command]
fn take_startup_opens(app: AppHandle) -> Vec<String> {
    let state = app.state::<AppState>();
    state.ui_ready.store(true, Ordering::Relaxed);
    std::mem::take(&mut *lock(&state.pending_opens))
}

fn dispatch(
    app: &AppHandle,
    run: impl FnOnce(
        &mut crate::jobs::ContractStub,
    ) -> Result<(JobStarted, Vec<crate::contract::ContractEvent>), AppError>,
) -> Result<JobStarted, AppError> {
    let (started, events) = {
        let state = app.state::<AppState>();
        let mut stub = lock(&state.stub);
        run(&mut stub)?
    };
    worker::emit(app, &events);
    let held = app.state::<AppState>();
    state::wake(&held);
    Ok(started)
}

pub fn handlers() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        open_paths,
        select_image,
        list_stocks,
        develop_preview,
        develop_full,
        get_region,
        render_thumbnails,
        as_shot_white_balance,
        take_frame,
        cancel,
        export_image,
        save_sidecar,
        reset_sidecar,
        recent_files,
        clear_recent,
        get_app_settings,
        set_app_settings,
        reset_app_settings,
        app_info,
        onboarding_sample,
        dispatch_menu_action,
        set_shortcuts_suspended,
        request_open,
        take_startup_opens,
    ]
}

trait ErrorName {
    fn code_name(&self) -> &'static str;
}

impl ErrorName for AppError {
    fn code_name(&self) -> &'static str {
        match self.code {
            crate::contract::ErrorCode::UnsupportedCamera => "unsupportedCamera",
            crate::contract::ErrorCode::CorruptFile => "corruptFile",
            crate::contract::ErrorCode::UnsupportedFormat => "unsupportedFormat",
            crate::contract::ErrorCode::Io => "io",
            crate::contract::ErrorCode::SidecarInvalid => "sidecarInvalid",
            crate::contract::ErrorCode::StockMissing => "stockMissing",
            crate::contract::ErrorCode::CacheMiss => "cacheMiss",
            crate::contract::ErrorCode::Internal => "internal",
        }
    }
}
