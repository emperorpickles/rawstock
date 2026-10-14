//! Film model: scene exposure to per-layer density, and the stock files that
//! fill [`Parameters`](crate::render::Parameters).
//!
//! Spectral (wavelength sensitivity and dye density) is intentionally not a
//! variant yet. The only model is [`FilmModel::Parametric`]. A stock file
//! names that choice with [`StockModel`], so a later `spectral` tag can be
//! added without changing the `parametric` tag existing files already use.

use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, SeqAccess, Visitor};
use thiserror::Error;

use crate::render::Parameters;

/// A monotone D-logH characteristic curve.
///
/// Points are `(log10(exposure), density)`. Log-exposure must strictly
/// increase and density must not decrease. Evaluation clamps to the end
/// densities outside the first and last points.
#[derive(Debug, Clone)]
pub struct Curve {
    log_h: Vec<f32>,
    density: Vec<f32>,
}

/// Per-layer parametric film.
///
/// Each layer has its own [`Curve`]. When `coupling` is set, that 3×3 matrix
/// multiplies the log-exposure vector before the curves run. `None` leaves
/// the layers independent.
#[derive(Debug, Clone)]
pub struct Parametric {
    curves: [Curve; 3],
    coupling: Option<[[f32; 3]; 3]>,
}

/// Maps scene-linear exposure to per-layer density.
#[derive(Debug, Clone)]
pub enum FilmModel {
    /// Per-layer D-logH curves and optional log-exposure coupling.
    Parametric(Parametric),
}

/// A film or print parameter that cannot be evaluated.
#[derive(Debug, Error)]
pub enum ModelError {
    /// Fewer than two control points.
    #[error("characteristic curve needs at least two points")]
    TooFewPoints,

    /// A log-exposure coordinate was not strictly greater than the one before it.
    #[error("characteristic curve log-exposure must strictly increase")]
    LogExposureOrder,

    /// A density sample was lower than the one before it.
    #[error("characteristic curve density must not decrease")]
    DensityOrder,

    /// A control point was NaN or infinite.
    #[error("characteristic curve has a non-finite control point")]
    NonFinitePoint,

    /// The coupling matrix contained NaN or infinity.
    #[error("interlayer coupling matrix has a non-finite entry")]
    NonFiniteCoupling,

    /// Exposure, in stops or print log-exposure, was NaN or infinite.
    #[error("exposure must be finite")]
    NonFiniteExposure,
}

/// Exposures at or below this are lifted before `log10`.
///
/// Scene-linear ACEScg can be zero or negative. A characteristic curve is
/// defined on positive exposure, so those samples sit on the toe.
const EXPOSURE_FLOOR: f32 = 1.0e-6;

impl Curve {
    /// Build a curve from `(log10(exposure), density)` points, in order.
    pub fn new(points: impl IntoIterator<Item = (f32, f32)>) -> Result<Self, ModelError> {
        let mut log_h = Vec::new();
        let mut density = Vec::new();
        for (exposure, value) in points {
            if !exposure.is_finite() || !value.is_finite() {
                return Err(ModelError::NonFinitePoint);
            }
            if let Some(previous) = log_h.last().copied() {
                if exposure <= previous {
                    return Err(ModelError::LogExposureOrder);
                }
                if value < density[density.len() - 1] {
                    return Err(ModelError::DensityOrder);
                }
            }
            log_h.push(exposure);
            density.push(value);
        }
        if log_h.len() < 2 {
            return Err(ModelError::TooFewPoints);
        }
        Ok(Self { log_h, density })
    }

    /// Density at `log_h`, clamped to the end points outside the knots.
    pub fn eval(&self, log_h: f32) -> f32 {
        let log_h = if log_h.is_finite() {
            log_h
        } else {
            self.log_h[0]
        };
        if log_h <= self.log_h[0] {
            return self.density[0];
        }
        if log_h >= self.log_h[self.log_h.len() - 1] {
            return self.density[self.density.len() - 1];
        }
        let upper = self.log_h.partition_point(|point| *point <= log_h);
        let lower = upper - 1;
        let span = self.log_h[upper] - self.log_h[lower];
        let weight = (log_h - self.log_h[lower]) / span;
        self.density[lower] + weight * (self.density[upper] - self.density[lower])
    }
}

