//! Lateral chromatic aberration: red and blue registered back onto green.
//!
//! A lens focuses red, green, and blue at slightly different magnifications,
//! so away from the center the three channels of an edge land a fraction of
//! a pixel apart. Cameras correct this in their own JPEGs from a lens
//! profile. A RAW file is uncorrected, and a saturated stock turns that
//! misregistration into pink and blue outlines on every dark-on-light edge.
//!
//! [`LateralCa::estimate`] measures the misregistration from the image
//! itself: on many small tiles it fits each channel to green with a gain, an
//! offset, and a sub-pixel shift, keeps the radial part of that shift, and
//! fits a radial polynomial through the tiles. [`LateralCa::apply`] then
//! resamples red and blue so they line up with green. Both run on demosaiced
//! camera RGB, before white balance, because the aberration is optical and
//! belongs to the sensor channels.

use rayon::prelude::*;

/// Radial displacement of one channel against green.
///
/// `δ(ρ) = c₁ρ + c₂ρ² + c₃ρ³` pixels, where `ρ` is the distance from the frame
/// center divided by the half-diagonal. A positive `δ` means the channel's
/// image is larger than green's: its content sits further from the center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RadialShift {
    coefficients: [f64; 3],
}

impl RadialShift {
    /// No displacement.
    pub(crate) const NONE: Self = Self {
        coefficients: [0.0; 3],
    };

    /// Displacement in pixels at normalized radius `rho`.
    pub(crate) fn at(self, rho: f64) -> f64 {
        let [c1, c2, c3] = self.coefficients;
        rho * (c1 + rho * (c2 + rho * c3))
    }

    /// Largest displacement magnitude between the center and the corner.
    pub(crate) fn peak(self) -> f64 {
        (0..=32)
            .map(|step| self.at(f64::from(step) / 32.0).abs())
            .fold(0.0, f64::max)
    }

    fn plus(self, other: Self) -> Self {
        let mut coefficients = self.coefficients;
        for (sum, add) in coefficients.iter_mut().zip(other.coefficients) {
            *sum += add;
        }
        Self { coefficients }
    }
}

/// Measured lateral chromatic aberration of one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LateralCa {
    /// Red against green.
    pub(crate) red: RadialShift,
    /// Blue against green.
    pub(crate) blue: RadialShift,
}

impl LateralCa {
    /// Side of an estimation tile, in pixels.
    const TILE: usize = 64;
    /// Estimate, warp, and estimate again: the shift fit is linearized, so
    /// later passes remove what the first one under-read.
    const PASSES: usize = 3;
    /// Below this, at the corner, resampling would only soften the channel.
    const MIN_PEAK_PX: f64 = 0.05;
    /// Real lenses stay well inside this. A larger fit means the image has
    /// no usable edges, and it is dropped instead of warping the frame.
    const MAX_PEAK_PX: f64 = 4.0;
    /// Fewest usable tiles for a radial fit.
    const MIN_TILES: usize = 24;

    /// Nothing to correct.
    pub(crate) const NONE: Self = Self {
        red: RadialShift::NONE,
        blue: RadialShift::NONE,
    };

    /// Measure red and blue against green in interleaved RGB samples.
    ///
    /// Returns [`Self::NONE`] for a channel when the frame has too few clean
    /// edges, when the fit is implausibly large, or when the displacement is
    /// too small to matter.
    pub(crate) fn estimate(width: usize, height: usize, samples: &[f32]) -> Self {
        let frame = Frame::new(width, height);
        if samples.len() != width * height * 3 || width < Self::TILE * 4 || height < Self::TILE * 4
        {
            return Self::NONE;
        }
        let estimate = |channel: usize| {
            let mut shift = RadialShift::NONE;
            let mut origins = frame.tile_origins();
            for _ in 0..Self::PASSES {
                match frame.fit_residual(samples, channel, shift, &mut origins) {
                    Some(residual) => shift = shift.plus(residual),
                    None => return RadialShift::NONE,
                }
            }
            let peak = shift.peak();
            if !(Self::MIN_PEAK_PX..=Self::MAX_PEAK_PX).contains(&peak) {
                return RadialShift::NONE;
            }
            shift
        };
        Self {
            red: estimate(0),
            blue: estimate(2),
        }
    }

