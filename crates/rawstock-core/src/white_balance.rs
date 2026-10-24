//! White balance as a color temperature and tint, mapped through the
//! camera's own color matrix.
//!
//! A raw file stores its as-shot white balance as camera RGB multipliers.
//! A custom balance is a temperature in kelvin and a tint. The two meet in
//! CIE XYZ: the camera matrix (XYZ → camera RGB) takes the light's white
//! point to the camera's response to it, and the multipliers are the
//! reciprocal of that response with green at 1. Read backwards, the as-shot
//! multipliers give the camera's estimate of the light, which
//! [`WhiteBalanceSolver::as_shot`] reports as a temperature and tint. Feeding
//! that temperature and tint forward gives the as-shot multipliers back, so
//! switching from As Shot to Custom without moving a slider changes nothing.
//!
//! Temperature walks the Planckian locus, computed from Planck's law and the
//! CIE 1931 2 degree observer, in the CIE 1960 UCS (u, v) plane. Tint is the
//! distance off the locus along its normal, where one unit is 1/3000 of a
//! uv unit, the scale Adobe's DNG SDK uses. Positive tint means the light is
//! greener than a black body, so the developed image turns more magenta,
//! as in common raw developers. A higher temperature means the light was
//! bluer, so the image turns warmer.

use crate::cie1931::{CIE_LAMBDA_MIN, CIE_X, CIE_Y, CIE_Z};
use crate::matrices;

/// A white balance chosen for a develop.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhiteBalance {
    /// The multipliers the camera recorded.
    #[default]
    AsShot,
    /// A light at this temperature and tint.
    Custom(Temperature),
}

/// Correlated color temperature and tint of a light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temperature {
    /// Temperature in kelvin.
    pub kelvin: f64,
    /// Offset from the Planckian locus. Positive is greener light, which
    /// develops more magenta.
    pub tint: f64,
}

/// Turns temperature and tint into camera multipliers and back, for one
/// camera matrix.
#[derive(Debug, Clone, Copy)]
pub struct WhiteBalanceSolver {
    xyz_to_camera: [[f64; 3]; 3],
    camera_to_xyz: [[f64; 3]; 3],
    /// Green-normalized as-shot multipliers.
    as_shot: [f64; 3],
}

impl WhiteBalanceSolver {
    /// Lowest temperature [`Self::as_shot`] reports and custom accepts.
    pub const MIN_KELVIN: f64 = 1_000.0;
    /// Highest temperature [`Self::as_shot`] reports and custom accepts.
    pub const MAX_KELVIN: f64 = 40_000.0;
    /// Tint units per CIE 1960 uv unit.
    const TINT_SCALE: f64 = 3_000.0;
    /// Second radiation constant, metre kelvin (CODATA 2018).
    const C2: f64 = 1.438_776_877e-2;
    /// Half-step, in mireds, of the locus tangent's central difference.
    const TANGENT_STEP_MIRED: f64 = 0.05;

    /// `None` when the matrix cannot be inverted or the as-shot multipliers
    /// are missing.
    ///
    /// `xyz_to_camera` is the file's matrix as decoded: rows are camera
    /// red, green, blue (a fourth RGBE row is ignored). `as_shot_wb` is the
    /// file's multipliers in RGBE order.
    pub fn new(xyz_to_camera: [[f32; 3]; 4], as_shot_wb: [f32; 4]) -> Option<Self> {
        let xyz_to_camera = [0, 1, 2].map(|row| xyz_to_camera[row].map(f64::from));
        let camera_to_xyz = matrices::inverse(xyz_to_camera)?;
        let rgb = [as_shot_wb[0], as_shot_wb[1], as_shot_wb[2]].map(f64::from);
        if rgb.iter().any(|coeff| !coeff.is_finite() || *coeff <= 0.0) {
            return None;
        }
        let as_shot = [rgb[0] / rgb[1], 1.0, rgb[2] / rgb[1]];
        Some(Self {
            xyz_to_camera,
            camera_to_xyz,
            as_shot,
        })
    }

    /// Green-normalized camera RGB multipliers for `balance`.
    ///
    /// `None` when a custom light lands outside what the camera matrix can
    /// see (a channel would respond with zero or less), or the temperature
    /// is outside [`Self::MIN_KELVIN`]..=[`Self::MAX_KELVIN`].
    pub fn multipliers(&self, balance: WhiteBalance) -> Option<[f64; 3]> {
        let temperature = match balance {
            WhiteBalance::AsShot => return Some(self.as_shot),
            WhiteBalance::Custom(temperature) => temperature,
        };
        if !temperature.tint.is_finite()
            || !(Self::MIN_KELVIN..=Self::MAX_KELVIN).contains(&temperature.kelvin)
        {
            return None;
        }
        let xy = Locus::xy(temperature);
        let response = matrices::apply(self.xyz_to_camera, matrices::xy_to_xyz(xy));
        if response
            .iter()
            .any(|channel| !channel.is_finite() || *channel <= 1.0e-9)
        {
            return None;
        }
        Some([response[1] / response[0], 1.0, response[1] / response[2]])
    }