impl Parametric {
    /// `curves` are red, green, and blue, in that order.
    ///
    /// `coupling` is row-major and multiplies the log-exposure column vector.
    pub fn new(curves: [Curve; 3], coupling: Option<[[f32; 3]; 3]>) -> Result<Self, ModelError> {
        if let Some(matrix) = coupling
            && matrix.iter().flatten().any(|entry| !entry.is_finite())
        {
            return Err(ModelError::NonFiniteCoupling);
        }
        Ok(Self { curves, coupling })
    }

    /// The red, green, and blue characteristic curves.
    pub fn curves(&self) -> &[Curve; 3] {
        &self.curves
    }

    /// Log-exposure mixing, when the layers are not independent.
    pub fn coupling(&self) -> Option<[[f32; 3]; 3]> {
        self.coupling
    }

    fn density(&self, exposure: &[f32]) -> Vec<f32> {
        let mut density = Vec::with_capacity(exposure.len());
        for pixel in exposure.as_chunks::<3>().0 {
            let mut log_h = pixel.map(|channel| channel.max(EXPOSURE_FLOOR).log10());
            if let Some(matrix) = self.coupling {
                log_h = mix(matrix, log_h);
            }
            density.extend([
                self.curves[0].eval(log_h[0]),
                self.curves[1].eval(log_h[1]),
                self.curves[2].eval(log_h[2]),
            ]);
        }
        density
    }
}

impl FilmModel {
    /// Per-layer density for interleaved scene-linear RGB exposure.
    ///
    /// The result has the same length as `exposure`. A trailing incomplete
    /// pixel is omitted.
    pub fn density(&self, exposure: &[f32]) -> Vec<f32> {
        match self {
            Self::Parametric(model) => model.density(exposure),
        }
    }
}

/// On-disk model tag.
///
/// `Spectral` is not a variant yet. Existing files say `parametric`, and that
/// tag stays valid when a spectral variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StockModel {
    /// Per-layer D-logH curves and optional log-exposure coupling.
    Parametric,
}

/// A stock file loaded into [`Parameters`].
///
/// Metadata stays here. The curves, coupling, exposure, and print stage are
/// the existing [`Parameters`] value, not a second parameter type.
#[derive(Debug, Clone)]
pub struct Stock {
    schema_version: u32,
    revision: u32,
    name: String,
    source: String,
    license: String,
    model: StockModel,
    parameters: Parameters,
}

/// A stock file could not be loaded.
#[derive(Debug, Error)]
pub enum StockError {
    /// The file could not be read.
    #[error("failed to read stock file {path}")]
    Read {
        /// The path that failed.
        path: String,
        /// The filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// TOML parsing failed, including an unknown field or model tag.
    #[error("failed to parse stock file {path}")]
    Parse {
        /// The path that failed.
        path: String,
        /// The TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// `schema_version` is not a version this build understands.
    #[error("unknown stock schema version {version}")]
    UnknownVersion {
        /// The version stored in the file.
        version: u32,
    },

    /// `name`, `source`, or `license` was missing its text.
    #[error("stock {field} is empty")]
    Empty {
        /// Which field was empty.
        field: &'static str,
    },

    /// A curve's log-exposure and density columns differ in length.
    #[error("{curve} log-exposure and density arrays have lengths {log_exposure} and {density}")]
    ArrayLength {
        /// `red`, `green`, `blue`, or `print`.
        curve: &'static str,
        /// Length of `log_exposure`.
        log_exposure: usize,
        /// Length of `density`.
        density: usize,
    },

    /// A curve was not a monotone characteristic curve.
    #[error("{curve} characteristic curve is invalid")]
    Curve {
        /// `red`, `green`, `blue`, or `print`.
        curve: &'static str,
        /// Why the curve was rejected.
        #[source]
        source: ModelError,
    },

    /// Coupling or exposure failed the film-model checks.
    #[error(transparent)]
    Model(ModelError),
}

/// Parser input. Not a second public parameter type: [`Stock::load`] turns it
/// into [`Parameters`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StockFile {
    schema_version: u32,
    revision: u32,
    name: String,
    source: String,
    license: String,
    model: StockModel,
    #[serde(deserialize_with = "de_f32")]
    exposure_ev: f32,
    #[serde(deserialize_with = "de_f32")]
    print_exposure: f32,
    red: CurveSamples,
    green: CurveSamples,
    blue: CurveSamples,
    print: CurveSamples,
    #[serde(default)]
    coupling: Option<CouplingSamples>,
}

/// Optional interlayer coupling. Absent means the layers stay independent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CouplingSamples {
    /// Row-major 3×3. Multiplies the log-exposure column vector.
    matrix: [[f32; 3]; 3],
}

