//! The real develop behind `develop_preview` and `develop_full`.
//!
//! One thread owns a [`PreviewSession`] for the open photo. X-Trans opens on
//! the third-size quick develop, so the first preview does not wait for
//! Markesteijn, which runs on its own thread and is installed when it lands.
//! A full develop waits for it. Renders use the GPU when one opens, else
//! the CPU. The session keeps every stage it can, so a slider tick only
//! re-runs the stages after the one it changed.
//!
//! Jobs stay on the [`crate::jobs::ContractStub`] board: supersede, cancel
//! and image switches end them there, and the render checks the board
//! between rows. An export is a full develop written as a JPEG to a
//! partial file, which the board renames to the destination only while the
//! job is live. Exports run in the order they were asked for.
//!
//! When Markesteijn lands while no develop is waiting, the thread installs
//! it and emits `image:refined`, so the fit preview, first drawn from the
//! quick develop, is developed again from the full frame.
//!
//! The edit maps onto the stock as it ships: exposure and print exposure
//! are offsets from the stock's own values, dye scales and grain amount are
//! multipliers, and halation overrides the stock's threshold, radius, and
//! red weight. Preview and full frames stay in camera orientation, so a
//! geometry edit never develops again: the viewer draws the edit's quarter
//! turns, straighten and crop itself, and its pan regions are in that
//! frame. An export frames the developed JPEG with the same
//! [`rawstock_core::geometry::Framing`]. Contrast, grain size and scan
//! looks are recorded on the edit but not developed yet.
//!
//! White balance is developed from camera RGB. The session keeps the
//! demosaiced camera RGB of the open photo beside its ACEScg frame, so a
//! custom temperature and tint re-convert instead of decoding again: the
//! fit proxy at once, the full frame when a full develop or export next
//! needs it. That buffer is freed with the session when another photo
//! opens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

use rawstock_core::color::CameraImage;
use rawstock_core::decode::RawImage;
use rawstock_core::gpu::Gpu;
use rawstock_core::render::{DevelopControl, Jpeg, Parameters, SrgbImage, Stage};
use rawstock_core::session::PreviewSession;
use rawstock_core::stock::{ModelError, Stock};
use rawstock_core::white_balance::{Temperature, WhiteBalance, WhiteBalanceSolver};
use rawstock_core::workers::Workers;
use tauri::{AppHandle, Manager};

use crate::contract::{self, AppError, ContractEvent, EditSettings, ImageRefinedEvent, JobKind};
use crate::export_dest;
use crate::frame;
use crate::state::{self, AppState, lock};
use crate::worker;

/// Proxy edge a session opens with. Each preview sets its own.
const OPEN_EDGE: u32 = 720;

/// One accepted `develop_preview` or `develop_full`.
#[derive(Debug, Clone)]
pub struct DevelopRequest {
    pub job_id: u64,
    pub kind: JobKind,
    pub image_id: String,
    pub path: PathBuf,
    pub settings: EditSettings,
    pub long_edge: u32,
    /// Set for an export.
    pub export: Option<ExportTarget>,
}

/// Where an export's JPEG goes, and its quality.
#[derive(Debug, Clone)]
pub struct ExportTarget {
    pub destination: PathBuf,
    pub quality: u8,
}

/// Sends requests to the develop thread.
pub struct Developer {
    sender: mpsc::Sender<DevelopRequest>,
}

