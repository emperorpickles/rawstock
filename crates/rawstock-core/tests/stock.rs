//! Stock files load into `Parameters` and reject malformed input.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rawstock_core::color::LinearImage;
use rawstock_core::render::{Parameters, Pipeline};
use rawstock_core::stock::{
    Emulsion, FilmModel, Grain, Granularity, ModelError, Process, Stock, StockError, StockModel,
};

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
    // Read through Status M and printed on Portra Endura's own dyes.
    assert!(model.scan_responses().is_some());
    assert!(model.print_dye_density().is_some());
    assert!(model.coupling().is_none());
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
    // The paper's laboratory aim prints middle gray as an 18 % reflector,
    // about 0.46 once sRGB-encoded.
    for &channel in pixel {
        assert!(
            (0.42..0.50).contains(&channel),
            "reference gray missed the paper aim: {pixel:?}"
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
    // Velvia 50 is a finer film than Portra 400: smaller dye clouds, and a
    // developed middle gray shows well under Portra's luma noise. The two
    // print their density through curves of different contrast, so the
    // comparison is on the developed image, not on the rms tables.
    let grain = stock.parameters().grain().expect("velvia has grain");
    let portra_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let portra = Stock::load(&portra_path).unwrap();
    let portra_grain = portra.parameters().grain().unwrap();
    for layer in 0..3 {
        assert!(grain.layers()[layer].eval(1.2) > 0.0);
        assert!(grain.size_mm()[layer] < portra_grain.size_mm()[layer]);
    }
    let velvia_noise = gray_luma_noise(stock.parameters());
    let portra_noise = gray_luma_noise(portra.parameters());
    assert!(
        velvia_noise > 0.0 && velvia_noise < 0.6 * portra_noise,
        "middle-gray luma noise {velvia_noise} vs Portra {portra_noise}"
    );
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

#[test]
fn spectral_velvia_is_a_reversal_stock_beside_the_parametric_one() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks");
    let stock = Stock::load(root.join("velvia50-spectral.stock.toml")).unwrap();
    let parametric = Stock::load(root.join("velvia.stock.toml")).unwrap();
    assert_ne!(stock.name(), parametric.name());
    assert_eq!(stock.model(), StockModel::Spectral);
    assert_eq!(stock.license(), "MIT");
    assert!(stock.source().contains("spectral_film_lut"));
    assert!(stock.source().to_ascii_lowercase().contains("approximate"));

    let FilmModel::Spectral(model) = stock.parameters().film() else {
        panic!("spectral velvia is spectral");
    };
    assert_eq!(model.process(), Process::Reversal);
    assert!(model.curves().iter().all(|curve| curve.falls()));
    assert!(model.dye_density().is_some());
    assert!(model.reference_log_exposure().is_some());
    assert!(model.neutral_scale().is_some());
    for row in model.coupling().expect("velvia coupling") {
        let sum = row.iter().sum::<f32>();
        assert!(
            (sum - 1.0).abs() < 1.0e-5,
            "coupling row {row:?} sums to {sum}"
        );
    }
    for layer in model.sensitivities() {
        let integral = trapezoid(model.wavelengths(), layer);
        assert!(
            (integral - 1.0).abs() < 1.0e-3,
            "sensitivity integral {integral}"
        );
    }

    // The grain is the parametric Velvia's table, carried over unchanged.
    let grain = stock.parameters().grain().expect("spectral velvia grain");
    let carried = parametric.parameters().grain().unwrap();
    assert_eq!(grain.size_mm(), carried.size_mm());
    for (layer, other) in grain.layers().iter().zip(carried.layers()) {
        assert!(layer.points().eq(other.points()));
    }

    // A slide is a positive: more light develops brighter, and with the
    // neutral scale every gray stays neutral, shadows to highlights.
    let mut parameters = stock.parameters().clone();
    parameters.set_grain(None).unwrap();
    let pipeline = Pipeline::new(parameters.clone());
    let mut previous = 0.0f32;
    for level in [0.005, 0.02, 0.06, 0.18, 0.5] {
        let pixel = pipeline.develop(&gray_image(level)).pixels().to_vec();
        let mean = pixel.iter().sum::<f32>() / 3.0;
        assert!(mean > previous, "gray {level} developed to {pixel:?}");
        previous = mean;
        let spread = pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2]);
        assert!(spread < 0.01, "gray {level} is not neutral: {pixel:?}");
    }
    // Placed like the parametric Velvia: a camera's middle gray, near
    // scene-linear 0.08 in rawstock's RAF decode, prints in the midtones.
    let reference = Pipeline::new(parametric.parameters().clone()).develop(&gray_image(0.08));
    let gray = pipeline.develop(&gray_image(0.08)).pixels().to_vec();
    for (&channel, &other) in gray.iter().zip(reference.pixels()) {
        assert!(
            (channel - other).abs() < 0.05,
            "middle gray {gray:?} is far from the parametric {:?}",
            reference.pixels()
        );
    }
}

