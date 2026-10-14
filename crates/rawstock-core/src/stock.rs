//! Film model: scene exposure to per-layer density.
//!
//! Spectral (wavelength sensitivity and dye density) is intentionally not a
//! variant yet. The only model is [`FilmModel::Parametric`].

use thiserror::Error;

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
