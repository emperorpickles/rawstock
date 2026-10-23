//! Preview session: dirty stages, cancel, crop grain, and stock text.

use std::path::PathBuf;

use rawstock_core::color::{LinearImage, QuickDevelop};
use rawstock_core::decode::Orientation;
use rawstock_core::render::{DevelopControl, Fidelity, NopControl, Parameters, Pipeline, Stage};
use rawstock_core::session::{DisplayRect, PreviewSession, ScanMode};
use rawstock_core::stock::{
    Curve, FilmModel, Grain, Granularity, Halation, ModelError, Parametric, Stock, StockMeta,
    StockModel,
};

struct Log {
    stages: Vec<Stage>,
    cancel_at: Option<Stage>,
    cancel: bool,
}

impl Log {
    fn new() -> Self {
        Self {
            stages: Vec::new(),
            cancel_at: None,
            cancel: false,
        }
    }

    fn starts_with(&self, stage: Stage) {
        assert_eq!(
            self.stages.first().copied(),
            Some(stage),
            "{:?}",
            self.stages
        );
    }
}

impl DevelopControl for Log {
    fn cancelled(&self) -> bool {
        self.cancel
    }

    fn on_progress(&mut self, stage: Stage, done_rows: u32, _rows: u32) {
        if done_rows == 0 {
            self.stages.push(stage);
            if self.cancel_at == Some(stage) {
                self.cancel = true;
            }
        }
    }
}

fn negative() -> Curve {
    Curve::new([(-4.0, 0.10), (-1.0, 0.55), (0.0, 1.20), (1.5, 2.10)]).unwrap()
}

fn print_curve() -> Curve {
    Curve::new([(-1.5, 0.05), (0.0, 0.50), (1.0, 1.30), (2.5, 2.40)]).unwrap()
}

fn parameters(grain: bool, halation: bool) -> Parameters {
    let curve = negative();
    let film = FilmModel::Parametric(
        Parametric::new([curve.clone(), curve.clone(), curve], None).unwrap(),
    );
    let mut parameters = Parameters::new(0.25, film, 0.8, print_curve()).unwrap();
    if grain {
        let layer = Granularity::new([(0.0, 40.0), (3.0, 40.0)]).unwrap();
        parameters = parameters
            .with_grain(Grain::new(35.0, 7, [layer.clone(), layer.clone(), layer]).unwrap());
    }
    if halation {
        parameters = parameters.with_halation(Halation::new(0.2, 0.01, [0.4, 0.05, 0.0]).unwrap());
    }
    parameters
}

fn frame(width: u32, height: u32) -> LinearImage {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
    for index in 0..width as usize * height as usize {
        let gray = 0.02 + (index % 17) as f32 * 0.03;
        pixels.extend([gray, gray * 0.9, gray * 1.1]);
    }
    LinearImage::new(width, height, pixels).unwrap()
}

fn session(width: u32, height: u32, grain: bool, halation: bool) -> PreviewSession {
    let mut session = PreviewSession::new(frame(width, height), Orientation::Normal, 64).unwrap();
    session.set_parameters(parameters(grain, halation));
    session
}

#[test]
fn edits_rerun_only_the_tail() {
    let mut session = session(24, 8, true, true);
    let mut log = Log::new();
    session.render_proxy(&mut log).unwrap();
    assert_eq!(log.stages.first().copied(), Some(Stage::Exposure));
    assert!(log.stages.contains(&Stage::Density));
    assert!(!log.stages.iter().any(|stage| stage.name() == "Decode"));

    log.stages.clear();
    session.set_grain_amount(1.5).unwrap();
    session.render_proxy(&mut log).unwrap();
    log.starts_with(Stage::Grain);
    assert!(!log.stages.contains(&Stage::Density));
    assert!(!log.stages.contains(&Stage::Exposure));

    log.stages.clear();
    session.set_print_exposure(1.1).unwrap();
    session.render_proxy(&mut log).unwrap();
    log.starts_with(Stage::Print);
    assert!(!log.stages.contains(&Stage::Density));
    assert!(!log.stages.contains(&Stage::Grain));
    assert!(!log.stages.contains(&Stage::DyeScan));

    log.stages.clear();
    let before = session.dimensions();
    session.set_exposure_ev(-0.4).unwrap();
    session.render_proxy(&mut log).unwrap();
    log.starts_with(Stage::Exposure);
    assert_eq!(session.dimensions(), before);
}

