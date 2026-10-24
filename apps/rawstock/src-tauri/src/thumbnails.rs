//! Stock thumbnails rendered from the open photo.
//!
//! One thread decodes the photo once, keeps a small ACEScg copy, and runs
//! every requested stock over it. A thumbnail shows tone and colour only:
//! grain and halation are off, because neither means anything at this
//! size. Exposure, white balance, rotation, straighten, crop, and the CA
//! switch come from the edit; the framing is the export's. The kept copy is
//! camera RGB, so a white balance change converts it again without a
//! decode.
//!
//! The job itself lives on the [`crate::jobs::ContractStub`] board, so cancel,
//! supersede, and image switches end it the same way as any other job.
//! The renderer checks the board between stocks.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use rawstock_core::color::{CameraImage, LinearImage};
use rawstock_core::decode::{Orientation, RawImage};
use rawstock_core::render::Pipeline;
use rawstock_core::stock::Stock;
use rawstock_core::white_balance::WhiteBalance;
use tauri::{AppHandle, Manager};

use crate::contract::{AppError, EditSettings};
use crate::develop::Opened;
use crate::frame;
use crate::state::{self, AppState, lock};
use crate::worker;

/// Long edge of the kept copy. Thumbnails are a few hundred pixels, so
/// this leaves room for a crop without decoding again.
const SOURCE_EDGE: u32 = 1024;

/// Largest thumbnail the renderer will draw.
const MAX_EDGE: u32 = 768;

/// One `render_thumbnails` call, after the board accepted it.
#[derive(Debug, Clone)]
pub struct ThumbnailRequest {
    pub job_id: u64,
    pub path: PathBuf,
    pub settings: EditSettings,
    pub stock_ids: Vec<String>,
    pub long_edge: u32,
}

/// Sends requests to the thumbnail thread.
pub struct Thumbnailer {
    sender: mpsc::Sender<ThumbnailRequest>,
}

impl Thumbnailer {
    /// Start the thread and take thumbnail jobs off the stub's ticker.
    pub fn spawn(app: AppHandle) -> Self {
        lock(&app.state::<AppState>().stub).render_thumbnails_elsewhere();
        let (sender, receiver) = mpsc::channel::<ThumbnailRequest>();
        thread::Builder::new()
            .name("rawstock-thumbnails".to_string())
            .spawn(move || {
                let mut source: Option<Source> = None;
                while let Ok(mut request) = receiver.recv() {
                    // A newer request supersedes everything queued before it.
                    while let Ok(newer) = receiver.try_recv() {
                        request = newer;
                    }
                    Self::run(&app, &mut source, &request);
                }
            })
            .expect("spawn the thumbnail thread");
        Self { sender }
    }

    pub fn submit(&self, request: ThumbnailRequest) {
        let _ = self.sender.send(request);
    }

    fn run(app: &AppHandle, source: &mut Option<Source>, request: &ThumbnailRequest) {
        let state = app.state::<AppState>();
        if !Self::live(app, request.job_id) {
            return;
        }
        let ca = request.settings.lens.ca_correction;
        let reuse = source
            .as_ref()
            .is_some_and(|kept| kept.path == request.path && kept.ca_correction == ca);
        if !reuse {
            *source = None;
            match Source::decode(&request.path, ca) {
                Ok(decoded) => *source = Some(decoded),
                Err(detail) => {
                    Self::fail(app, request.job_id, &request.path, &detail);
                    return;
                }
            }
        }
        let Some(kept) = source.as_ref() else {
            return;
        };
        let base = match kept.framed(&request.settings, request.long_edge) {
            Ok(base) => base,
            Err(detail) => {
                Self::fail(app, request.job_id, &request.path, &detail);
                return;
            }
        };
        let stocks = crate::stocks::directory();
        for stock_id in &request.stock_ids {
            if !Self::live(app, request.job_id) {
                return;
            }
            let packed = match ThumbnailRender::stock(&stocks, stock_id, &request.settings, &base) {
                Ok(packed) => packed,
                Err(error) => {
                    state::log_detail(
                        &state.log_dir,
                        &format!(
                            "thumbnail {stock_id} for {}: {}",
                            request.path.display(),
                            error.message
                        ),
                    );
                    let events = lock(&state.stub)
                        .fail_job(request.job_id, error)
                        .unwrap_or_default();
                    worker::emit(app, &events);
                    return;
                }
            };
            let events = lock(&state.stub).deliver_thumbnail(request.job_id, stock_id, packed);
            worker::emit(app, &events);
        }
        let events =
            lock(&state.stub).finish_thumbnails(request.job_id, base.width(), base.height());
        worker::emit(app, &events);
    }

    fn live(app: &AppHandle, job_id: u64) -> bool {
        let (live, events) = lock(&app.state::<AppState>().stub).thumbnail_check(job_id);
        worker::emit(app, &events);
        live
    }

