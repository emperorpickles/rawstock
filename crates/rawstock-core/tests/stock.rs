//! Stock files load into `Parameters` and reject malformed input.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rawstock_core::color::LinearImage;
use rawstock_core::render::{Parameters, Pipeline};
use rawstock_core::stock::{FilmModel, Grain, ModelError, Stock, StockError, StockModel};

fn neutral_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/neutral.stock.toml")
}

fn load_text(body: &str) -> Result<Stock, StockError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rawstock-stock-{}-{}.toml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, body).unwrap();
    let loaded = Stock::load(&path);
    let _ = std::fs::remove_file(&path);
    loaded
}

fn neutral_text() -> String {
    std::fs::read_to_string(neutral_path()).unwrap()
}

#[test]
fn example_stock_round_trips_into_parameters() {
    let stock = Stock::load(neutral_path()).unwrap();
    assert_eq!(stock.schema_version(), Stock::SCHEMA_VERSION);
    assert_eq!(stock.revision(), 2);
    assert_eq!(stock.name(), "Neutral");
    assert_eq!(
        stock.source(),
        "Synthetic reference written for rawstock. Red, green, and blue share one D-logH curve, and interlayer coupling is omitted. The curve is a view transform: middle gray stays near a straight sRGB encoding, highlights still shoulder, and a neutral stays neutral. It is not a measurement of a manufactured film."
    );
    assert_eq!(stock.license(), "CC0-1.0");
    assert_eq!(stock.model(), StockModel::Parametric);

    let parameters: &Parameters = stock.parameters();
    assert_eq!(parameters.exposure_ev(), 0.0);
    assert!((parameters.print_exposure() - 1.25).abs() < 1.0e-6);

    let FilmModel::Parametric(model) = parameters.film() else {
        panic!("neutral stock is parametric");
    };
    assert!(model.coupling().is_none());
    let knots = [-3.8_f32, -2.6, -1.6, -0.7447, 0.0, 0.5, 1.1];
    let density = [0.18_f32, 0.42, 0.72, 1.10, 1.70, 2.02, 2.28];
    for curve in model.curves() {
        assert_curve(curve, &knots, &density);
    }
    assert_curve(
        parameters.print_curve(),
        &[-1.6, -0.7, 0.15, 0.85, 1.55, 2.3],
        &[0.04, 0.12, 0.74, 1.70, 2.30, 2.55],
    );
    assert!(parameters.grain().is_none());
    assert!(parameters.halation().is_none());
}

#[test]
fn neutral_stock_holds_gray_and_color() {
    let stock = Stock::load(neutral_path()).unwrap();
    let pipeline = Pipeline::new(stock.parameters().clone());
    let code = |rgb: [f32; 3]| {
        let image = LinearImage::new(1, 1, rgb.to_vec()).unwrap();
        let bytes = pipeline.develop(&image).to_rgb8();
        [bytes[0], bytes[1], bytes[2]]
    };

    let gray = code([0.18, 0.18, 0.18]);
    assert_eq!(gray[0], gray[1]);
    assert_eq!(gray[1], gray[2]);
    assert!(
        (110..=126).contains(&gray[0]),
        "middle gray {gray:?} left the straight sRGB code"
    );
    let shadow = code([0.02, 0.02, 0.02]);
    assert!(shadow[0] < 75, "shadows are lifted: {shadow:?}");
    let white = code([1.0, 1.0, 1.0]);
    assert!(white[0] > 180, "highlights are flat: {white:?}");
    assert!(white[0] > gray[0] + 50, "gray {gray:?} white {white:?}");
    assert_eq!(white, [white[0], white[0], white[0]]);

    let red = code([0.083, 0.017, 0.009]);
    assert!(red[0] > red[1] + 25 && red[0] > red[2] + 25, "{red:?}");
    let blue = code([0.001, 0.002, 0.07]);
    assert!(blue[2] > blue[0] + 40 && blue[2] > blue[1] + 40, "{blue:?}");
}