#[test]
fn process_is_a_spectral_key_that_must_match_the_curves() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks");
    let reversal = std::fs::read_to_string(root.join("velvia50-spectral.stock.toml")).unwrap();

    // Falling curves without the key are read as a negative and rejected.
    let unmarked = reversal.replace("process = \"reversal\"\n", "");
    assert!(matches!(
        load_text(&unmarked).unwrap_err(),
        StockError::Curve {
            curve: "red",
            source: ModelError::DensityOrder
        }
    ));

    // A reversal key on rising curves is rejected.
    let portra = std::fs::read_to_string(root.join("portra400.stock.toml")).unwrap();
    let marked = portra.replace(
        "model = \"spectral\"\n",
        "model = \"spectral\"\nprocess = \"reversal\"\n",
    );
    assert!(matches!(
        load_text(&marked).unwrap_err(),
        StockError::Curve {
            curve: "red",
            source: ModelError::ReversalDensityOrder
        }
    ));

    // Parametric stocks are negatives and do not take the key.
    let parametric = neutral_text().replace(
        "model = \"parametric\"\n",
        "model = \"parametric\"\nprocess = \"negative\"\n",
    );
    assert!(matches!(
        load_text(&parametric).unwrap_err(),
        StockError::NotParametric { field: "process" }
    ));

    // The neutral scale is spectral only, and needs dye curves.
    let parametric = neutral_text().replace(
        "model = \"parametric\"\n",
        "model = \"parametric\"\nneutral_scale = true\n",
    );
    assert!(matches!(
        load_text(&parametric).unwrap_err(),
        StockError::NotParametric {
            field: "neutral_scale"
        }
    ));

    // Unknown processes fail to parse.
    let unknown = reversal.replace("process = \"reversal\"", "process = \"positive\"");
    assert!(matches!(
        load_text(&unknown).unwrap_err(),
        StockError::Parse { .. }
    ));
}

#[test]
fn scan_responses_and_print_dyes_are_spectral_keys() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks");
    let portra = Stock::load(root.join("portra400.stock.toml")).unwrap();
    let FilmModel::Spectral(model) = portra.parameters().film() else {
        panic!("portra is spectral");
    };

    // Both tables survive the writer.
    let written = load_text(&portra.to_toml()).unwrap();
    let FilmModel::Spectral(reread) = written.parameters().film() else {
        panic!("portra is spectral");
    };
    assert_eq!(reread.scan_responses(), model.scan_responses());
    assert_eq!(reread.print_dye_density(), model.print_dye_density());

    // A scan needs dyes to read, and a response needs some weight.
    let responses = model.scan_responses().unwrap().clone();
    let bare = rawstock_core::stock::Spectral::new(
        model.wavelengths().to_vec(),
        model.sensitivities().clone(),
        None,
        model.curves().clone(),
        None,
        None,
    )
    .unwrap();
    assert!(matches!(
        bare.clone()
            .with_scan_responses(responses.clone())
            .unwrap_err(),
        ModelError::ScanWithoutDye
    ));
    let dyed = rawstock_core::stock::Spectral::new(
        model.wavelengths().to_vec(),
        model.sensitivities().clone(),
        model.dye_density().cloned(),
        model.curves().clone(),
        None,
        None,
    )
    .unwrap();
    let mut dark = responses.clone();
    dark[1] = vec![0.0; dark[1].len()];
    assert!(matches!(
        dyed.with_scan_responses(dark).unwrap_err(),
        ModelError::ScanResponse
    ));
    let mut negative = model.print_dye_density().unwrap().clone();
    negative[2][0] = -0.1;
    assert!(matches!(
        bare.with_print_dyes(negative).unwrap_err(),
        ModelError::PrintDyeDensity
    ));

    // Parametric stocks take neither table.
    let samples = "red = [1.0]\ngreen = [1.0]\nblue = [1.0]\n";
    for table in ["scan_response", "print_dye_density"] {
        let parametric = format!("{}\n[{table}]\n{samples}", neutral_text());
        match load_text(&parametric).unwrap_err() {
            StockError::NotParametric { field } => assert_eq!(field, table),
            other => panic!("{table}: {other}"),
        }
    }
}