    /// Resample red and blue in place so they register with green.
    pub(crate) fn apply(&self, width: usize, height: usize, samples: &mut [f32]) {
        if samples.len() != width * height * 3 {
            return;
        }
        let frame = Frame::new(width, height);
        for (channel, shift) in [(0, self.red), (2, self.blue)] {
            if shift == RadialShift::NONE {
                continue;
            }
            let plane: Vec<f32> = samples.iter().skip(channel).step_by(3).copied().collect();
            let mut corrected = vec![0.0f32; plane.len()];
            corrected
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, out) in row.iter_mut().enumerate() {
                        let (sx, sy) = frame.source(x as f64, y as f64, shift);
                        *out = frame.cubic(&plane, 1, 0, sx, sy);
                    }
                });
            for (pixel, value) in samples.as_chunks_mut::<3>().0.iter_mut().zip(corrected) {
                pixel[channel] = value;
            }
        }
    }
}

/// Frame geometry shared by estimation and resampling.
struct Frame {
    width: usize,
    height: usize,
    center_x: f64,
    center_y: f64,
    half_diagonal: f64,
}

/// One tile's least-squares shift, projected on the radius.
struct TileShift {
    rho: f64,
    shift: f64,
    weight: f64,
}

impl Frame {
    fn new(width: usize, height: usize) -> Self {
        let center_x = (width as f64 - 1.0) / 2.0;
        let center_y = (height as f64 - 1.0) / 2.0;
        Self {
            width,
            height,
            center_x,
            center_y,
            half_diagonal: center_x.hypot(center_y).max(1.0),
        }
    }

    /// Where the channel's content for output pixel `(x, y)` sits.
    fn source(&self, x: f64, y: f64, shift: RadialShift) -> (f64, f64) {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let radius = (dx * dx + dy * dy).sqrt();
        if radius < 1e-9 {
            return (x, y);
        }
        let scale = shift.at(radius / self.half_diagonal) / radius;
        (x + dx * scale, y + dy * scale)
    }

    /// Catmull-Rom sample of one channel of `stride`-interleaved samples,
    /// edges clamped.
    fn cubic(&self, samples: &[f32], stride: usize, channel: usize, x: f64, y: f64) -> f32 {
        let x0 = x.floor();
        let y0 = y.floor();
        let wx = Self::catmull_rom(x - x0);
        let wy = Self::catmull_rom(y - y0);
        let (x0, y0) = (x0 as i64 - 1, y0 as i64 - 1);
        let max_x = self.width as i64 - 1;
        let max_y = self.height as i64 - 1;
        let columns: [usize; 4] =
            std::array::from_fn(|i| (x0 + i as i64).clamp(0, max_x) as usize * stride + channel);
        let mut sum = 0.0;
        for (j, weight_y) in wy.iter().enumerate() {
            let row = &samples[(y0 + j as i64).clamp(0, max_y) as usize * self.width * stride..];
            let line = wx[0] * row[columns[0]]
                + wx[1] * row[columns[1]]
                + wx[2] * row[columns[2]]
                + wx[3] * row[columns[3]];
            sum += weight_y * line;
        }
        sum
    }

    fn catmull_rom(t: f64) -> [f32; 4] {
        let t = t as f32;
        let t2 = t * t;
        let t3 = t2 * t;
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    }

