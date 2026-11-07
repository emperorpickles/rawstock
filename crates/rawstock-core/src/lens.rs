//! Lens distortion and vignetting, from the camera's own correction tables.
//!
//! Fujifilm writes the corrections its JPEG engine applies into every RAF:
//! `GeometricDistortionParams` (0xf00b) and `VignettingParams` (0xf010) in
//! the Fuji IFD (0xf000). `rawler` names those tags but does not apply them,
//! so [`LensProfile::read`] parses them here. Each holds a scale, a list of
//! radius knots from the frame center (0) to the corner (1), and one value
//! per knot:
//!
//! - distortion: the percentage by which the sensor image at that radius sits
//!   off its ideal radius. An X-T2 with the XF35mmF2 R WR records
//!   −7.6% at the corner, which is barrel distortion.
//! - vignetting: the percentage of the center's brightness that reaches that
//!   radius. 100 everywhere means no falloff, which the camera writes at
//!   apertures where the lens has none worth correcting.
//!
//! The knots are indexed by the sensor-side (distorted) radius, normalized
//! to the half-diagonal of the camera's default crop. Checked against the
//! camera's own JPEGs: on three X-T2 frames, 2,700 matched tiles fit the
//! model below to a median 0.04% of their radius.
//!
//! [`LensWarp`] undistorts, then scales the result so it just fills the
//! frame, as the camera does: the output keeps its size and no edge samples
//! outside the sensor. [`RadialGain`] is the vignetting gain at each output
//! radius. The warp runs on demosaiced camera RGB after the lateral CA
//! correction. The gain runs after highlight recovery in
//! [`crate::color::CameraImage`], so a pixel brightened toward a corner is
//! not clipped at the sensor white first.
//!
//! Lateral CA reads the same Fuji IFD. `ChromaticAberrationParams` (0xf00f)
//! is not one percentage curve: a scale, then the knots, then a red radial
//! scale and a blue one. On an X-S10 the tag is 29 rationals (9 knots, and
//! the scale repeated at the end). On an X-T2 it is 31 (10 knots, the center
//! knot left out). A value of 0.001 means that channel is sampled 0.1%
//! further from the center than green. An empty or implausible table is
//! dropped, and the develop then measures the shift from the image
//! ([`crate::lateral_ca`]).

use std::io::{Read, Seek, SeekFrom};

use rawler::formats::tiff::IFD;
use rawler::rawsource::RawSource;
use rayon::prelude::*;

/// A correction value at each radius, from the frame center (0) to the
/// corner (1), interpolated with a monotone cubic.
#[derive(Debug, Clone, PartialEq)]
pub struct RadialTable {
    knots: Vec<f64>,
    values: Vec<f64>,
    slopes: Vec<f64>,
}

impl RadialTable {
    /// A table through `values` at `knots`. `None` unless there are at least
    /// two knots, they rise strictly from a non-negative start, and every
    /// number is finite.
    pub fn new(knots: Vec<f64>, values: Vec<f64>) -> Option<Self> {
        if knots.len() < 2
            || knots.len() != values.len()
            || knots.iter().chain(&values).any(|value| !value.is_finite())
            || knots[0] < 0.0
            || knots.windows(2).any(|pair| pair[1] <= pair[0])
        {
            return None;
        }
        let slopes = Self::monotone_slopes(&knots, &values);
        Some(Self {
            knots,
            values,
            slopes,
        })
    }

    /// Value at normalized radius `radius`. Outside the knots it continues
    /// along the end slope.
    pub fn at(&self, radius: f64) -> f64 {
        let last = self.knots.len() - 1;
        if radius <= self.knots[0] {
            return self.values[0] + self.slopes[0] * (radius - self.knots[0]);
        }
        if radius >= self.knots[last] {
            return self.values[last] + self.slopes[last] * (radius - self.knots[last]);
        }
        let upper = self.knots.partition_point(|knot| *knot <= radius);
        let lower = upper - 1;
        let span = self.knots[upper] - self.knots[lower];
        let t = (radius - self.knots[lower]) / span;
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.values[lower]
            + (t3 - 2.0 * t2 + t) * span * self.slopes[lower]
            + (-2.0 * t3 + 3.0 * t2) * self.values[upper]
            + (t3 - t2) * span * self.slopes[upper]
    }

    /// Whether every value equals `value`, so the table changes nothing.
    fn is_constant(&self, value: f64) -> bool {
        self.values.iter().all(|entry| (entry - value).abs() < 1e-9)
    }