#[test]
fn cancel_during_density_returns_no_image() {
    let mut session = session(12, 6, true, false);
    let mut log = Log::new();
    log.cancel_at = Some(Stage::Density);
    let err = session.render_proxy(&mut log).unwrap_err();
    assert_eq!(err, rawstock_core::render::Cancelled);
    assert!(log.stages.contains(&Stage::Density));

    log.cancel = false;
    log.cancel_at = None;
    log.stages.clear();
    assert!(session.render_proxy(&mut log).is_ok());
}

#[test]
fn crop_matches_the_full_frame_including_grain() {
    let mut session = session(800, 8, true, false);
    let full = session.render_full(&mut NopControl).unwrap();
    let rect = DisplayRect {
        x: 20,
        y: 1,
        width: 30,
        height: 4,
    };
    let crop = session
        .render_region(rect, ScanMode::Exact, &mut NopControl)
        .unwrap();
    assert_eq!(crop.fidelity(), Fidelity::Exact);
    let full_rgb = full.to_rgb8();
    let crop_rgb = crop.to_rgb8();
    for y in 0..4u32 {
        for x in 0..30u32 {
            let full_index = (((y + 1) * 800 + (x + 20)) * 3) as usize;
            let crop_index = ((y * 30 + x) * 3) as usize;
            assert_eq!(
                crop_rgb[crop_index..crop_index + 3],
                full_rgb[full_index..full_index + 3],
                "pixel {x},{y}"
            );
        }
    }
}

#[test]
fn halation_crop_matches_the_interior_of_the_full_frame() {
    let mut session = session(48, 36, false, true);
    let full = session.render_full(&mut NopControl).unwrap();
    let support = session.parameters().halation().unwrap().support_px(48, 36);
    let rect = DisplayRect {
        x: support + 2,
        y: support + 2,
        width: 10,
        height: 8,
    };
    let crop = session
        .render_region(rect, ScanMode::Exact, &mut NopControl)
        .unwrap();
    let full_rgb = full.to_rgb8();
    let crop_rgb = crop.to_rgb8();
    for y in 0..8u32 {
        for x in 0..10u32 {
            let full_index = (((y + rect.y) * 48 + (x + rect.x)) * 3) as usize;
            let crop_index = ((y * 10 + x) * 3) as usize;
            assert_eq!(
                crop_rgb[crop_index..crop_index + 3],
                full_rgb[full_index..full_index + 3]
            );
        }
    }
}

#[test]
fn orientation_is_applied_before_the_pipeline() {
    let orientations = [
        Orientation::Normal,
        Orientation::HorizontalFlip,
        Orientation::Rotate180,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];
    for orientation in orientations {
        let source = frame(16, 10);
        let (width, height, pixels) =
            orientation.apply_rgb(source.width(), source.height(), source.pixels());
        let oriented = LinearImage::new(width, height, pixels).unwrap();
        let mut session = PreviewSession::new(source, orientation, 8).unwrap();
        assert_eq!(session.sensor_dimensions(), (16, 10), "{orientation}");
        assert_eq!(
            session.display_dimensions(),
            (width, height),
            "{orientation}"
        );
        assert_eq!(session.camera_orientation(), orientation);
        assert_eq!(session.orientation(), orientation);
        assert_eq!(session.quarter_turns(), 0);
        session.set_parameters(parameters(false, false));
        let from_session = session.render_full(&mut NopControl).unwrap();
        let from_pipeline = Pipeline::new(session.parameters().clone()).develop(&oriented);
        assert_eq!(
            from_session.to_rgb8(),
            from_pipeline.to_rgb8(),
            "{orientation}"
        );
        assert_eq!(
            (from_session.width(), from_session.height()),
            (width, height)
        );
    }
}