impl Developer {
    /// Start the thread, open the GPU in the background, and take preview
    /// and full develops off the stub's ticker.
    pub fn spawn(app: AppHandle) -> Self {
        lock(&app.state::<AppState>().stub).develop_elsewhere();
        let (sender, receiver) = mpsc::channel::<DevelopRequest>();
        let gpu = GpuStart::begin();
        thread::Builder::new()
            .name("rawstock-develop".to_string())
            .spawn(move || {
                let mut service = Service {
                    app,
                    gpu,
                    open: None,
                    stocks: HashMap::new(),
                };
                loop {
                    let first = if service.demosaic_pending() {
                        match receiver.recv_timeout(Duration::from_millis(50)) {
                            Ok(request) => request,
                            Err(RecvTimeoutError::Timeout) => {
                                service.refine();
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match receiver.recv() {
                            Ok(request) => request,
                            Err(_) => break,
                        }
                    };
                    let mut queue = vec![first];
                    queue.extend(receiver.try_iter());
                    for request in Service::latest(queue) {
                        service.run(&request);
                    }
                }
            })
            .expect("spawn the develop thread");
        Self { sender }
    }

    pub fn submit(&self, request: DevelopRequest) {
        let _ = self.sender.send(request);
    }
}

/// The GPU opens on its own thread so launch does not wait for it.
enum GpuStart {
    Starting(Receiver<Option<Gpu>>),
    Ready(Option<Gpu>),
}

impl GpuStart {
    fn begin() -> Self {
        if std::env::var("RAWSTOCK_GPU").as_deref() == Ok("0") {
            return Self::Ready(None);
        }
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("rawstock-gpu".to_string())
            .spawn(move || {
                let _ = tx.send(Gpu::new_blocking().ok());
            });
        match spawned {
            Ok(_) => Self::Starting(rx),
            Err(_) => Self::Ready(None),
        }
    }

    fn device(&mut self) -> Option<Gpu> {
        if let Self::Starting(rx) = self {
            *self = Self::Ready(rx.recv().ok().flatten());
        }
        match self {
            Self::Ready(gpu) => gpu.clone(),
            Self::Starting(_) => None,
        }
    }
}

/// The photo the session was built from.
pub(crate) struct Opened {
    image_id: String,
    path: PathBuf,
    ca_correction: bool,
    session: PreviewSession,
    /// Markesteijn still running for a staged session.
    full: Option<Receiver<Result<FullDevelop, String>>>,
    stock_id: String,
    /// The file's as-shot light, when it can take a custom balance.
    as_shot: Option<Temperature>,
}

struct Service {
    app: AppHandle,
    gpu: GpuStart,
    open: Option<Opened>,
    stocks: HashMap<String, Parameters>,
}

impl Service {
    /// Newest preview and full develop, previews first, then every export
    /// in order. The board has already cancelled the older develops and
    /// never supersedes an export.
    fn latest(queue: Vec<DevelopRequest>) -> Vec<DevelopRequest> {
        let (exports, develops): (Vec<_>, Vec<_>) = queue
            .into_iter()
            .partition(|request| request.kind == JobKind::Export);
        let mut kept: Vec<DevelopRequest> = Vec::new();
        for request in develops.into_iter().rev() {
            if !kept.iter().any(|seen| seen.kind == request.kind) {
                kept.push(request);
            }
        }
        kept.sort_by_key(|request| request.kind != JobKind::Preview);
        kept.extend(exports);
        kept
    }

    fn run(&mut self, request: &DevelopRequest) {
        if !self.live(request.job_id) {
            return;
        }
        let developed = self.develop(request);
        if let (Some(target), Ok(Some(image))) = (&request.export, &developed) {
            let events = match ExportFile::write(image, &request.settings, target) {
                Ok((partial, width, height)) => lock(&self.app.state::<AppState>().stub)
                    .finish_export(request.job_id, &partial, width, height),
                Err(detail) => self.failed(request, &detail),
            };
            worker::emit(&self.app, &events);
            return;
        }
        match developed {
            Ok(Some(image)) => {
                let (width, height) = (image.width(), image.height());
                let packed = frame::pack(width, height, &Rgba::from(&image));
                let events = lock(&self.app.state::<AppState>().stub).finish_develop(
                    request.job_id,
                    packed,
                    width,
                    height,
                );
                worker::emit(&self.app, &events);
            }
            // Cancelled: the board ends the job here. A job that is somehow
            // still live must not hang.
            Ok(None) => {
                if self.live(request.job_id) {
                    let events = lock(&self.app.state::<AppState>().stub)
                        .fail_job(
                            request.job_id,
                            AppError::internal("This develop stopped before it finished."),
                        )
                        .unwrap_or_default();
                    worker::emit(&self.app, &events);
                }
            }
            Err(detail) => {
                let events = self.failed(request, &detail);
                worker::emit(&self.app, &events);
            }
        }
    }

    /// Log `detail` and fail the job with a sentence for the user.
    fn failed(
        &self,
        request: &DevelopRequest,
        detail: &str,
    ) -> Vec<crate::contract::ContractEvent> {
        let state = self.app.state::<AppState>();
        state::log_detail(
            &state.log_dir,
            &format!("develop {}: {detail}", request.path.display()),
        );
        let message = if request.export.is_some() {
            "This photo could not be exported. The RAW photo is not changed."
        } else {
            "This photo could not be developed. The last preview is still on screen."
        };
        lock(&state.stub)
            .fail_job(request.job_id, AppError::internal(message))
            .unwrap_or_default()
    }

    /// `Ok(None)` when the job was cancelled on the way.
    fn develop(&mut self, request: &DevelopRequest) -> Result<Option<SrgbImage>, String> {
        let settings = &request.settings;
        let reuse = self.open.as_ref().is_some_and(|open| {
            open.path == request.path && open.ca_correction == settings.lens.ca_correction
        });
        if !reuse {
            self.open = None;
            let gpu = self.gpu.device();
            // One line per decode, so a white balance drag can be checked
            // for none: only a new photo or the CA switch decodes again.
            state::log_info(
                &self.app.state::<AppState>().log_dir,
                &format!("develop decode {}", request.path.display()),
            );
            let opened = Opened::decode(
                &request.image_id,
                &request.path,
                settings.lens.ca_correction,
                gpu,
            )?;
            lock(&self.app.state::<AppState>().as_shot).insert(
                request.path.clone(),
                opened.as_shot.map(contract::AsShotWhiteBalance::from),
            );
            self.open = Some(opened);
        }
        let parameters = self.stock(&settings.stock.id)?;
        let open = self.open.as_mut().expect("opened above");
        open.apply(settings, parameters)?;
        let app = &self.app;
        let job_id = request.job_id;
        let mut control = BoardControl {
            live: Box::new(move || lock(&app.state::<AppState>().stub).job_live(job_id)),
        };
        let image = match request.kind {
            JobKind::Preview => {
                open.take_full(false, &control)?;
                if open.session.proxy_long_edge() != request.long_edge.max(1) {
                    open.session
                        .set_proxy_long_edge(request.long_edge.max(1))
                        .map_err(|error| error.to_string())?;
                }
                open.session.render_proxy(&mut control).ok()
            }
            JobKind::Full | JobKind::Export => {
                if !open.take_full(true, &control)? {
                    return Ok(None);
                }
                open.session.render_full(&mut control).ok()
            }
            JobKind::Thumbnails => None,
        };
        Ok(image)
    }

    /// The stock as it ships, loaded once per id.
    fn stock(&mut self, id: &str) -> Result<Parameters, String> {
        if let Some(parameters) = self.stocks.get(id) {
            return Ok(parameters.clone());
        }
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(format!("stock id {id:?}"));
        }
        let path = crate::stocks::directory().join(format!("{id}.stock.toml"));
        let parameters = Stock::load(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .into_parameters();
        self.stocks.insert(id.to_string(), parameters.clone());
        Ok(parameters)
    }

    /// Markesteijn is still running for the open photo.
    fn demosaic_pending(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.full.is_some())
    }

    /// Install Markesteijn if it has landed, and tell the UI to develop
    /// its preview again. A failed demosaic is logged and dropped: the next
    /// develop of this photo decodes it again.
    fn refine(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let never = BoardControl {
            live: Box::new(|| true),
        };
        match open.take_full(false, &never) {
            Ok(false) => {}
            Ok(true) => {
                let event = ContractEvent::ImageRefined(ImageRefinedEvent {
                    image_id: open.image_id.clone(),
                });
                worker::emit(&self.app, &[event]);
            }
            Err(detail) => {
                state::log_detail(
                    &self.app.state::<AppState>().log_dir,
                    &format!("develop {}: {detail}", open.path.display()),
                );
                self.open = None;
            }
        }
    }

    /// Ends a cancelled job on the board and reports whether it still runs.
    fn live(&self, job_id: u64) -> bool {
        let (live, events) = lock(&self.app.state::<AppState>().stub).job_check(job_id);
        worker::emit(&self.app, &events);
        live
    }
}

impl Opened {
    fn decode(
        image_id: &str,
        path: &Path,
        ca_correction: bool,
        gpu: Option<Gpu>,
    ) -> Result<Self, String> {
        let mut raw = RawImage::open(path).map_err(|error| error.to_string())?;
        raw.set_chromatic_aberration_correction(ca_correction);
        let orientation = raw.orientation();
        let as_shot = WhiteBalanceSolver::new(raw.camera_matrix(), raw.as_shot_wb())
            .and_then(|solver| solver.as_shot());
        let quick = raw
            .develop_quick_camera()
            .map_err(|error| error.to_string())?;
        // Opened as shot. `apply` converts again for a custom balance,
        // which only touches the proxy until a full frame is asked for.
        let (mut session, full) = match quick {
            Some(quick) => {
                let session = PreviewSession::staged_camera(
                    quick,
                    WhiteBalance::AsShot,
                    orientation,
                    OPEN_EDGE,
                )
                .map_err(|error| error.to_string())?;
                (session, Some(Demosaic::spawn(raw)?))
            }
            None => {
                let camera = raw.develop_camera().map_err(|error| error.to_string())?;
                let session = PreviewSession::with_camera(
                    camera,
                    WhiteBalance::AsShot,
                    orientation,
                    OPEN_EDGE,
                )
                .map_err(|error| error.to_string())?;
                (session, None)
            }
        };
        session.set_gpu(gpu);
        Ok(Self {
            image_id: image_id.to_string(),
            path: path.to_path_buf(),
            ca_correction,
            session,
            full,
            stock_id: String::new(),
            as_shot,
        })
    }