    /// Fritsch–Carlson tangents: smooth, and never overshooting between
    /// knots, so a monotone table stays monotone.
    fn monotone_slopes(knots: &[f64], values: &[f64]) -> Vec<f64> {
        let secants: Vec<f64> = knots
            .windows(2)
            .zip(values.windows(2))
            .map(|(k, v)| (v[1] - v[0]) / (k[1] - k[0]))
            .collect();
        let count = knots.len();
        let mut slopes = vec![0.0; count];
        slopes[0] = secants[0];
        slopes[count - 1] = secants[count - 2];
        for index in 1..count - 1 {
            let (before, after) = (secants[index - 1], secants[index]);
            slopes[index] = if before * after <= 0.0 {
                0.0
            } else {
                (before + after) / 2.0
            };
        }
        for (index, secant) in secants.iter().enumerate() {
            if *secant == 0.0 {
                slopes[index] = 0.0;
                slopes[index + 1] = 0.0;
                continue;
            }
            let alpha = slopes[index] / secant;
            let beta = slopes[index + 1] / secant;
            let length = alpha.hypot(beta);
            if length > 3.0 {
                let tau = 3.0 / length;
                slopes[index] = tau * alpha * secant;
                slopes[index + 1] = tau * beta * secant;
            }
        }
        slopes
    }
}

/// The lens corrections a file records for itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LensProfile {
    /// Fractional offset of the sensor image from its ideal radius, at each
    /// sensor-side radius. −0.05 means content sits 5% closer to the center
    /// than it should.
    distortion: Option<RadialTable>,
    /// Fraction of the center's brightness reaching each sensor-side radius.
    vignetting: Option<RadialTable>,
    /// Red and blue radial scale against green. Each value is
    /// `source_radius / output_radius − 1` at the normalized radius.
    chromatic_aberration: Option<(RadialTable, RadialTable)>,
}

impl LensProfile {
    /// Fuji IFD inside a RAF.
    const FUJI_IFD: u16 = 0xf000;
    /// Fuji `GeometricDistortionParams`.
    const FUJI_DISTORTION: u16 = 0xf00b;
    /// Fuji `ChromaticAberrationParams`.
    const FUJI_CA: u16 = 0xf00f;
    /// Fuji `VignettingParams`.
    const FUJI_VIGNETTING: u16 = 0xf010;
    /// Byte offset of the RAF header's pointer to the Fuji IFD.
    const RAF_FUJI_IFD_POINTER: u64 = 100;
    /// Real lenses stay well inside this. A larger value means the table is
    /// not what we think it is, and it is ignored.
    const MAX_DISTORTION: f64 = 0.25;
    /// Darkest corner a vignetting table may claim, as a fraction.
    const MIN_VIGNETTING: f64 = 0.1;
    /// Largest |red| or |blue| scale kept. Measured X-T2 and X-S10 tables
    /// stay under 0.001. Anything larger is not this layout.
    const MAX_CA: f64 = 0.02;

    /// No correction recorded.
    pub const NONE: Self = Self {
        distortion: None,
        vignetting: None,
        chromatic_aberration: None,
    };

    /// A profile from tables already in fractions: distortion as an offset
    /// (−0.05 for −5%) and vignetting as transmission (0.8 for 80%). Each is
    /// dropped when it is outside a plausible range or changes nothing.
    pub fn new(distortion: Option<RadialTable>, vignetting: Option<RadialTable>) -> Self {
        let distortion = distortion.filter(|table| {
            !table.is_constant(0.0)
                && table
                    .values
                    .iter()
                    .all(|value| value.abs() < Self::MAX_DISTORTION)
        });
        let vignetting = vignetting.filter(|table| {
            !table.is_constant(1.0)
                && table
                    .values
                    .iter()
                    .all(|value| (Self::MIN_VIGNETTING..=2.0).contains(value))
        });
        Self {
            distortion,
            vignetting,
            chromatic_aberration: None,
        }
    }

    /// The tables a RAW file carries. Only Fujifilm RAF is read today. Any
    /// other file gives [`Self::NONE`]. A RAF keeps each table that is
    /// usable and drops the rest.
    pub fn read(source: &RawSource) -> Self {
        Self::fuji(source).unwrap_or(Self::NONE)
    }

    /// Distortion table, when the file has one that changes the image.
    pub fn distortion(&self) -> Option<&RadialTable> {
        self.distortion.as_ref()
    }

    /// Vignetting table, when the file has one that changes the image.
    pub fn vignetting(&self) -> Option<&RadialTable> {
        self.vignetting.as_ref()
    }