#[test]
fn step_pairs_and_coupling_load() {
    // Whole-number steps, the way a densitometer table is often typed.
    let stock = load_text(
        r#"
schema_version = 1
revision = 3
name = "Coupled"
source = "Synthetic test stock."
license = "CC0-1.0"
model = "parametric"
exposure_ev = 0
print_exposure = 1

[red]
log_exposure = [-2, 2]
density = [0, 1]

[green]
log_exposure = [-2, 2]
density = [0.0, 1.0]

[blue]
log_exposure = [-2.0, 2.0]
density = [0, 1]

[print]
log_exposure = [-1, 1]
density = [0.0, 2]

[coupling]
matrix = [[1.0, 0.25, 0.0], [0.0, 1.0, 0.5], [0.0, 0.0, 1.0]]
"#,
    )
    .unwrap();
    assert_eq!(stock.revision(), 3);
    assert_eq!(stock.parameters().exposure_ev(), 0.0);
    assert_eq!(stock.parameters().print_exposure(), 1.0);
    let FilmModel::Parametric(model) = stock.parameters().film() else {
        panic!("coupled stock is parametric");
    };
    assert_eq!(
        model.coupling(),
        Some([[1.0, 0.25, 0.0], [0.0, 1.0, 0.5], [0.0, 0.0, 1.0]])
    );
    assert_eq!(model.curves()[0].eval(-2.0), 0.0);
    assert_eq!(model.curves()[0].eval(2.0), 1.0);
}

#[test]
fn rejects_a_non_monotone_curve() {
    let text = neutral_text().replacen(
        "density = [0.18, 0.42, 0.72, 1.10, 1.70, 2.02, 2.28]",
        "density = [0.18, 0.42, 0.10, 1.10, 1.70, 2.02, 2.28]",
        1,
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::Curve {
            curve: "red",
            source: ModelError::DensityOrder,
        }
    ));

    let text = neutral_text().replacen(
        "log_exposure = [-3.8, -2.6, -1.6, -0.7447, 0.0, 0.5, 1.1]",
        "log_exposure = [-3.8, -2.6, -1.6, -0.7447, -0.7447, 0.5, 1.1]",
        1,
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::Curve {
            curve: "red",
            source: ModelError::LogExposureOrder,
        }
    ));
}

#[test]
fn rejects_arrays_of_different_lengths() {
    let text = neutral_text().replacen(
        "log_exposure = [-3.8, -2.6, -1.6, -0.7447, 0.0, 0.5, 1.1]",
        "log_exposure = [-3.8, -2.6]",
        1,
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::ArrayLength {
            curve: "red",
            log_exposure: 2,
            density: 7,
        }
    ));
}

#[test]
fn grain_and_halation_tables_load_on_schema_1() {
    let stock = load_text(&format!("{}{GRAIN_AND_HALATION}", neutral_text())).unwrap();
    assert_eq!(stock.schema_version(), 1);
    let grain = stock.parameters().grain().expect("grain");
    assert_eq!(grain.frame_mm(), Grain::FRAME_MM);
    assert_eq!(grain.seed(), 7);
    assert_eq!(grain.size_mm(), Grain::DEFAULT_SIZE_MM);
    let red: Vec<_> = grain.layers()[0].points().collect();
    assert_eq!(red, vec![(0.0, 2.0), (1.0, 5.0), (2.0, 3.5)]);
    // RMS is allowed to fall after the peak. That is the Vision3 curve shape.
    assert!((grain.layers()[0].eval(1.0) - 5.0).abs() < 1.0e-5);
    assert!((grain.layers()[0].eval(1.5) - 4.25).abs() < 1.0e-4);
    assert!((grain.layers()[2].eval(0.0) - 4.0).abs() < 1.0e-5);

    let halation = stock.parameters().halation().expect("halation");
    assert!((halation.threshold() - 1.25).abs() < 1.0e-6);
    assert!((halation.radius() - 0.02).abs() < 1.0e-6);
    assert_eq!(halation.weight(), [0.8, 0.15, 0.05]);
    let short = 400u32;
    assert!((halation.sigma_px(short * 2, short) - 0.02 * short as f32).abs() < 1.0e-4);
}

