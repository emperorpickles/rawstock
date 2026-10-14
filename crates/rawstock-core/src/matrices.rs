//! Constants for the scene-linear camera → ACEScg conversion.
//!
//! A developed camera RGB value (black/white normalized, as-shot white
//! balance applied) becomes ACEScg by:
//!
//! 1. camera RGB → CIE XYZ, inverting the file's XYZ→camera matrix after
//!    row-scaling it so D65 lands on camera white;
//! 2. Bradford chromatic adaptation from D65 to the ACES white;
//! 3. CIE XYZ (ACES white) → ACEScg (AP1).
//!
//! The numbers below are the cited inputs. The 3×3 transforms are derived
//! from them.

/// Bradford cone response, XYZ → LMS, acting on column vectors.
///
/// Source: K. M. Lam, *Metamerism and Colour Constancy*, University of
/// Bradford, 1985, in the form published by Bruce Lindbloom
/// (<http://www.brucelindbloom.com/Eqn_ChromAdapt.html>). ACES stores the
/// transpose as `CONE_RESP_MAT_BRADFORD` in `ACESlib.Utilities_Color.ctl`
/// because CTL multiplies row vectors on the right.
pub(crate) const BRADFORD_XYZ_TO_LMS: [[f64; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// CIE 1931 xy of illuminant D65, the white ACES utility transforms use for
/// D65 (`REC709_PRI` in `ACESlib.Utilities_Color.ctl`). XYZ with Y = 1 is
/// derived from this chromaticity rather than from the rounded tristimulus
/// (0.95047, 1, 1.08883).
pub(crate) const D65_XY: [f64; 2] = [0.3127, 0.3290];

/// ACES / ACEScg white, CIE 1931 xy.
///
/// Source: SMPTE ST 2065-1 and the ACEScg specification
/// (<https://docs.acescentral.com/specifications/acescg/>).
pub(crate) const ACES_WHITE_XY: [f64; 2] = [0.32168, 0.33767];

/// AP1 primaries, CIE 1931 xy, in red, green, blue order.
///
/// Source: ACEScg specification, "Color space chromaticities".
pub(crate) const AP1_PRIMARIES_XY: [[f64; 2]; 3] = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];

/// CIE XYZ (Y = 1) of the D65 chromaticity in [`D65_XY`].
pub(crate) fn d65_xyz() -> [f64; 3] {
    xy_to_xyz(D65_XY)
}

/// CIE XYZ (Y = 1) of the ACES white in [`ACES_WHITE_XY`].
pub(crate) fn aces_white_xyz() -> [f64; 3] {
    xy_to_xyz(ACES_WHITE_XY)
}

/// Bradford adaptation that maps D65 onto the ACES white.
pub(crate) fn bradford_d65_to_aces() -> [[f64; 3]; 3] {
    let source = apply(BRADFORD_XYZ_TO_LMS, d65_xyz());
    let destination = apply(BRADFORD_XYZ_TO_LMS, aces_white_xyz());
    let scale = [
        [destination[0] / source[0], 0.0, 0.0],
        [0.0, destination[1] / source[1], 0.0],
        [0.0, 0.0, destination[2] / source[2]],
    ];
    // The Bradford matrix is a fixed, invertible cone response.
    let inverse_bradford = inverse(BRADFORD_XYZ_TO_LMS).expect("Bradford matrix is invertible");
    multiply(inverse_bradford, multiply(scale, BRADFORD_XYZ_TO_LMS))
}

/// XYZ (relative to the ACES white) → ACEScg.
///
/// Derived from [`AP1_PRIMARIES_XY`] and [`ACES_WHITE_XY`] by SMPTE RP 177:1993.
pub(crate) fn xyz_d60_to_ap1() -> [[f64; 3]; 3] {
    let columns = AP1_PRIMARIES_XY.map(xy_to_xyz);
    // Primary XYZ values are columns.
    let primaries = [
        [columns[0][0], columns[1][0], columns[2][0]],
        [columns[0][1], columns[1][1], columns[2][1]],
        [columns[0][2], columns[1][2], columns[2][2]],
    ];
    let inverse_primaries = inverse(primaries).expect("AP1 primaries are linearly independent");
    let scales = apply(inverse_primaries, aces_white_xyz());
    let rgb_to_xyz = [
        [
            primaries[0][0] * scales[0],
            primaries[0][1] * scales[1],
            primaries[0][2] * scales[2],
        ],
        [
            primaries[1][0] * scales[0],
            primaries[1][1] * scales[1],
            primaries[1][2] * scales[2],
        ],
        [
            primaries[2][0] * scales[0],
            primaries[2][1] * scales[1],
            primaries[2][2] * scales[2],
        ],
    ];
    inverse(rgb_to_xyz).expect("AP1 RGB to XYZ matrix is invertible")
}

/// CIE XYZ under D65 → ACEScg: AP1 after the Bradford D65 → ACES adaptation.
pub(crate) fn xyz_d65_to_acescg() -> [[f64; 3]; 3] {
    multiply(xyz_d60_to_ap1(), bradford_d65_to_aces())
}

/// Camera RGB → ACEScg for a D65 XYZ→camera matrix, before white balance.
///
/// The matrix is row-scaled first so that D65 XYZ lands on camera
/// `(1, 1, 1)`. As-shot white balance is defined the same way: it makes the
/// scene neutral equal camera RGB, not the unscaled `ColorMatrix * D65`
/// response. dcraw normalizes `xyz_to_cam` before inverting it (see rawler's
/// `cam_to_xyz_normalized`, which targets equal-energy white). The scale
/// here targets D65, which is the source white of [`bradford_d65_to_aces`].
pub(crate) fn camera_to_acescg(xyz_to_camera: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let normalized = normalize_xyz_to_camera(xyz_to_camera)?;
    let camera_to_xyz = inverse(normalized)?;
    Some(multiply(xyz_d65_to_acescg(), camera_to_xyz))
}

/// Scale each row so `matrix * D65 = (1, 1, 1)`.
fn normalize_xyz_to_camera(xyz_to_camera: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let camera_white = apply(xyz_to_camera, d65_xyz());
    if camera_white
        .iter()
        .any(|channel| !channel.is_finite() || *channel <= 1.0e-8)
    {
        return None;
    }
    let mut normalized = xyz_to_camera;
    for (row, scale) in normalized.iter_mut().zip(camera_white) {
        for value in row.iter_mut() {
            *value /= scale;
        }
    }
    Some(normalized)
}

pub(crate) fn apply(matrix: [[f64; 3]; 3], value: [f64; 3]) -> [f64; 3] {
    [
        matrix[0][0] * value[0] + matrix[0][1] * value[1] + matrix[0][2] * value[2],
        matrix[1][0] * value[0] + matrix[1][1] * value[1] + matrix[1][2] * value[2],
        matrix[2][0] * value[0] + matrix[2][1] * value[1] + matrix[2][2] * value[2],
    ]
}

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let (x, y) = (xy[0], xy[1]);
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn multiply(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = left[row][0] * right[0][col]
                + left[row][1] * right[1][col]
                + left[row][2] * right[2][col];
        }
    }
    out
}

