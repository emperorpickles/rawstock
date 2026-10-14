//! Film model: scene exposure to per-layer density, and the stock files that
//! fill [`Parameters`](crate::render::Parameters).
//!
//! Spectral (wavelength sensitivity and dye density) is intentionally not a
//! variant yet. The only model is [`FilmModel::Parametric`]. A stock file
//! names that choice with [`StockModel`], so a later `spectral` tag can be
//! added without changing the `parametric` tag existing files already use.
//!
//! Optional `[grain]` and `[halation]` tables are part of schema 1. A file
//! that omits them still loads; the pipeline then skips that stage.

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

    /// A granularity table had no samples.
    #[error("granularity table needs at least one sample")]
    EmptyGranularity,

    /// Granularity density did not strictly increase.
    #[error("granularity density must strictly increase")]
    GranularityOrder,

    /// A granularity sample was negative or not finite.
    #[error("granularity rms must be finite and non-negative")]
    GranularitySample,

    /// Halation threshold, radius, or a weight was unusable.
    #[error(
        "halation threshold and radius must be finite, radius must be non-negative, and weights must be finite and non-negative"
    )]
    HalationParameters,

    /// The physical frame width was missing, zero, or not finite.
    #[error("frame width must be a positive finite number of millimetres")]
    FrameWidth,
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

/// RMS granularity of one film layer, as a function of density.
///
/// Points are `(density, rms)`. Density must strictly increase. RMS may rise
/// and fall: a Kodak Vision3 granularity curve peaks and then drops. Outside
/// the first and last density, evaluation clamps to the end RMS values.
///
/// `rms` is the datasheet number, 1000 times the density standard deviation
/// read through a 48 µm aperture. [`Grain`] divides by 1000 when it builds
/// the noise.
#[derive(Debug, Clone)]
pub struct Granularity {
    density: Vec<f32>,
    rms: Vec<f32>,
}

/// Per-layer film grain in density.
///
/// The noise value comes from [`Self::hash`], a PCG-style `u32` hash of the
/// grain-cell coordinate, the layer, and a seed. There is no `rand` and no
/// floating-point inside the hash, so a WGSL port can match it bit for bit.
///
/// Grain size is the Kodak 48 µm RMS aperture. The image width spans
/// [`Self::frame_mm`] millimetres (35 by default), so the cell is
/// `width * 0.048 / frame_mm` pixels. Preview and export share that size
/// when they share a frame width. Pixels in one cell share a hash. While
/// the cell covers at least one pixel, the density standard deviation is
/// `rms / 1000`, independent of resolution, and the cell size in pixels
/// scales linearly with width. Below one pixel per aperture, each pixel
/// averages several apertures and the standard deviation scales with
/// `sqrt(cell_px)`.
#[derive(Debug, Clone)]
pub struct Grain {
    frame_mm: f32,
    seed: u32,
    layers: [Granularity; 3],
}

/// Highlight halation on scene-linear exposure.
///
/// Samples whose peak channel is at or below `threshold` contribute nothing.
/// The excess is blurred by a three-pass box approximation of a Gaussian
/// whose standard deviation is `radius` times the shorter image side, then
/// added back as `weight[channel] * blurred`. The radius is a fraction of
/// the frame, so a preview and a full export get the same physical glow.
#[derive(Debug, Clone)]
pub struct Halation {
    threshold: f32,
    radius: f32,
    weight: [f32; 3],
}

impl Granularity {
    /// Build a table from `(density, rms)` points, in density order.
    pub fn new(points: impl IntoIterator<Item = (f32, f32)>) -> Result<Self, ModelError> {
        let mut density = Vec::new();
        let mut rms = Vec::new();
        for (point, value) in points {
            if !point.is_finite() || !value.is_finite() || value < 0.0 {
                return Err(ModelError::GranularitySample);
            }
            if let Some(previous) = density.last().copied()
                && point <= previous
            {
                return Err(ModelError::GranularityOrder);
            }
            density.push(point);
            rms.push(value);
        }
        if density.is_empty() {
            return Err(ModelError::EmptyGranularity);
        }
        Ok(Self { density, rms })
    }

    /// RMS granularity at `density`, clamped outside the knots.
    pub fn eval(&self, density: f32) -> f32 {
        let density = if density.is_finite() {
            density
        } else {
            self.density[0]
        };
        if density <= self.density[0] {
            return self.rms[0];
        }
        if density >= self.density[self.density.len() - 1] {
            return self.rms[self.rms.len() - 1];
        }
        let upper = self.density.partition_point(|point| *point <= density);
        let lower = upper - 1;
        let span = self.density[upper] - self.density[lower];
        let weight = (density - self.density[lower]) / span;
        self.rms[lower] + weight * (self.rms[upper] - self.rms[lower])
    }

