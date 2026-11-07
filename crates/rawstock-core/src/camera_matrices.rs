//! The camera's XYZ → camera RGB color matrices, one per calibration light.
//!
//! A raw file can carry two color matrices, each measured under its own
//! illuminant: DNG `ColorMatrix1` / `ColorMatrix2` with their
//! `CalibrationIlluminant` tags, or the pair rawler ships for a camera
//! (Fujifilm bodies have one for illuminant A and one for D65). For a scene
//! lit at some temperature, the DNG specification interpolates the two
//! matrices linearly in inverse temperature (mireds), clamped to the pair,
//! and uses one matrix as is when only one exists. [`CameraMatrices`] does
//! that. Finding the scene's temperature from the as-shot balance needs the
//! matrix it is looking for; [`crate::white_balance::WhiteBalanceSolver`]
//! iterates the two, as the DNG SDK does.

use rawler::imgop::xyz::Illuminant;

use crate::matrices;

/// One or two XYZ → camera RGB matrices and the temperatures they were
/// measured at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraMatrices {
    /// The calibration under the warmer (lower kelvin) light.
    warm: Calibration,
    /// The calibration under the cooler light. Equal to `warm` when the
    /// file has only one matrix.
    cool: Calibration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Calibration {
    kelvin: f64,
    /// XYZ → camera RGB, rows red, green, blue.
    xyz_to_camera: [[f64; 3]; 3],
}

impl CameraMatrices {
    /// Correlated color temperature of CIE D65, in kelvin.
    pub const D65_KELVIN: f64 = 6504.0;

    /// A single matrix, used for every light. Rows are camera red, green,
    /// blue; a fourth RGBE row is ignored. It is labeled D65, which only
    /// matters to [`Self::calibrations`].
    pub fn single(xyz_to_camera: [[f32; 3]; 4]) -> Self {
        let calibration = Calibration::new(Self::D65_KELVIN, xyz_to_camera);
        Self {
            warm: calibration,
            cool: calibration,
        }
    }

    /// Two matrices, each with the temperature of the light it was measured
    /// under, in either order. Two matrices at the same temperature, or a
    /// temperature that is not a positive number, keep only the first.
    pub fn dual(first: (f64, [[f32; 3]; 4]), second: (f64, [[f32; 3]; 4])) -> Self {
        let valid = |kelvin: f64| kelvin.is_finite() && kelvin > 0.0;
        if !valid(first.0) || !valid(second.0) || first.0 == second.0 {
            let mut matrices = Self::single(first.1);
            if valid(first.0) {
                matrices.warm.kelvin = first.0;
                matrices.cool.kelvin = first.0;
            }
            return matrices;
        }
        let first = Calibration::new(first.0, first.1);
        let second = Calibration::new(second.0, second.1);
        let (warm, cool) = if first.kelvin < second.kelvin {
            (first, second)
        } else {
            (second, first)
        };
        Self { warm, cool }
    }

    /// The calibration matrices and their temperatures, warmest first: one
    /// entry, or two.
    pub fn calibrations(&self) -> Vec<(f64, [[f32; 3]; 3])> {
        let entry = |calibration: Calibration| {
            (
                calibration.kelvin,
                calibration
                    .xyz_to_camera
                    .map(|row| row.map(|value| value as f32)),
            )
        };
        if self.is_dual() {
            vec![entry(self.warm), entry(self.cool)]
        } else {
            vec![entry(self.warm)]
        }
    }

    /// Whether the file gave two matrices to interpolate between.
    pub fn is_dual(&self) -> bool {
        self.warm != self.cool
    }

    /// XYZ → camera RGB for a light at `kelvin`.
    ///
    /// Linear in mireds between the two calibrations, clamped to the nearer
    /// one outside them (DNG specification, "Mapping Camera Color Space to
    /// CIE XYZ Space").
    pub fn at_kelvin(&self, kelvin: f64) -> [[f64; 3]; 3] {
        if !self.is_dual() || !kelvin.is_finite() || kelvin <= 0.0 {
            return self.warm.xyz_to_camera;
        }
        let inverse = |value: f64| 1.0 / value;
        let weight = ((inverse(kelvin) - inverse(self.cool.kelvin))
            / (inverse(self.warm.kelvin) - inverse(self.cool.kelvin)))
        .clamp(0.0, 1.0);
        let mut matrix = [[0.0; 3]; 3];
        for (row, (warm, cool)) in matrix
            .iter_mut()
            .zip(self.warm.xyz_to_camera.iter().zip(&self.cool.xyz_to_camera))
        {
            for (value, (warm, cool)) in row.iter_mut().zip(warm.iter().zip(cool)) {
                *value = weight * warm + (1.0 - weight) * cool;
            }
        }
        matrix
    }

    /// Camera RGB → ACEScg, before white balance, for a scene lit at
    /// `kelvin` with chromaticity `white_xy`. `None` when that matrix does
    /// not invert or the light is outside what it can see.
    pub(crate) fn camera_to_acescg(
        &self,
        kelvin: f64,
        white_xy: [f64; 2],
    ) -> Option<[[f64; 3]; 3]> {
        matrices::camera_to_acescg(self.at_kelvin(kelvin), white_xy)
    }

