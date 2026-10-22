//! The one develop thread.
//!
//! The UI thread sends jobs and waits on a reply channel. It never touches
//! the session. Each preview, export, open, and stock load takes an epoch.
//! The epoch is bumped when the job is submitted, and the in-flight develop
//! sees [`DevelopControl::cancelled`] as soon as a newer epoch exists. A
//! cancelled job applies its edit (so a dragged slider is not dropped) and
//! then returns `cancelled` without an image and without writing a file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rawstock_core::decode::{Orientation, RawImage};
use rawstock_core::gpu::Gpu;
use rawstock_core::render::{Cancelled, DevelopControl, Stage};
use rawstock_core::session::{DRAG_LONG_EDGE, DisplayRect, PreviewSession, ScanMode};
use rawstock_core::stock::{Stock, StockMeta};
use tauri::{AppHandle, Emitter};

use crate::api::{
    ControlEdit, DevelopRequest, ProgressEvent, RenderView, SaveRequest, SessionState, StockState,
    TurnDirection,
};
use crate::catalog::StockCatalog;
use crate::frame::FrameBytes;
use crate::paths::DevelopPaths;

const STOPPED: &str = "The develop worker has stopped.";

type Reply<T> = Sender<Result<T, String>>;

enum Job {
    Bootstrap {
        raw: PathBuf,
        stock: Option<PathBuf>,
    },
    Snapshot {
        reply: Reply<Option<SessionState>>,
    },
    Open {
        path: PathBuf,
        epoch: u64,
        reply: Reply<SessionState>,
    },
    LoadStock {
        path: PathBuf,
        epoch: u64,
        reply: Reply<SessionState>,
    },
    Develop {
        epoch: u64,
        request: DevelopRequest,
        reply: Reply<Vec<u8>>,
    },
    Export {
        path: PathBuf,
        epoch: u64,
        reply: Reply<()>,
    },
    Rotate {
        direction: TurnDirection,
        reply: Reply<SessionState>,
    },
    Save {
        request: SaveRequest,
        reply: Reply<StockState>,
    },
}

/// Handle the UI holds. Cloning it does not clone the session.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<Job>,
    latest: Arc<AtomicU64>,
}

/// A file to open before the window is interactive. `RAWSTOCK_OPEN` is the
/// RAW path. `RAWSTOCK_STOCK` is optional and may be a file name in `stocks/`.
#[derive(Clone)]
pub struct Bootstrap {
    raw: PathBuf,
    stock: Option<PathBuf>,
}

impl Bootstrap {
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var_os("RAWSTOCK_OPEN")?;
        let stock = std::env::var_os("RAWSTOCK_STOCK")
            .map(PathBuf::from)
            .map(StockCatalog::resolve);
        Some(Self {
            raw: PathBuf::from(raw),
            stock,
        })
    }
}

impl WorkerHandle {
    pub fn spawn(app: AppHandle) -> Self {
        let latest = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let shared = Arc::clone(&latest);
        thread::Builder::new()
            .name("rawstock-develop".to_string())
            .spawn(move || Worker::new(app, shared).run(rx))
            .expect("start the develop thread");
        Self { tx, latest }
    }

    pub fn bootstrap(&self, open: Bootstrap) {
        let _ = self.bump();
        let _ = self.tx.send(Job::Bootstrap {
            raw: open.raw,
            stock: open.stock,
        });
    }

    pub async fn snapshot(&self) -> Result<Option<SessionState>, String> {
        let (tx, rx) = mpsc::channel();
        self.send(Job::Snapshot { reply: tx })?;
        wait(rx).await
    }

    pub async fn open_raw(&self, path: PathBuf) -> Result<SessionState, String> {
        let epoch = self.bump();
        let (tx, rx) = mpsc::channel();
        self.send(Job::Open {
            path,
            epoch,
            reply: tx,
        })?;
        wait(rx).await
    }