    /// The `(density, rms)` knots, in order.
    pub fn points(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.density.iter().copied().zip(self.rms.iter().copied())
    }
}

impl Grain {
    /// Default frame width. 35 mm is the film gauge; the stored image width
    /// is taken to span this many millimetres.
    pub const FRAME_MM: f32 = 35.0;

    /// Kodak RMS measurement aperture, in millimetres.
    pub const APERTURE_MM: f32 = 0.048;

    /// `layers` are red, green, and blue, in that order.
    pub fn new(frame_mm: f32, seed: u32, layers: [Granularity; 3]) -> Result<Self, ModelError> {
        if !frame_mm.is_finite() || frame_mm <= 0.0 {
            return Err(ModelError::FrameWidth);
        }
        Ok(Self {
            frame_mm,
            seed,
            layers,
        })
    }

    /// PCG-style hash of a grain-cell coordinate and a layer.
    ///
    /// `x` and `y` are cell indices, not raw pixels. [`Self::apply`] derives
    /// them by dividing the pixel coordinate by [`Self::cell_px`]. Only `u32`
    /// wrapping arithmetic is used.
    ///
    /// The output step is the PCG hash from Jarzynski & Hill, "Hash Functions
    /// for GPU Rendering", Journal of Computer Graphics Techniques, 2020.
    pub fn hash(x: u32, y: u32, layer: u32, seed: u32) -> u32 {
        let mut n = seed
            .wrapping_add(layer.wrapping_mul(0x9E37_79B9))
            .wrapping_add(x.wrapping_mul(0x85EB_CA6B))
            ^ y.wrapping_mul(0xC2B2_AE35);
        n = n.wrapping_mul(747796405).wrapping_add(2891336453);
        let word = ((n >> ((n >> 28) + 4)) ^ n).wrapping_mul(277803737);
        (word >> 22) ^ word
    }

    /// Physical width of the frame, in millimetres.
    pub fn frame_mm(&self) -> f32 {
        self.frame_mm
    }

    /// Seed mixed into [`Self::hash`].
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Red, green, and blue granularity tables.
    pub fn layers(&self) -> &[Granularity; 3] {
        &self.layers
    }

    /// Grain-cell size in pixels when the image is `width` pixels across.
    ///
    /// This is the 48 µm aperture on a frame of [`Self::frame_mm`]. It scales
    /// linearly with `width`.
    pub fn cell_px(&self, width: u32) -> f32 {
        if width == 0 {
            0.0
        } else {
            Self::APERTURE_MM / self.frame_mm * width as f32
        }
    }

    /// Add per-layer grain to interleaved density.
    ///
    /// `density` is row-major RGB. A trailing incomplete pixel is left
    /// unchanged. Mean density is unchanged in expectation: the hash is
    /// mapped to a zero-mean unit-variance value and multiplied by the
    /// granularity at that pixel's density.
    pub fn apply(&self, density: &mut [f32], width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let cell = self.cell_px(width).max(1.0);
        // σ_D = (rms / 1000) * scale. scale is 1 when a cell covers a pixel,
        // and sqrt(cell_px) when one pixel covers several apertures.
        let scale = self.cell_px(width).min(1.0).sqrt() / 1000.0;
        let row_stride = width as usize * 3;
        for (y, row) in density
            .chunks_mut(row_stride)
            .take(height as usize)
            .enumerate()
        {
            let cy = (y as f32 / cell).floor() as u32;
            for (x, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let cx = (x as f32 / cell).floor() as u32;
                for (layer, sample) in pixel.iter_mut().enumerate() {
                    let rms = self.layers[layer].eval(*sample);
                    let noise = unit_noise(Self::hash(cx, cy, layer as u32, self.seed));
                    *sample += noise * rms * scale;
                }
            }
        }
    }
}

impl Halation {
    /// `weight` is the red, green, and blue add-back, in that order.
    pub fn new(threshold: f32, radius: f32, weight: [f32; 3]) -> Result<Self, ModelError> {
        if !threshold.is_finite() || !radius.is_finite() || radius < 0.0 {
            return Err(ModelError::HalationParameters);
        }
        if weight
            .iter()
            .any(|channel| !channel.is_finite() || *channel < 0.0)
        {
            return Err(ModelError::HalationParameters);
        }
        Ok(Self {
            threshold,
            radius,
            weight,
        })
    }

    /// Scene-linear peak at or below this contributes no halation.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Gaussian standard deviation as a fraction of the shorter image side.
    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// Red, green, and blue add-back weights.
    pub fn weight(&self) -> [f32; 3] {
        self.weight
    }