    /// The as-shot light as a temperature and tint.
    ///
    /// `None` when the light the camera recorded is not a real color. A
    /// temperature beyond [`Self::MIN_KELVIN`]..=[`Self::MAX_KELVIN`] is
    /// clamped, and then custom at that value is not exactly as shot.
    pub fn as_shot(&self) -> Option<Temperature> {
        let response = self.as_shot.map(|multiplier| 1.0 / multiplier);
        let xyz = matrices::apply(self.camera_to_xyz, response);
        let sum = xyz[0] + xyz[1] + xyz[2];
        if !sum.is_finite() || sum <= 0.0 || xyz[1] <= 0.0 {
            return None;
        }
        Locus::temperature([xyz[0] / sum, xyz[1] / sum])
    }
}

/// The Planckian locus in CIE 1960 UCS.
struct Locus;

impl Locus {
    /// Chromaticity xy of a light at `temperature`.
    fn xy(temperature: Temperature) -> [f64; 2] {
        let mired = 1.0e6 / temperature.kelvin;
        let point = Self::uv(mired);
        let normal = Self::normal(mired);
        let offset = temperature.tint / WhiteBalanceSolver::TINT_SCALE;
        Self::uv_to_xy([point[0] + offset * normal[0], point[1] + offset * normal[1]])
    }