fn inverse(matrix: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = determinant(matrix);
    if !det.is_finite() || det.abs() < 1.0e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        [
            (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1]) * inv_det,
            (matrix[0][2] * matrix[2][1] - matrix[0][1] * matrix[2][2]) * inv_det,
            (matrix[0][1] * matrix[1][2] - matrix[0][2] * matrix[1][1]) * inv_det,
        ],
        [
            (matrix[1][2] * matrix[2][0] - matrix[1][0] * matrix[2][2]) * inv_det,
            (matrix[0][0] * matrix[2][2] - matrix[0][2] * matrix[2][0]) * inv_det,
            (matrix[0][2] * matrix[1][0] - matrix[0][0] * matrix[1][2]) * inv_det,
        ],
        [
            (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]) * inv_det,
            (matrix[0][1] * matrix[2][0] - matrix[0][0] * matrix[2][1]) * inv_det,
            (matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]) * inv_det,
        ],
    ])
}

fn determinant(matrix: [[f64; 3]; 3]) -> f64 {
    matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XYZ→AP1 rounded to 10 digits, as published for `RGB_COLOURSPACE_ACESCG`
    /// by colour-science from the ACEScg chromaticities (SMPTE RP 177).
    const PUBLISHED_XYZ_TO_AP1: [[f64; 3]; 3] = [
        [1.6410233797, -0.3248032942, -0.2364246952],
        [-0.6636628587, 1.6153315917, 0.0167563477],
        [0.0117218943, -0.0082844420, 0.9883948585],
    ];

    fn assert_vec(actual: [f64; 3], expected: [f64; 3], tolerance: f64) {
        for (got, want) in actual.into_iter().zip(expected) {
            assert!(
                (got - want).abs() <= tolerance,
                "{actual:?} != {expected:?} (tolerance {tolerance})"
            );
        }
    }

    #[test]
    fn d65_white_maps_to_acescg_white() {
        let acescg = apply(xyz_d65_to_acescg(), d65_xyz());
        assert_vec(acescg, [1.0, 1.0, 1.0], 1.0e-9);
    }

    #[test]
    fn aces_white_maps_to_acescg_white() {
        let acescg = apply(xyz_d60_to_ap1(), aces_white_xyz());
        assert_vec(acescg, [1.0, 1.0, 1.0], 1.0e-9);
    }

    #[test]
    fn xyz_to_ap1_matches_published_matrix() {
        let matrix = xyz_d60_to_ap1();
        for (row, expected) in matrix.into_iter().zip(PUBLISHED_XYZ_TO_AP1) {
            assert_vec(row, expected, 1.0e-10);
        }
    }

    #[test]
    fn known_xyz_maps_to_acescg() {
        // Linear sRGB red, from the sRGB→XYZ matrix published by Bruce Lindbloom
        // (the IEC 61966-2-1 primary with more than four decimal places).
        let xyz = [0.4124564, 0.2126729, 0.0193339];
        let acescg = apply(xyz_d65_to_acescg(), xyz);
        assert_vec(
            acescg,
            [
                0.6131949071903661,
                0.07020499916444305,
                0.020618877756098185,
            ],
            1.0e-9,
        );

        // A second point that is not a white: CIE XYZ (1, 1, 1).
        let acescg = apply(xyz_d65_to_acescg(), [1.0, 1.0, 1.0]);
        assert_vec(
            acescg,
            [1.1037804380569356, 0.9657640010473456, 0.9190790055106011],
            1.0e-9,
        );
    }

    #[test]
    fn singular_camera_matrix_is_rejected() {
        let singular = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]];
        assert!(camera_to_acescg(singular).is_none());
    }
}