    /// Red and blue radial scales, when the file has a usable CA table.
    pub fn chromatic_aberration(&self) -> Option<(&RadialTable, &RadialTable)> {
        self.chromatic_aberration
            .as_ref()
            .map(|(red, blue)| (red, blue))
    }

    /// Register red and blue onto green. The camera table wins when this
    /// profile has one; otherwise the shift is measured from `samples`.
    pub(crate) fn correct_lateral_ca(&self, width: usize, height: usize, samples: &mut [f32]) {
        if self.apply_chromatic_aberration(width, height, samples) {
            return;
        }
        let estimated = crate::lateral_ca::LateralCa::estimate(width, height, samples);
        estimated.apply(width, height, samples);
    }

    /// Resample red and blue from the camera table. `false` when there is
    /// nothing to apply, so the caller can measure the shift instead.
    fn apply_chromatic_aberration(&self, width: usize, height: usize, samples: &mut [f32]) -> bool {
        let Some((red, blue)) = &self.chromatic_aberration else {
            return false;
        };
        if width == 0 || height == 0 || samples.len() != width * height * 3 {
            return false;
        }
        for (channel, table) in [(0, red), (2, blue)] {
            let corrected = Self::resample_channel(width, height, samples, channel, table);
            for (pixel, value) in samples.as_chunks_mut::<3>().0.iter_mut().zip(corrected) {
                pixel[channel] = value;
            }
        }
        true
    }

    /// Red or blue resampled onto green. `samples` stays immutable so the
    /// rows can be read in parallel; the caller writes `channel` back.
    fn resample_channel(
        width: usize,
        height: usize,
        samples: &[f32],
        channel: usize,
        table: &RadialTable,
    ) -> Vec<f32> {
        let frame = Frame::new(width, height);
        let mut corrected = vec![0.0_f32; width * height];
        corrected
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let dy = y as f64 - frame.center_y;
                for (x, out) in row.iter_mut().enumerate() {
                    let dx = x as f64 - frame.center_x;
                    let radius = dx.hypot(dy);
                    let scale = if radius < 1e-9 {
                        1.0
                    } else {
                        1.0 + table.at(radius / frame.half_diagonal)
                    };
                    *out = frame.cubic_channel(
                        samples,
                        frame.center_x + dx * scale,
                        frame.center_y + dy * scale,
                        channel,
                    );
                }
            });
        corrected
    }

    fn with_chromatic_aberration(mut self, ca: Option<(RadialTable, RadialTable)>) -> Self {
        self.chromatic_aberration = ca.filter(|(red, blue)| Self::usable_ca(red, blue));
        self
    }

    fn usable_ca(red: &RadialTable, blue: &RadialTable) -> bool {
        !(red.is_constant(0.0) && blue.is_constant(0.0))
            && red
                .values
                .iter()
                .chain(&blue.values)
                .all(|value| value.abs() < Self::MAX_CA)
    }

    fn fuji(source: &RawSource) -> Option<Self> {
        if !rawler::decoders::raf::is_raf(source) {
            return None;
        }
        let mut reader = source.reader();
        reader
            .seek(SeekFrom::Start(Self::RAF_FUJI_IFD_POINTER))
            .ok()?;
        let mut pointer = [0_u8; 4];
        reader.read_exact(&mut pointer).ok()?;
        let base = u32::from_be_bytes(pointer);
        let ifd =
            IFD::new_root_with_correction(&mut reader, 0, base, 0, 10, &[Self::FUJI_IFD]).ok()?;
        let distortion = Self::fuji_table(&ifd, Self::FUJI_DISTORTION);
        let vignetting = Self::fuji_table(&ifd, Self::FUJI_VIGNETTING);
        let chromatic_aberration = Self::fuji_ca(&ifd);
        Some(Self::new(distortion, vignetting).with_chromatic_aberration(chromatic_aberration))
    }

    /// Floats of one Fuji tag. Distortion, vignetting, and CA are all
    /// stored as signed rationals; `rawler` exposes them as `f32`.
    fn fuji_numbers(ifd: &IFD, tag: u16) -> Option<Vec<f64>> {
        let value = &ifd.get_entry_recursive(tag)?.value;
        (0..value.count())
            .map(|index| value.get_f32(index).ok().flatten().map(f64::from))
            .collect()
    }

    /// One Fuji table: a scale, then the knots, then a percentage per knot.
    fn fuji_table(ifd: &IFD, tag: u16) -> Option<RadialTable> {
        let numbers = Self::fuji_numbers(ifd, tag)?;
        let pairs = numbers.len().checked_sub(1)?;
        if pairs < 4 || pairs % 2 != 0 {
            return None;
        }
        let count = pairs / 2;
        let knots = numbers[1..=count].to_vec();
        let values = numbers[count + 1..]
            .iter()
            .map(|percent| percent / 100.0)
            .collect();
        RadialTable::new(knots, values)
    }

    /// Red and blue scales from `ChromaticAberrationParams`.
    ///
    /// Both layouts measured in RAF files start with the same scale word as
    /// distortion. The newer one (29 values) repeats that scale at the end
    /// and carries 9 knots. The older one (31 values) carries 10 knots and
    /// omits the center. Values are already fractions of radius.
    fn fuji_ca(ifd: &IFD) -> Option<(RadialTable, RadialTable)> {
        Self::ca_tables(&Self::fuji_numbers(ifd, Self::FUJI_CA)?)
    }

    fn ca_tables(numbers: &[f64]) -> Option<(RadialTable, RadialTable)> {
        let count = Self::ca_knot_count(numbers)?;
        let end = 1 + 3 * count;
        if count < 2 || numbers.len() < end {
            return None;
        }
        let knots = numbers[1..1 + count].to_vec();
        let red = numbers[1 + count..1 + 2 * count].to_vec();
        let blue = numbers[1 + 2 * count..end].to_vec();
        if knots.last().is_none_or(|knot| *knot < 0.5)
            || knots.iter().any(|knot| !(0.0..=1.5).contains(knot))
        {
            return None;
        }
        let (knots, red, blue) = Self::with_center(knots, red, blue);
        let red = RadialTable::new(knots.clone(), red)?;
        let blue = RadialTable::new(knots, blue)?;
        Self::usable_ca(&red, &blue).then_some((red, blue))
    }

    /// Knot count, after a trailing copy of the leading scale is set aside.
    fn ca_knot_count(numbers: &[f64]) -> Option<usize> {
        let n = numbers.len();
        if n >= 5 && (n - 2).is_multiple_of(3) && (numbers[n - 1] - numbers[0]).abs() < 1e-3 {
            return Some((n - 2) / 3);
        }
        (n >= 4 && (n - 1).is_multiple_of(3)).then_some((n - 1) / 3)
    }

    /// Lateral CA is zero at the center. Files that omit that knot get one.
    fn with_center(
        mut knots: Vec<f64>,
        mut red: Vec<f64>,
        mut blue: Vec<f64>,
    ) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        if knots.first().is_some_and(|knot| *knot > 0.0) {
            knots.insert(0, 0.0);
            red.insert(0, 0.0);
            blue.insert(0, 0.0);
        }
        (knots, red, blue)
    }
}