#[test]
fn omitted_grain_frame_uses_35_mm() {
    let text = neutral_text() + GRAIN_AND_HALATION;
    let text = text.replace("frame_mm = 35\n", "");
    let stock = load_text(&text).unwrap();
    assert_eq!(
        stock.parameters().grain().unwrap().frame_mm(),
        Grain::FRAME_MM
    );
}

#[test]
fn grain_size_is_optional_per_layer() {
    let text = (neutral_text() + GRAIN_AND_HALATION).replace(
        "[grain.red]\ndensity = [0.0, 1.0, 2.0]",
        "[grain.red]\nsize_mm = 0.04\ndensity = [0.0, 1.0, 2.0]",
    );
    let stock = load_text(&text).unwrap();
    let size = stock.parameters().grain().unwrap().size_mm();
    assert!((size[0] - 0.04).abs() < 1.0e-6);
    assert_eq!(size[1], Grain::DEFAULT_SIZE_MM[1]);
    assert_eq!(size[2], Grain::DEFAULT_SIZE_MM[2]);

    let text = (neutral_text() + GRAIN_AND_HALATION).replace(
        "[grain.green]\ndensity = [0, 1, 2]",
        "[grain.green]\nsize_mm = 0\ndensity = [0, 1, 2]",
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(err, StockError::Model(ModelError::GrainSize)));
}

#[test]
fn rejects_a_falling_granularity_density() {
    let text = (neutral_text() + GRAIN_AND_HALATION).replace(
        "density = [0.0, 1.0, 2.0]\nrms = [2.0, 5.0, 3.5]",
        "density = [0.0, 1.0, 1.0]\nrms = [2.0, 5.0, 3.5]",
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::Grain {
            layer: "red",
            source: ModelError::GranularityOrder,
        }
    ));
}

#[test]
fn rejects_a_negative_halation_radius() {
    let text = (neutral_text() + GRAIN_AND_HALATION).replace("radius = 0.02", "radius = -0.02");
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::Halation(ModelError::HalationParameters)
    ));
}

const GRAIN_AND_HALATION: &str = r#"
[grain]
frame_mm = 35
seed = 7

[grain.red]
density = [0.0, 1.0, 2.0]
rms = [2.0, 5.0, 3.5]

[grain.green]
density = [0, 1, 2]
rms = [2, 4, 3]

[grain.blue]
density = [0.0, 2.0]
rms = [4.0, 4.0]

[halation]
threshold = 1.25
radius = 0.02
weight = [0.8, 0.15, 0.05]
"#;

#[test]
fn rejects_an_unknown_field() {
    let mut text = neutral_text();
    text.push_str("\ngrain = 0.0\n");
    let err = load_text(&text).unwrap_err();
    let StockError::Parse { source, .. } = err else {
        panic!("expected a parse error, got {err:?}");
    };
    assert!(
        source.to_string().contains("grain"),
        "unknown field was not named: {source}"
    );
}

#[test]
fn rejects_an_unknown_schema_version() {
    let text = neutral_text().replacen("schema_version = 1", "schema_version = 99", 1);
    let err = load_text(&text).unwrap_err();
    assert!(matches!(err, StockError::UnknownVersion { version: 99 }));
}

#[test]
fn rejects_an_unknown_model_tag() {
    let text = neutral_text().replacen("model = \"parametric\"", "model = \"emulsion\"", 1);
    let err = load_text(&text).unwrap_err();
    let StockError::Parse { source, .. } = err else {
        panic!("expected a parse error, got {err:?}");
    };
    assert!(
        source.to_string().contains("emulsion"),
        "unknown model was not named: {source}"
    );
}

