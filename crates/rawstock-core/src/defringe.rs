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
//! channels. A pixel takes the color of the dark side around it, at its own
//! luminance, when two things hold: it is well below the brightest pixel within [`Defringe::RADIUS`]
//! (it sits on the dark side of a high-contrast edge), and its color is in
//! the fringe range, blue or purple to magenta, with blue the strongest
//! channel or red and blue both above green. A red whose blue is a small
//! fraction of its red is not counted as magenta. The dark side's color is
//! the luminance-weighted mean of nearby pixels that are neither fringe nor
//! bright: neutral beside a dark neutral subject, red beside a red sign,
//! where desaturating would leave a gray outline. Luminance is kept, so
//! edges do not move or soften. A blue or purple subject away from bright
//! edges keeps its color.

use rayon::prelude::*;

/// Recolors blue/purple fringes next to bright edges with their surroundings.
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
    /// Blue as a fraction of red where a magenta reading starts to count,
    /// and where it counts fully. A saturated red with green and blue both
    /// near zero (a red sign) is not a fringe, even when noise lifts blue a
    /// hair above green.
    const BLUE_TO_RED: (f32, f32) = (0.25, 0.5);
    /// Pixels darker than this fraction of the local maximum are the dark
    /// side of the edge, whose color a fringe takes.
    const DARK_SIDE: f32 = 0.5;
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
        let amount: Vec<f32> = samples
            .as_chunks::<3>()
            .0
            .par_iter()
            .zip(luma.par_iter().zip(&bright))
            .map(|(pixel, (&y, &peak))| Self::edge_weight(y, peak) * Self::hue_weight(*pixel, y))
            .collect();
        let source = samples.to_vec();
        samples
            .par_chunks_mut(width * 3)
            .enumerate()
            .for_each(|(row_index, row)| {
                for (x, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let index = row_index * width + x;
                    let weight = amount[index];
                    if weight <= 0.0 {
                        continue;
                    }
                    let y = luma[index];
                    let chroma = Self::surround_chroma(
                        width,
                        height,
                        (x, row_index),
                        &source,
                        &luma,
                        &amount,
                        bright[index],
                    );
                    for (channel, tint) in pixel.iter_mut().zip(chroma) {
                        let target = y * tint;
                        *channel += (target - *channel) * weight;
                    }
                }
            });
    }

    /// Color of the dark side around a fringe, as channel / luminance.
    ///
    /// The luminance-weighted mean over the window of pixels that are
    /// neither fringe nor on the bright side of the edge. A fringe beside a
    /// red sign takes the red; one beside a dark neutral turns neutral, as
    /// before. Neutral when the window has no such pixel.
    fn surround_chroma(
        width: usize,
        height: usize,
        (x, y): (usize, usize),
        samples: &[f32],
        luma: &[f32],
        amount: &[f32],
        peak: f32,
    ) -> [f32; 3] {
        let radius = Self::RADIUS;
        let dark = peak * Self::DARK_SIDE;
        let mut sum = [0.0f32; 3];
        let mut weight = 0.0f32;
        for row in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
            for column in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                let index = row * width + column;
                let y = luma[index];
                if amount[index] > 0.0 || y >= dark || y <= 0.0 {
                    continue;
                }
                let pixel = &samples[index * 3..index * 3 + 3];
                if pixel.iter().any(|channel| *channel < 0.0) {
                    continue;
                }
                for (total, channel) in sum.iter_mut().zip(pixel) {
                    *total += channel;
                }
                weight += y;
            }
        }
        if weight <= 0.0 {
            return [1.0; 3];
        }
        sum.map(|total| total / weight)
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
        let magenta_excess = (red.min(blue) - green) / luma
            * if red > blue {
                Self::smoothstep(Self::BLUE_TO_RED.0, Self::BLUE_TO_RED.1, blue / red)
            } else {
                1.0
            };
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

    #[test]
    fn a_saturated_red_beside_a_bright_edge_keeps_its_color() {
        // DSCF7392's red sign: blue a hair above green read as magenta and
        // drew a gray outline around the white letters.
        let mut samples = fringed_edge();
        for y in 0..HEIGHT {
            let index = (y * WIDTH + 21) * 3;
            samples[index..index + 3].copy_from_slice(&[0.169, 0.014, 0.019]);
        }
        let original = samples.clone();
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        assert_eq!(pixel(&samples, 21, 4), pixel(&original, 21, 4));
    }

    #[test]
    fn a_purple_edge_on_a_red_field_takes_the_red_not_gray() {
        // DSCF7392: the one purple pixel between a white letter and the red
        // sign turned gray and outlined every letter.
        let red = [0.3, 0.035, 0.04];
        let mut samples = fringed_edge();
        for y in 0..HEIGHT {
            for x in 20..WIDTH {
                let index = (y * WIDTH + x) * 3;
                let pixel = if x == 20 { [0.31, 0.03, 0.16] } else { red };
                samples[index..index + 3].copy_from_slice(&pixel);
            }
        }
        let before = pixel(&samples, 20, 4);
        Defringe::apply(WIDTH, HEIGHT, &mut samples);
        let after = pixel(&samples, 20, 4);
        assert!((Defringe::luma(before) - Defringe::luma(after)).abs() < 1.0e-6);
        let tint = |pixel: [f32; 3]| pixel.map(|channel| channel / Defringe::luma(pixel));
        for (got, want) in tint(after).iter().zip(tint(red)) {
            assert!(
                (got - want).abs() < 1.0e-3,
                "{after:?} is not the red {red:?}"
            );
        }
        assert_eq!(pixel(&samples, 25, 4), red);
    }
}