/// One measured curve: index `i` is the step-wedge pair
/// `(log_exposure[i], density[i])`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurveSamples {
    #[serde(deserialize_with = "de_f32_vec")]
    log_exposure: Vec<f32>,
    #[serde(deserialize_with = "de_f32_vec")]
    density: Vec<f32>,
}

impl Stock {
    /// Schema this build reads. A file with any other `schema_version` is rejected.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Read a stock file and validate it into [`Parameters`].
    ///
    /// Each curve's log-exposure and density arrays must be the same length.
    /// Log-exposure must strictly increase and density must not decrease.
    /// `schema_version` must be [`Self::SCHEMA_VERSION`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, StockError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| StockError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml(&text, path)
    }

    /// Schema version of the file. Equal to [`Self::SCHEMA_VERSION`] for a loaded stock.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// The stock's own revision, independent of [`Self::schema_version`].
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the numbers came from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// License of those numbers.
    pub fn license(&self) -> &str {
        &self.license
    }

    /// Model tag from the file.
    pub fn model(&self) -> StockModel {
        self.model
    }

    /// Development parameters stored in the file.
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }

    /// Take the development parameters out of the stock.
    pub fn into_parameters(self) -> Parameters {
        self.parameters
    }

    fn from_toml(text: &str, path: &Path) -> Result<Self, StockError> {
        let file: StockFile = toml::from_str(text).map_err(|source| StockError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        if file.schema_version != Self::SCHEMA_VERSION {
            return Err(StockError::UnknownVersion {
                version: file.schema_version,
            });
        }
        let name = require_text("name", &file.name)?.to_string();
        let source = require_text("source", &file.source)?.to_string();
        let license = require_text("license", &file.license)?.to_string();
        let red = curve_from_samples("red", &file.red)?;
        let green = curve_from_samples("green", &file.green)?;
        let blue = curve_from_samples("blue", &file.blue)?;
        let print = curve_from_samples("print", &file.print)?;
        let coupling = file.coupling.map(|coupling| coupling.matrix);
        let film = match file.model {
            StockModel::Parametric => FilmModel::Parametric(
                Parametric::new([red, green, blue], coupling).map_err(StockError::Model)?,
            ),
        };
        let parameters = Parameters::new(file.exposure_ev, film, file.print_exposure, print)
            .map_err(StockError::Model)?;
        Ok(Self {
            schema_version: file.schema_version,
            revision: file.revision,
            name,
            source,
            license,
            model: file.model,
            parameters,
        })
    }
}

fn require_text<'a>(field: &'static str, value: &'a str) -> Result<&'a str, StockError> {
    if value.trim().is_empty() {
        Err(StockError::Empty { field })
    } else {
        Ok(value)
    }
}

fn curve_from_samples(curve: &'static str, samples: &CurveSamples) -> Result<Curve, StockError> {
    if samples.log_exposure.len() != samples.density.len() {
        return Err(StockError::ArrayLength {
            curve,
            log_exposure: samples.log_exposure.len(),
            density: samples.density.len(),
        });
    }
    let points = samples
        .log_exposure
        .iter()
        .copied()
        .zip(samples.density.iter().copied());
    Curve::new(points).map_err(|source| StockError::Curve { curve, source })
}

fn de_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(F32Visitor)
}

fn de_f32_vec<'de, D>(deserializer: D) -> Result<Vec<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(F32VecVisitor)
}