/// Which corrections a develop applies from a [`LensProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LensCorrections {
    /// Undistort and fill the frame.
    pub distortion: bool,
    /// Brighten the corners back to the center's exposure.
    pub vignetting: bool,
}

impl Default for LensCorrections {
    fn default() -> Self {
        Self::ALL
    }
}

impl LensCorrections {
    /// Every lens correction on: the default, as the camera's JPEG has it.
    pub const ALL: Self = Self {
        distortion: true,
        vignetting: true,
    };
    /// Every lens correction off.
    pub const OFF: Self = Self {
        distortion: false,
        vignetting: false,
    };
}

/// Output radius → sensor radius, for one frame aspect.
///
/// Both radii are normalized to the frame's half-diagonal, so one warp
/// serves the full develop and a third-size preview alike.
#[derive(Debug, Clone)]
pub(crate) struct LensWarp {
    /// Sensor radius over output radius, at `index / (STEPS - 1)`.
    factors: Vec<f64>,
}

impl LensWarp {
    const STEPS: usize = 2048;
    /// Sensor radii searched when inverting the table.
    const SEARCH: usize = 16384;

    /// The warp for `distortion` on a frame with half extents `half_width`
    /// and `half_height` (pixel centers, so `(width - 1) / 2`). `None` when
    /// the table would fold the image over on itself.
    pub(crate) fn new(distortion: &RadialTable, half_width: f64, half_height: f64) -> Option<Self> {
        let half_diagonal = half_width.hypot(half_height);
        if half_diagonal <= 0.0 {
            return None;
        }
        let scale = Self::fill_scale(distortion, half_width.min(half_height) / half_diagonal)?;
        // Output radius for each sensor radius: q = s·p·(1 + d(q)).
        let output = |sensor: f64| sensor / (scale * (1.0 + distortion.at(sensor)));
        let mut factors = Vec::with_capacity(Self::STEPS);
        factors.push(scale * (1.0 + distortion.at(0.0)));
        let mut previous = (0.0, 0.0);
        let mut search = 1;
        for step in 1..Self::STEPS {
            let target = step as f64 / (Self::STEPS - 1) as f64;
            loop {
                if search > Self::SEARCH {
                    return None;
                }
                let sensor = search as f64 / Self::SEARCH as f64;
                let radius = output(sensor);
                if radius <= previous.1 {
                    return None;
                }
                if radius >= target {
                    let t = (target - previous.1) / (radius - previous.1);
                    let sensor = previous.0 + t * (sensor - previous.0);
                    factors.push(sensor / target);
                    break;
                }
                previous = (sensor, radius);
                search += 1;
            }
        }
        Some(Self { factors })
    }