    pub async fn load_stock(&self, path: PathBuf) -> Result<SessionState, String> {
        let epoch = self.bump();
        let (tx, rx) = mpsc::channel();
        self.send(Job::LoadStock {
            path,
            epoch,
            reply: tx,
        })?;
        wait(rx).await
    }

    pub async fn develop(&self, request: DevelopRequest) -> Result<Vec<u8>, String> {
        let epoch = self.bump();
        let (tx, rx) = mpsc::channel();
        self.send(Job::Develop {
            epoch,
            request,
            reply: tx,
        })?;
        wait(rx).await
    }

    pub async fn rotate(&self, direction: TurnDirection) -> Result<SessionState, String> {
        self.bump();
        let (tx, rx) = mpsc::channel();
        self.send(Job::Rotate {
            direction,
            reply: tx,
        })?;
        wait(rx).await
    }

    pub async fn export(&self, path: PathBuf) -> Result<(), String> {
        let epoch = self.bump();
        let (tx, rx) = mpsc::channel();
        self.send(Job::Export {
            path,
            epoch,
            reply: tx,
        })?;
        wait(rx).await
    }

    pub async fn save(&self, request: SaveRequest) -> Result<StockState, String> {
        let (tx, rx) = mpsc::channel();
        self.send(Job::Save { request, reply: tx })?;
        wait(rx).await
    }

    fn bump(&self) -> u64 {
        self.latest.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn send(&self, job: Job) -> Result<(), String> {
        self.tx.send(job).map_err(|_| STOPPED.to_string())
    }
}

async fn wait<T: Send + 'static>(rx: Receiver<Result<T, String>>) -> Result<T, String> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        rx.recv().unwrap_or_else(|_| Err(STOPPED.to_string()))
    })
    .await;
    joined.unwrap_or_else(|_| Err(STOPPED.to_string()))
}

struct Worker {
    app: AppHandle,
    latest: Arc<AtomicU64>,
    studio: Studio,
}

impl Worker {
    fn new(app: AppHandle, latest: Arc<AtomicU64>) -> Self {
        Self {
            app,
            latest,
            studio: Studio::default(),
        }
    }

    fn run(mut self, rx: Receiver<Job>) {
        while let Ok(job) = rx.recv() {
            self.dispatch(job);
        }
    }

    fn dispatch(&mut self, job: Job) {
        match job {
            Job::Bootstrap { raw, stock } => {
                let mut progress = self.control(self.latest.load(Ordering::Relaxed));
                let result = self.studio.bootstrap(&raw, stock.as_deref(), &mut progress);
                self.publish(result);
            }
            Job::Snapshot { reply } => {
                let _ = reply.send(Ok(self.studio.snapshot()));
            }
            Job::Open { path, epoch, reply } => {
                let mut progress = self.control(epoch);
                finish(self.studio.open_raw(&path, &mut progress), reply);
            }
            Job::LoadStock { path, epoch, reply } => {
                let mut progress = self.control(epoch);
                finish(self.studio.load_stock(&path, &mut progress), reply);
            }
            Job::Develop {
                epoch,
                request,
                reply,
            } => {
                let mut progress = self.control(epoch);
                finish(self.studio.develop(request, &mut progress), reply);
            }
            Job::Export { path, epoch, reply } => {
                let mut progress = self.control(epoch);
                finish(self.studio.export(path, &mut progress), reply);
            }
            Job::Rotate { direction, reply } => finish(self.studio.rotate(direction), reply),
            Job::Save { request, reply } => finish(self.studio.save(request), reply),
        }
    }

    fn control(&self, epoch: u64) -> JobControl {
        JobControl {
            latest: Arc::clone(&self.latest),
            running: epoch,
            app: self.app.clone(),
            last_emit: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            last_stage: String::new(),
        }
    }

