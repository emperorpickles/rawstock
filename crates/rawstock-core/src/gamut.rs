//! The sRGB gamut map in the output transform.
//!
//! A developed color can land outside sRGB: a channel goes below 0 or
//! above 1. Clipping each channel on its own changes the color's hue (a
//! saturated orange whose blue is negative and red above 1 turns yellower,
//! a red with a little negative green turns less pink than it is) and
//! every color past the edge piles onto the same clipped values.
//!
//! [`GamutMap`] works in Oklab (Björn Ottosson, "A perceptual color space
//! for image processing", 2020, <https://bottosson.github.io/posts/oklab/>),
//! whose hue angle was fitted to stay constant to the eye as chroma
//! changes. A color inside sRGB is left exactly as it is. A color outside
//! keeps its Oklab lightness and hue, and loses only as much chroma as it
//! takes to reach the sRGB edge: Ottosson's "preserve lightness" gamut
//! clip, <https://bottosson.github.io/posts/gamutclipping/>. The edge is
//! found by bisection on the chroma scale, which the GPU kernel in
//! `gpu/pixel.wgsl` repeats step for step.
//!
//! Lightness at or above 1 is white, at or below 0 black.

/// Hue- and lightness-preserving gamut clip into sRGB, for linear sRGB.
pub(crate) struct GamutMap;

impl GamutMap {
    /// Linear sRGB → LMS (Ottosson's `M1`, sRGB primaries, D65 white).
    const RGB_TO_LMS: [[f64; 3]; 3] = [
        [0.4122214708, 0.5363325363, 0.0514459929],
        [0.2119034982, 0.6806995451, 0.1073969566],
        [0.0883024619, 0.2817188376, 0.6299787005],
    ];
    /// Cube-root LMS → Oklab (`M2`).
    const LMS_TO_LAB: [[f64; 3]; 3] = [
        [0.2104542553, 0.7936177850, -0.0040720468],
        [1.9779984951, -2.4285922050, 0.4505937099],
        [0.0259040371, 0.7827717662, -0.8086757660],
    ];
    /// Oklab → cube-root LMS (inverse of `M2`, as Ottosson publishes it).
    const LAB_TO_LMS: [[f64; 3]; 3] = [
        [1.0, 0.3963377774, 0.2158037573],
        [1.0, -0.1055613458, -0.0638541728],
        [1.0, -0.0894841775, -1.2914855480],
    ];
    /// LMS → linear sRGB (inverse of `M1`).
    const LMS_TO_RGB: [[f64; 3]; 3] = [
        [4.0767416621, -3.3077115913, 0.2309699292],
        [-1.2684380046, 2.6097574011, -0.3413193965],
        [-0.0041960863, -0.7034186147, 1.7076147010],
    ];
    /// Bisection steps on the chroma scale: the scale is found to 2⁻²⁴,
    /// about what an f32 on the GPU can resolve.
    pub(crate) const STEPS: u32 = 24;

    /// Map linear sRGB into `0..=1`, keeping Oklab lightness and hue.
    /// Non-finite values pass through unchanged; the encode writes them as
    /// 0.
    pub(crate) fn apply(rgb: [f64; 3]) -> [f64; 3] {
        if !rgb.iter().all(|channel| channel.is_finite()) || Self::inside(rgb) {
            return rgb;
        }
        let lab = Self::to_oklab(rgb);
        if lab[0] >= 1.0 {
            return [1.0; 3];
        }
        if lab[0] <= 0.0 {
            return [0.0; 3];
        }
        // The gray at this lightness is inside, the color is not: find the
        // largest chroma scale that stays inside.
        let at = |scale: f64| Self::from_oklab([lab[0], lab[1] * scale, lab[2] * scale]);
        let (mut inside, mut outside) = (0.0, 1.0);
        for _ in 0..Self::STEPS {
            let middle = 0.5 * (inside + outside);
            if Self::inside(at(middle)) {
                inside = middle;
            } else {
                outside = middle;
            }
        }
        // The middle of the last bracket, clamped: the channel on the edge
        // lands on it rather than a bracket's width inside.
        at(0.5 * (inside + outside)).map(|channel| channel.clamp(0.0, 1.0))
    }

    fn inside(rgb: [f64; 3]) -> bool {
        rgb.iter().all(|channel| (0.0..=1.0).contains(channel))
    }