#[test]
fn oriented_crop_matches_the_full_frame() {
    let orientations = [
        Orientation::Normal,
        Orientation::Rotate180,
        Orientation::Rotate90,
        Orientation::Rotate270,
        Orientation::HorizontalFlip,
        Orientation::VerticalFlip,
        Orientation::Transpose,
        Orientation::Transverse,
    ];
    for orientation in orientations {
        let mut session = PreviewSession::new(frame(40, 24), orientation, 20).unwrap();
        session.set_parameters(parameters(true, false));
        let (width, height) = session.display_dimensions();
        let full = session.render_full(&mut NopControl).unwrap();
        assert_eq!((full.width(), full.height()), (width, height));
        let rect = DisplayRect {
            x: 2,
            y: 3,
            width: 11,
            height: 7,
        };
        assert!(rect.x + rect.width <= width && rect.y + rect.height <= height);
        let crop = session
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .unwrap();
        assert_eq!((crop.width(), crop.height()), (rect.width, rect.height));
        let full_rgb = full.to_rgb8();
        let crop_rgb = crop.to_rgb8();
        for y in 0..rect.height {
            for x in 0..rect.width {
                let full_index = (((y + rect.y) * width + (x + rect.x)) * 3) as usize;
                let crop_index = ((y * rect.width + x) * 3) as usize;
                assert_eq!(
                    crop_rgb[crop_index..crop_index + 3],
                    full_rgb[full_index..full_index + 3],
                    "{orientation} pixel {x},{y}"
                );
            }
        }
    }
}

#[test]
fn oriented_halation_crop_matches_the_interior() {
    for orientation in [
        Orientation::Rotate90,
        Orientation::Rotate270,
        Orientation::Rotate180,
    ] {
        let mut session = PreviewSession::new(frame(80, 48), orientation, 40).unwrap();
        session.set_parameters(parameters(true, true));
        let (width, height) = session.display_dimensions();
        let support = session
            .parameters()
            .halation()
            .unwrap()
            .support_px(width, height);
        let rect = DisplayRect {
            x: support + 1,
            y: support + 1,
            width: 8,
            height: 6,
        };
        assert!(
            rect.x + rect.width <= width && rect.y + rect.height <= height,
            "{orientation} {width}x{height} support {support}"
        );
        let full = session.render_full(&mut NopControl).unwrap();
        let crop = session
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .unwrap();
        let full_rgb = full.to_rgb8();
        let crop_rgb = crop.to_rgb8();
        for y in 0..rect.height {
            for x in 0..rect.width {
                let full_index = (((y + rect.y) * width + (x + rect.x)) * 3) as usize;
                let crop_index = ((y * rect.width + x) * 3) as usize;
                assert_eq!(
                    crop_rgb[crop_index..crop_index + 3],
                    full_rgb[full_index..full_index + 3],
                    "{orientation} pixel {x},{y}"
                );
            }
        }
    }
}

#[test]
fn manual_turn_reorients_without_changing_the_sensor_size() {
    let source = frame(16, 10);
    let mut session = PreviewSession::new(source.clone(), Orientation::Rotate90, 8).unwrap();
    session.set_parameters(parameters(true, false));
    let (display_width, display_height) = session.display_dimensions();
    assert_eq!(
        session.proxy_dimensions(),
        LinearImage::proxy_dimensions(display_width, display_height, 8)
    );
    assert_ne!(
        session.proxy_dimensions(),
        LinearImage::proxy_dimensions(16, 10, 8)
    );
    let before = session.render_full(&mut NopControl).unwrap();
    session.rotate_clockwise().unwrap();
    assert_eq!(session.sensor_dimensions(), (16, 10));
    assert_eq!(session.quarter_turns(), 1);
    let composed = Orientation::Rotate90.then(Orientation::Rotate90);
    assert_eq!(session.orientation(), composed);
    assert_eq!(session.display_dimensions(), composed.dimensions(16, 10));
    let (width, height, pixels) = composed.apply_rgb(16, 10, source.pixels());
    let oriented = LinearImage::new(width, height, pixels).unwrap();
    let from_session = session.render_full(&mut NopControl).unwrap();
    let from_pipeline = Pipeline::new(session.parameters().clone()).develop(&oriented);
    assert_eq!(from_session.to_rgb8(), from_pipeline.to_rgb8());
    assert_ne!(
        (from_session.width(), from_session.height()),
        (before.width(), before.height())
    );

    session.rotate_counterclockwise().unwrap();
    let restored = session.render_full(&mut NopControl).unwrap();
    assert_eq!(restored.to_rgb8(), before.to_rgb8());
    assert_eq!(session.quarter_turns(), 0);

    for _ in 0..4 {
        session.rotate_clockwise().unwrap();
    }
    let around = session.render_full(&mut NopControl).unwrap();
    assert_eq!(around.to_rgb8(), before.to_rgb8());
    assert_eq!(session.quarter_turns(), 0);
}