    /// The scale that makes the corrected frame just fill the output.
    ///
    /// The frame's edge runs from `min_radius` (the middle of the short
    /// side) to the corner at 1. An edge point at sensor radius q lands at
    /// q / (1 + d(q)), so the largest scale that keeps every edge inside the
    /// sensor is the smallest 1 / (1 + d(q)) along that range.
    fn fill_scale(distortion: &RadialTable, min_radius: f64) -> Option<f64> {
        const SAMPLES: usize = 512;
        let scale = (0..=SAMPLES)
            .map(|step| {
                let radius = min_radius + (1.0 - min_radius) * step as f64 / SAMPLES as f64;
                1.0 / (1.0 + distortion.at(radius))
            })
            .fold(f64::INFINITY, f64::min);
        (scale.is_finite() && scale > 0.0).then_some(scale)
    }

    /// Sensor radius over output radius at normalized output radius `radius`.
    fn factor(&self, radius: f64) -> f64 {
        let position = radius.clamp(0.0, 1.0) * (Self::STEPS - 1) as f64;
        let lower = (position as usize).min(Self::STEPS - 2);
        let t = position - lower as f64;
        self.factors[lower] + t * (self.factors[lower + 1] - self.factors[lower])
    }

    /// Normalized sensor radius sampled for normalized output radius `radius`.
    pub(crate) fn sensor_radius(&self, radius: f64) -> f64 {
        radius * self.factor(radius)
    }

    /// Resample interleaved RGB so straight lines come out straight.
    pub(crate) fn apply(&self, width: usize, height: usize, samples: &[f32]) -> Vec<f32> {
        let frame = Frame::new(width, height);
        let mut out = vec![0.0_f32; samples.len()];
        out.par_chunks_mut(width * 3)
            .enumerate()
            .for_each(|(y, row)| {
                let dy = y as f64 - frame.center_y;
                for (x, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let dx = x as f64 - frame.center_x;
                    let factor = self.factor(dx.hypot(dy) / frame.half_diagonal);
                    let source_x = frame.center_x + dx * factor;
                    let source_y = frame.center_y + dy * factor;
                    frame.sample(samples, source_x, source_y, pixel);
                }
            });
        out
    }
}

/// Vignetting gain at each normalized output radius.
#[derive(Debug, Clone, PartialEq)]
pub struct RadialGain {
    /// Gain at `index / (STEPS - 1)`.
    gains: Vec<f32>,
}

impl RadialGain {
    const STEPS: usize = 1024;

    /// Gain undoing `vignetting`, read at the sensor radius each output
    /// radius samples: through `warp` when the frame was undistorted.
    pub(crate) fn new(vignetting: &RadialTable, warp: Option<&LensWarp>) -> Self {
        let gains = (0..Self::STEPS)
            .map(|step| {
                let radius = step as f64 / (Self::STEPS - 1) as f64;
                let sensor = warp.map_or(radius, |warp| warp.sensor_radius(radius));
                let transmission = vignetting.at(sensor).max(LensProfile::MIN_VIGNETTING);
                (1.0 / transmission) as f32
            })
            .collect();
        Self { gains }
    }

    /// Gain at normalized output radius `radius`.
    pub(crate) fn at(&self, radius: f32) -> f32 {
        let position = radius.clamp(0.0, 1.0) * (Self::STEPS - 1) as f32;
        let lower = (position as usize).min(Self::STEPS - 2);
        let t = position - lower as f32;
        self.gains[lower] + t * (self.gains[lower + 1] - self.gains[lower])
    }

