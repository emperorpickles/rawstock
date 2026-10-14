//! Film model: scene exposure to per-layer density, and the stock files that
//! fill [`Parameters`](crate::render::Parameters).
//!
//! [`FilmModel::Parametric`] reads scene RGB as three layer exposures.
//! [`FilmModel::Spectral`] replaces that step: ACEScg is upsampled to a
//! spectrum and integrated against per-layer sensitivities. Both then use the
//! same D-logH curves, coupling, and grain. A spectral stock that also stores
//! dye-density curves turns those densities into a print exposure; otherwise
//! the print stage is the existing per-channel inversion. A stock file names
//! the choice with [`StockModel`]. `parametric` is unchanged.
//!
//! Optional `[grain]` and `[halation]` tables are part of schema 1. A file
//! that omits them still loads; the pipeline then skips that stage.

use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, SeqAccess, Visitor};
use thiserror::Error;

use crate::render::Parameters;
use crate::spectrum::{self, Observer};

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

/// Per-layer spectral film.
///
/// Sensitivities are non-negative and each integrates to 1 on `wavelengths`,
/// so a flat spectrum of level 1 exposes every layer equally. The D-logH
/// curves then carry the relative layer speed. `dye_density`, when present,
/// is the spectral density of one unit of that layer's characteristic
/// density. The print stage integrates `10` to the power of minus the summed
/// dye density against a D65 / CIE 1931 observer.
#[derive(Debug, Clone)]
pub struct Spectral {
    wavelengths: Vec<f32>,
    /// `sensitivity[layer][i] * trapezoid weight[i]`, so exposure is a dot product.
    weighted_sensitivity: [Vec<f32>; 3],
    sensitivity: [Vec<f32>; 3],
    dye_density: Option<[Vec<f32>; 3]>,
    /// Scan weights. `None` when the stock has no dye curves.
    observer: Option<Observer>,
    /// Subtracted from the scanned density so `reference_log_exposure` prints neutral.
    scan_balance: [f32; 3],
    reference_log_exposure: Option<f32>,
    curves: [Curve; 3],
    coupling: Option<[[f32; 3]; 3]>,
}

/// Maps scene-linear exposure to per-layer density.
#[derive(Debug, Clone)]
pub enum FilmModel {
    /// Per-layer D-logH curves and optional log-exposure coupling.
    Parametric(Parametric),
    /// Spectral sensitivities into the same curves, plus optional dye spectra.
    ///
    /// Boxed so the wavelength tables do not inflate every parametric stock.
    Spectral(Box<Spectral>),
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

    /// A dye-cloud size was missing, zero, or not finite.
    #[error("grain size must be a positive finite number of millimetres")]
    GrainSize,

    /// The shared wavelength grid was too short, non-finite, or not increasing.
    #[error("wavelength samples must be finite and strictly increasing, with at least two samples")]
    WavelengthGrid,