struct F32Visitor;

impl Visitor<'_> for F32Visitor {
    type Value = f32;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a number")
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<f32, E> {
        Ok(value as f32)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<f32, E> {
        Ok(value as f32)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<f32, E> {
        Ok(value as f32)
    }
}

struct F32VecVisitor;

impl<'de> Visitor<'de> for F32VecVisitor {
    type Value = Vec<f32>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an array of numbers")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<f32>, A::Error> {
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(value) = seq.next_element_seed(F32Seed)? {
            values.push(value);
        }
        Ok(values)
    }
}

struct F32Seed;

impl<'de> DeserializeSeed<'de> for F32Seed {
    type Value = f32;

    fn deserialize<D>(self, deserializer: D) -> Result<f32, D::Error>
    where
        D: Deserializer<'de>,
    {
        de_f32(deserializer)
    }
}

fn mix(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * value[0] + matrix[0][1] * value[1] + matrix[0][2] * value[2],
        matrix[1][0] * value[0] + matrix[1][1] * value[1] + matrix[1][2] * value[2],
        matrix[2][0] * value[0] + matrix[2][1] * value[1] + matrix[2][2] * value[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_curve() -> Curve {
        Curve::new([(-3.0, 0.1), (-1.0, 0.4), (0.0, 1.2), (1.0, 2.0), (2.0, 2.0)]).unwrap()
    }

    #[test]
    fn characteristic_curve_is_monotone() {
        let curve = sample_curve();
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=200 {
            let log_h = -4.0 + step as f32 * 0.035;
            let density = curve.eval(log_h);
            assert!(
                density >= previous,
                "density fell from {previous} to {density} at logH {log_h}"
            );
            previous = density;
        }
        assert_eq!(curve.eval(-10.0), 0.1);
        assert_eq!(curve.eval(10.0), 2.0);
    }

    #[test]
    fn flat_shoulder_is_monotone() {
        let curve = sample_curve();
        assert_eq!(curve.eval(1.0), curve.eval(1.5));
        assert_eq!(curve.eval(1.5), curve.eval(2.0));
    }

    #[test]
    fn decreasing_density_is_rejected() {
        let err = Curve::new([(0.0, 1.0), (1.0, 0.5)]).unwrap_err();
        assert!(matches!(err, ModelError::DensityOrder));
    }

    #[test]
    fn unordered_log_exposure_is_rejected() {
        let err = Curve::new([(1.0, 0.2), (1.0, 0.3)]).unwrap_err();
        assert!(matches!(err, ModelError::LogExposureOrder));
    }

    #[test]
    fn parametric_density_rises_with_exposure() {
        let curve = sample_curve();
        let model = FilmModel::Parametric(
            Parametric::new([curve.clone(), curve.clone(), curve], None).unwrap(),
        );
        let mut previous = 0.0;
        for step in 0..40 {
            let exposure = 10f32.powf(-3.0 + step as f32 * 0.12);
            let density = model.density(&[exposure, exposure, exposure]);
            assert!(density[0] + 1.0e-6 >= previous);
            assert!((density[0] - density[1]).abs() < 1.0e-6);
            assert!((density[1] - density[2]).abs() < 1.0e-6);
            previous = density[0];
        }
    }

    #[test]
    fn coupling_mixes_log_exposure_before_the_curves() {
        // Density equals log-exposure inside the knots, so the matrix shows
        // through unchanged.
        let identity = Curve::new([(-4.0, -4.0), (4.0, 4.0)]).unwrap();
        let swap_red_green = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let model = FilmModel::Parametric(
            Parametric::new(
                [identity.clone(), identity.clone(), identity],
                Some(swap_red_green),
            )
            .unwrap(),
        );
        let density = model.density(&[10.0, 1.0, 0.1]);
        let log_h = [10.0f32, 1.0, 0.1].map(|channel| channel.log10());
        assert!((density[0] - log_h[1]).abs() < 1.0e-5, "{density:?}");
        assert!((density[1] - log_h[0]).abs() < 1.0e-5, "{density:?}");
        assert!((density[2] - log_h[2]).abs() < 1.0e-5, "{density:?}");
    }
}