    /// Temperature and tint of chromaticity `xy`: the locus point whose
    /// normal passes through it, and the signed distance along that normal.
    fn temperature(xy: [f64; 2]) -> Option<Temperature> {
        let uv = Self::xy_to_uv(xy)?;
        let low = 1.0e6 / WhiteBalanceSolver::MAX_KELVIN;
        let high = 1.0e6 / WhiteBalanceSolver::MIN_KELVIN;
        // Bracket the nearest locus point on a coarse mired grid, then
        // bisect where the offset turns perpendicular to the locus.
        const STEPS: usize = 400;
        let step = (high - low) / STEPS as f64;
        let distance = |mired: f64| {
            let point = Self::uv(mired);
            (uv[0] - point[0]).powi(2) + (uv[1] - point[1]).powi(2)
        };
        let nearest = (0..=STEPS)
            .map(|index| low + step * index as f64)
            .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))?;
        let along = |mired: f64| {
            let point = Self::uv(mired);
            let tangent = Self::tangent(mired);
            (uv[0] - point[0]) * tangent[0] + (uv[1] - point[1]) * tangent[1]
        };
        let mut lower = (nearest - step).max(low);
        let mut upper = (nearest + step).min(high);
        let mired = if along(lower).signum() == along(upper).signum() {
            // The foot of the normal is past an end of the range: clamp.
            nearest
        } else {
            for _ in 0..80 {
                let middle = 0.5 * (lower + upper);
                if along(middle).signum() == along(lower).signum() {
                    lower = middle;
                } else {
                    upper = middle;
                }
            }
            0.5 * (lower + upper)
        };
        let point = Self::uv(mired);
        let normal = Self::normal(mired);
        let offset = (uv[0] - point[0]) * normal[0] + (uv[1] - point[1]) * normal[1];
        Some(Temperature {
            kelvin: 1.0e6 / mired,
            tint: offset * WhiteBalanceSolver::TINT_SCALE,
        })
    }

    /// Black body at `mired` (10⁶ / kelvin), CIE 1960 uv.
    fn uv(mired: f64) -> [f64; 2] {
        let kelvin = 1.0e6 / mired;
        let mut xyz = [0.0f64; 3];
        for (index, ((x, y), z)) in CIE_X.iter().zip(&CIE_Y).zip(&CIE_Z).enumerate() {
            let lambda = (f64::from(CIE_LAMBDA_MIN) + 5.0 * index as f64) * 1.0e-9;
            let radiance =
                1.0 / (lambda.powi(5) * ((WhiteBalanceSolver::C2 / (lambda * kelvin)).exp_m1()));
            xyz[0] += radiance * f64::from(*x);
            xyz[1] += radiance * f64::from(*y);
            xyz[2] += radiance * f64::from(*z);
        }
        let denominator = xyz[0] + 15.0 * xyz[1] + 3.0 * xyz[2];
        [4.0 * xyz[0] / denominator, 6.0 * xyz[1] / denominator]
    }

    /// Unit tangent toward lower mireds (hotter).
    fn tangent(mired: f64) -> [f64; 2] {
        let step = WhiteBalanceSolver::TANGENT_STEP_MIRED;
        let hotter = Self::uv(mired - step);
        let cooler = Self::uv(mired + step);
        let (du, dv) = (hotter[0] - cooler[0], hotter[1] - cooler[1]);
        let length = du.hypot(dv);
        [du / length, dv / length]
    }

    /// Unit normal on the green side of the locus (larger v).
    fn normal(mired: f64) -> [f64; 2] {
        let [du, dv] = Self::tangent(mired);
        let normal = [-dv, du];
        if normal[1] < 0.0 {
            [-normal[0], -normal[1]]
        } else {
            normal
        }
    }

    fn uv_to_xy(uv: [f64; 2]) -> [f64; 2] {
        let denominator = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
        [3.0 * uv[0] / denominator, 2.0 * uv[1] / denominator]
    }

    fn xy_to_uv(xy: [f64; 2]) -> Option<[f64; 2]> {
        let denominator = -2.0 * xy[0] + 12.0 * xy[1] + 3.0;
        (denominator.abs() > 1.0e-12)
            .then(|| [4.0 * xy[0] / denominator, 6.0 * xy[1] / denominator])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fujifilm X-T3 ColorMatrix2 (D65), XYZ → camera, as rawler reports it.
    const XT3: [[f32; 3]; 4] = [
        [1.3426, -0.6334, -0.1177],
        [-0.4244, 1.2136, 0.2371],
        [-0.0580, 0.1303, 0.5980],
        [0.0, 0.0, 0.0],
    ];

    fn solver(as_shot: [f32; 4]) -> WhiteBalanceSolver {
        WhiteBalanceSolver::new(XT3, as_shot).unwrap()
    }

    #[test]
    fn locus_matches_published_white_points() {
        // CIE illuminant A is a 2856 K black body, x = 0.44757, y = 0.40745.
        let xy = Locus::xy(Temperature {
            kelvin: 2856.0,
            tint: 0.0,
        });
        assert!((xy[0] - 0.44757).abs() < 2.0e-4, "{xy:?}");
        assert!((xy[1] - 0.40745).abs() < 2.0e-4, "{xy:?}");
        // D65 is near 6504 K and slightly green of the locus.
        let d65 = Locus::temperature([0.3127, 0.3290]).unwrap();
        assert!((d65.kelvin - 6504.0).abs() < 25.0, "{d65:?}");
        assert!(d65.tint > 0.0 && d65.tint < 15.0, "{d65:?}");
    }

    #[test]
    fn temperature_and_tint_round_trip_through_xy() {
        for kelvin in [1_500.0, 2_856.0, 5_000.0, 6_500.0, 12_000.0, 30_000.0] {
            for tint in [-80.0, 0.0, 35.0] {
                let back = Locus::temperature(Locus::xy(Temperature { kelvin, tint })).unwrap();
                assert!(
                    (back.kelvin - kelvin).abs() < 1.0e-6 * kelvin,
                    "{kelvin} {back:?}"
                );
                assert!((back.tint - tint).abs() < 1.0e-6, "{tint} {back:?}");
            }
        }
    }

    #[test]
    fn custom_at_the_as_shot_values_is_as_shot() {
        let solver = solver([1.6953642, 1.0, 2.3509934, 0.0]);
        let as_shot = solver.as_shot().unwrap();
        assert!((3_000.0..9_000.0).contains(&as_shot.kelvin), "{as_shot:?}");
        let custom = solver.multipliers(WhiteBalance::Custom(as_shot)).unwrap();
        let expected = solver.multipliers(WhiteBalance::AsShot).unwrap();
        for channel in 0..3 {
            assert!(
                (custom[channel] - expected[channel]).abs() < 1.0e-9,
                "{custom:?} {expected:?}"
            );
        }
        // Rounded to the slider's resolution it is still within rounding.
        let rounded = Temperature {
            kelvin: as_shot.kelvin.round(),
            tint: (as_shot.tint * 10.0).round() / 10.0,
        };
        let custom = solver.multipliers(WhiteBalance::Custom(rounded)).unwrap();
        for channel in 0..3 {
            assert!((custom[channel] / expected[channel] - 1.0).abs() < 2.0e-3);
        }
    }

    #[test]
    fn warmer_temperature_and_magenta_tint_move_the_right_way() {
        let solver = solver([1.6953642, 1.0, 2.3509934, 0.0]);
        let at = |kelvin, tint| {
            solver
                .multipliers(WhiteBalance::Custom(Temperature { kelvin, tint }))
                .unwrap()
        };
        // A neutral under the as-shot light, developed for a bluer light,
        // gains red against blue: warmer.
        let base = at(5_000.0, 0.0);
        let hotter = at(7_000.0, 0.0);
        assert!(hotter[0] / hotter[2] > base[0] / base[2]);
        // Positive tint assumes greener light, so red and blue gain on
        // green: magenta.
        let magenta = at(5_000.0, 40.0);
        assert!(magenta[0] > base[0] && magenta[2] > base[2]);
    }

    #[test]
    fn missing_or_out_of_range_values_are_refused() {
        assert!(WhiteBalanceSolver::new(XT3, [0.0, 1.0, 1.0, 0.0]).is_none());
        let mut singular = XT3;
        singular[2] = singular[1];
        assert!(WhiteBalanceSolver::new(singular, [2.0, 1.0, 1.5, 0.0]).is_none());
        let solver = solver([2.0, 1.0, 1.5, 0.0]);
        let custom =
            |kelvin| solver.multipliers(WhiteBalance::Custom(Temperature { kelvin, tint: 0.0 }));
        assert!(custom(500.0).is_none());
        assert!(custom(f64::NAN).is_none());
        assert!(custom(5_000.0).is_some());
    }
}