#[test]
fn portra_paper_white_is_display_white() {
    // A scene far above middle gray leaves a dense negative, the paper
    // unexposed, and the print at its own white.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let mut parameters = Stock::load(&path).unwrap().parameters().clone();
    parameters.set_exposure_ev(0.0).unwrap();
    let developed = Pipeline::new(parameters).develop(&gray_image(4000.0));
    for &channel in developed.pixels() {
        assert!(channel > 0.99, "paper white: {:?}", developed.pixels());
    }
}

fn hp5_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/ilford-hp5-plus.stock.toml")
}

#[test]
fn hp5_is_one_silver_layer() {
    let stock = Stock::load(hp5_path()).unwrap();
    assert_eq!(stock.name(), "Ilford HP5 Plus");
    assert!(stock.source().starts_with("Approximate"));
    let FilmModel::Spectral(model) = stock.parameters().film() else {
        panic!("HP5 Plus is spectral");
    };
    assert_eq!(model.emulsion(), Emulsion::Monochrome);
    assert_eq!(model.process(), Process::Negative);
    assert!(model.dye_density().is_none());
    assert!(model.coupling().is_none());
    let [red, green, blue] = model.sensitivities();
    assert_eq!(red, green);
    assert_eq!(green, blue);
    assert!((trapezoid(model.wavelengths(), red) - 1.0).abs() < 1.0e-3);
    let grain = stock.parameters().grain().unwrap();
    assert!(grain.is_monochrome());
    assert_eq!(grain.size_mm()[0], grain.size_mm()[1]);
    assert_eq!(grain.size_mm()[1], grain.size_mm()[2]);
}

#[test]
fn hp5_develops_every_color_to_a_neutral() {
    // Saturated primaries, a skin tone, and grays, with grain and halation
    // on. Silver has no color, so every output pixel has equal channels.
    let parameters = Stock::load(hp5_path()).unwrap().into_parameters();
    let (width, height) = (64u32, 48u32);
    let colors = [
        [0.6, 0.02, 0.02],
        [0.02, 0.5, 0.03],
        [0.02, 0.03, 0.7],
        [0.35, 0.2, 0.12],
        [0.18, 0.18, 0.18],
        [4.0, 3.0, 2.0],
    ];
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for index in 0..width * height {
        pixels.extend(colors[(index / 7) as usize % colors.len()]);
    }
    let image = LinearImage::new(width, height, pixels).unwrap();
    let developed = Pipeline::new(parameters).develop(&image);
    for pixel in developed.pixels().as_chunks::<3>().0 {
        assert_eq!(pixel[0], pixel[1], "{pixel:?}");
        assert_eq!(pixel[1], pixel[2], "{pixel:?}");
    }
}

#[test]
fn hp5_sees_red_less_than_blue() {
    // Panchromatic, with the red edge near 670 nm: deep red prints darker
    // than an equally bright deep blue, as on real HP5 Plus.
    let parameters = Stock::load(hp5_path()).unwrap().into_parameters();
    let film = parameters.film();
    let density = film.density(&[0.3, 0.0, 0.0, 0.0, 0.0, 0.3]);
    assert!(density[0] < density[3], "{density:?}");
    // All three layers carry the one silver density.
    assert_eq!(density[0], density[1]);
    assert_eq!(density[3], density[5]);
}