    /// XYZ → camera RGBE at D65, in the shape rawler reports a matrix:
    /// rows red, green, blue, and an empty fourth row.
    pub fn at_d65(&self) -> [[f32; 3]; 4] {
        let matrix = self.at_kelvin(Self::D65_KELVIN);
        let mut rgbe = [[0.0f32; 3]; 4];
        for (row, values) in rgbe.iter_mut().zip(matrix) {
            *row = values.map(|value| value as f32);
        }
        rgbe
    }

    /// Correlated color temperature of a calibration illuminant, as the
    /// DNG SDK assigns it (CIE values for the standard illuminants). `None`
    /// for an unknown light.
    pub(crate) fn illuminant_kelvin(illuminant: Illuminant) -> Option<f64> {
        Some(match illuminant {
            Illuminant::Unknown => return None,
            Illuminant::A => 2856.0,
            Illuminant::Tungsten => 2850.0,
            Illuminant::IsoStudioTungsten => 3200.0,
            Illuminant::WhiteFluorescent => 3450.0,
            Illuminant::Fluorescent | Illuminant::CoolWhiteFluorescent => 4150.0,
            Illuminant::B => 4874.0,
            Illuminant::D50 => 5003.0,
            Illuminant::DaylightWhiteFluorescent => 5000.0,
            Illuminant::Daylight | Illuminant::FineWeather | Illuminant::Flash => 5500.0,
            Illuminant::D55 => 5503.0,
            Illuminant::DaylightFluorescent => 6430.0,
            Illuminant::CloudyWeather => 6500.0,
            Illuminant::D65 => Self::D65_KELVIN,
            Illuminant::C => 6774.0,
            Illuminant::Shade => 7500.0,
            Illuminant::D75 => 7504.0,
        })
    }
}

impl Calibration {
    fn new(kelvin: f64, xyz_to_camera: [[f32; 3]; 4]) -> Self {
        Self {
            kelvin,
            xyz_to_camera: [0, 1, 2].map(|row| xyz_to_camera[row].map(f64::from)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fujifilm X-T3, XYZ → camera, as rawler ships them.
    const XT3_A: [[f32; 3]; 4] = [
        [1.6064, -0.9942, 0.1120],
        [-0.3760, 1.2096, 0.1853],
        [-0.0007, 0.0545, 0.6654],
        [0.0, 0.0, 0.0],
    ];
    const XT3_D65: [[f32; 3]; 4] = [
        [1.3426, -0.6334, -0.1177],
        [-0.4244, 1.2136, 0.2371],
        [-0.0580, 0.1303, 0.5980],
        [0.0, 0.0, 0.0],
    ];

    fn close(a: [[f64; 3]; 3], b: [[f32; 3]; 4]) -> bool {
        (0..3).all(|row| (0..3).all(|col| (a[row][col] - f64::from(b[row][col])).abs() < 1.0e-9))
    }

    #[test]
    fn single_matrix_is_used_for_every_light() {
        let matrices = CameraMatrices::single(XT3_D65);
        assert!(!matrices.is_dual());
        for kelvin in [2000.0, 2856.0, 6504.0, 12000.0] {
            assert!(close(matrices.at_kelvin(kelvin), XT3_D65));
        }
        assert_eq!(matrices.calibrations().len(), 1);
    }

    #[test]
    fn dual_matrices_interpolate_in_mireds_and_clamp() {
        let matrices = CameraMatrices::dual((6504.0, XT3_D65), (2856.0, XT3_A));
        assert!(matrices.is_dual());
        assert!(close(matrices.at_kelvin(2856.0), XT3_A));
        assert!(close(matrices.at_kelvin(6504.0), XT3_D65));
        // Outside the pair: the nearer calibration.
        assert!(close(matrices.at_kelvin(1500.0), XT3_A));
        assert!(close(matrices.at_kelvin(20000.0), XT3_D65));
        // Halfway in mireds, not in kelvin.
        let middle = 2.0 / (1.0 / 2856.0 + 1.0 / 6504.0);
        let blended = matrices.at_kelvin(middle);
        let expected = 0.5 * (f64::from(XT3_A[0][0]) + f64::from(XT3_D65[0][0]));
        assert!((blended[0][0] - expected).abs() < 1.0e-9, "{blended:?}");
        let calibrations = matrices.calibrations();
        assert_eq!(calibrations.len(), 2);
        assert_eq!(calibrations[0].0, 2856.0);
    }

    #[test]
    fn a_degenerate_pair_keeps_one_matrix() {
        let matrices = CameraMatrices::dual((6504.0, XT3_D65), (6504.0, XT3_A));
        assert!(!matrices.is_dual());
        assert!(close(matrices.at_kelvin(2856.0), XT3_D65));
        let matrices = CameraMatrices::dual((f64::NAN, XT3_D65), (2856.0, XT3_A));
        assert!(close(matrices.at_kelvin(2856.0), XT3_D65));
    }

    #[test]
    fn standard_illuminants_have_their_temperatures() {
        assert_eq!(
            CameraMatrices::illuminant_kelvin(Illuminant::A),
            Some(2856.0)
        );
        assert_eq!(
            CameraMatrices::illuminant_kelvin(Illuminant::D65),
            Some(CameraMatrices::D65_KELVIN)
        );
        assert_eq!(CameraMatrices::illuminant_kelvin(Illuminant::Unknown), None);
    }
}