    /// Multiply interleaved RGB pixels of a `width × height` frame by the
    /// gain at each pixel's radius. `chunk` holds whole pixels, starting at
    /// pixel index `first_pixel`.
    pub(crate) fn apply(&self, width: usize, height: usize, first_pixel: usize, chunk: &mut [f32]) {
        let frame = Frame::new(width, height);
        let inverse = (1.0 / frame.half_diagonal) as f32;
        for (offset, pixel) in chunk.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let index = first_pixel + offset;
            let dx = (index % width) as f32 - frame.center_x as f32;
            let dy = (index / width) as f32 - frame.center_y as f32;
            let gain = self.at(dx.hypot(dy) * inverse);
            for value in pixel {
                *value *= gain;
            }
        }
    }
}

/// Frame geometry and the resampler.
struct Frame {
    width: usize,
    height: usize,
    center_x: f64,
    center_y: f64,
    half_diagonal: f64,
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

    /// Catmull-Rom sample of all three channels at `(x, y)`, held inside
    /// the four nearest pixels' range so hard edges do not ring. Edges are
    /// clamped.
    fn sample(&self, samples: &[f32], x: f64, y: f64, out: &mut [f32]) {
        for (channel, value) in out.iter_mut().enumerate() {
            *value = self.cubic_channel(samples, x, y, channel);
        }
    }