    /// A sensitivity sample was negative, non-finite, the wrong length, or not normalised.
    #[error(
        "sensitivity samples must be finite and non-negative, match the wavelength grid, and integrate to 1"
    )]
    Sensitivity,

    /// A dye-density sample was negative, non-finite, or the wrong length.
    #[error("dye density samples must be finite and non-negative and match the wavelength grid")]
    DyeDensity,

    /// The printer-balance log exposure was NaN or infinite.
    #[error("reference log exposure must be finite")]
    ReferenceExposure,

    /// A reference log exposure was set without dye curves to balance.
    #[error("reference log exposure is only valid with dye density curves")]
    ReferenceWithoutDye,

    /// The wavelength grid missed the visible observer, so dye density cannot be scanned.
    #[error("wavelength grid does not meet the visible scan observer")]
    Observer,
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

    /// The `(log10(exposure), density)` knots, in order.
    pub fn points(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.log_h.iter().copied().zip(self.density.iter().copied())
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

    fn density_controlled(
        &self,
        exposure: &[f32],
        width: u32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Vec<f32>, crate::render::Cancelled> {
        let (stride, rows) = row_span(exposure.len(), width);
        crate::render::begin(crate::render::Stage::Density, rows, control)?;
        let mut density = Vec::with_capacity(exposure.len());
        for (y, row) in exposure.chunks(stride).take(rows as usize).enumerate() {
            if control.cancelled() {
                return Err(crate::render::Cancelled);
            }
            for pixel in row.as_chunks::<3>().0 {
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
            control.on_progress(crate::render::Stage::Density, y as u32 + 1, rows);
        }
        Ok(density)
    }
}

impl Spectral {
    /// `sensitivity` and `curves` are red, green, and blue, in that order.
    ///
    /// Each sensitivity is non-negative and integrates to 1 with the
    /// trapezoidal rule on `wavelengths`. `dye_density` uses the same grid
    /// and the same channel order. `reference_log_exposure`, when dye curves
    /// are present, is the log exposure the print balances to neutral.
    pub fn new(
        wavelengths: Vec<f32>,
        sensitivity: [Vec<f32>; 3],
        dye_density: Option<[Vec<f32>; 3]>,
        curves: [Curve; 3],
        coupling: Option<[[f32; 3]; 3]>,
        reference_log_exposure: Option<f32>,
    ) -> Result<Self, ModelError> {
        if wavelengths.len() < 2
            || wavelengths
                .windows(2)
                .any(|pair| !pair[0].is_finite() || !pair[1].is_finite() || pair[1] <= pair[0])
        {
            return Err(ModelError::WavelengthGrid);
        }
        for layer in &sensitivity {
            if layer.len() != wavelengths.len()
                || layer
                    .iter()
                    .any(|sample| !sample.is_finite() || *sample < 0.0)
                || (integrate(&wavelengths, layer) - 1.0).abs() > 1.0e-3
            {
                return Err(ModelError::Sensitivity);
            }
        }
        if let Some(dyes) = &dye_density {
            for layer in dyes {
                if layer.len() != wavelengths.len()
                    || layer
                        .iter()
                        .any(|sample| !sample.is_finite() || *sample < 0.0)
                {
                    return Err(ModelError::DyeDensity);
                }
            }
        }
        if let Some(exposure) = reference_log_exposure {
            if !exposure.is_finite() {
                return Err(ModelError::ReferenceExposure);
            }
            if dye_density.is_none() {
                return Err(ModelError::ReferenceWithoutDye);
            }
        }
        if let Some(matrix) = coupling
            && matrix.iter().flatten().any(|entry| !entry.is_finite())
        {
            return Err(ModelError::NonFiniteCoupling);
        }
        let weights = trapezoid_weights(&wavelengths);
        let weighted_sensitivity = sensitivity.clone().map(|layer| {
            layer
                .iter()
                .zip(&weights)
                .map(|(sample, weight)| sample * weight)
                .collect()
        });
        let observer = if dye_density.is_some() {
            Some(Observer::acescg_d65(&wavelengths).ok_or(ModelError::Observer)?)
        } else {
            None
        };
        let mut model = Self {
            wavelengths,
            weighted_sensitivity,
            sensitivity,
            dye_density,
            observer,
            scan_balance: [0.0; 3],
            reference_log_exposure,
            curves,
            coupling,
        };
        if let Some(log_h) = reference_log_exposure {
            let density = [
                model.curves[0].eval(log_h),
                model.curves[1].eval(log_h),
                model.curves[2].eval(log_h),
            ];
            let effective = model.scanned_density(density);
            let mean = (effective[0] + effective[1] + effective[2]) / 3.0;
            model.scan_balance = effective.map(|channel| channel - mean);
        }
        Ok(model)
    }

    /// Shared wavelength grid, in nanometres.
    pub fn wavelengths(&self) -> &[f32] {
        &self.wavelengths
    }

    /// Red, green, and blue sensitivities. Each integrates to 1.
    pub fn sensitivities(&self) -> &[Vec<f32>; 3] {
        &self.sensitivity
    }

    /// Spectral dye density per unit characteristic density, when the stock has it.
    pub fn dye_density(&self) -> Option<&[Vec<f32>; 3]> {
        self.dye_density.as_ref()
    }

    /// The red, green, and blue characteristic curves.
    pub fn curves(&self) -> &[Curve; 3] {
        &self.curves
    }

    /// Log-exposure mixing, when the layers are not independent.
    pub fn coupling(&self) -> Option<[[f32; 3]; 3]> {
        self.coupling
    }

    /// Log exposure the spectral print balances to a neutral, when set.
    pub fn reference_log_exposure(&self) -> Option<f32> {
        self.reference_log_exposure
    }

    /// Dye curves multiplied by `scales`, with the scan balance recomputed.
    ///
    /// Each scale is finite and non-negative. The stock must already have dye
    /// curves. This is the dye-scale slider: the session keeps the loaded
    /// curves as the base and writes `base × scale`.
    pub fn with_dye_scales(&self, scales: [f32; 3]) -> Result<Self, ModelError> {
        if scales
            .iter()
            .any(|scale| !scale.is_finite() || *scale < 0.0)
        {
            return Err(ModelError::DyeDensity);
        }
        let Some(dyes) = &self.dye_density else {
            return Err(ModelError::DyeDensity);
        };
        let scaled = [
            dyes[0].iter().map(|sample| sample * scales[0]).collect(),
            dyes[1].iter().map(|sample| sample * scales[1]).collect(),
            dyes[2].iter().map(|sample| sample * scales[2]).collect(),
        ];
        Self::new(
            self.wavelengths.clone(),
            self.sensitivity.clone(),
            Some(scaled),
            self.curves.clone(),
            self.coupling,
            self.reference_log_exposure,
        )
    }

    fn density_controlled(
        &self,
        exposure: &[f32],
        width: u32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Vec<f32>, crate::render::Cancelled> {
        let (stride, rows) = row_span(exposure.len(), width);
        crate::render::begin(crate::render::Stage::Density, rows, control)?;
        let mut spectrum = vec![0.0f32; self.wavelengths.len()];
        let mut density = Vec::with_capacity(exposure.len());
        for (y, row) in exposure.chunks(stride).take(rows as usize).enumerate() {
            if control.cancelled() {
                return Err(crate::render::Cancelled);
            }
            for pixel in row.as_chunks::<3>().0 {
                spectrum::fill_scene_spectrum(*pixel, &self.wavelengths, &mut spectrum);
                let mut log_h = [0.0f32; 3];
                for (layer, log_exposure) in log_h.iter_mut().enumerate() {
                    let mut layer_exposure = 0.0f32;
                    for (sample, weight) in spectrum.iter().zip(&self.weighted_sensitivity[layer]) {
                        layer_exposure += sample * weight;
                    }
                    *log_exposure = layer_exposure.max(EXPOSURE_FLOOR).log10();
                }
                if let Some(matrix) = self.coupling {
                    log_h = mix(matrix, log_h);
                }
                density.extend([
                    self.curves[0].eval(log_h[0]),
                    self.curves[1].eval(log_h[1]),
                    self.curves[2].eval(log_h[2]),
                ]);
            }
            control.on_progress(crate::render::Stage::Density, y as u32 + 1, rows);
        }
        Ok(density)
    }

    /// Equivalent per-channel density for the existing print curve.
    ///
    /// `None` when this stock has no dye spectra: the characteristic density
    /// is already what the print curve expects.
    fn printing_density_controlled(
        &self,
        density: &[f32],
        width: u32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Option<Vec<f32>>, crate::render::Cancelled> {
        let Some(dyes) = self.dye_density.as_ref() else {
            return Ok(None);
        };
        let Some(observer) = self.observer.as_ref() else {
            return Ok(None);
        };
        let (stride, rows) = row_span(density.len(), width);
        crate::render::begin(crate::render::Stage::DyeScan, rows, control)?;
        let mut transmittance = vec![0.0f32; self.wavelengths.len()];
        let mut printed = Vec::with_capacity(density.len());
        for (y, row) in density.chunks(stride).take(rows as usize).enumerate() {
            if control.cancelled() {
                return Err(crate::render::Cancelled);
            }
            for pixel in row.as_chunks::<3>().0 {
                for index in 0..transmittance.len() {
                    let optical = dyes[0][index] * pixel[0]
                        + dyes[1][index] * pixel[1]
                        + dyes[2][index] * pixel[2];
                    transmittance[index] = 10f32.powf(-optical);
                }
                let scanned = observer
                    .apply(&transmittance)
                    .map(|channel| -channel.max(EXPOSURE_FLOOR).log10());
                printed.extend([
                    scanned[0] - self.scan_balance[0],
                    scanned[1] - self.scan_balance[1],
                    scanned[2] - self.scan_balance[2],
                ]);
            }
            control.on_progress(crate::render::Stage::DyeScan, y as u32 + 1, rows);
        }
        Ok(Some(printed))
    }

    pub(crate) fn scan_pixel(
        &self,
        pixel: [f32; 3],
        transmittance: &mut [f32],
    ) -> Option<[f32; 3]> {
        let dyes = self.dye_density.as_ref()?;
        let observer = self.observer.as_ref()?;
        for index in 0..transmittance.len() {
            let optical =
                dyes[0][index] * pixel[0] + dyes[1][index] * pixel[1] + dyes[2][index] * pixel[2];
            transmittance[index] = 10f32.powf(-optical);
        }
        let scanned = observer
            .apply(transmittance)
            .map(|channel| -channel.max(EXPOSURE_FLOOR).log10());
        Some([
            scanned[0] - self.scan_balance[0],
            scanned[1] - self.scan_balance[1],
            scanned[2] - self.scan_balance[2],
        ])
    }

    /// Density of a neutral filter with the same ACEScg transmittance.
    ///
    /// Balance is not applied. The caller subtracts [`Self::scan_balance`].
    fn scanned_density(&self, density: [f32; 3]) -> [f32; 3] {
        let dyes = self
            .dye_density
            .as_ref()
            .expect("scanned density is only used when dye curves exist");
        let observer = self
            .observer
            .as_ref()
            .expect("dye curves build a scan observer");
        let mut transmittance = vec![0.0f32; self.wavelengths.len()];
        for index in 0..transmittance.len() {
            let optical = dyes[0][index] * density[0]
                + dyes[1][index] * density[1]
                + dyes[2][index] * density[2];
            transmittance[index] = 10f32.powf(-optical);
        }
        observer
            .apply(&transmittance)
            .map(|channel| -channel.max(EXPOSURE_FLOOR).log10())
    }
}

impl FilmModel {
    /// Per-layer density for interleaved scene-linear RGB exposure.
    ///
    /// The result has the same length as `exposure`. A trailing incomplete
    /// pixel is omitted.
    pub fn density(&self, exposure: &[f32]) -> Vec<f32> {
        self.density_controlled(exposure, 0, &mut crate::render::NopControl)
            .expect("nop control does not cancel")
    }

    pub(crate) fn density_controlled(
        &self,
        exposure: &[f32],
        width: u32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Vec<f32>, crate::render::Cancelled> {
        match self {
            Self::Parametric(model) => model.density_controlled(exposure, width, control),
            Self::Spectral(model) => model.density_controlled(exposure, width, control),
        }
    }

    /// Density the print curve should see when dye spectra change the exposure.
    ///
    /// `None` means `density` is already that input. Parametric stocks, and
    /// spectral stocks without dye curves, return `None`.
    pub fn printing_density(&self, density: &[f32]) -> Option<Vec<f32>> {
        self.printing_density_controlled(density, 0, &mut crate::render::NopControl)
            .expect("nop control does not cancel")
    }

    pub(crate) fn printing_density_controlled(
        &self,
        density: &[f32],
        width: u32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Option<Vec<f32>>, crate::render::Cancelled> {
        match self {
            Self::Parametric(_) => Ok(None),
            Self::Spectral(model) => model.printing_density_controlled(density, width, control),
        }
    }

    /// The first stage a change from `self` to `next` has to recompute.
    ///
    /// [`Stage::Encode`](crate::render::Stage::Encode) means the two models
    /// agree. Dye-only edits stay at the dye scan, so grain is kept.
    pub(crate) fn edit_stage(&self, next: &Self) -> crate::render::Stage {
        if self.same_density_inputs(next) {
            if self.same_dyes(next) {
                crate::render::Stage::Encode
            } else {
                crate::render::Stage::DyeScan
            }
        } else {
            crate::render::Stage::Density
        }
    }

    fn same_density_inputs(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Parametric(left), Self::Parametric(right)) => {
                curves_eq(&left.curves, &right.curves) && left.coupling == right.coupling
            }
            (Self::Spectral(left), Self::Spectral(right)) => {
                left.wavelengths == right.wavelengths
                    && left.sensitivity == right.sensitivity
                    && curves_eq(&left.curves, &right.curves)
                    && left.coupling == right.coupling
            }
            _ => false,
        }
    }

    fn same_dyes(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Parametric(_), Self::Parametric(_)) => true,
            (Self::Spectral(left), Self::Spectral(right)) => {
                left.dye_density == right.dye_density
                    && left.reference_log_exposure == right.reference_log_exposure
            }
            _ => false,
        }
    }
}

fn curves_eq(left: &[Curve; 3], right: &[Curve; 3]) -> bool {
    left.iter()
        .zip(right)
        .all(|(a, b)| a.log_h == b.log_h && a.density == b.density)
}

fn row_span(samples: usize, width: u32) -> (usize, u32) {
    let pixels = samples / 3;
    if pixels == 0 {
        return (3, 0);
    }
    let width_px = if width == 0 { pixels } else { width as usize };
    let rows = pixels / width_px.max(1);
    (width_px.max(1) * 3, rows as u32)
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

/// Where a grain crop sits in the full frame.
///
/// Pitch and the 48 µm amplitude use `frame_width`. `origin_x` and
/// `origin_y` are the crop's top-left in that frame, in pixels.
#[derive(Debug, Clone, Copy)]
pub struct GrainRegion {
    /// Crop width in pixels.
    pub width: u32,
    /// Crop height in pixels.
    pub height: u32,
    /// Crop left edge in the full frame.
    pub origin_x: u32,
    /// Crop top edge in the full frame.
    pub origin_y: u32,
    /// Full-frame width. This sets millimetres per pixel.
    pub frame_width: u32,
}

impl GrainRegion {
    /// Image rows available in an interleaved RGB buffer of `samples`.
    pub(crate) fn rows(self, samples: usize) -> usize {
        let stride = self.width as usize * 3;
        if self.width == 0 || self.height == 0 || stride == 0 || samples < stride {
            0
        } else {
            (samples / stride).min(self.height as usize)
        }
    }
}

/// Per-layer film grain in density.
///
/// Dye clouds are a Poisson field of compact grains in the film plane, summed
/// in density. Centers, radii, and orientations come from [`Self::hash`], so
/// there is no `rand` and the pattern repeats for a given seed. The hash
/// itself is integer-only, so a WGSL port can match it bit for bit.
///
/// The model follows the Boolean grain construction in Newson, Delon, and
/// Galerne, "A Stochastic Film Grain Model for Resolution-Independent
/// Rendering", Computer Graphics Forum 36(8), 2017 (doi:10.1111/cgf.13159):
/// Poisson centers and i.i.d. compact grains, evaluated in millimetres.
/// agx-emulsion (now spektrafilm) is prior art for doing that on the
/// negative. This is an original shot-noise sum, not either implementation.
///
/// [`Self::APERTURE_MM`] is the Kodak 48 µm measurement window, not a tile.
/// While that window covers at least one pixel, the density standard
/// deviation is `rms / 1000`. Below one pixel per aperture, each pixel
/// averages several windows and the standard deviation scales with
/// `sqrt(cell_px)`. Cloud size is [`Self::size_mm`], per dye layer, and
/// scales with [`Self::frame_mm`] rather than with the pixel.
#[derive(Debug, Clone)]
pub struct Grain {
    frame_mm: f32,
    seed: u32,
    layers: [Granularity; 3],
    size_mm: [f32; 3],
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

    /// Multiply every RMS sample by `amount`.
    ///
    /// `amount` is finite and non-negative. `1` leaves the table unchanged.
    /// The session uses `None` grain, rather than a zero table, when the
    /// amount slider is 0.
    pub fn scaled(&self, amount: f32) -> Result<Self, ModelError> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(ModelError::GranularitySample);
        }
        Self::new(self.points().map(|(density, rms)| (density, rms * amount)))
    }
}

impl Grain {
    /// Default frame width. 35 mm is the film gauge; the stored image width
    /// is taken to span this many millimetres.
    pub const FRAME_MM: f32 = 35.0;

    /// Kodak RMS measurement aperture, in millimetres.
    pub const APERTURE_MM: f32 = 0.048;

    /// Characteristic dye-cloud size, red, green, then blue, in millimetres.
    ///
    /// Cyan dye (the red record) is the coarsest, yellow dye (the blue
    /// record) the finest.
    pub const DEFAULT_SIZE_MM: [f32; 3] = [0.028, 0.019, 0.013];

    /// `layers` are red, green, and blue, in that order.
    ///
    /// Cloud sizes start at [`Self::DEFAULT_SIZE_MM`].
    pub fn new(frame_mm: f32, seed: u32, layers: [Granularity; 3]) -> Result<Self, ModelError> {
        if !frame_mm.is_finite() || frame_mm <= 0.0 {
            return Err(ModelError::FrameWidth);
        }
        Ok(Self {
            frame_mm,
            seed,
            layers,
            size_mm: Self::DEFAULT_SIZE_MM,
        })
    }

    /// Replace the per-layer dye-cloud sizes, in millimetres.
    pub fn with_size_mm(mut self, size_mm: [f32; 3]) -> Result<Self, ModelError> {
        if size_mm.iter().any(|size| !size.is_finite() || *size <= 0.0) {
            return Err(ModelError::GrainSize);
        }
        self.size_mm = size_mm;
        Ok(self)
    }

    /// PCG-style hash of a film-plane coordinate and a stream index.
    ///
    /// `x` and `y` are integer coordinates, not raw pixels. `layer` selects a
    /// draw stream (a dye layer, or a counter inside one Poisson cell). Only
    /// `u32` wrapping arithmetic is used. Negative cell indices are passed
    /// as their wrapping `u32` bit pattern.
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

    /// Characteristic dye-cloud size of each layer, in millimetres.
    pub fn size_mm(&self) -> [f32; 3] {
        self.size_mm
    }

    /// Multiply every layer's RMS by `amount`.
    ///
    /// Frame width, seed, and dye-cloud size stay. `amount` follows
    /// [`Granularity::scaled`]. The splatted field does not depend on RMS,
    /// so a preview can keep the clouds and only rerun this scale.
    pub fn scaled(&self, amount: f32) -> Result<Self, ModelError> {
        Ok(Self {
            frame_mm: self.frame_mm,
            seed: self.seed,
            size_mm: self.size_mm,
            layers: [
                self.layers[0].scaled(amount)?,
                self.layers[1].scaled(amount)?,
                self.layers[2].scaled(amount)?,
            ],
        })
    }

    /// Kodak measurement aperture in pixels when the image is `width` across.
    ///
    /// This is the 48 µm window on a frame of [`Self::frame_mm`]. It scales
    /// the grain amplitude. It is not a rendered tile.
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
    /// unchanged. Each layer is a zero-mean dye-cloud field, normalized to
    /// unit variance and multiplied by the granularity at that pixel. Shared
    /// clouds correlate the layers; each layer also has clouds of its own.
    pub fn apply(&self, density: &mut [f32], width: u32, height: u32) {
        let _ = self.apply_region(
            density,
            GrainRegion {
                width,
                height,
                origin_x: 0,
                origin_y: 0,
                frame_width: width,
            },
            &mut crate::render::NopControl,
        );
    }

    /// Add grain to a crop of the full frame.
    ///
    /// [`GrainRegion::width`] and `height` are the crop. The origin is that
    /// crop's top-left in the full frame. Pitch and the 48 µm amplitude use
    /// [`GrainRegion::frame_width`], not the crop width, so a 100% view
    /// matches [`Self::apply`] on the whole frame. `control` is polled while
    /// the clouds are splatted.
    pub fn apply_region(
        &self,
        density: &mut [f32],
        region: GrainRegion,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<(), crate::render::Cancelled> {
        let frame_width = region.frame_width;
        if region.width == 0 || region.height == 0 || frame_width == 0 {
            return Ok(());
        }
        let rows = region.rows(density.len());
        if rows == 0 {
            return Ok(());
        }
        let field = self.unit_field(region, rows, control)?;
        self.mix_field(density, &field, frame_width);
        Ok(())
    }

    /// Normalized dye-cloud field for `region`, one value per density sample.
    ///
    /// The field does not depend on the granularity tables. [`Self::mix_field`]
    /// applies RMS. A crop uses the full frame's pitch, so the same film-plane
    /// coordinate gets the same cloud.
    pub(crate) fn unit_field(
        &self,
        region: GrainRegion,
        rows: usize,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<Vec<f32>, crate::render::Cancelled> {
        let width = region.width;
        let frame_width = region.frame_width;
        crate::render::begin(crate::render::Stage::Grain, rows as u32, control)?;
        let pitch = self.frame_mm / frame_width as f32;
        let mut field = vec![0.0_f32; rows * width as usize * 3];
        let (mean, deviation) = self.accumulate_clouds(&mut field, region, rows, pitch, control)?;
        for (index, sample) in field.iter_mut().enumerate() {
            let spread = deviation[index % 3];
            if spread == 0.0 {
                *sample = 0.0;
            } else {
                *sample = (*sample - mean[index % 3]) / spread;
            }
        }
        control.on_progress(crate::render::Stage::Grain, rows as u32, rows as u32);
        if control.cancelled() {
            return Err(crate::render::Cancelled);
        }
        Ok(field)
    }

    /// Add `field` from [`Self::unit_field`] onto density.
    ///
    /// `frame_width` is the full frame, matching the field. RMS comes from
    /// the tables on `self`, so an amount change can reuse `field`.
    pub(crate) fn mix_field(&self, density: &mut [f32], field: &[f32], frame_width: u32) {
        let scale = self.cell_px(frame_width).min(1.0).sqrt() / 1000.0;
        for (index, (sample, unit)) in density.iter_mut().zip(field.iter()).enumerate() {
            let rms = self.layers[index % 3].eval(*sample);
            if rms == 0.0 {
                continue;
            }
            *sample += *unit * rms * scale;
        }
    }

    /// Splats every cloud process and returns the analytical mean and
    /// standard deviation of the raw field, per layer.
    fn accumulate_clouds(
        &self,
        field: &mut [f32],
        region: GrainRegion,
        rows: usize,
        pitch: f32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<([f32; 3], [f32; 3]), crate::render::Cancelled> {
        let mut mean = [0.0_f32; 3];
        let mut variance = [0.0_f32; 3];
        for cloud in DYE_CLOUDS {
            let (cloud_mean, cloud_variance) =
                self.splat_cloud(field, region, rows, pitch, *cloud, control)?;
            for layer in 0..3 {
                mean[layer] += cloud_mean[layer];
                variance[layer] += cloud_variance[layer];
            }
        }
        Ok((mean, variance.map(f32::sqrt)))
    }

    /// Add one Poisson process. Layers named by the mask share grain centers.
    fn splat_cloud(
        &self,
        field: &mut [f32],
        region: GrainRegion,
        rows: usize,
        pitch: f32,
        cloud: DyeCloud,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<([f32; 3], [f32; 3]), crate::render::Cancelled> {
        let GrainRegion {
            width,
            origin_x,
            origin_y,
            ..
        } = region;
        let (amp_mean, amp_second) = amplitude_moments();
        let lambda = cloud.mu / (cloud.cell_mm * cloud.cell_mm);
        let scale_second = radius_scale_second(cloud.radius_scale);
        let mut amplitude = [0.0_f32; 3];
        let mut mean = [0.0_f32; 3];
        let mut variance = [0.0_f32; 3];
        let mut radius_lo = [0.0_f32; 3];
        let mut radius_hi = [0.0_f32; 3];
        let mut margin = 0.0_f32;
        for layer in 0..3 {
            if cloud.layers & (1 << layer) == 0 || cloud.variance <= 0.0 {
                continue;
            }
            let size = self.size_mm[layer];
            radius_lo[layer] = cloud.radius_scale[0] * size;
            radius_hi[layer] = cloud.radius_scale[1] * size;
            let expected_r2 = size * size * scale_second;
            let amp = (cloud.variance * KERNEL_SECOND
                / (lambda * std::f32::consts::PI * expected_r2 * amp_second))
                .sqrt();
            amplitude[layer] = amp;
            mean[layer] =
                lambda * amp * amp_mean * std::f32::consts::PI * expected_r2 / KERNEL_MEAN;
            variance[layer] = cloud.variance;
            margin = margin.max(radius_hi[layer] * cloud.aspect_max.sqrt());
        }
        if margin == 0.0 {
            return Ok((mean, variance));
        }

        let cell = cloud.cell_mm;
        let origin_x_mm = origin_x as f32 * pitch;
        let origin_y_mm = origin_y as f32 * pitch;
        let width_mm = width as f32 * pitch;
        let height_mm = rows as f32 * pitch;
        let x_first = ((origin_x_mm - margin) / cell).floor() as i32;
        let y_first = ((origin_y_mm - margin) / cell).floor() as i32;
        let x_last = ((origin_x_mm + width_mm + margin) / cell).floor() as i32;
        let y_last = ((origin_y_mm + height_mm + margin) / cell).floor() as i32;
        let seed = self.seed.wrapping_add(cloud.salt);
        for cy in y_first..=y_last {
            if control.cancelled() {
                return Err(crate::render::Cancelled);
            }
            for cx in x_first..=x_last {
                let mut rng = CloudRng::new(cx as u32, cy as u32, seed);
                let count = poisson_count(cloud.mu, &mut rng);
                for _ in 0..count {
                    let ux = rng.unit();
                    let uy = rng.unit();
                    let ur = rng.unit();
                    let ua = rng.unit();
                    let ut = rng.unit();
                    let uf = rng.unit();
                    let x_mm = (cx as f32 + ux) * cell;
                    let y_mm = (cy as f32 + uy) * cell;
                    let aspect = log_lerp(1.0, cloud.aspect_max, ua);
                    let axis = aspect.sqrt();
                    let (sin, cos) = (ut * std::f32::consts::TAU).sin_cos();
                    let strength = AMPLITUDE_LO + (AMPLITUDE_HI - AMPLITUDE_LO) * uf;
                    for layer in 0..3 {
                        let amp = amplitude[layer];
                        if amp == 0.0 {
                            continue;
                        }
                        let radius = log_lerp(radius_lo[layer], radius_hi[layer], ur);
                        stamp_cloud(
                            field,
                            width,
                            rows,
                            pitch,
                            origin_x as f32,
                            origin_y as f32,
                            Stamp {
                                x_mm,
                                y_mm,
                                rx: radius / axis,
                                ry: radius * axis,
                                sin,
                                cos,
                                amplitude: amp * strength,
                                layer,
                            },
                        );
                    }
                }
            }
        }
        Ok((mean, variance))
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

    /// How far the box-blur support reaches, in pixels, for this sigma.
    ///
    /// A 100% crop is padded by this many pixels so the edge of the view
    /// sees the same glow as the full frame. Sigma comes from the full
    /// frame, not from the crop.
    pub fn blur_support(sigma: f32) -> u32 {
        if !sigma.is_finite() || sigma < 0.5 {
            return 0;
        }
        box_radii(sigma).iter().sum::<usize>() as u32
    }

    /// [`Self::blur_support`] at [`Self::sigma_px`] for this frame.
    pub fn support_px(&self, width: u32, height: u32) -> u32 {
        Self::blur_support(self.sigma_px(width, height))
    }

    /// Add halation to interleaved scene-linear RGB exposure.
    ///
    /// The highlight measure is the peak channel. The blur is normalized, so
    /// a uniform field is unchanged by the blur itself and the added energy
    /// of an interior highlight stays within the excess energy times the
    /// weight sum. Sigma is `radius` times the shorter side of this buffer.
    pub fn apply(&self, exposure: &mut [f32], width: u32, height: u32) {
        self.apply_with_sigma(exposure, width, height, self.sigma_px(width, height));
    }

    /// [`Self::apply`] with an explicit Gaussian sigma, in pixels.
    ///
    /// A crop passes the full frame's sigma so the glow is not resized to
    /// the crop.
    pub fn apply_with_sigma(&self, exposure: &mut [f32], width: u32, height: u32, sigma: f32) {
        let _ = self.apply_controlled(
            exposure,
            width,
            height,
            sigma,
            &mut crate::render::NopControl,
        );
    }

    pub(crate) fn apply_controlled(
        &self,
        exposure: &mut [f32],
        width: u32,
        height: u32,
        sigma: f32,
        control: &mut dyn crate::render::DevelopControl,
    ) -> Result<(), crate::render::Cancelled> {
        let width_px = width as usize;
        let height_px = height as usize;
        if width_px == 0 || height_px == 0 {
            return Ok(());
        }
        let Some(pixels) = width_px.checked_mul(height_px) else {
            return Ok(());
        };
        let Some(samples) = pixels.checked_mul(3) else {
            return Ok(());
        };
        if exposure.len() < samples {
            return Ok(());
        }
        crate::render::begin(crate::render::Stage::Halation, height, control)?;
        let mut excess = vec![0.0_f32; pixels];
        for (index, pixel) in exposure[..samples].as_chunks::<3>().0.iter().enumerate() {
            if index % width_px == 0 && control.cancelled() {
                return Err(crate::render::Cancelled);
            }
            let peak = pixel[0].max(pixel[1]).max(pixel[2]);
            let over = peak - self.threshold;
            if over > 0.0 {
                excess[index] = over;
            }
        }
        widen(&mut excess, width_px, height_px, sigma, control)?;
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
        control.on_progress(crate::render::Stage::Halation, height, height);
        Ok(())
    }
}

/// Map a hash to a zero-mean value of variance 1.
///
/// The top 24 bits become a uniform `[0, 1)` sample. Uniform `[0, 1)` has
/// variance `1/12`, so the scale is `sqrt(12)`.
/// Kernel `A (1 - d²)^1` on an ellipse. `∫k = A π R² / KERNEL_MEAN` and
/// `∫k² = A² π R² / KERNEL_SECOND`, with `R²` the product of the radii.
const KERNEL_MEAN: f32 = 2.0;
const KERNEL_SECOND: f32 = 3.0;

/// Per-grain amplitude is uniform on this interval, so clouds are not equally dense.
const AMPLITUDE_LO: f32 = 0.45;
const AMPLITUDE_HI: f32 = 1.0;

/// One Poisson process of dye clouds.
///
/// `layers` is a bitmask, red = 1, green = 2, blue = 4. Bits that are set
/// share grain centers and differ only in radius. `variance` is the
/// variance that process contributes to each active layer. `radius_scale`
/// is the min and max radius as a multiple of that layer's `size_mm`.
#[derive(Clone, Copy)]
struct DyeCloud {
    cell_mm: f32,
    mu: f32,
    salt: u32,
    variance: f32,
    radius_scale: [f32; 2],
    aspect_max: f32,
    layers: u8,
}

/// Fine shared grains, coarse shared clumps, then a private process per layer.
///
/// Variances sum to 1, so the normalized field has unit standard deviation
/// before the RMS scale.
const DYE_CLOUDS: &[DyeCloud] = &[
    DyeCloud {
        cell_mm: 0.012,
        mu: 1.35,
        salt: 0x51C0_0001,
        variance: 0.22,
        radius_scale: [0.30, 0.58],
        aspect_max: 2.1,
        layers: 0b111,
    },
    DyeCloud {
        cell_mm: 0.020,
        mu: 1.05,
        salt: 0xA11C_0002,
        variance: 0.40,
        radius_scale: [0.62, 1.15],
        aspect_max: 2.5,
        layers: 0b111,
    },
    DyeCloud {
        cell_mm: 0.046,
        mu: 0.62,
        salt: 0xC0A5_0003,
        variance: 0.18,
        radius_scale: [1.45, 2.70],
        aspect_max: 2.8,
        layers: 0b111,
    },
    DyeCloud {
        cell_mm: 0.016,
        mu: 0.85,
        salt: 0xB1E7_0004,
        variance: 0.20,
        radius_scale: [0.42, 0.90],
        aspect_max: 2.3,
        layers: 0b001,
    },
    DyeCloud {
        cell_mm: 0.016,
        mu: 0.85,
        salt: 0xB1E7_0005,
        variance: 0.20,
        radius_scale: [0.42, 0.90],
        aspect_max: 2.3,
        layers: 0b010,
    },
    DyeCloud {
        cell_mm: 0.016,
        mu: 0.85,
        salt: 0xB1E7_0006,
        variance: 0.20,
        radius_scale: [0.42, 0.90],
        aspect_max: 2.3,
        layers: 0b100,
    },
];

/// Mean and second moment of a uniform draw on [`AMPLITUDE_LO`], [`AMPLITUDE_HI`].
fn amplitude_moments() -> (f32, f32) {
    let span = AMPLITUDE_HI - AMPLITUDE_LO;
    let mean = 0.5 * (AMPLITUDE_LO + AMPLITUDE_HI);
    let second = AMPLITUDE_LO * AMPLITUDE_LO + AMPLITUDE_LO * span + span * span / 3.0;
    (mean, second)
}

/// `E[S²]` for `S` log-uniform on `scale`.
fn radius_scale_second(scale: [f32; 2]) -> f32 {
    let lo = scale[0];
    let hi = scale[1];
    let ratio = hi / lo;
    (hi * hi - lo * lo) / (2.0 * ratio.ln())
}

fn log_lerp(lo: f32, hi: f32, unit: f32) -> f32 {
    lo * (hi / lo).powf(unit)
}

struct CloudRng {
    x: u32,
    y: u32,
    seed: u32,
    seq: u32,
}

impl CloudRng {
    fn new(x: u32, y: u32, seed: u32) -> Self {
        Self { x, y, seed, seq: 0 }
    }

    fn unit(&mut self) -> f32 {
        self.seq = self.seq.wrapping_add(1);
        let hash = Grain::hash(self.x, self.y, self.seq, self.seed);
        (hash >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Knuth's Poisson generator. `mu` is small for every cloud process.
fn poisson_count(mu: f32, rng: &mut CloudRng) -> u32 {
    let limit = (-mu).exp();
    let mut product = 1.0_f32;
    let mut count = 0_u32;
    while count < 64 {
        product *= rng.unit();
        if product <= limit {
            return count;
        }
        count += 1;
    }
    count
}

struct Stamp {
    x_mm: f32,
    y_mm: f32,
    rx: f32,
    ry: f32,
    sin: f32,
    cos: f32,
    amplitude: f32,
    layer: usize,
}

fn stamp_cloud(
    field: &mut [f32],
    width: u32,
    rows: usize,
    pitch: f32,
    origin_x: f32,
    origin_y: f32,
    stamp: Stamp,
) {
    let Stamp {
        x_mm,
        y_mm,
        rx,
        ry,
        sin,
        cos,
        amplitude,
        layer,
    } = stamp;
    let major = rx.max(ry);
    let px = x_mm / pitch - origin_x;
    let py = y_mm / pitch - origin_y;
    let rad = major / pitch;
    let x0 = ((px - rad - 0.5).ceil() as i32).max(0);
    let y0 = ((py - rad - 0.5).ceil() as i32).max(0);
    let x1 = ((px + rad - 0.5).floor() as i32).min(width as i32 - 1);
    let y1 = ((py + rad - 0.5).floor() as i32).min(rows as i32 - 1);
    if x0 > x1 || y0 > y1 || rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let inv_rx = 1.0 / rx;
    let inv_ry = 1.0 / ry;
    let width = width as usize;
    for y in y0 as usize..=y1 as usize {
        let dy = (origin_y + y as f32 + 0.5) * pitch - y_mm;
        let row_start = y * width * 3;
        for x in x0 as usize..=x1 as usize {
            let dx = (origin_x + x as f32 + 0.5) * pitch - x_mm;
            let dxr = dx * cos + dy * sin;
            let dyr = -dx * sin + dy * cos;
            let d2 = (dxr * inv_rx) * (dxr * inv_rx) + (dyr * inv_ry) * (dyr * inv_ry);
            if d2 < 1.0 {
                field[row_start + x * 3 + layer] += amplitude * (1.0 - d2);
            }
        }
    }
}

/// On-disk model tag.
///
/// `parametric` stays the tag existing files already use. `spectral` adds
/// sensitivity samples, and dye density when the stock has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StockModel {
    /// Per-layer D-logH curves and optional log-exposure coupling.
    Parametric,
    /// Spectral sensitivities into those same curves, plus optional dye density.
    Spectral,
}

impl fmt::Display for StockModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Parametric => "parametric",
            Self::Spectral => "spectral",
        })
    }
}

/// Name, provenance, and model tag for [`Stock::from_parts`].
///
/// The save dialog fills this. The film parameters travel separately, and
/// [`Stock::from_parts`] rejects a tag that does not match the film model.
#[derive(Debug, Clone)]
pub struct StockMeta {
    revision: u32,
    name: String,
    source: String,
    license: String,
    model: StockModel,
}

impl StockMeta {
    /// `name`, `source`, and `license` must contain text.
    pub fn new(
        revision: u32,
        name: impl Into<String>,
        source: impl Into<String>,
        license: impl Into<String>,
        model: StockModel,
    ) -> Result<Self, StockError> {
        let name = name.into();
        let source = source.into();
        let license = license.into();
        require_text("name", &name)?;
        require_text("source", &source)?;
        require_text("license", &license)?;
        Ok(Self {
            revision,
            name,
            source,
            license,
            model,
        })
    }

    /// Stock revision, independent of the schema version.
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

    /// Model tag. It has to match the [`FilmModel`] passed to [`Stock::from_parts`].
    pub fn model(&self) -> StockModel {
        self.model
    }
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

    /// A spectral stock omitted a table the model needs.
    #[error("spectral stock is missing {field}")]
    MissingSpectral {
        /// `wavelength_nm` or `sensitivity`.
        field: &'static str,
    },

    /// A parametric stock set a field only the spectral model reads.
    #[error("parametric stock cannot include {field}")]
    NotParametric {
        /// The field that does not belong on a parametric stock.
        field: &'static str,
    },

    /// A spectral array does not match the shared wavelength grid.
    #[error("{field} has {len} samples, expected {expected}")]
    SpectralLength {
        /// Which array was the wrong length.
        field: &'static str,
        /// Samples found.
        len: usize,
        /// Samples the wavelength grid requires.
        expected: usize,
    },

    /// The model tag does not match the film model.
    #[error("stock model is {model:?}, but the film model is the other kind")]
    ModelMismatch {
        /// The tag [`StockMeta`] carried.
        model: StockModel,
    },

    /// The stock file could not be written.
    #[error("failed to write stock file {path}")]
    Write {
        /// The path that failed.
        path: String,
        /// The filesystem error.
        #[source]
        source: std::io::Error,
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
    /// Shared wavelength grid, in nanometres. Spectral stocks only.
    #[serde(default, deserialize_with = "de_option_f32_vec")]
    wavelength_nm: Option<Vec<f32>>,
    /// Per-layer spectral sensitivity. Spectral stocks only.
    #[serde(default)]
    sensitivity: Option<RgbSamples>,
    /// Per-layer spectral dye density. Optional on a spectral stock.
    #[serde(default)]
    dye_density: Option<RgbSamples>,
    /// Log exposure the spectral print balances to neutral.
    #[serde(default, deserialize_with = "de_option_f32")]
    reference_log_exposure: Option<f32>,
}

/// Three sampled spectra on the stock's wavelength grid.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RgbSamples {
    #[serde(deserialize_with = "de_f32_vec")]
    red: Vec<f32>,
    #[serde(deserialize_with = "de_f32_vec")]
    green: Vec<f32>,
    #[serde(deserialize_with = "de_f32_vec")]
    blue: Vec<f32>,
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
    /// Characteristic dye-cloud size, in millimetres. Omitted layers keep
    /// [`Grain::DEFAULT_SIZE_MM`].
    #[serde(default, deserialize_with = "de_opt_f32")]
    size_mm: Option<f32>,
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

    /// Parse stock text. [`Self::load`] reads the file, then calls this.
    ///
    /// Wasm hosts and the save dialog's preview use this so they never need
    /// a path. Parse errors name the input `<input>`.
    pub fn parse(text: &str) -> Result<Self, StockError> {
        Self::from_toml(text, Path::new("<input>"))
    }

    /// Build a stock from the save dialog's fields and the current parameters.
    ///
    /// `meta.model` has to be the same kind as `parameters`' film model.
    pub fn from_parts(meta: StockMeta, parameters: Parameters) -> Result<Self, StockError> {
        let matches = matches!(
            (meta.model, parameters.film()),
            (StockModel::Parametric, FilmModel::Parametric(_))
                | (StockModel::Spectral, FilmModel::Spectral(_))
        );
        if !matches {
            return Err(StockError::ModelMismatch { model: meta.model });
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            revision: meta.revision,
            name: meta.name,
            source: meta.source,
            license: meta.license,
            model: meta.model,
            parameters,
        })
    }

    /// Schema 1 text. Optional tables are omitted when they are off.
    ///
    /// The first line says rawstock wrote the file and that comments are not
    /// kept. Floats use a decimal that round-trips through `f32`.
    pub fn to_toml(&self) -> String {
        StockText::write(self)
    }

    /// Write [`Self::to_toml`] to `path`.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), StockError> {
        let path = path.as_ref();
        std::fs::write(path, self.to_toml()).map_err(|source| StockError::Write {
            path: path.display().to_string(),
            source,
        })
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
        let coupling = file.coupling.as_ref().map(|coupling| coupling.matrix);
        let film = match file.model {
            StockModel::Parametric => {
                reject_spectral_fields(&file)?;
                FilmModel::Parametric(
                    Parametric::new([red, green, blue], coupling).map_err(StockError::Model)?,
                )
            }
            StockModel::Spectral => {
                let wavelengths =
                    file.wavelength_nm
                        .clone()
                        .ok_or(StockError::MissingSpectral {
                            field: "wavelength_nm",
                        })?;
                let sensitivity = file
                    .sensitivity
                    .as_ref()
                    .ok_or(StockError::MissingSpectral {
                        field: "sensitivity",
                    })?;
                let sensitivity = rgb_samples("sensitivity", sensitivity, wavelengths.len())?;
                let dye_density = file
                    .dye_density
                    .as_ref()
                    .map(|samples| rgb_samples("dye_density", samples, wavelengths.len()))
                    .transpose()?;
                FilmModel::Spectral(Box::new(
                    Spectral::new(
                        wavelengths,
                        sensitivity,
                        dye_density,
                        [red, green, blue],
                        coupling,
                        file.reference_log_exposure,
                    )
                    .map_err(StockError::Model)?,
                ))
            }
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
    let mut grain = Grain::new(samples.frame_mm, samples.seed, [red, green, blue])
        .map_err(StockError::Model)?;
    if samples.red.size_mm.is_some()
        || samples.green.size_mm.is_some()
        || samples.blue.size_mm.is_some()
    {
        let size_mm = [
            samples.red.size_mm.unwrap_or(Grain::DEFAULT_SIZE_MM[0]),
            samples.green.size_mm.unwrap_or(Grain::DEFAULT_SIZE_MM[1]),
            samples.blue.size_mm.unwrap_or(Grain::DEFAULT_SIZE_MM[2]),
        ];
        grain = grain.with_size_mm(size_mm).map_err(StockError::Model)?;
    }
    Ok(grain)
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
fn widen(
    samples: &mut [f32],
    width: usize,
    height: usize,
    sigma: f32,
    control: &mut dyn crate::render::DevelopControl,
) -> Result<(), crate::render::Cancelled> {
    if !sigma.is_finite() || sigma < 0.5 || width == 0 || height == 0 {
        return Ok(());
    }
    let mut scratch = vec![0.0_f32; samples.len()];
    for radius in box_radii(sigma) {
        if control.cancelled() {
            return Err(crate::render::Cancelled);
        }
        if radius == 0 {
            continue;
        }
        box_blur_horizontal(samples, &mut scratch, width, height, radius);
        if control.cancelled() {
            return Err(crate::render::Cancelled);
        }
        box_blur_vertical(&scratch, samples, width, height, radius);
    }
    Ok(())
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

fn reject_spectral_fields(file: &StockFile) -> Result<(), StockError> {
    if file.wavelength_nm.is_some() {
        return Err(StockError::NotParametric {
            field: "wavelength_nm",
        });
    }
    if file.sensitivity.is_some() {
        return Err(StockError::NotParametric {
            field: "sensitivity",
        });
    }
    if file.dye_density.is_some() {
        return Err(StockError::NotParametric {
            field: "dye_density",
        });
    }
    if file.reference_log_exposure.is_some() {
        return Err(StockError::NotParametric {
            field: "reference_log_exposure",
        });
    }
    Ok(())
}

fn rgb_samples(
    table: &'static str,
    samples: &RgbSamples,
    expected: usize,
) -> Result<[Vec<f32>; 3], StockError> {
    for (name, values) in [
        ("red", &samples.red),
        ("green", &samples.green),
        ("blue", &samples.blue),
    ] {
        if values.len() != expected {
            return Err(StockError::SpectralLength {
                field: match (table, name) {
                    ("sensitivity", "red") => "sensitivity.red",
                    ("sensitivity", "green") => "sensitivity.green",
                    ("sensitivity", "blue") => "sensitivity.blue",
                    ("dye_density", "red") => "dye_density.red",
                    ("dye_density", "green") => "dye_density.green",
                    _ => "dye_density.blue",
                },
                len: values.len(),
                expected,
            });
        }
    }
    Ok([
        samples.red.clone(),
        samples.green.clone(),
        samples.blue.clone(),
    ])
}

fn de_option_f32<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    de_f32(deserializer).map(Some)
}

fn de_option_f32_vec<'de, D>(deserializer: D) -> Result<Option<Vec<f32>>, D::Error>
where
    D: Deserializer<'de>,
{
    de_f32_vec(deserializer).map(Some)
}

fn de_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(F32Visitor)
}

fn de_opt_f32<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: Deserializer<'de>,
{
    de_f32(deserializer).map(Some)
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

fn trapezoid_weights(wavelengths: &[f32]) -> Vec<f32> {
    let mut weights = vec![0.0f32; wavelengths.len()];
    for index in 0..wavelengths.len() - 1 {
        let half = 0.5 * (wavelengths[index + 1] - wavelengths[index]);
        weights[index] += half;
        weights[index + 1] += half;
    }
    weights
}

fn integrate(wavelengths: &[f32], values: &[f32]) -> f32 {
    let mut total = 0.0f32;
    for index in 0..wavelengths.len() - 1 {
        let half = 0.5 * (wavelengths[index + 1] - wavelengths[index]);
        total += half * (values[index] + values[index + 1]);
    }
    total
}

fn mix(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * value[0] + matrix[0][1] * value[1] + matrix[0][2] * value[2],
        matrix[1][0] * value[0] + matrix[1][1] * value[1] + matrix[1][2] * value[2],
        matrix[2][0] * value[0] + matrix[2][1] * value[1] + matrix[2][2] * value[2],
    ]
}

/// Schema 1 writer. Key order follows `stocks/README.md`.
struct StockText {
    out: String,
}

impl StockText {
    fn write(stock: &Stock) -> String {
        let mut text = Self {
            out: String::from("# Written by rawstock. Comments are not preserved.\n"),
        };
        text.line("");
        text.key_int("schema_version", stock.schema_version);
        text.key_int("revision", stock.revision);
        text.key_str("name", &stock.name);
        text.key_str("source", &stock.source);
        text.key_str("license", &stock.license);
        text.key_raw("model", &stock.model.to_string());
        text.key_float("exposure_ev", stock.parameters.exposure_ev());
        text.key_float("print_exposure", stock.parameters.print_exposure());
        if let FilmModel::Spectral(model) = stock.parameters.film() {
            if let Some(reference) = model.reference_log_exposure() {
                text.key_float("reference_log_exposure", reference);
            }
            text.key_floats("wavelength_nm", model.wavelengths());
        }
        text.line("");
        let (curves, coupling) = match stock.parameters.film() {
            FilmModel::Parametric(model) => (model.curves(), model.coupling()),
            FilmModel::Spectral(model) => (model.curves(), model.coupling()),
        };
        for (name, curve) in ["red", "green", "blue"].into_iter().zip(curves) {
            text.curve(name, curve);
        }
        text.curve("print", stock.parameters.print_curve());
        if let Some(matrix) = coupling {
            text.line("");
            text.table("coupling");
            text.matrix(matrix);
        }
        if let Some(grain) = stock.parameters.grain() {
            text.line("");
            text.table("grain");
            text.key_float("frame_mm", grain.frame_mm());
            text.key_int("seed", grain.seed());
            for (index, (name, layer)) in ["red", "green", "blue"]
                .into_iter()
                .zip(grain.layers())
                .enumerate()
            {
                text.table(&format!("grain.{name}"));
                let (density, rms): (Vec<f32>, Vec<f32>) = layer.points().unzip();
                text.key_floats("density", &density);
                text.key_floats("rms", &rms);
                text.key_float("size_mm", grain.size_mm()[index]);
            }
        }
        if let Some(halation) = stock.parameters.halation() {
            text.line("");
            text.table("halation");
            text.key_float("threshold", halation.threshold());
            text.key_float("radius", halation.radius());
            text.key_floats("weight", &halation.weight());
        }
        if let FilmModel::Spectral(model) = stock.parameters.film() {
            text.line("");
            text.rgb_table("sensitivity", model.sensitivities());
            if let Some(dyes) = model.dye_density() {
                text.line("");
                text.rgb_table("dye_density", dyes);
            }
        }
        text.out.push('\n');
        text.out
    }

    fn curve(&mut self, name: &str, curve: &Curve) {
        self.line("");
        self.table(name);
        let (log_h, density): (Vec<f32>, Vec<f32>) = curve.points().unzip();
        self.key_floats("log_exposure", &log_h);
        self.key_floats("density", &density);
    }

    fn rgb_table(&mut self, name: &str, channels: &[Vec<f32>; 3]) {
        self.table(name);
        self.key_floats("red", &channels[0]);
        self.key_floats("green", &channels[1]);
        self.key_floats("blue", &channels[2]);
    }

    fn matrix(&mut self, matrix: [[f32; 3]; 3]) {
        let rows: Vec<String> = matrix.iter().map(|row| format_f32_array(row)).collect();
        self.line(&format!("matrix = [{}]", rows.join(", ")));
    }

    fn table(&mut self, name: &str) {
        self.line(&format!("[{name}]"));
    }

    fn key_int(&mut self, key: &str, value: u32) {
        self.line(&format!("{key} = {value}"));
    }

    fn key_float(&mut self, key: &str, value: f32) {
        self.line(&format!("{key} = {}", format_f32(value)));
    }

    fn key_floats(&mut self, key: &str, values: &[f32]) {
        self.line(&format!("{key} = {}", format_f32_array(values)));
    }

    fn key_str(&mut self, key: &str, value: &str) {
        self.line(&format!("{key} = \"{}\"", escape_toml(value)));
    }

    fn key_raw(&mut self, key: &str, value: &str) {
        self.line(&format!("{key} = \"{value}\""));
    }

    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }
}

fn format_f32_array(values: &[f32]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format_f32(*value));
    }
    out.push(']');
    out
}

/// Shortest debug decimal, which round-trips through `f32`.
fn format_f32(value: f32) -> String {
    let text = format!("{value:?}");
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        format!("{text}.0")
    }
}