    fn publish(&self, result: Result<()>) {
        match result {
            Ok(()) => {
                let _ = self.app.emit("session-changed", ());
            }
            Err(err) => {
                let message = stopped(err);
                if message != "cancelled" {
                    eprintln!("rawstock: {message}");
                    let _ = self.app.emit("session-error", message);
                }
            }
        }
    }
}

fn finish<T>(result: Result<T>, reply: Reply<T>) {
    let result = result.map_err(stopped);
    if let Err(ref message) = result
        && message != "cancelled"
    {
        eprintln!("rawstock: {message}");
    }
    let _ = reply.send(result);
}

fn stopped(err: anyhow::Error) -> String {
    if err.is::<Cancelled>() {
        "cancelled".to_string()
    } else {
        format!("{err:#}")
    }
}

struct JobControl {
    latest: Arc<AtomicU64>,
    running: u64,
    app: AppHandle,
    last_emit: Instant,
    last_stage: String,
}

impl JobControl {
    fn emit_progress(&mut self, stage: &str, done_rows: u32, rows: u32) {
        let now = Instant::now();
        let stage_changed = self.last_stage != stage;
        let finished = rows > 0 && done_rows == rows;
        let due = now.duration_since(self.last_emit) >= Duration::from_millis(80);
        if !stage_changed && !finished && !due {
            return;
        }
        self.last_stage = stage.to_string();
        self.last_emit = now;
        let _ = self.app.emit(
            "develop-progress",
            ProgressEvent {
                stage: stage.to_string(),
                done_rows,
                rows,
            },
        );
    }
}

impl DevelopControl for JobControl {
    fn cancelled(&self) -> bool {
        self.latest.load(Ordering::Relaxed) != self.running
    }

    fn on_progress(&mut self, stage: Stage, done_rows: u32, rows: u32) {
        self.emit_progress(stage.name(), done_rows, rows);
    }
}

#[derive(Clone)]
struct LoadedFile {
    path: PathBuf,
    revision: u32,
    name: String,
    source: String,
    license: String,
    model: rawstock_core::stock::StockModel,
}

impl LoadedFile {
    fn from_stock(path: &Path, stock: &Stock) -> Self {
        Self {
            path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            revision: stock.revision(),
            name: stock.name().to_string(),
            source: stock.source().to_string(),
            license: stock.license().to_string(),
            model: stock.model(),
        }
    }
}

#[derive(Default)]
struct Studio {
    raw_path: Option<PathBuf>,
    stock: Option<LoadedFile>,
    session: Option<PreviewSession>,
    renderer: Renderer,
}

/// The GPU, found once on the first open. `RAWSTOCK_GPU=0` keeps every
/// render on the CPU.
#[derive(Default)]
enum Renderer {
    #[default]
    Unknown,
    Gpu(Gpu),
    Cpu,
}

impl Renderer {
    fn device(&mut self) -> Option<Gpu> {
        if let Self::Unknown = self {
            *self = if std::env::var("RAWSTOCK_GPU").as_deref() == Ok("0") {
                eprintln!("rawstock: RAWSTOCK_GPU=0, rendering on the CPU");
                Self::Cpu
            } else {
                match Gpu::new_blocking() {
                    Ok(gpu) => {
                        eprintln!("rawstock: rendering on {}", gpu.adapter_name());
                        Self::Gpu(gpu)
                    }
                    Err(error) => {
                        eprintln!("rawstock: {error}; rendering on the CPU");
                        Self::Cpu
                    }
                }
            };
        }
        match self {
            Self::Gpu(gpu) => Some(gpu.clone()),
            _ => None,
        }
    }
}

impl Studio {
    fn bootstrap(
        &mut self,
        raw: &Path,
        stock: Option<&Path>,
        progress: &mut JobControl,
    ) -> Result<()> {
        self.open_raw(raw, progress)?;
        if let Some(stock) = stock {
            self.load_stock(stock, progress)?;
        }
        Ok(())
    }

    fn snapshot(&self) -> Option<SessionState> {
        self.session
            .as_ref()
            .map(|session| self.session_state(session))
    }