    fn fail(app: &AppHandle, job_id: u64, path: &Path, detail: &str) {
        let state = app.state::<AppState>();
        state::log_detail(
            &state.log_dir,
            &format!("thumbnails for {}: {detail}", path.display()),
        );
        let error = AppError::internal(
            "The stock thumbnails could not be drawn for this photo. Stocks can still be chosen by name.",
        );
        let events = lock(&state.stub)
            .fail_job(job_id, error)
            .unwrap_or_default();
        worker::emit(app, &events);
    }
}

/// The open photo at [`SOURCE_EDGE`], camera RGB before white balance, as
/// the sensor sees it. White balance and orientation are applied per
/// request, so neither needs a decode.
struct Source {
    path: PathBuf,
    ca_correction: bool,
    camera: Orientation,
    rgb: CameraImage,
}

impl Source {
    /// X-Trans uses the third-size quick develop, which is plenty for a
    /// thumbnail. Bayer and linear DNG develop in full, once per photo.
    fn decode(path: &Path, ca_correction: bool) -> Result<Self, String> {
        let mut raw = RawImage::open(path).map_err(|error| error.to_string())?;
        raw.set_chromatic_aberration_correction(ca_correction);
        let camera = raw.orientation();
        let rgb = match raw
            .develop_quick_camera()
            .map_err(|error| error.to_string())?
        {
            Some(quick) => quick.image,
            None => raw.develop_camera().map_err(|error| error.to_string())?,
        };
        let rgb = rgb
            .area_downsample(SOURCE_EDGE)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path: path.to_path_buf(),
            ca_correction,
            camera,
            rgb,
        })
    }

    /// Oriented, framed, and shrunk to `long_edge`.
    fn framed(&self, settings: &EditSettings, long_edge: u32) -> Result<LinearImage, String> {
        // As shot when the file cannot take the custom balance, like the
        // main develop.
        let mut image = self
            .rgb
            .develop(Opened::balance(settings.white_balance))
            .or_else(|_| self.rgb.develop(WhiteBalance::AsShot))
            .map_err(|error| error.to_string())?;
        image.apply_orientation(self.camera);
        ThumbnailRender::frame(&image, settings)?
            .area_downsample(long_edge.clamp(1, MAX_EDGE))
            .map_err(|error| error.to_string())
    }
}

/// Stock-by-stock rendering, kept apart from the thread so tests can run it.
pub struct ThumbnailRender;