    /// Blur standard deviation in pixels for an image of this size.
    pub fn sigma_px(&self, width: u32, height: u32) -> f32 {
        self.radius * width.min(height) as f32
    }

    /// Add halation to interleaved scene-linear RGB exposure.
    ///
    /// The highlight measure is the peak channel. The blur is normalized, so
    /// a uniform field is unchanged by the blur itself and the added energy
    /// of an interior highlight stays within the excess energy times the
    /// weight sum.
    pub fn apply(&self, exposure: &mut [f32], width: u32, height: u32) {
        let width_px = width as usize;
        let height_px = height as usize;
        if width_px == 0 || height_px == 0 {
            return;
        }
        let Some(pixels) = width_px.checked_mul(height_px) else {
            return;
        };
        let Some(samples) = pixels.checked_mul(3) else {
            return;
        };
        if exposure.len() < samples {
            return;
        }
        let mut excess = vec![0.0_f32; pixels];
        for (index, pixel) in exposure[..samples].as_chunks::<3>().0.iter().enumerate() {
            let peak = pixel[0].max(pixel[1]).max(pixel[2]);
            let over = peak - self.threshold;
            if over > 0.0 {
                excess[index] = over;
            }
        }
        widen(
            &mut excess,
            width_px,
            height_px,
            self.sigma_px(width, height),
        );
        for (index, pixel) in exposure[..samples]
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let glow = excess[index];
            pixel[0] += self.weight[0] * glow;
            pixel[1] += self.weight[1] * glow;
            pixel[2] += self.weight[2] * glow;
        }
    }
}

/// Map a hash to a zero-mean value of variance 1.
///
/// The top 24 bits become a uniform `[0, 1)` sample. Uniform `[0, 1)` has
/// variance `1/12`, so the scale is `sqrt(12)`.
fn unit_noise(hash: u32) -> f32 {
    let unit = (hash >> 8) as f32 * (1.0 / 16_777_216.0);
    (unit - 0.5) * 12.0_f32.sqrt()
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
/// Metadata stays here. The curves, coupling, exposure, grain, halation, and
/// print stage are the existing [`Parameters`] value, not a second parameter type.
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

    /// A grain layer's density and rms columns differ in length.
    #[error("grain {layer} density and rms arrays have lengths {density} and {rms}")]
    GrainLength {
        /// `red`, `green`, or `blue`.
        layer: &'static str,
        /// Length of `density`.
        density: usize,
        /// Length of `rms`.
        rms: usize,
    },

    /// A grain layer was not a usable granularity table.
    #[error("grain {layer} granularity table is invalid")]
    Grain {
        /// `red`, `green`, or `blue`.
        layer: &'static str,
        /// Why the table was rejected.
        #[source]
        source: ModelError,
    },

    /// Halation failed the parameter checks.
    #[error("halation parameters are invalid")]
    Halation(#[source] ModelError),

    /// `weight` was not three channels.
    #[error("halation weight has {len} values, expected 3")]
    HalationWeight {
        /// Number of values found.
        len: usize,
    },
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
    #[serde(default)]
    grain: Option<GrainSamples>,
    #[serde(default)]
    halation: Option<HalationSamples>,
}

/// Optional per-layer grain. Absent means the pipeline adds none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrainSamples {
    /// Physical width of the frame, in millimetres. Defaults to 35.
    #[serde(default = "default_frame_mm", deserialize_with = "de_f32")]
    frame_mm: f32,
    /// Mixed into the grain hash. Defaults to 0.
    #[serde(default)]
    seed: u32,
    red: RmsSamples,
    green: RmsSamples,
    blue: RmsSamples,
}

fn default_frame_mm() -> f32 {
    Grain::FRAME_MM
}

/// One layer of a Kodak-style RMS granularity curve.
///
/// Index `i` is the pair `(density[i], rms[i])`. `rms` is 1000 × σ of density
/// through a 48 µm aperture, which is the number the Vision3 datasheet tells
/// you to record after reading the Sigma D scale.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RmsSamples {
    #[serde(deserialize_with = "de_f32_vec")]
    density: Vec<f32>,
    #[serde(deserialize_with = "de_f32_vec")]
    rms: Vec<f32>,
}

/// Optional highlight halation. Absent means the pipeline adds none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HalationSamples {
    #[serde(deserialize_with = "de_f32")]
    threshold: f32,
    /// Gaussian sigma as a fraction of the shorter image side.
    #[serde(deserialize_with = "de_f32")]
    radius: f32,
    /// Red, green, and blue add-back. Red is the one a stock sets highest.
    #[serde(deserialize_with = "de_f32_vec")]
    weight: Vec<f32>,
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
        let mut parameters = Parameters::new(file.exposure_ev, film, file.print_exposure, print)
            .map_err(StockError::Model)?;
        if let Some(grain) = file.grain {
            parameters = parameters.with_grain(grain_from_samples(grain)?);
        }
        if let Some(halation) = file.halation {
            parameters = parameters.with_halation(halation_from_samples(halation)?);
        }
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