    fn open_raw(&mut self, path: &Path, progress: &mut JobControl) -> Result<SessionState> {
        if !DevelopPaths::is_raw(path) {
            anyhow::bail!("Open a Fuji RAF or an iPhone ProRAW DNG.");
        }
        progress.emit_progress("Opening", 0, 1);
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        let raw = RawImage::open(path).with_context(|| format!("open {}", path.display()))?;
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        let orientation = raw.orientation();
        let linear = raw
            .develop_linear()
            .with_context(|| format!("develop {}", path.display()))?;
        drop(raw);
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        let previous = self.stock.clone();
        let mut session = PreviewSession::new(linear, orientation, DRAG_LONG_EDGE)?;
        session.set_gpu(self.renderer.device());
        self.session = Some(session);
        self.raw_path = Some(path.to_path_buf());
        self.stock = None;
        progress.emit_progress("Opening", 1, 1);
        if let Some(previous) = previous {
            self.load_stock(&previous.path, progress)?;
        }
        self.ready_state()
    }

    fn load_stock(&mut self, path: &Path, progress: &mut JobControl) -> Result<SessionState> {
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        let path = StockCatalog::resolve(path.to_path_buf());
        if !DevelopPaths::is_stock(&path) {
            anyhow::bail!("Open a .stock.toml file.");
        }
        let stock = Stock::load(&path).with_context(|| format!("load {}", path.display()))?;
        {
            let session = self.session.as_mut().context("Open a RAW file first.")?;
            session.set_parameters(stock.parameters().clone());
        }
        self.stock = Some(LoadedFile::from_stock(&path, &stock));
        self.ready_state()
    }

    fn develop(&mut self, request: DevelopRequest, progress: &mut JobControl) -> Result<Vec<u8>> {
        if let Some(edit) = request.edit {
            self.apply_edit(edit)?;
        }
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        let image = self.render_view(request.view, request.compare, progress)?;
        Ok(FrameBytes::from_image(request.ticket, &image))
    }

    fn export(&mut self, path: PathBuf, progress: &mut JobControl) -> Result<()> {
        let path = DevelopPaths::ensure_jpeg(path);
        let session = self.require_ready()?;
        let image = session.render_full(progress)?;
        if progress.cancelled() {
            return Err(Cancelled.into());
        }
        // The session image is already oriented. Tag 1 keeps the pixels upright.
        image
            .write(&path, Orientation::Normal)
            .with_context(|| format!("export {}", path.display()))?;
        Ok(())
    }

    fn rotate(&mut self, direction: TurnDirection) -> Result<SessionState> {
        let session = self.session.as_mut().context("Open a RAW file first.")?;
        match direction {
            TurnDirection::Clockwise => session.rotate_clockwise()?,
            TurnDirection::CounterClockwise => session.rotate_counterclockwise()?,
        }
        self.ready_state()
    }

    fn save(&mut self, request: SaveRequest) -> Result<StockState> {
        let path = DevelopPaths::ensure_stock(PathBuf::from(&request.path));
        if StockCatalog::is_bundled(&path) {
            anyhow::bail!("Save as a new file. Bundled stocks stay as they shipped.");
        }
        let model = self.stock.as_ref().context("Pick a stock.")?.model;
        let meta = StockMeta::new(
            request.revision,
            request.name.trim(),
            request.source.trim(),
            request.license.trim(),
            model,
        )?;
        let session = self.session.as_mut().context("Open a RAW file first.")?;
        let written = Stock::from_parts(meta, session.parameters().clone())?;
        written
            .write(&path)
            .with_context(|| format!("save {}", path.display()))?;
        session.commit_edit();
        let loaded = LoadedFile::from_stock(&path, &written);
        let state = describe(&loaded, session);
        self.stock = Some(loaded);
        Ok(state)
    }