    /// Install Markesteijn once it lands. With `wait`, block for it while
    /// the job is live. Returns whether the full frame is installed.
    fn take_full(&mut self, wait: bool, control: &BoardControl<'_>) -> Result<bool, String> {
        let Some(receiver) = &self.full else {
            return Ok(true);
        };
        let (camera, as_shot) = loop {
            match receiver.try_recv() {
                Ok(result) => break result?,
                Err(TryRecvError::Disconnected) => {
                    return Err("the demosaic thread stopped".to_string());
                }
                Err(TryRecvError::Empty) if !wait => return Ok(false),
                Err(TryRecvError::Empty) => {
                    if control.cancelled() {
                        return Ok(false);
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        };
        self.full = None;
        self.session
            .install_full_camera(camera, Some(as_shot))
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Bring the session to `settings`. Setters that do not change a value
    /// leave the session's stage caches alone.
    fn apply(&mut self, settings: &EditSettings, stock: Parameters) -> Result<(), String> {
        let session = &mut self.session;
        if self.stock_id != settings.stock.id {
            session.set_parameters(stock);
            self.stock_id = settings.stock.id.clone();
        }
        // A file that cannot take the edit's custom balance develops as
        // shot. The adjust panel says so; `as_shot_white_balance` is `null`.
        if session
            .set_white_balance(Self::balance(settings.white_balance))
            .is_err()
        {
            session
                .set_white_balance(WhiteBalance::AsShot)
                .map_err(|error| error.to_string())?;
        }
        let base = session.baseline_parameters().clone();
        let fail = |error: ModelError| error.to_string();
        let exposure = base.exposure_ev() + settings.exposure_ev as f32;
        if session.parameters().exposure_ev() != exposure {
            session.set_exposure_ev(exposure).map_err(fail)?;
        }
        let print = base.print_exposure() + settings.print_exposure as f32;
        if session.parameters().print_exposure() != print {
            session.set_print_exposure(print).map_err(fail)?;
        }
        if let Some(current) = session.dye_scales() {
            let wanted = settings
                .dye
                .map(|dye| dye.scales.map(|scale| scale as f32))
                .unwrap_or([1.0; 3]);
            if current != wanted {
                session.set_dye_scales(wanted).map_err(fail)?;
            }
        }
        if session.grain_available() && session.grain_amount() != settings.grain.amount as f32 {
            session
                .set_grain_amount(settings.grain.amount as f32)
                .map_err(fail)?;
        }
        let halation = session.halation_controls();
        let wanted = &settings.halation;
        if halation.enabled != wanted.enabled {
            session.set_halation_enabled(wanted.enabled).map_err(fail)?;
        }
        if wanted.enabled {
            if let Some(threshold) = wanted.threshold.map(|value| value as f32)
                && threshold != halation.threshold
            {
                session.set_halation_threshold(threshold).map_err(fail)?;
            }
            if let Some(radius) = wanted.radius.map(|value| value as f32)
                && radius != halation.radius
            {
                session.set_halation_radius(radius).map_err(fail)?;
            }
            if let Some(red) = wanted.weight.map(|weight| weight[0] as f32)
                && red != halation.weight[0]
            {
                session.set_halation_red(red).map_err(fail)?;
            }
        }
        Ok(())
    }
}

impl Opened {
    /// The edit's white balance in core's terms.
    pub(crate) fn balance(balance: contract::WhiteBalance) -> WhiteBalance {
        match balance {
            contract::WhiteBalance::AsShot => WhiteBalance::AsShot,
            contract::WhiteBalance::Custom {
                temperature_k,
                tint,
            } => WhiteBalance::Custom(Temperature {
                kelvin: temperature_k,
                tint,
            }),
        }
    }
}

/// Full-size camera RGB, and the same frame developed as shot.
type FullDevelop = (CameraImage, rawstock_core::color::LinearImage);

/// Markesteijn for a staged session, on its own thread.
struct Demosaic;

impl Demosaic {
    fn spawn(raw: RawImage) -> Result<Receiver<Result<FullDevelop, String>>, String> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("rawstock-demosaic".to_string())
            .spawn(move || {
                // Half the capped pool, so a preview rendering meanwhile
                // keeps the total near the cap.
                let threads = (Workers::threads() / 2).max(1);
                // Converted as shot here too, so installing it does not
                // hold up the develop thread. A custom balance converts
                // again, lazily.
                let developed = raw.develop_camera_on(threads).and_then(|camera| {
                    let as_shot = camera.develop_on(WhiteBalance::AsShot, threads)?;
                    Ok((camera, as_shot))
                });
                let _ = tx.send(developed.map_err(|e| e.to_string()));
            })
            .map_err(|error| error.to_string())?;
        Ok(rx)
    }
}

/// The cropped JPEG of an export, written beside its destination.
struct ExportFile;

impl ExportFile {
    /// Encode and write `<destination>.partial`. Returns the partial and
    /// the JPEG's size.
    fn write(
        image: &SrgbImage,
        settings: &EditSettings,
        target: &ExportTarget,
    ) -> Result<(PathBuf, u32, u32), String> {
        Self::write_rgb8(
            image.to_rgb8(),
            image.width(),
            image.height(),
            settings,
            target,
        )
    }

    fn write_rgb8(
        rgb: Vec<u8>,
        width: u32,
        height: u32,
        settings: &EditSettings,
        target: &ExportTarget,
    ) -> Result<(PathBuf, u32, u32), String> {
        let (width, height, rgb) = settings.geometry.framing().apply_rgb8(width, height, &rgb);
        let bytes =
            Jpeg::encode(&rgb, width, height, target.quality).map_err(|error| error.to_string())?;
        let partial = export_dest::partial_path(&target.destination);
        std::fs::write(&partial, bytes)
            .map_err(|error| format!("{}: {error}", partial.display()))?;
        Ok((partial, width, height))
    }
}

/// Cancels a render when the board has ended or cancelled its job.
struct BoardControl<'a> {
    live: Box<dyn Fn() -> bool + 'a>,
}

impl DevelopControl for BoardControl<'_> {
    fn cancelled(&self) -> bool {
        !(self.live)()
    }

    fn on_progress(&mut self, _stage: Stage, _done_rows: u32, _rows: u32) {}
}

/// Encoded sRGB as the RGBA8 the frame header expects.
struct Rgba;

impl Rgba {
    fn from(image: &SrgbImage) -> Vec<u8> {
        let rgb = image.to_rgb8();
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for pixel in rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(job_id: u64, kind: JobKind) -> DevelopRequest {
        DevelopRequest {
            job_id,
            kind,
            image_id: "a".to_string(),
            path: PathBuf::from("/photos/a.RAF"),
            settings: EditSettings::default(),
            long_edge: 720,
            export: None,
        }
    }

    #[test]
    fn a_cancelled_job_stops_the_render_and_a_live_one_finishes() {
        let mut pixels = Vec::new();
        for index in 0..64 * 48 {
            let value = 0.02 + (index % 64) as f32 / 128.0;
            pixels.extend_from_slice(&[value, value * 0.8, value * 0.6]);
        }
        let linear = rawstock_core::color::LinearImage::new(64, 48, pixels).unwrap();
        let mut session =
            PreviewSession::new(linear, rawstock_core::decode::Orientation::Normal, 32).unwrap();
        let directory = crate::stocks::directory();
        session.set_parameters(
            Stock::load(directory.join("velvia.stock.toml"))
                .unwrap()
                .into_parameters(),
        );
        let live = std::cell::Cell::new(true);
        let mut control = BoardControl {
            live: Box::new(|| live.get()),
        };
        let image = session.render_proxy(&mut control).unwrap();
        assert_eq!((image.width(), image.height()), (32, 24));
        session.set_exposure_ev(1.0).unwrap();
        live.set(false);
        assert!(session.render_proxy(&mut control).is_err());
        assert!(session.render_full(&mut control).is_err());
    }

    #[test]
    fn only_the_newest_request_of_each_kind_runs_and_previews_go_first() {
        let kept = Service::latest(vec![
            request(1, JobKind::Preview),
            request(2, JobKind::Full),
            request(3, JobKind::Preview),
            request(4, JobKind::Preview),
        ]);
        let ids: Vec<u64> = kept.iter().map(|request| request.job_id).collect();
        assert_eq!(ids, [4, 2]);

        let kept = Service::latest(vec![
            request(1, JobKind::Export),
            request(2, JobKind::Preview),
            request(3, JobKind::Export),
        ]);
        let ids: Vec<u64> = kept.iter().map(|request| request.job_id).collect();
        assert_eq!(ids, [2, 1, 3]);
    }

    #[test]
    fn export_crops_and_writes_only_the_partial() {
        let directory =
            std::env::temp_dir().join(format!("rawstock-export-file-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("out.jpg");
        let mut settings = EditSettings::default();
        settings.geometry.quarter_turns = 1;
        settings.geometry.crop = Some(crate::contract::CropRect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        let target = ExportTarget {
            destination: destination.clone(),
            quality: 80,
        };
        let (partial, width, height) =
            ExportFile::write_rgb8(vec![128; 8 * 6 * 3], 8, 6, &settings, &target).unwrap();
        assert_eq!((width, height), (3, 4));
        assert_eq!(partial, export_dest::partial_path(&destination));
        assert!(!destination.exists());
        let bytes = std::fs::read(&partial).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
