//! Defringe: the blue and purple glow lenses leave on the dark side of a
//! bright edge.
//!
//! Lateral chromatic aberration ([`crate::lateral_ca`]) is a misregistration
//! and can be resampled away. Longitudinal (axial) aberration is not: blue
//! and red focus in front of or behind green, so they spread as a soft halo
//! around every bright edge. Against a dark subject that halo reads as a
//! blue or purple outline, and a saturated stock makes it worse.
//!
//! [`Defringe::apply`] runs on scene-linear ACEScg, where a neutral has equal
//! channels. A pixel is desaturated toward its own luminance when two things
//! hold: it is well below the brightest pixel within [`Defringe::RADIUS`]
//! (it sits on the dark side of a high-contrast edge), and its color is in
//! the fringe range, blue or purple to magenta, with blue the strongest
//! channel or red and blue both above green. Luminance is kept, so edges do
//! not move or soften. A blue or purple subject away from bright edges keeps
//! its color.

use rayon::prelude::*;

/// Desaturates blue/purple fringes next to bright edges.
pub(crate) struct Defringe;

impl Defringe {
    /// How far from a bright edge a fringe can reach, in pixels.
    const RADIUS: usize = 4;
    /// Stops below the local maximum where defringe starts, and where it is
    /// at full strength.
    const CONTRAST_STOPS: (f32, f32) = (1.0, 2.5);
    /// Fringe color strength, as a fraction of luminance, where defringe
    /// starts and where it is at full strength.
    const HUE: (f32, f32) = (0.03, 0.25);
    /// AP1 luminance weights (ACEScg to CIE Y).
    const LUMA: [f32; 3] = [0.272_228_7, 0.674_081_8, 0.053_689_5];

    /// Defringe interleaved ACEScg samples in place.
    pub(crate) fn apply(width: usize, height: usize, samples: &mut [f32]) {
        if width == 0 || height == 0 || samples.len() != width * height * 3 {
            return;
        }
        let luma: Vec<f32> = samples
            .as_chunks::<3>()
            .0
            .par_iter()
            .map(|pixel| Self::luma(*pixel).max(0.0))
            .collect();
        let bright = Self::local_max(width, height, &luma);
        samples
            .par_chunks_mut(width * 3)
            .zip(luma.par_chunks(width).zip(bright.par_chunks(width)))
            .for_each(|(row, (luma, bright))| {
                for ((pixel, &y), &peak) in
                    row.as_chunks_mut::<3>().0.iter_mut().zip(luma).zip(bright)
                {
                    let amount = Self::edge_weight(y, peak) * Self::hue_weight(*pixel, y);
                    if amount > 0.0 {
                        for channel in pixel.iter_mut() {
                            *channel = y + (*channel - y) * (1.0 - amount);
                        }
                    }
                }
            });
    }

    fn luma(pixel: [f32; 3]) -> f32 {
        Self::LUMA[0] * pixel[0] + Self::LUMA[1] * pixel[1] + Self::LUMA[2] * pixel[2]
    }

    /// How far into the dark side of an edge this pixel sits.
    fn edge_weight(luma: f32, peak: f32) -> f32 {
        if peak <= 0.0 {
            return 0.0;
        }
        let stops = (peak / luma.max(peak * 1.0e-4)).log2();
        Self::smoothstep(Self::CONTRAST_STOPS.0, Self::CONTRAST_STOPS.1, stops)
    }

    /// How much this pixel's color looks like a blue/purple fringe.
    fn hue_weight(pixel: [f32; 3], luma: f32) -> f32 {
        if luma <= 0.0 {
            // A pixel at or below black has no luminance to keep; any color
            // there is fringe or noise.
            return f32::from(pixel[2] > pixel[1]);
        }
        let [red, green, blue] = pixel;
        let blue_excess = (blue - red.max(green)) / luma;
        let magenta_excess = (red.min(blue) - green) / luma;
        let strength = blue_excess.max(magenta_excess);
        Self::smoothstep(Self::HUE.0, Self::HUE.1, strength)
    }

    fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Maximum of `plane` over a square of side `2 × RADIUS + 1`, separably.
    fn local_max(width: usize, height: usize, plane: &[f32]) -> Vec<f32> {
        let radius = Self::RADIUS;
        let mut across = vec![0.0f32; plane.len()];
        across
            .par_chunks_mut(width)
            .zip(plane.par_chunks(width))
            .for_each(|(out, row)| {
                for (x, value) in out.iter_mut().enumerate() {
                    let lo = x.saturating_sub(radius);
                    let hi = (x + radius).min(width - 1);
                    *value = row[lo..=hi].iter().copied().fold(0.0, f32::max);
                }
            });
        let mut out = vec![0.0f32; plane.len()];
        out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(height - 1);
            for (x, value) in row.iter_mut().enumerate() {
                *value = (lo..=hi)
                    .map(|source| across[source * width + x])
                    .fold(0.0, f32::max);
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 40;
    const HEIGHT: usize = 8;

    /// Bright neutral on the left, a dark bar from column 20, and a blue glow
    /// in the first three dark columns.
    fn fringed_edge() -> Vec<f32> {
        let mut samples = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for _ in 0..HEIGHT {
            for x in 0..WIDTH {
                let pixel = match x {
                    0..20 => [0.8, 0.8, 0.8],
                    20..23 => [0.02, 0.02, 0.09],
                    _ => [0.02, 0.02, 0.02],
                };
                samples.extend(pixel);
            }
        }
        samples
    }

    fn pixel(samples: &[f32], x: usize, y: usize) -> [f32; 3] {
        let index = (y * WIDTH + x) * 3;
        [samples[index], samples[index + 1], samples[index + 2]]
    }

    fn chroma(pixel: [f32; 3]) -> f32 {
        let luma = Defringe::luma(pixel);
        pixel
            .iter()
            .map(|channel| (channel - luma).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_blue_glow_beside_a_bright_edge_turns_neutral() {
        let mut samples = fringed_edge();
        let before = pixel(&samples, 21, 4);
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        let after = pixel(&samples, 21, 4);
        assert!(chroma(before) > 0.05, "{before:?}");
        assert!(chroma(after) < 1.0e-4, "{after:?}");
        let (luma_before, luma_after) = (Defringe::luma(before), Defringe::luma(after));
        assert!((luma_before - luma_after).abs() < 1.0e-6);
    }

    #[test]
    fn the_bright_side_and_neutral_pixels_are_untouched() {
        let original = fringed_edge();
        let mut samples = original.clone();
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        for x in (0..20).chain(23..WIDTH) {
            assert_eq!(pixel(&samples, x, 4), pixel(&original, x, 4), "column {x}");
        }
    }

    #[test]
    fn a_blue_subject_without_a_bright_edge_keeps_its_color() {
        let mut samples: Vec<f32> = (0..WIDTH * HEIGHT)
            .flat_map(|index| {
                if index % WIDTH < 20 {
                    [0.10, 0.15, 0.45]
                } else {
                    [0.05, 0.08, 0.25]
                }
            })
            .collect();
        let original = samples.clone();
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        assert_eq!(samples, original);
    }

    #[test]
    fn a_warm_color_beside_a_bright_edge_keeps_its_color() {
        let mut samples = fringed_edge();
        for y in 0..HEIGHT {
            let index = (y * WIDTH + 21) * 3;
            samples[index..index + 3].copy_from_slice(&[0.09, 0.03, 0.01]);
        }
        let original = samples.clone();
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        assert_eq!(pixel(&samples, 21, 4), pixel(&original, 21, 4));
    }
}