#[test]
fn portra_400_loads_and_keeps_a_gray_near_neutral() {
    use rawstock_core::render::Pipeline;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let stock = Stock::load(&path).unwrap();
    assert_eq!(stock.model(), StockModel::Spectral);
    assert!(stock.source().contains("spectral_film_lut"));
    assert!(stock.source().contains("E-4050"));
    assert_eq!(stock.license(), "MIT");
    assert!(stock.parameters().grain().is_some());
    let halation = stock.parameters().halation().expect("portra halation");
    assert!(halation.weight()[0] > halation.weight()[1]);
    assert!(halation.weight()[1] >= halation.weight()[2]);

    let FilmModel::Spectral(model) = stock.parameters().film() else {
        panic!("portra is spectral");
    };
    assert!(model.wavelengths().len() >= 2);
    assert!(model.dye_density().is_some());
    let coupling = model.coupling().expect("portra coupling");
    for row in coupling {
        let sum = row.iter().sum::<f32>();
        assert!(
            (sum - 1.0).abs() < 1.0e-5,
            "coupling row {row:?} sums to {sum}"
        );
    }
    assert!(model.reference_log_exposure().is_some());
    for layer in model.sensitivities() {
        assert_eq!(layer.len(), model.wavelengths().len());
        assert!(
            layer
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0)
        );
        let integral = trapezoid(model.wavelengths(), layer);
        assert!(
            (integral - 1.0).abs() < 1.0e-3,
            "sensitivity integral {integral}"
        );
    }
    for layer in model.dye_density().unwrap() {
        assert!(
            layer
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0)
        );
    }

    let developed = Pipeline::new(stock.parameters().clone()).develop(&gray_image(0.18));
    let pixel = developed.pixels();
    let spread = pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2]);
    assert!(
        spread < 0.03,
        "Portra reference gray is not near neutral: {pixel:?}"
    );
    // The hand-tuned print curve holds this gray open, short of a crushed
    // shadow and short of a blown highlight.
    for &channel in pixel {
        assert!(
            (0.45..0.62).contains(&channel),
            "reference gray left the open midtone: {pixel:?}"
        );
    }
}

#[test]
fn velvia_is_a_parametric_approximation_with_fine_grain() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/velvia.stock.toml");
    let stock = Stock::load(&path).unwrap();
    assert_eq!(stock.name(), "Velvia");
    assert_eq!(stock.model(), StockModel::Parametric);
    let source = stock.source().to_ascii_lowercase();
    assert!(
        source.contains("approximat"),
        "source should say this is an approximation: {source}"
    );
    assert!(
        source.contains("not a 3d lut"),
        "source should say this is not a LUT: {source}"
    );
    // Velvia 50 is a finer film than Portra 400: lower rms in every layer
    // across the midtones, and smaller dye clouds.
    let grain = stock.parameters().grain().expect("velvia has grain");
    let portra_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let portra = Stock::load(&portra_path).unwrap();
    let portra_grain = portra.parameters().grain().unwrap();
    for layer in 0..3 {
        for density in [0.4, 0.8, 1.2, 1.7] {
            let velvia_rms = grain.layers()[layer].eval(density);
            let portra_rms = portra_grain.layers()[layer].eval(density);
            assert!(
                velvia_rms > 0.0 && velvia_rms < 0.5 * portra_rms,
                "layer {layer} at density {density}: {velvia_rms} vs Portra {portra_rms}"
            );
        }
        assert!(grain.size_mm()[layer] < portra_grain.size_mm()[layer]);
    }
    assert!(stock.parameters().halation().is_none());
    let FilmModel::Parametric(model) = stock.parameters().film() else {
        panic!("velvia is parametric");
    };
    assert!(model.coupling().is_some());
    let developed = Pipeline::new(stock.parameters().clone()).develop(&gray_image(0.18));
    assert_eq!(developed.pixels().len(), 3);
    for channel in developed.pixels() {
        assert!(channel.is_finite(), "{channel}");
    }
}

fn gray_image(level: f32) -> LinearImage {
    LinearImage::new(1, 1, vec![level, level, level]).unwrap()
}

fn trapezoid(wavelengths: &[f32], values: &[f32]) -> f32 {
    let mut total = 0.0f32;
    for index in 0..wavelengths.len() - 1 {
        let half = 0.5 * (wavelengths[index + 1] - wavelengths[index]);
        total += half * (values[index] + values[index + 1]);
    }
    total
}

fn assert_curve(curve: &rawstock_core::stock::Curve, log_exposure: &[f32], density: &[f32]) {
    assert_eq!(log_exposure.len(), density.len());
    for (exposure, value) in log_exposure.iter().zip(density) {
        let got = curve.eval(*exposure);
        assert!(
            (got - value).abs() < 1.0e-5,
            "log exposure {exposure}: density {got} != {value}"
        );
    }
}