fn grain_from_samples(samples: GrainSamples) -> Result<Grain, StockError> {
    let red = granularity_from_samples("red", &samples.red)?;
    let green = granularity_from_samples("green", &samples.green)?;
    let blue = granularity_from_samples("blue", &samples.blue)?;
    Grain::new(samples.frame_mm, samples.seed, [red, green, blue]).map_err(StockError::Model)
}

fn granularity_from_samples(
    layer: &'static str,
    samples: &RmsSamples,
) -> Result<Granularity, StockError> {
    if samples.density.len() != samples.rms.len() {
        return Err(StockError::GrainLength {
            layer,
            density: samples.density.len(),
            rms: samples.rms.len(),
        });
    }
    let points = samples
        .density
        .iter()
        .copied()
        .zip(samples.rms.iter().copied());
    Granularity::new(points).map_err(|source| StockError::Grain { layer, source })
}

fn halation_from_samples(samples: HalationSamples) -> Result<Halation, StockError> {
    let &[red, green, blue] = samples.weight.as_slice() else {
        return Err(StockError::HalationWeight {
            len: samples.weight.len(),
        });
    };
    Halation::new(samples.threshold, samples.radius, [red, green, blue])
        .map_err(StockError::Halation)
}

/// Three box passes whose variance matches a Gaussian of standard deviation
/// `sigma`. Widths follow Peter Kovesi, "Fast Almost-Gaussian Filtering"
/// (DICTA 2010). A sigma below half a pixel is left unblurred.
fn widen(samples: &mut [f32], width: usize, height: usize, sigma: f32) {
    if !sigma.is_finite() || sigma < 0.5 || width == 0 || height == 0 {
        return;
    }
    let mut scratch = vec![0.0_f32; samples.len()];
    for radius in box_radii(sigma) {
        if radius == 0 {
            continue;
        }
        box_blur_horizontal(samples, &mut scratch, width, height, radius);
        box_blur_vertical(&scratch, samples, width, height, radius);
    }
}

fn box_radii(sigma: f32) -> [usize; 3] {
    let passes = 3.0_f32;
    let ideal = (12.0 * sigma * sigma / passes + 1.0)
        .sqrt()
        .floor()
        .max(1.0);
    let mut lower = ideal as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    if lower < 1 {
        lower = 1;
    }
    let upper = lower + 2;
    let lower_f = lower as f32;
    let numerator =
        12.0 * sigma * sigma - passes * lower_f * lower_f - 4.0 * passes * lower_f - 3.0 * passes;
    let denominator = -4.0 * lower_f - 4.0;
    let narrow = (numerator / denominator).round().clamp(0.0, passes) as usize;
    let mut radii = [0; 3];
    for (index, radius) in radii.iter_mut().enumerate() {
        let window = if index < narrow { lower } else { upper };
        *radius = (window as usize - 1) / 2;
    }
    radii
}

fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = radius * 2 + 1;
    let window_f = window as f64;
    for y in 0..height {
        let row = y * width;
        let mut sum = 0.0_f64;
        for tap in 0..window {
            let x = (tap as isize - radius as isize).clamp(0, width as isize - 1) as usize;
            sum += f64::from(src[row + x]);
        }
        for x in 0..width {
            dst[row + x] = (sum / window_f) as f32;
            let remove = (x as isize - radius as isize).clamp(0, width as isize - 1) as usize;
            let add = (x as isize + radius as isize + 1).clamp(0, width as isize - 1) as usize;
            sum += f64::from(src[row + add]) - f64::from(src[row + remove]);
        }
    }
}

fn box_blur_vertical(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: usize) {
    let window = radius * 2 + 1;
    let window_f = window as f64;
    for x in 0..width {
        let mut sum = 0.0_f64;
        for tap in 0..window {
            let y = (tap as isize - radius as isize).clamp(0, height as isize - 1) as usize;
            sum += f64::from(src[y * width + x]);
        }
        for y in 0..height {
            dst[y * width + x] = (sum / window_f) as f32;
            let remove = (y as isize - radius as isize).clamp(0, height as isize - 1) as usize;
            let add = (y as isize + radius as isize + 1).clamp(0, height as isize - 1) as usize;
            sum += f64::from(src[add * width + x]) - f64::from(src[remove * width + x]);
        }
    }
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