    /// One channel of [`Self::sample`].
    fn cubic_channel(&self, samples: &[f32], x: f64, y: f64, channel: usize) -> f32 {
        let x0 = x.floor();
        let y0 = y.floor();
        let wx = Self::catmull_rom(x - x0);
        let wy = Self::catmull_rom(y - y0);
        let (x0, y0) = (x0 as i64 - 1, y0 as i64 - 1);
        let max_x = self.width as i64 - 1;
        let max_y = self.height as i64 - 1;
        let columns: [usize; 4] =
            std::array::from_fn(|i| (x0 + i as i64).clamp(0, max_x) as usize * 3);
        let rows: [usize; 4] =
            std::array::from_fn(|j| (y0 + j as i64).clamp(0, max_y) as usize * self.width * 3);
        let mut sum = 0.0;
        for (row, weight_y) in rows.iter().zip(wy) {
            let line: f32 = columns
                .iter()
                .zip(wx)
                .map(|(column, weight_x)| weight_x * samples[row + column + channel])
                .sum();
            sum += weight_y * line;
        }
        let near = [
            samples[rows[1] + columns[1] + channel],
            samples[rows[1] + columns[2] + channel],
            samples[rows[2] + columns[1] + channel],
            samples[rows[2] + columns[2] + channel],
        ];
        let low = near.iter().copied().fold(f32::INFINITY, f32::min);
        let high = near.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        sum.clamp(low, high)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The XF35mmF2 R WR table an X-T2 writes, as fractions.
    fn xf35_distortion() -> RadialTable {
        let knots = (0..=10).map(|step| f64::from(step) / 10.0).collect();
        let values = [
            0.0, -0.027, -0.11, -0.302, -0.656, -1.191, -1.898, -2.816, -3.999, -5.575, -7.568,
        ]
        .iter()
        .map(|percent| percent / 100.0)
        .collect();
        RadialTable::new(knots, values).unwrap()
    }

    /// The same lens wide open at f/2.8.
    fn xf35_vignetting() -> RadialTable {
        let knots = (0..=10).map(|step| f64::from(step) / 10.0).collect();
        let values = [
            100.0, 99.83, 99.69, 99.61, 99.6, 99.21, 97.84, 96.21, 93.85, 89.49, 78.82,
        ]
        .iter()
        .map(|percent| percent / 100.0)
        .collect();
        RadialTable::new(knots, values).unwrap()
    }

    #[test]
    fn the_table_passes_through_its_knots_without_overshoot() {
        let table = xf35_distortion();
        assert!((table.at(0.5) + 0.01191).abs() < 1e-12);
        assert!((table.at(1.0) + 0.07568).abs() < 1e-12);
        let mut previous = table.at(0.0);
        for step in 1..=1000 {
            let value = table.at(f64::from(step) / 1000.0);
            assert!(value <= previous + 1e-15, "not monotone at {step}");
            previous = value;
        }
    }

    #[test]
    fn a_flat_table_or_an_implausible_one_is_dropped() {
        let knots: Vec<f64> = vec![0.0, 0.5, 1.0];
        let flat = RadialTable::new(knots.clone(), vec![0.0; 3]);
        let unlit = RadialTable::new(knots.clone(), vec![1.0; 3]);
        assert_eq!(LensProfile::new(flat, unlit), LensProfile::NONE);
        let wild = RadialTable::new(knots, vec![0.0, -0.2, -0.6]);
        assert_eq!(LensProfile::new(wild, None), LensProfile::NONE);
        assert!(RadialTable::new(vec![0.0, 0.5, 0.5], vec![0.0; 3]).is_none());
    }

    #[test]
    fn the_fill_scale_matches_the_cameras_framing() {
        // Measured from the X-T2's own JPEGs: s = 1.0156 ± 0.0005.
        let half: (f64, f64) = (2999.5, 1999.5);
        let scale = LensWarp::fill_scale(&xf35_distortion(), half.1 / half.0.hypot(half.1));
        let scale = scale.unwrap();
        assert!((scale - 1.0156).abs() < 0.001, "scale {scale}");
    }

    #[test]
    fn every_output_edge_samples_inside_the_sensor_and_one_edge_touches_it() {
        let (half_width, half_height) = (2999.5, 1999.5);
        let half_diagonal = f64::hypot(half_width, half_height);
        let warp = LensWarp::new(&xf35_distortion(), half_width, half_height).unwrap();
        let mut closest = f64::INFINITY;
        for step in 0..=400 {
            let t = f64::from(step) / 400.0;
            for (x, y) in [(half_width * t, half_height), (half_width, half_height * t)] {
                let factor = warp.factor(x.hypot(y) / half_diagonal);
                let (sx, sy) = (x * factor, y * factor);
                assert!(sx <= half_width + 0.01 && sy <= half_height + 0.01);
                closest = closest.min((half_width - sx).min(half_height - sy));
            }
        }
        assert!(closest < 0.5, "edge gap {closest}");
    }

    #[test]
    fn the_warp_inverts_the_table() {
        let table = xf35_distortion();
        let warp = LensWarp::new(&table, 2999.5, 1999.5).unwrap();
        let scale = LensWarp::fill_scale(&table, 1999.5 / f64::hypot(2999.5, 1999.5)).unwrap();
        for step in 1..=20 {
            let output = f64::from(step) / 20.0;
            let sensor = warp.sensor_radius(output);
            let back = sensor / (scale * (1.0 + table.at(sensor)));
            assert!((back - output).abs() < 1e-5, "{output} → {sensor} → {back}");
        }
    }

    #[test]
    fn undistorting_a_barrel_grid_straightens_its_lines() {
        // A vertical line bowed by the lens: render where the sensor saw it.
        let (width, height) = (300, 200);
        let table = xf35_distortion();
        let warp = LensWarp::new(&table, 149.5, 99.5).unwrap();
        let frame = Frame::new(width, height);
        // Output pixels within 0.5 px of x = 40 are the line; find, for each
        // row, the sensor x the warp reads there, and draw the line there.
        let mut samples = vec![0.0_f32; width * height * 3];
        let mut bow = (f64::INFINITY, f64::NEG_INFINITY);
        for y in 0..height {
            let dy = y as f64 - frame.center_y;
            let dx = 40.0 - frame.center_x;
            let factor = warp.factor(dx.hypot(dy) / frame.half_diagonal);
            let sx = frame.center_x + dx * factor;
            bow = (bow.0.min(sx), bow.1.max(sx));
            for x in 0..width {
                let coverage = (1.0 - (x as f64 - sx).abs()).max(0.0) as f32;
                samples[(y * width + x) * 3..][..3].fill(coverage);
            }
        }
        assert!(bow.1 - bow.0 > 2.0, "the sensor line bows {bow:?}");
        let out = warp.apply(width, height, &samples);
        for y in 0..height {
            let row = &out[y * width * 3..(y + 1) * width * 3];
            let (mut mass, mut moment) = (0.0, 0.0);
            for (x, pixel) in row.as_chunks::<3>().0.iter().enumerate() {
                mass += pixel[1] as f64;
                moment += pixel[1] as f64 * x as f64;
            }
            let center = moment / mass;
            assert!((center - 40.0).abs() < 0.2, "row {y} line at {center}");
        }
    }

    #[test]
    fn the_resampler_does_not_ring_on_a_hard_edge() {
        let (width, height) = (64, 64);
        let mut samples = vec![0.0_f32; width * height * 3];
        for (index, pixel) in samples.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            if index % width >= 32 {
                pixel.fill(1.0);
            }
        }
        let frame = Frame::new(width, height);
        let mut out = [0.0_f32; 3];
        for step in 0..=20 {
            let x = 29.0 + f64::from(step) * 0.25;
            frame.sample(&samples, x, 20.0, &mut out);
            assert!((0.0..=1.0).contains(&out[0]), "{x}: {}", out[0]);
        }
    }