    /// Fit what is left of `channel`'s radial shift after warping by `shift`.
    ///
    /// `origins` holds the tiles to measure. Tiles that give no usable shift
    /// are removed, so later passes only revisit tiles with clean edges.
    fn fit_residual(
        &self,
        samples: &[f32],
        channel: usize,
        shift: RadialShift,
        origins: &mut Vec<(usize, usize)>,
    ) -> Option<RadialShift> {
        let found: Vec<Option<TileShift>> = origins
            .par_iter()
            .map(|&(left, top)| self.tile_shift(samples, channel, shift, left, top))
            .collect();
        let mut tiles = Vec::with_capacity(origins.len());
        let mut kept = Vec::with_capacity(origins.len());
        for (origin, found) in origins.iter().zip(found) {
            if let Some(tile) = found {
                kept.push(*origin);
                tiles.push(tile);
            }
        }
        *origins = kept;
        if tiles.len() < LateralCa::MIN_TILES {
            return None;
        }
        Self::robust_radial_fit(&tiles)
    }

    /// Top-left corners of the estimation tiles, two pixels in from the
    /// frame edge so the gradient has neighbors.
    fn tile_origins(&self) -> Vec<(usize, usize)> {
        let tile = LateralCa::TILE;
        let mut origins = Vec::new();
        let mut top = 2;
        while top + tile + 2 <= self.height {
            let mut left = 2;
            while left + tile + 2 <= self.width {
                origins.push((left, top));
                left += tile;
            }
            top += tile;
        }
        origins
    }

    /// Least-squares fit `c ≈ k·g + b + e·∇g` on one tile.
    ///
    /// `g` and `c` are green and the (warped) channel, each divided by its
    /// tile mean, so `k` is near 1 on a neutral edge. The channel's content
    /// is displaced by `d = -e / k`. Tiles that are flat, clipped, or whose
    /// channel is not an affine copy of green (colored texture) are skipped.
    fn tile_shift(
        &self,
        samples: &[f32],
        channel: usize,
        shift: RadialShift,
        left: usize,
        top: usize,
    ) -> Option<TileShift> {
        let tile = LateralCa::TILE;
        let width = self.width;
        let green = |x: usize, y: usize| f64::from(samples[(y * width + x) * 3 + 1]);
        let warped = |x: usize, y: usize| {
            if shift == RadialShift::NONE {
                return f64::from(samples[(y * width + x) * 3 + channel]);
            }
            let (sx, sy) = self.source(x as f64, y as f64, shift);
            f64::from(self.cubic(samples, 3, channel, sx, sy))
        };

        // Green and its gradient first: flat or clipped tiles stop here,
        // before the channel is resampled.
        let mut points = Vec::with_capacity(tile * tile);
        let mut green_sum = 0.0;
        for y in top..top + tile {
            for x in left..left + tile {
                let pixel = &samples[(y * width + x) * 3..(y * width + x) * 3 + 3];
                if pixel.iter().any(|value| !value.is_finite() || *value > 0.9) {
                    return None;
                }
                let g = green(x, y);
                let gx = 0.5 * (green(x + 1, y) - green(x - 1, y));
                let gy = 0.5 * (green(x, y + 1) - green(x, y - 1));
                green_sum += g;
                points.push([g, 0.0, gx, gy]);
            }
        }
        let count = points.len() as f64;
        let green_mean = green_sum / count;
        if green_mean < 1e-3 {
            return None;
        }
        let green_var = points
            .iter()
            .map(|[g, ..]| (g / green_mean - 1.0).powi(2))
            .sum::<f64>()
            / count;
        // Too little contrast for a shift to be measurable above noise.
        if green_var < 0.02 {
            return None;
        }

        let mut channel_sum = 0.0;
        for (index, point) in points.iter_mut().enumerate() {
            let c = warped(left + index % tile, top + index / tile);
            channel_sum += c;
            point[1] = c;
        }
        let channel_mean = channel_sum / count;
        if channel_mean < 1e-3 {
            return None;
        }

        let mut normal = [[0.0f64; 4]; 4];
        let mut rhs = [0.0f64; 4];
        let mut channel_sq = 0.0;
        for [g, c, gx, gy] in &points {
            let row = [g / green_mean, 1.0, gx / green_mean, gy / green_mean];
            let target = c / channel_mean;
            for i in 0..4 {
                for j in 0..4 {
                    normal[i][j] += row[i] * row[j];
                }
                rhs[i] += row[i] * target;
            }
            channel_sq += (target - 1.0) * (target - 1.0);
        }
        let solution = Self::solve::<4>(normal, rhs)?;
        let [gain, offset, ex, ey] = solution;
        if !(0.3..=3.0).contains(&gain) {
            return None;
        }

        let mut residual = 0.0;
        for [g, c, gx, gy] in &points {
            let predicted =
                gain * g / green_mean + offset + ex * gx / green_mean + ey * gy / green_mean;
            let error = c / channel_mean - predicted;
            residual += error * error;
        }
        // The channel must be nearly an affine copy of green.
        if residual > 0.05 * channel_sq {
            return None;
        }

        let center_x = (left + tile / 2) as f64 - 0.5;
        let center_y = (top + tile / 2) as f64 - 0.5;
        let dx = center_x - self.center_x;
        let dy = center_y - self.center_y;
        let radius = dx.hypot(dy);
        if radius < tile as f64 {
            return None;
        }
        let (ux, uy) = (dx / radius, dy / radius);
        let shift_x = -ex / gain;
        let shift_y = -ey / gain;
        let radial = shift_x * ux + shift_y * uy;
        if radial.abs() > 2.0 * LateralCa::MAX_PEAK_PX {
            return None;
        }
        // Information about the radial shift: green's gradient along the radius.
        let weight: f64 = points
            .iter()
            .map(|[_, _, gx, gy]| {
                let along = (gx * ux + gy * uy) / green_mean;
                along * along
            })
            .sum();
        Some(TileShift {
            rho: radius / self.half_diagonal,
            shift: radial,
            weight,
        })
    }

