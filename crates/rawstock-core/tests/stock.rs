//! Stock files load into `Parameters` and reject malformed input.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rawstock_core::render::Parameters;
use rawstock_core::stock::{FilmModel, ModelError, Stock, StockError, StockModel};

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
    assert_eq!(stock.revision(), 1);
    assert_eq!(stock.name(), "Neutral");
    assert_eq!(
        stock.source(),
        "Synthetic reference written for rawstock. Red, green, and blue share one D-logH curve, and interlayer coupling is omitted. The numbers are a mild negative toe and shoulder, not a measurement of a manufactured film."
    );
    assert_eq!(stock.license(), "CC0-1.0");
    assert_eq!(stock.model(), StockModel::Parametric);

    let parameters: &Parameters = stock.parameters();
    assert_eq!(parameters.exposure_ev(), 0.0);
    assert!((parameters.print_exposure() - 0.9).abs() < 1.0e-6);

    let FilmModel::Parametric(model) = parameters.film();
    assert!(model.coupling().is_none());
    let knots = [-3.5_f32, -2.0, -0.7, 0.0, 0.6, 1.5];
    let density = [0.15_f32, 0.30, 0.70, 1.15, 1.70, 2.05];
    for curve in model.curves() {
        assert_curve(curve, &knots, &density);
    }
    assert_curve(
        parameters.print_curve(),
        &[-1.5, -0.5, 0.2, 1.0, 2.0],
        &[0.04, 0.20, 0.70, 1.50, 2.20],
    );
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
    let FilmModel::Parametric(model) = stock.parameters().film();
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
        "density = [0.15, 0.30, 0.70, 1.15, 1.70, 2.05]",
        "density = [0.15, 0.30, 0.10, 1.15, 1.70, 2.05]",
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
        "log_exposure = [-3.5, -2.0, -0.7, 0.0, 0.6, 1.5]",
        "log_exposure = [-3.5, -2.0, -0.7, -0.7, 0.6, 1.5]",
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
        "log_exposure = [-3.5, -2.0, -0.7, 0.0, 0.6, 1.5]",
        "log_exposure = [-3.5, -2.0]",
        1,
    );
    let err = load_text(&text).unwrap_err();
    assert!(matches!(
        err,
        StockError::ArrayLength {
            curve: "red",
            log_exposure: 2,
            density: 6,
        }
    ));
}

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
    let text = neutral_text().replacen("model = \"parametric\"", "model = \"spectral\"", 1);
    let err = load_text(&text).unwrap_err();
    let StockError::Parse { source, .. } = err else {
        panic!("expected a parse error, got {err:?}");
    };
    assert!(
        source.to_string().contains("spectral"),
        "unknown model was not named: {source}"
    );
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