#[test]
fn export_matches_the_pipeline() {
    let mut session = session(16, 10, true, true);
    let from_session = session.render_full(&mut NopControl).unwrap();
    let from_pipeline = Pipeline::new(session.parameters().clone()).develop(&frame_of(&session));
    assert_eq!(from_session.to_rgb8(), from_pipeline.to_rgb8());
    assert_eq!(from_session.fidelity(), Fidelity::Exact);
}

fn frame_of(session: &PreviewSession) -> LinearImage {
    // Rebuild the same pattern `frame` used. The session does not hand the
    // buffer back out; the pipeline comparison uses an equal input.
    let (width, height) = session.dimensions();
    frame(width, height)
}

#[test]
fn shipped_stocks_round_trip() {
    for name in [
        "neutral.stock.toml",
        "portra400.stock.toml",
        "velvia.stock.toml",
        "velvia50-spectral.stock.toml",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../stocks/{name}"));
        let stock = Stock::load(&path).unwrap();
        let text = stock.to_toml();
        assert!(text.starts_with("# Written by rawstock. Comments are not preserved.\n"));
        let parsed = Stock::parse(&text).unwrap();
        assert_same_stock(&stock, &parsed);
    }
}

#[test]
fn saving_twice_does_not_square_the_dye_scale() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let stock = Stock::load(&path).unwrap();
    let mut session = PreviewSession::new(frame(4, 2), Orientation::Normal, 4).unwrap();
    session.set_parameters(stock.parameters().clone());
    session.set_dye_scales([1.25, 0.8, 1.1]).unwrap();
    let once = written(&session, &stock);
    session.commit_edit();
    assert_eq!(session.dye_scales(), Some([1.0, 1.0, 1.0]));
    let twice = written(&session, &stock);
    assert_same_stock(&once, &twice);
}

#[test]
fn zero_grain_and_disabled_halation_are_omitted() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let stock = Stock::load(&path).unwrap();
    let mut session = PreviewSession::new(frame(4, 2), Orientation::Normal, 4).unwrap();
    session.set_parameters(stock.parameters().clone());
    session.set_grain_amount(0.0).unwrap();
    session.set_halation_enabled(false).unwrap();
    let text = written(&session, &stock).to_toml();
    assert!(!text.contains("[grain]"));
    assert!(!text.contains("[halation]"));
    let parsed = Stock::parse(&text).unwrap();
    assert!(parsed.parameters().grain().is_none());
    assert!(parsed.parameters().halation().is_none());
}

#[test]
fn model_tag_has_to_match_the_film() {
    let stock = Stock::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/neutral.stock.toml"),
    )
    .unwrap();
    let meta = StockMeta::new(2, "Nope", "source", "CC0-1.0", StockModel::Spectral).unwrap();
    let err = Stock::from_parts(meta, stock.parameters().clone()).unwrap_err();
    assert!(matches!(
        err,
        rawstock_core::stock::StockError::ModelMismatch { .. }
    ));
}

#[test]
fn dye_scales_are_absent_on_neutral() {
    let mut session = session(8, 4, false, false);
    assert_eq!(session.dye_scales(), None);
    let err = session.set_dye_scales([1.0, 1.0, 1.0]).unwrap_err();
    assert!(matches!(err, ModelError::DyeDensity));
}

#[test]
fn stock_switch_does_not_replace_the_frame() {
    let mut session = session(18, 6, false, false);
    let size = session.dimensions();
    let neutral = Stock::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/neutral.stock.toml"),
    )
    .unwrap();
    let portra = Stock::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml"),
    )
    .unwrap();
    session.set_parameters(neutral.into_parameters());
    session.set_parameters(portra.parameters().clone());
    assert_eq!(session.dimensions(), size);
    session.set_proxy_long_edge(32).unwrap();
    assert_eq!(session.dimensions(), size);
    assert_eq!(session.proxy_long_edge(), 32);
}

fn written(session: &PreviewSession, opened: &Stock) -> Stock {
    let meta = StockMeta::new(
        opened.revision() + 1,
        opened.name(),
        format!("Tuned from {} in rawstock.", opened.name()),
        opened.license(),
        opened.model(),
    )
    .unwrap();
    Stock::from_parts(meta, session.parameters().clone()).unwrap()
}