    /// Weighted cubic fit through the origin, with Tukey reweighting so a few
    /// tiles of colored texture do not bend the curve.
    fn robust_radial_fit(tiles: &[TileShift]) -> Option<RadialShift> {
        let mut robust = vec![1.0f64; tiles.len()];
        let mut fit = RadialShift::NONE;
        for _ in 0..6 {
            let mut normal = [[0.0f64; 3]; 3];
            let mut rhs = [0.0f64; 3];
            for (tile, extra) in tiles.iter().zip(&robust) {
                let weight = tile.weight * extra;
                let row = [
                    tile.rho,
                    tile.rho * tile.rho,
                    tile.rho * tile.rho * tile.rho,
                ];
                for i in 0..3 {
                    for j in 0..3 {
                        normal[i][j] += weight * row[i] * row[j];
                    }
                    rhs[i] += weight * row[i] * tile.shift;
                }
            }
            fit = RadialShift {
                coefficients: Self::solve::<3>(normal, rhs)?,
            };
            let mut deviations: Vec<f64> = tiles
                .iter()
                .map(|tile| (tile.shift - fit.at(tile.rho)).abs())
                .collect();
            deviations.sort_by(f64::total_cmp);
            let scale = (deviations[deviations.len() / 2] * 1.4826).max(0.02);
            for (tile, extra) in tiles.iter().zip(robust.iter_mut()) {
                let u = (tile.shift - fit.at(tile.rho)) / (4.685 * scale);
                *extra = if u.abs() < 1.0 {
                    (1.0 - u * u) * (1.0 - u * u)
                } else {
                    0.0
                };
            }
        }
        Some(fit)
    }