impl ThumbnailRender {
    /// One stock over `base`, packed for `take_frame`.
    pub fn stock(
        directory: &Path,
        stock_id: &str,
        settings: &EditSettings,
        base: &LinearImage,
    ) -> Result<Vec<u8>, AppError> {
        let valid = !stock_id.is_empty()
            && stock_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(AppError::stock_missing(stock_id));
        }
        let path = directory.join(format!("{stock_id}.stock.toml"));
        let stock = Stock::load(&path).map_err(|_| AppError::stock_missing(stock_id))?;
        let mut parameters = stock.into_parameters();
        let exposure = parameters.exposure_ev() + settings.exposure_ev as f32;
        parameters.set_exposure_ev(exposure).map_err(|_| {
            AppError::internal("The exposure could not be applied to the thumbnail.")
        })?;
        // Neither reads at thumbnail size, and both need the whole frame.
        let _ = parameters.set_grain(None);
        let _ = parameters.set_halation(None);
        let developed = Pipeline::new(parameters).develop(base);
        let rgb = developed.to_rgb8();
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for pixel in rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        Ok(frame::pack(developed.width(), developed.height(), &rgba))
    }

    /// Turns, straighten and crop of a camera-oriented image, as the
    /// export frames it.
    pub fn frame(image: &LinearImage, settings: &EditSettings) -> Result<LinearImage, String> {
        let (width, height, pixels) =
            settings
                .geometry
                .framing()
                .apply_f32(image.width(), image.height(), image.pixels());
        LinearImage::new(width, height, pixels).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::ContractStub;

    fn gradient(width: u32, height: u32) -> LinearImage {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                let value = 0.02 + 0.5 * (x as f32 / width as f32);
                pixels.extend_from_slice(&[value, value * (1.0 - y as f32 / height as f32), 0.1]);
            }
        }
        LinearImage::new(width, height, pixels).unwrap()
    }

    fn photo() -> crate::contract::ImageInfo {
        crate::contract::ImageInfo {
            image_id: "a".to_string(),
            path: "/photos/a.RAF".to_string(),
            file_name: "a.RAF".to_string(),
            camera: crate::contract::CameraInfo {
                make: "Fujifilm".to_string(),
                model: "X-T2".to_string(),
            },
            support: crate::contract::SupportLevel::Decodes,
            exif: crate::contract::ExifInfo::default(),
            width: 0,
            height: 0,
            sidecar: crate::contract::SidecarInfo::none(),
            error: None,
        }
    }

    #[test]
    fn every_repo_stock_renders_a_packed_thumbnail() {
        let base = gradient(24, 16);
        let settings = EditSettings::default();
        for stock in crate::stocks::list().unwrap() {
            let packed =
                ThumbnailRender::stock(&crate::stocks::directory(), &stock.id, &settings, &base)
                    .unwrap();
            assert_eq!(frame::dimensions(&packed), Some((24, 16)), "{}", stock.id);
        }
    }

    #[test]
    fn stocks_differ_and_exposure_brightens() {
        let base = gradient(24, 16);
        let directory = crate::stocks::directory();
        let settings = EditSettings::default();
        let portra = ThumbnailRender::stock(&directory, "portra400", &settings, &base).unwrap();
        let velvia = ThumbnailRender::stock(&directory, "velvia", &settings, &base).unwrap();
        assert_ne!(portra, velvia);
        let brighter = EditSettings {
            exposure_ev: 1.0,
            ..EditSettings::default()
        };
        let lifted = ThumbnailRender::stock(&directory, "portra400", &brighter, &base).unwrap();
        let mean = |packed: &[u8]| -> f64 {
            let body = &packed[frame::HEADER..];
            body.iter().map(|&value| f64::from(value)).sum::<f64>() / body.len() as f64
        };
        assert!(mean(&lifted) > mean(&portra));
    }

    #[test]
    fn a_path_like_stock_id_is_refused() {
        let base = gradient(4, 4);
        let error = ThumbnailRender::stock(
            &crate::stocks::directory(),
            "../stocks/portra400",
            &EditSettings::default(),
            &base,
        )
        .unwrap_err();
        assert_eq!(error.code, crate::contract::ErrorCode::StockMissing);
    }

    #[test]
    fn frame_matches_the_export_framing() {
        let image = gradient(20, 10);
        let mut settings = EditSettings::default();
        settings.geometry.crop = Some(crate::contract::CropRect {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        });
        let cropped = ThumbnailRender::frame(&image, &settings).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (10, 5));
        assert_eq!(cropped.pixels()[0], image.pixels()[10 * 3]);

        settings.geometry.quarter_turns = 1;
        settings.geometry.straighten_deg = 3.0;
        let framed = ThumbnailRender::frame(&image, &settings).unwrap();
        let size = settings.geometry.framing().output_size(20, 10);
        assert_eq!((framed.width(), framed.height()), size);
    }

    #[test]
    fn board_ends_a_cancelled_thumbnail_job_between_stocks() {
        let mut stub = ContractStub::default();
        stub.render_thumbnails_elsewhere();
        stub.insert_image(photo());
        let stocks = vec!["portra400".to_string(), "velvia".to_string()];
        let (job, _) = stub
            .render_thumbnails("a", &EditSettings::default(), stocks, 32)
            .unwrap();
        assert!(
            stub.tick().is_empty(),
            "the ticker leaves real thumbnails alone"
        );
        assert!(stub.thumbnail_check(job.job_id).0);
        let events = stub.deliver_thumbnail(job.job_id, "portra400", frame::placeholder(2, 2, 0));
        assert!(events.iter().any(|event| matches!(
            event,
            crate::contract::ContractEvent::ThumbnailReady(ready) if ready.stock_id == "portra400"
        )));
        assert!(stub.take_frame(job.job_id, Some("portra400")).is_ok());

        let (newer, events) = stub
            .render_thumbnails(
                "a",
                &EditSettings::default(),
                vec!["velvia".to_string()],
                32,
            )
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            crate::contract::ContractEvent::Cancelled(cancelled) if cancelled.job_id == job.job_id
        )));
        assert!(!stub.thumbnail_check(job.job_id).0);
        assert!(
            stub.deliver_thumbnail(job.job_id, "velvia", frame::placeholder(2, 2, 0))
                .is_empty()
        );

        stub.cancel(newer.job_id);
        let (live, events) = stub.thumbnail_check(newer.job_id);
        assert!(!live);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, crate::contract::ContractEvent::Cancelled(_)))
        );
    }

    #[test]
    fn finishing_reports_the_real_size() {
        let mut stub = ContractStub::default();
        stub.render_thumbnails_elsewhere();
        stub.insert_image(photo());
        let (job, _) = stub
            .render_thumbnails(
                "a",
                &EditSettings::default(),
                vec!["velvia".to_string()],
                300,
            )
            .unwrap();
        let events = stub.finish_thumbnails(job.job_id, 300, 200);
        assert!(events.iter().any(|event| matches!(
            event,
            crate::contract::ContractEvent::Done(done) if done.width == 300 && done.height == 200
        )));
    }
}