fn assert_same_stock(left: &Stock, right: &Stock) {
    assert_eq!(left.schema_version(), right.schema_version());
    assert_eq!(left.revision(), right.revision());
    assert_eq!(left.name(), right.name());
    assert_eq!(left.source(), right.source());
    assert_eq!(left.license(), right.license());
    assert_eq!(left.model(), right.model());
    assert_same_parameters(left.parameters(), right.parameters());
}

fn assert_same_parameters(left: &Parameters, right: &Parameters) {
    assert_eq!(left.exposure_ev().to_bits(), right.exposure_ev().to_bits());
    assert_eq!(
        left.print_exposure().to_bits(),
        right.print_exposure().to_bits()
    );
    assert_curve(left.print_curve(), right.print_curve());
    match (left.halation(), right.halation()) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            assert_eq!(a.threshold().to_bits(), b.threshold().to_bits());
            assert_eq!(a.radius().to_bits(), b.radius().to_bits());
            for (left, right) in a.weight().into_iter().zip(b.weight()) {
                assert_eq!(left.to_bits(), right.to_bits());
            }
        }
        _ => panic!("halation presence differs"),
    }
    match (left.grain(), right.grain()) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            assert_eq!(a.frame_mm().to_bits(), b.frame_mm().to_bits());
            assert_eq!(a.seed(), b.seed());
            for (left, right) in a.size_mm().into_iter().zip(b.size_mm()) {
                assert_eq!(left.to_bits(), right.to_bits());
            }
            for (left, right) in a.layers().iter().zip(b.layers()) {
                let left: Vec<_> = left.points().collect();
                let right: Vec<_> = right.points().collect();
                assert_eq!(left.len(), right.len());
                for (left, right) in left.into_iter().zip(right) {
                    assert_eq!(left.0.to_bits(), right.0.to_bits());
                    assert_eq!(left.1.to_bits(), right.1.to_bits());
                }
            }
        }
        _ => panic!("grain presence differs"),
    }
    match (left.film(), right.film()) {
        (FilmModel::Parametric(a), FilmModel::Parametric(b)) => {
            assert_curves(a.curves(), b.curves());
            assert_eq!(a.coupling(), b.coupling());
        }
        (FilmModel::Spectral(a), FilmModel::Spectral(b)) => {
            assert_samples(a.wavelengths(), b.wavelengths());
            for (left, right) in a.sensitivities().iter().zip(b.sensitivities()) {
                assert_samples(left, right);
            }
            assert_curves(a.curves(), b.curves());
            assert_eq!(a.coupling(), b.coupling());
            assert_eq!(
                a.reference_log_exposure().map(f32::to_bits),
                b.reference_log_exposure().map(f32::to_bits)
            );
            match (a.dye_density(), b.dye_density()) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    for (left, right) in a.iter().zip(b) {
                        assert_samples(left, right);
                    }
                }
                _ => panic!("dye presence differs"),
            }
        }
        _ => panic!("film model kind differs"),
    }
}

fn assert_curves(left: &[Curve; 3], right: &[Curve; 3]) {
    for (left, right) in left.iter().zip(right) {
        assert_curve(left, right);
    }
}

fn assert_curve(left: &Curve, right: &Curve) {
    let left: Vec<_> = left.points().collect();
    let right: Vec<_> = right.points().collect();
    assert_eq!(left.len(), right.len());
    for (left, right) in left.into_iter().zip(right) {
        assert_eq!(left.0.to_bits(), right.0.to_bits());
        assert_eq!(left.1.to_bits(), right.1.to_bits());
    }
}

fn assert_samples(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    for (left, right) in left.iter().zip(right) {
        assert_eq!(left.to_bits(), right.to_bits(), "{left} vs {right}");
    }
}