    fn apply_edit(&mut self, edit: ControlEdit) -> Result<()> {
        let session = self.require_ready()?;
        match edit {
            ControlEdit::Exposure(value) => {
                session.set_exposure_ev(value)?;
            }
            ControlEdit::PrintExposure(value) => {
                session.set_print_exposure(value)?;
            }
            ControlEdit::DyeScales(scales) => {
                session.set_dye_scales(scales)?;
            }
            ControlEdit::GrainAmount(value) => {
                session.set_grain_amount(value)?;
            }
            ControlEdit::HalationEnabled(enabled) => {
                session.set_halation_enabled(enabled)?;
            }
            ControlEdit::HalationThreshold(value) => {
                session.set_halation_threshold(value)?;
            }
            ControlEdit::HalationRadius(value) => {
                session.set_halation_radius(value)?;
            }
            ControlEdit::HalationRed(value) => {
                session.set_halation_red(value)?;
            }
        }
        Ok(())
    }

    fn render_view(
        &mut self,
        view: RenderView,
        compare: bool,
        progress: &mut JobControl,
    ) -> Result<rawstock_core::render::SrgbImage> {
        match view {
            RenderView::Fit { long_edge } => {
                let session = self.require_ready()?;
                session.set_proxy_long_edge(long_edge.clamp(1, 4096))?;
                if compare {
                    Ok(session.render_baseline_proxy(progress)?)
                } else {
                    Ok(session.render_proxy(progress)?)
                }
            }
            RenderView::Full => {
                let session = self.require_ready()?;
                if compare {
                    Ok(session.render_baseline_full(progress)?)
                } else {
                    Ok(session.render_full(progress)?)
                }
            }
            RenderView::Hundred {
                x,
                y,
                width,
                height,
                exact,
            } => {
                let session = self.require_ready()?;
                let rect = DisplayRect {
                    x,
                    y,
                    width,
                    height,
                };
                let scan = if exact {
                    ScanMode::Exact
                } else {
                    ScanMode::Preview
                };
                if compare {
                    Ok(session.render_baseline_region(rect, scan, progress)?)
                } else {
                    Ok(session.render_region(rect, scan, progress)?)
                }
            }
        }
    }

    fn require_ready(&mut self) -> Result<&mut PreviewSession> {
        if self.stock.is_none() {
            anyhow::bail!("Pick a stock.");
        }
        self.session.as_mut().context("Open a RAW file first.")
    }

    fn ready_state(&self) -> Result<SessionState> {
        let session = self.session.as_ref().context("Open a RAW file first.")?;
        Ok(self.session_state(session))
    }

    fn session_state(&self, session: &PreviewSession) -> SessionState {
        let path = self.raw_path.clone().unwrap_or_default();
        let (sensor_width, sensor_height) = session.sensor_dimensions();
        let (display_width, display_height) = session.display_dimensions();
        SessionState {
            file_name: file_name(&path),
            path: path.display().to_string(),
            sensor_width,
            sensor_height,
            display_width,
            display_height,
            orientation: session.orientation().to_string(),
            quarter_turns: session.quarter_turns(),
            stock: self.stock.as_ref().map(|stock| describe(stock, session)),
        }
    }
}

fn describe(stock: &LoadedFile, session: &PreviewSession) -> StockState {
    let halation = session.halation_controls();
    let parameters = session.parameters();
    StockState {
        id: StockCatalog::id(&stock.path),
        path: stock.path.display().to_string(),
        name: stock.name.clone(),
        model: stock.model.to_string(),
        revision: stock.revision,
        source: stock.source.clone(),
        license: stock.license.clone(),
        exposure_ev: parameters.exposure_ev(),
        print_exposure: parameters.print_exposure(),
        dye_scales: session.dye_scales(),
        grain_amount: session.grain_amount(),
        grain_available: session.grain_available(),
        halation_enabled: halation.enabled,
        halation_threshold: halation.threshold,
        halation_radius: halation.radius,
        halation_weight: halation.weight,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "image".to_string())
}