    /// Gaussian elimination with partial pivoting.
    fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
        for col in 0..N {
            let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
            if a[pivot][col].abs() < 1e-12 {
                return None;
            }
            a.swap(col, pivot);
            b.swap(col, pivot);
            let pivot_row = a[col];
            for row in col + 1..N {
                let factor = a[row][col] / pivot_row[col];
                for (value, above) in a[row][col..].iter_mut().zip(&pivot_row[col..]) {
                    *value -= factor * above;
                }
                b[row] -= factor * b[col];
            }
        }
        let mut x = [0.0; N];
        for row in (0..N).rev() {
            let mut sum = b[row];
            for k in row + 1..N {
                sum -= a[row][k] * x[k];
            }
            x[row] = sum / a[row][row];
        }
        x.iter().all(|value| value.is_finite()).then_some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gray scene of soft-edged dark bars and dots on a light ground, with
    /// red and blue rendered through `truth`. Values stay inside `0.05..0.8`.
    fn scene(width: usize, height: usize, truth: LateralCa) -> Vec<f32> {
        let frame = Frame::new(width, height);
        let ground = |x: f64, y: f64| -> f64 {
            // Bars every 23 px in x and 31 px in y, rotated a little so every
            // tile sees edges in both directions.
            let u = x * 0.96 + y * 0.28;
            let v = -x * 0.28 + y * 0.96;
            let bars = |t: f64, period: f64| {
                let phase = (t / period).fract();
                let phase = if phase < 0.0 { phase + 1.0 } else { phase };
                let edge = 0.5 - (phase - 0.5).abs();
                ((edge - 0.15) * 2.0).clamp(0.0, 1.0)
            };
            0.75 - 0.65 * bars(u, 23.0).max(bars(v, 31.0))
        };
        let mut samples = vec![0.0f32; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) * 3;
                let (fx, fy) = (x as f64, y as f64);
                for (channel, shift) in [(0, truth.red), (2, truth.blue)] {
                    // Content displaced outward by δ: the channel shows what
                    // green has at the radius δ further in.
                    let inverse = RadialShift {
                        coefficients: shift.coefficients.map(|c| -c),
                    };
                    let (sx, sy) = frame.source(fx, fy, inverse);
                    samples[index + channel] = (ground(sx, sy) * 0.9) as f32;
                }
                samples[index + 1] = ground(fx, fy) as f32;
            }
        }
        samples
    }

    fn lens() -> LateralCa {
        LateralCa {
            red: RadialShift {
                coefficients: [0.1, 0.0, 0.4],
            },
            blue: RadialShift {
                coefficients: [-0.2, 0.0, -0.5],
            },
        }
    }

    #[test]
    fn recovers_a_known_radial_shift() {
        let (width, height) = (768, 512);
        let truth = lens();
        let samples = scene(width, height, truth);
        let found = LateralCa::estimate(width, height, &samples);
        for rho in [0.25, 0.5, 0.75, 1.0] {
            for (name, want, got) in [
                ("red", truth.red, found.red),
                ("blue", truth.blue, found.blue),
            ] {
                let error = (want.at(rho) - got.at(rho)).abs();
                assert!(
                    error < 0.08,
                    "{name} at ρ={rho}: want {:.3}, got {:.3}",
                    want.at(rho),
                    got.at(rho)
                );
            }
        }
    }

    #[test]
    fn applying_the_estimate_registers_the_channels() {
        let (width, height) = (768, 512);
        let mut samples = scene(width, height, lens());
        let found = LateralCa::estimate(width, height, &samples);
        found.apply(width, height, &mut samples);
        let after = LateralCa::estimate(width, height, &samples);
        assert_eq!(after, LateralCa::NONE, "residual {after:?}");
    }

    #[test]
    fn a_registered_frame_is_left_alone() {
        let (width, height) = (768, 512);
        let samples = scene(width, height, LateralCa::NONE);
        assert_eq!(
            LateralCa::estimate(width, height, &samples),
            LateralCa::NONE
        );
    }

    #[test]
    fn a_flat_frame_is_left_alone() {
        let (width, height) = (512, 512);
        let samples = vec![0.3f32; width * height * 3];
        assert_eq!(
            LateralCa::estimate(width, height, &samples),
            LateralCa::NONE
        );
    }
}