/// Release timings for the latency section of `docs/UI_PLAN.md`.
///
/// Run with the fixtures present:
/// `RAWSTOCK_FIXTURES=/tmp/rawstock-fixtures cargo test -p rawstock-core --release --test session release_preview_timings -- --ignored --nocapture`
#[test]
#[ignore = "measures release timings against RAWSTOCK_FIXTURES"]
fn release_preview_timings() {
    let root = PathBuf::from(
        std::env::var("RAWSTOCK_FIXTURES").unwrap_or_else(|_| "/tmp/rawstock-fixtures".into()),
    );
    let stock = Stock::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml"),
    )
    .unwrap();
    for name in ["Fujifilm-X-S10-compressed.RAF", "IMG_1361.DNG"] {
        let path = root.join(name);
        let started = std::time::Instant::now();
        let raw = rawstock_core::decode::RawImage::open(&path).unwrap();
        let orientation = raw.orientation();
        let linear = raw.develop_linear().unwrap();
        eprintln!(
            "{name} decode+acescg: {:.3}s",
            started.elapsed().as_secs_f64()
        );
        let mut session = PreviewSession::new(linear, orientation, 720).unwrap();
        session.set_parameters(stock.parameters().clone());
        let mut control = NopControl;
        timed(&format!("{name} proxy 720 cold"), || {
            session.render_proxy(&mut control).unwrap()
        });
        session.set_grain_amount(1.25).unwrap();
        timed(&format!("{name} proxy 720 grain"), || {
            session.render_proxy(&mut control).unwrap()
        });
        session
            .set_exposure_ev(stock.parameters().exposure_ev() + 0.1)
            .unwrap();
        timed(&format!("{name} proxy 720 exposure"), || {
            session.render_proxy(&mut control).unwrap()
        });
        session.set_proxy_long_edge(1440).unwrap();
        timed(&format!("{name} proxy 1440 cold"), || {
            session.render_proxy(&mut control).unwrap()
        });
        session
            .set_print_exposure(stock.parameters().print_exposure() + 0.1)
            .unwrap();
        timed(&format!("{name} proxy 1440 print"), || {
            session.render_proxy(&mut control).unwrap()
        });
        let started = std::time::Instant::now();
        let image = session.render_full(&mut control).unwrap();
        let out = std::env::temp_dir().join(format!("rawstock-timing-{name}.jpg"));
        image.write(&out, Orientation::Normal).unwrap();
        eprintln!(
            "{name} full export: {:.3}s -> {}",
            started.elapsed().as_secs_f64(),
            out.display()
        );
    }
}

fn timed<T>(label: &str, body: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let value = body();
    eprintln!("{label}: {:.3}s", started.elapsed().as_secs_f64());
    value
}

#[test]
fn staged_session_previews_first_and_keeps_edits_when_the_full_frame_lands() {
    let full = frame(90, 60);
    let quick = QuickDevelop {
        image: full.area_downsample(30).unwrap(),
        full_width: 90,
        full_height: 60,
    };
    let mut session = PreviewSession::staged(quick, Orientation::Rotate90, 24).unwrap();
    session.set_parameters(parameters(true, true));
    assert!(session.is_staged());
    // Dimensions are the full frame's, turned by the camera tag.
    assert_eq!(session.sensor_dimensions(), (90, 60));
    assert_eq!(session.display_dimensions(), (60, 90));
    assert!(session.render_proxy(&mut NopControl).is_ok());
    let rect = DisplayRect {
        x: 4,
        y: 4,
        width: 16,
        height: 16,
    };
    assert!(
        session
            .render_region(rect, ScanMode::Exact, &mut NopControl)
            .is_err()
    );
    assert!(session.render_full(&mut NopControl).is_err());

    session.set_exposure_ev(0.75).unwrap();
    session.rotate_clockwise().unwrap();
    assert!(session.install_full(frame(60, 90)).is_err());
    session.install_full(full.clone()).unwrap();
    assert!(!session.is_staged());
    assert_eq!(session.display_dimensions(), (90, 60));
    assert_eq!(session.parameters().exposure_ev(), 0.75);

    // The same file opened on the full frame and edited the same way.
    let mut direct = PreviewSession::new(full, Orientation::Rotate90, 24).unwrap();
    direct.set_parameters(parameters(true, true));
    direct.set_exposure_ev(0.75).unwrap();
    direct.rotate_clockwise().unwrap();
    let staged = session
        .render_region(rect, ScanMode::Exact, &mut NopControl)
        .unwrap();
    let expected = direct
        .render_region(rect, ScanMode::Exact, &mut NopControl)
        .unwrap();
    assert_eq!(staged.pixels(), expected.pixels());
    let staged = session.render_proxy(&mut NopControl).unwrap();
    let expected = direct.render_proxy(&mut NopControl).unwrap();
    assert_eq!(staged.pixels(), expected.pixels());
}