fn escape_toml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
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

    fn flat_sensitivity(wavelengths: &[f32]) -> Vec<f32> {
        let integral = integrate(wavelengths, &vec![1.0; wavelengths.len()]);
        vec![1.0 / integral; wavelengths.len()]
    }

    #[test]
    fn spectral_neutral_gray_matches_parametric() {
        use crate::color::LinearImage;
        use crate::render::{Parameters, Pipeline};

        let wavelengths: Vec<f32> = (0..41).map(|index| 380.0 + index as f32 * 10.0).collect();
        let flat = flat_sensitivity(&wavelengths);
        let curve = sample_curve();
        let print = Curve::new([(-1.5, 0.05), (0.0, 0.50), (1.0, 1.30), (2.5, 2.40)]).unwrap();
        let spectral = FilmModel::Spectral(Box::new(
            Spectral::new(
                wavelengths,
                [flat.clone(), flat.clone(), flat],
                None,
                [curve.clone(), curve.clone(), curve.clone()],
                None,
                None,
            )
            .unwrap(),
        ));
        let parametric = FilmModel::Parametric(
            Parametric::new([curve.clone(), curve.clone(), curve], None).unwrap(),
        );
        let levels = [0.02_f32, 0.18, 0.5, 1.0, 2.5];
        let mut pixels = Vec::new();
        for gray in levels {
            pixels.extend([gray, gray, gray]);
        }
        let image = LinearImage::new(levels.len() as u32, 1, pixels).unwrap();
        let spectral = Pipeline::new(Parameters::new(0.0, spectral, 0.8, print.clone()).unwrap())
            .develop(&image);
        let parametric =
            Pipeline::new(Parameters::new(0.0, parametric, 0.8, print).unwrap()).develop(&image);
        for (got, want) in spectral.pixels().iter().zip(parametric.pixels()) {
            assert!(
                (got - want).abs() < 1.0e-4,
                "spectral gray {got} diverged from parametric {want}"
            );
        }
        for pixel in spectral.pixels().as_chunks::<3>().0 {
            let spread =
                pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2]);
            assert!(spread < 1.0e-4, "spectral gray drifted: {pixel:?}");
        }
    }

    #[test]
    fn spectral_color_follows_sensitivity() {
        let wavelengths: Vec<f32> = (0..41).map(|index| 380.0 + index as f32 * 10.0).collect();
        let mut red = vec![0.0f32; wavelengths.len()];
        let mut blue = vec![0.0f32; wavelengths.len()];
        for (index, wave) in wavelengths.iter().enumerate() {
            if *wave >= 600.0 {
                red[index] = 1.0;
            }
            if *wave <= 480.0 {
                blue[index] = 1.0;
            }
        }
        let red = {
            let scale = integrate(&wavelengths, &red);
            red.iter().map(|sample| sample / scale).collect::<Vec<_>>()
        };
        let blue = {
            let scale = integrate(&wavelengths, &blue);
            blue.iter().map(|sample| sample / scale).collect::<Vec<_>>()
        };
        let green = flat_sensitivity(&wavelengths);
        let curve = Curve::new([(-4.0, -4.0), (4.0, 4.0)]).unwrap();
        let model = Spectral::new(
            wavelengths,
            [red, green, blue],
            None,
            [curve.clone(), curve.clone(), curve],
            None,
            None,
        )
        .unwrap();
        // ACEScg red should put more exposure on the long-wavelength layer
        // than on the short-wavelength layer. The identity curve reports
        // log-exposure as density.
        let density = FilmModel::Spectral(Box::new(model)).density(&[1.0, 0.0, 0.0]);
        assert!(
            density[0] > density[2] + 0.3,
            "red light did not prefer the red layer: {density:?}"
        );
    }

    #[test]
    fn sensitivity_must_be_normalized_and_non_negative() {
        let wavelengths = vec![400.0, 500.0, 600.0, 700.0];
        let flat = flat_sensitivity(&wavelengths);
        let curve = sample_curve();
        let curves = [curve.clone(), curve.clone(), curve];
        let mut negative = flat.clone();
        negative[0] = -0.01;
        let err = Spectral::new(
            wavelengths.clone(),
            [negative, flat.clone(), flat.clone()],
            None,
            curves.clone(),
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::Sensitivity));

        let err = Spectral::new(
            wavelengths,
            [vec![1.0, 1.0, 1.0, 1.0], flat.clone(), flat],
            None,
            curves,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::Sensitivity));
    }

    #[test]
    fn crop_grain_matches_the_full_frame() {
        let layer = Granularity::new([(0.0, 40.0), (3.0, 40.0)]).unwrap();
        let grain = Grain::new(35.0, 9, [layer.clone(), layer.clone(), layer]).unwrap();
        let frame_width = 96;
        let frame_height = 40;
        let mut full = vec![1.0_f32; frame_width as usize * frame_height as usize * 3];
        grain.apply(&mut full, frame_width, frame_height);
        let origin_x = 17;
        let origin_y = 9;
        let width = 20;
        let height = 12;
        let mut crop = vec![1.0_f32; width as usize * height as usize * 3];
        grain
            .apply_region(
                &mut crop,
                GrainRegion {
                    width,
                    height,
                    origin_x,
                    origin_y,
                    frame_width,
                },
                &mut crate::render::NopControl,
            )
            .unwrap();
        for y in 0..height as usize {
            for x in 0..width as usize {
                for channel in 0..3 {
                    let full_index =
                        ((origin_y as usize + y) * frame_width as usize + origin_x as usize + x)
                            * 3
                            + channel;
                    let crop_index = (y * width as usize + x) * 3 + channel;
                    assert_eq!(
                        crop[crop_index].to_bits(),
                        full[full_index].to_bits(),
                        "pixel {x},{y} channel {channel}"
                    );
                }
            }
        }
    }
}