    #[test]
    fn the_vignetting_gain_lifts_the_corner_by_the_table() {
        let vignetting = xf35_vignetting();
        let gain = RadialGain::new(&vignetting, None);
        assert!((gain.at(0.0) - 1.0).abs() < 1e-6);
        assert!((gain.at(1.0) - 1.0 / 0.7882).abs() < 1e-4);
        // Through the warp, the corner reads the sensor inside the corner,
        // which is less vignetted.
        let warp = LensWarp::new(&xf35_distortion(), 2999.5, 1999.5).unwrap();
        let warped = RadialGain::new(&vignetting, Some(&warp));
        assert!(warped.at(1.0) < gain.at(1.0));
        assert!(warped.at(1.0) > 1.1);
    }

    fn rationals(parts: &[(i32, i32)]) -> Vec<f64> {
        parts
            .iter()
            .map(|(numerator, denominator)| f64::from(*numerator) / f64::from(*denominator))
            .collect()
    }

    #[test]
    fn measured_ca_tables_parse_and_empty_or_wild_ones_are_dropped() {
        // X-S10, 29 rationals. The knot at radius 1 is −48/65536 red, 30/65536 blue.
        let mut xs10 = vec![(3749, 9)];
        for numerator in [502, 710, 870, 1004, 1123, 1230, 1328, 1420, 1506] {
            xs10.push((numerator, 1420));
        }
        for numerator in [-30, -34, -38, -42, -44, -46, -46, -48, -48] {
            xs10.push((numerator, 65536));
        }
        for numerator in [12, 16, 20, 24, 26, 28, 30, 30, 30] {
            xs10.push((numerator, 65536));
        }
        xs10.push((3749, 9));
        let (red, blue) = LensProfile::ca_tables(&rationals(&xs10)).unwrap();
        assert!(red.at(0.0).abs() < 1e-12);
        assert!((red.at(1.0) + 48.0 / 65536.0).abs() < 1e-12);
        assert!((blue.at(1.0) - 30.0 / 65536.0).abs() < 1e-12);

        // X-T2, 31 rationals. The center knot is omitted; red at 0.1 is 115e-6.
        let mut xt2 = vec![(3605, 10)];
        for numerator in 1..=10 {
            xt2.push((numerator, 10));
        }
        for numerator in [115, 216, 285, 310, 305, 305, 305, 304, 310, 244] {
            xt2.push((numerator, 1_000_000));
        }
        for numerator in [-36, -72, -105, -135, -152, -152, -153, -115, -43, 0] {
            xt2.push((numerator, 1_000_000));
        }
        let (red, blue) = LensProfile::ca_tables(&rationals(&xt2)).unwrap();
        assert!(red.at(0.0).abs() < 1e-12);
        assert!((red.at(0.1) - 115.0 / 1_000_000.0).abs() < 1e-12);
        assert!(blue.at(1.0).abs() < 1e-12);

        let mut flat = vec![1.0, 0.25, 0.5, 0.75, 1.0];
        flat.extend([0.0; 8]);
        assert!(LensProfile::ca_tables(&flat).is_none());
        let mut wild = vec![1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0];
        assert!(LensProfile::ca_tables(&wild).is_none());
        wild[5] = 0.001;
        assert!(LensProfile::ca_tables(&wild).is_some());
    }

    #[test]
    fn a_camera_table_is_applied_where_the_estimate_cannot_run() {
        let profile = LensProfile::NONE.with_chromatic_aberration(Some((
            RadialTable::new(vec![0.0, 1.0], vec![0.001, 0.001]).unwrap(),
            RadialTable::new(vec![0.0, 1.0], vec![-0.001, -0.001]).unwrap(),
        )));
        let (width, height) = (32, 32);
        let mut samples = vec![0.0_f32; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let pixel = &mut samples[(y * width + x) * 3..][..3];
                pixel[0] = x as f32;
                pixel[1] = 0.4;
                pixel[2] = y as f32;
            }
        }
        assert_eq!(
            crate::lateral_ca::LateralCa::estimate(width, height, &samples),
            crate::lateral_ca::LateralCa::NONE,
            "this frame is too small for the image estimate"
        );
        let original = samples.clone();
        profile.correct_lateral_ca(width, height, &mut samples);
        // (24, 16) sits 8.5 px right and 0.5 px below center. Red is read
        // 0.1% further out, blue 0.1% closer in.
        let index = (16 * width + 24) * 3;
        assert!(
            (samples[index] - 24.0085).abs() < 0.002,
            "red {}",
            samples[index]
        );
        assert!((samples[index + 1] - 0.4).abs() < 1e-6);
        assert!(
            (samples[index + 2] - 15.9995).abs() < 0.002,
            "blue {}",
            samples[index + 2]
        );
        assert_ne!(samples, original);
    }
}