    fn to_oklab(rgb: [f64; 3]) -> [f64; 3] {
        let lms = Self::mul(Self::RGB_TO_LMS, rgb).map(f64::cbrt);
        Self::mul(Self::LMS_TO_LAB, lms)
    }

    fn from_oklab(lab: [f64; 3]) -> [f64; 3] {
        let lms = Self::mul(Self::LAB_TO_LMS, lab).map(|value| value * value * value);
        Self::mul(Self::LMS_TO_RGB, lms)
    }

    fn mul(matrix: [[f64; 3]; 3], value: [f64; 3]) -> [f64; 3] {
        matrix.map(|row| row[0] * value[0] + row[1] * value[1] + row[2] * value[2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue(rgb: [f64; 3]) -> f64 {
        let lab = GamutMap::to_oklab(rgb);
        lab[2].atan2(lab[1])
    }

    #[test]
    fn oklab_round_trips_and_white_is_neutral() {
        let white = GamutMap::to_oklab([1.0; 3]);
        assert!((white[0] - 1.0).abs() < 1.0e-6, "{white:?}");
        assert!(
            white[1].abs() < 1.0e-6 && white[2].abs() < 1.0e-6,
            "{white:?}"
        );
        for rgb in [[0.2, 0.5, 0.9], [1.3, -0.1, 0.05], [0.01, 0.02, 0.0]] {
            let back = GamutMap::from_oklab(GamutMap::to_oklab(rgb));
            for (got, want) in back.iter().zip(rgb) {
                assert!((got - want).abs() < 1.0e-6, "{rgb:?} {back:?}");
            }
        }
    }

    #[test]
    fn colors_inside_srgb_are_unchanged() {
        for rgb in [
            [0.0; 3],
            [0.18; 3],
            [1.0; 3],
            [0.5, 0.3, 0.2],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.95, 0.9, 0.02],
        ] {
            assert_eq!(GamutMap::apply(rgb), rgb);
        }
    }

    #[test]
    fn out_of_gamut_colors_keep_lightness_and_hue() {
        for rgb in [
            [1.3, 0.2, -0.15],
            [0.9, -0.05, 0.4],
            [-0.1, 0.6, 0.1],
            [0.05, 0.1, 1.4],
            [1.2, 0.9, -0.2],
            [0.30, -0.009, 0.0055],
        ] {
            let mapped = GamutMap::apply(rgb);
            assert!(
                mapped.iter().all(|channel| (0.0..=1.0).contains(channel)),
                "{rgb:?} -> {mapped:?}"
            );
            let (before, after) = (GamutMap::to_oklab(rgb), GamutMap::to_oklab(mapped));
            assert!((before[0] - after[0]).abs() < 1.0e-5, "{rgb:?} {mapped:?}");
            assert!(
                (hue(rgb) - hue(mapped)).abs() < 1.0e-4,
                "{rgb:?} {mapped:?}"
            );
            // On the edge: one channel is at 0 or 1.
            assert!(
                mapped
                    .iter()
                    .any(|channel| *channel < 1.0e-5 || *channel > 1.0 - 1.0e-5),
                "{mapped:?}"
            );
        }
    }

    #[test]
    fn clipping_one_channel_would_shift_the_hue() {
        // A Velvia red sign pixel, linear sRGB: a little negative green.
        // Clipping that green to 0 turns the red about 3 degrees yellower.
        let red = [0.30, -0.009, 0.0055];
        let clipped = red.map(|channel: f64| channel.clamp(0.0, 1.0));
        assert!((hue(clipped) - hue(red)).abs() > 0.04);
        assert!((hue(GamutMap::apply(red)) - hue(red)).abs() < 1.0e-4);
    }

    #[test]
    fn colors_past_the_edge_keep_their_lightness_differences() {
        let darker = GamutMap::apply([1.1, -0.05, 0.02]);
        let lighter = GamutMap::apply([1.3, 0.05, -0.02]);
        assert!(GamutMap::to_oklab(lighter)[0] > GamutMap::to_oklab(darker)[0] + 0.02);
    }

    #[test]
    fn lightness_past_the_ends_is_white_or_black() {
        assert_eq!(GamutMap::apply([1.5, 1.2, 0.9]), [1.0; 3]);
        assert_eq!(GamutMap::apply([-0.2, -0.1, -0.1]), [0.0; 3]);
    }

    #[test]
    fn non_finite_values_pass_through() {
        assert!(GamutMap::apply([f64::NAN, 0.5, 0.5])[0].is_nan());
    }
}