#[test]
fn monochrome_stock_tables_are_checked() {
    let hp5 = std::fs::read_to_string(hp5_path()).unwrap();
    let curve = "log_exposure = [-2.0, 0.0]\ndensity = [0.0, 1.0]\n";

    // Color-only tables do not belong on a monochrome stock.
    for (field, table) in [
        ("red", format!("[red]\n{curve}")),
        (
            "coupling",
            "[coupling]\nmatrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]]\n".to_string(),
        ),
        ("neutral_scale", "neutral_scale = true\n".to_string()),
    ] {
        let text = if table.starts_with('[') {
            format!("{hp5}\n{table}")
        } else {
            hp5.replacen(
                "model = \"spectral\"",
                &format!("model = \"spectral\"\n{table}"),
                1,
            )
        };
        match load_text(&text).unwrap_err() {
            StockError::WrongLayer {
                emulsion,
                field: got,
            } => {
                assert_eq!(emulsion, Emulsion::Monochrome);
                assert_eq!(got, field);
            }
            other => panic!("{field}: {other}"),
        }
    }

    // The silver curve is required.
    let start = hp5.find("[silver]").unwrap();
    let end = start + hp5[start..].find("\n\n").unwrap();
    let missing = format!("{}{}", &hp5[..start], &hp5[end..]);
    assert!(matches!(
        load_text(&missing).unwrap_err(),
        StockError::MissingLayer {
            field: "silver",
            ..
        }
    ));

    // Silver grain has one table, not three.
    let colored_grain = hp5.replace("[grain.silver]", "[grain.red]");
    assert!(matches!(
        load_text(&colored_grain).unwrap_err(),
        StockError::WrongLayer {
            field: "grain.red",
            ..
        }
    ));

    // A color stock cannot carry a silver layer, and a parametric stock
    // has no emulsion key.
    let colored = format!("{}\n[silver]\n{curve}", neutral_text());
    assert!(matches!(
        load_text(&colored).unwrap_err(),
        StockError::NotParametric { .. } | StockError::WrongLayer { .. }
    ));
    let parametric = neutral_text().replace(
        "model = \"parametric\"",
        "model = \"parametric\"\nemulsion = \"monochrome\"",
    );
    assert!(matches!(
        load_text(&parametric).unwrap_err(),
        StockError::NotParametric { field: "emulsion" }
    ));
}

#[test]
fn monochrome_grain_is_the_same_in_every_channel() {
    let layer = Granularity::new([(0.0, 10.0), (2.0, 18.0)]).unwrap();
    let grain = Grain::monochrome(35.0, 9, layer, 0.006).unwrap();
    assert!(grain.is_monochrome());
    let (width, height) = (96u32, 64u32);
    let mut density: Vec<f32> = (0..width * height)
        .flat_map(|index| {
            let level = 0.2 + 1.6 * (index % width) as f32 / width as f32;
            [level; 3]
        })
        .collect();
    let before = density.clone();
    grain.apply(&mut density, width, height);
    assert_ne!(density, before);
    for pixel in density.as_chunks::<3>().0 {
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
    }
    // One clump size for all three channels.
    assert!(matches!(
        grain.with_size_mm([0.006, 0.006, 0.008]).unwrap_err(),
        ModelError::GrainSize
    ));
}

/// Standard deviation of developed luma over a flat middle gray.
///
/// The frame is 2048 pixels wide, so a pixel is finer than the 48 µm
/// granularity aperture and the grain is not averaged away.
fn gray_luma_noise(parameters: &Parameters) -> f32 {
    let (width, height) = (2048u32, 96u32);
    let image = LinearImage::new(width, height, vec![0.18; (width * height * 3) as usize]).unwrap();
    let developed = Pipeline::new(parameters.clone()).develop(&image);
    let luma: Vec<f32> = developed
        .pixels()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|pixel| 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2])
        .collect();
    let mean = luma.iter().sum::<f32>() / luma.len() as f32;
    let variance = luma.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / luma.len() as f32;
    variance.sqrt()
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
