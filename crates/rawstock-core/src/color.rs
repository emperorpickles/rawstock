//! Scene-linear color: demosaiced camera RGB → ACEScg, and the sRGB output transform.
//!
//! [`RawImage::develop_linear`](crate::decode::RawImage::develop_linear) asks
//! [`rawler`] to rescale black/white levels and demosaic (PPG for Bayer,
//! bilinear for X-Trans, pass-through for linear DNG). White balance and the
//! camera → ACEScg matrix live here. ACEScg negatives are kept.
//! [`encode_srgb`] is the output transform: ACEScg to encoded sRGB, applied
//! after the film pipeline.

use std::path::Path;

use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::sensor::Demosaic;
use rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic;
use rawler::imgop::{Point, Rect};
use rawler::rawimage::RawPhotometricInterpretation;
use rayon::prelude::*;
use thiserror::Error;

use crate::decode::{CfaKind, RawImage};
use crate::matrices;
use crate::workers::Workers;

/// Failure while developing a RAW file to scene-linear ACEScg or writing it.
#[derive(Debug, Error)]
pub enum DevelopError {
    /// `rawler` could not rescale or demosaic the file.
    #[error("failed to demosaic RAW data")]
    Demosaic {
        /// The decoder error.
        #[source]
        source: rawler::RawlerError,
    },

    /// Demosaic returned a channel count this path does not convert.
    #[error("demosaic produced {channels} color channels")]
    UnexpectedChannels {
        /// Channel count `rawler` returned.
        channels: usize,
    },

    /// A dimension did not fit in `u32`.
    #[error("image dimension {value} does not fit in u32")]
    Dimension {
        /// The dimension that overflowed.
        value: usize,
    },

    /// The sample buffer does not match width × height × 3.
    #[error("linear image has {actual} samples, expected {expected}")]
    Samples {
        /// Samples present in the buffer.
        actual: usize,
        /// Samples required by the dimensions.
        expected: usize,
    },

    /// A full develop installed over a quick one is not the frame the quick
    /// develop promised.
    #[error("full develop is {actual_width}x{actual_height}, expected {width}x{height}")]
    FrameMismatch {
        /// Width the quick develop promised.
        width: u32,
        /// Height the quick develop promised.
        height: u32,
        /// Width of the full develop.
        actual_width: u32,
        /// Height of the full develop.
        actual_height: u32,
    },

    /// The XYZ→camera matrix cannot be inverted.
    #[error("camera color matrix is singular")]
    SingularCameraMatrix,

    /// As-shot white balance has no positive finite RGB coefficients.
    #[error("as-shot white balance is missing")]
    MissingWhiteBalance,

    /// The OpenEXR writer rejected the image.
    #[error("failed to write OpenEXR")]
    WriteExr {
        /// The writer error.
        #[source]
        source: exr::error::Error,
    },

    /// The OpenEXR reader rejected the image.
    #[error("failed to read OpenEXR")]
    ReadExr {
        /// The reader error.
        #[source]
        source: exr::error::Error,
    },

    /// A fit proxy was asked for a long edge of zero.
    #[error("proxy long edge must be positive")]
    EmptyProxy,
}

/// Scene-linear ACEScg image, interleaved f32 RGB, row-major.
#[derive(Clone)]
pub struct LinearImage {
    width: u32,
    height: u32,
    pixels: Vec<f32>,
}

impl std::fmt::Debug for LinearImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &self.pixels.len())
            .finish()
    }
}

impl LinearImage {
    /// Build an image from interleaved RGB samples.
    ///
    /// `pixels` must contain `width * height * 3` finite-layout samples.
    /// Values are not required to be finite or inside `0..=1`: ACEScg is
    /// scene-referred.
    pub fn new(width: u32, height: u32, pixels: Vec<f32>) -> Result<Self, DevelopError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(3))
            .ok_or(DevelopError::Samples {
                actual: pixels.len(),
                expected: 0,
            })?;
        if pixels.len() != expected {
            return Err(DevelopError::Samples {
                actual: pixels.len(),
                expected,
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Interleaved RGB samples, length [`Self::width`] × [`Self::height`] × 3.
    pub fn pixels(&self) -> &[f32] {
        &self.pixels
    }

    /// Bake `orientation` into this image.
    ///
    /// An identity orientation leaves the buffer in place. The sample count
    /// does not change, so this cannot fail the way [`Self::new`] can.
    pub fn apply_orientation(&mut self, orientation: crate::decode::Orientation) {
        if orientation.is_identity() {
            return;
        }
        let (width, height, pixels) = orientation.apply_rgb(self.width, self.height, &self.pixels);
        self.width = width;
        self.height = height;
        self.pixels = pixels;
    }

    /// Width and height of a proxy whose longer side is `long_edge`.
    ///
    /// The shorter side is rounded to the nearest pixel and is at least 1.
    /// An image already smaller than `long_edge` keeps its size. `long_edge`
    /// of zero yields `(0, 0)`.
    pub fn proxy_dimensions(width: u32, height: u32, long_edge: u32) -> (u32, u32) {
        if width == 0 || height == 0 || long_edge == 0 {
            return (0, 0);
        }
        let long = width.max(height);
        if long <= long_edge {
            return (width, height);
        }
        if width >= height {
            let short = round_div(u64::from(height) * u64::from(long_edge), u64::from(width));
            (long_edge, short)
        } else {
            let short = round_div(u64::from(width) * u64::from(long_edge), u64::from(height));
            (short, long_edge)
        }
    }

    /// Area-downsample so the longer side is `long_edge`.
    ///
    /// Each output pixel is the mean of the source rectangle that maps onto
    /// it, weighted by coverage. This runs after demosaic, so X-Trans and
    /// ProRAW share one proxy. An image already within `long_edge` is cloned.
    pub fn area_downsample(&self, long_edge: u32) -> Result<Self, DevelopError> {
        if long_edge == 0 {
            return Err(DevelopError::EmptyProxy);
        }
        let (width, height) = Self::proxy_dimensions(self.width, self.height, long_edge);
        if width == self.width && height == self.height {
            return Ok(self.clone());
        }
        let pixels = area_box(&self.pixels, self.width, self.height, width, height);
        Self::new(width, height, pixels)
    }

    /// Read scene-linear ACEScg RGB written by [`Self::write_exr`].
    ///
    /// The file is interleaved RGB. A display transform is not applied.
    pub fn read_exr(path: impl AsRef<Path>) -> Result<Self, DevelopError> {
        let decoded = exr::prelude::read_first_rgba_layer_from_file(
            path,
            |resolution, _channels| {
                (
                    resolution.x(),
                    resolution.y(),
                    vec![[0.0f32; 4]; resolution.x() * resolution.y()],
                )
            },
            |buffer: &mut (usize, usize, Vec<[f32; 4]>), position, pixel: (f32, f32, f32, f32)| {
                let width = buffer.0;
                buffer.2[position.y() * width + position.x()] =
                    [pixel.0, pixel.1, pixel.2, pixel.3];
            },
        )
        .map_err(|source| DevelopError::ReadExr { source })?;
        let (width, height, pixels) = decoded.layer_data.channel_data.pixels;
        let rgb = pixels
            .into_iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        let width = u32::try_from(width).map_err(|_| DevelopError::Dimension { value: width })?;
        let height =
            u32::try_from(height).map_err(|_| DevelopError::Dimension { value: height })?;
        Self::new(width, height, rgb)
    }

    /// Write scene-linear ACEScg RGB as a 32-bit float OpenEXR file.
    ///
    /// Channels are `R`, `G`, and `B`. The encoding is linear: nothing in
    /// this method applies a display transform.
    pub fn write_exr(&self, path: impl AsRef<Path>) -> Result<(), DevelopError> {
        let width = self.width as usize;
        let height = self.height as usize;
        let pixels = &self.pixels;
        exr::prelude::write_rgb_file(path, width, height, |x, y| {
            let index = (y * width + x) * 3;
            (pixels[index], pixels[index + 1], pixels[index + 2])
        })
        .map_err(|source| DevelopError::WriteExr { source })
    }
}

impl RawImage {
    /// Demosaic and convert this file to scene-linear ACEScg.
    ///
    /// Black/white normalization and demosaic are `rawler`'s rescale and
    /// demosaic steps (Bayer via PPG, X-Trans via bilinear, linear DNG
    /// unchanged). Rescale does not clip above the white level, so highlights
    /// are cut to that level in camera RGB, white-balanced with green at 1,
    /// then cut again at the smallest multiplier. That second cut is what
    /// keeps a clipped neutral from turning magenta: the short channel would
    /// otherwise be the only one still at the sensor white. The D65 camera
    /// matrix is row-scaled so D65 maps to camera white, inverted, then a
    /// Bradford adaptation takes D65 to the ACES white and AP1 takes that XYZ
    /// into ACEScg. White balance is already in the samples, so the matrix
    /// sees an identity balance.
    ///
    /// Markesteijn runs on the capped [`Workers`] pool.
    pub fn develop_linear(&self) -> Result<LinearImage, DevelopError> {
        Workers::install(|| self.develop_linear_here())
    }

    /// [`Self::develop_linear`] on whatever pool the caller is on.
    fn develop_linear_here(&self) -> Result<LinearImage, DevelopError> {
        let developed = if self.cfa() == CfaKind::XTrans {
            self.demosaic_xtrans()?
        } else {
            RawDevelop::new_with(&[ProcessingStep::Rescale, ProcessingStep::Demosaic])
                .develop_intermediate(self.decoded())
                .map_err(|source| DevelopError::Demosaic { source })?
        };

        let (width, height, samples) = match developed {
            Intermediate::ThreeColor(pixels) => take_rgb(pixels)?,
            Intermediate::FourColor(_) => {
                return Err(DevelopError::UnexpectedChannels { channels: 4 });
            }
            Intermediate::Monochrome(_) => {
                return Err(DevelopError::UnexpectedChannels { channels: 1 });
            }
        };

        self.finish_linear(width, height, samples)
    }

    /// [`Self::develop_linear`] on a pool of its own, `threads` wide.
    ///
    /// A preview that renders on the shared pool while a background
    /// demosaic runs there waits behind every Markesteijn tile. On its own
    /// pool the two only share the cores.
    pub fn develop_linear_on(&self, threads: usize) -> Result<LinearImage, DevelopError> {
        Workers::install_separate(threads, || self.develop_linear_here())
    }

    /// A third-size develop of an X-Trans frame, for the first preview.
    ///
    /// Every 3×3 window of the X-Trans pattern holds five green photosites
    /// and at least one red and one blue. Each block's per-color means are
    /// one RGB pixel, so this takes a fraction of a second where Markesteijn
    /// takes seconds. A preview downsampled from it is within about 0.5% of
    /// one downsampled from Markesteijn. The
    /// frame is the one [`Self::develop_linear`] returns, a third the size.
    /// `None` for a sensor that is not X-Trans.
    pub fn develop_quick_linear(&self) -> Result<Option<QuickDevelop>, DevelopError> {
        if self.cfa() != CfaKind::XTrans {
            return Ok(None);
        }
        let scaled = RawDevelop::new_with(&[ProcessingStep::Rescale])
            .develop_intermediate(self.decoded())
            .map_err(|source| DevelopError::Demosaic { source })?;
        let Intermediate::Monochrome(mosaic) = scaled else {
            return Err(DevelopError::UnexpectedChannels { channels: 0 });
        };
        let RawPhotometricInterpretation::Cfa(config) = &self.decoded().photometric else {
            return Err(DevelopError::UnexpectedChannels { channels: 0 });
        };
        let frame = self.xtrans_frame(mosaic.rect());
        let (full_width, full_height) = (frame.d.w, frame.d.h);
        let width = full_width / 3;
        let height = full_height / 3;
        if width == 0 || height == 0 {
            return Err(DevelopError::Dimension { value: 0 });
        }
        let mut samples = vec![0.0_f32; width * height * 3];
        Workers::install(|| {
            samples
                .par_chunks_mut(width * 3)
                .enumerate()
                .for_each(|(row, out)| {
                    for column in 0..width {
                        let mut sum = [0.0_f32; 3];
                        let mut count = [0_u32; 3];
                        for dy in 0..3 {
                            let y = frame.p.y + row * 3 + dy;
                            for dx in 0..3 {
                                let x = frame.p.x + column * 3 + dx;
                                let color = config.cfa.color_at(y, x).min(2);
                                sum[color] += mosaic.data[y * mosaic.width + x];
                                count[color] += 1;
                            }
                        }
                        for channel in 0..3 {
                            out[column * 3 + channel] = if count[channel] == 0 {
                                0.0
                            } else {
                                sum[channel] / count[channel] as f32
                            };
                        }
                    }
                });
        });
        let image = self.finish_linear(
            u32::try_from(width).map_err(|_| DevelopError::Dimension { value: width })?,
            u32::try_from(height).map_err(|_| DevelopError::Dimension { value: height })?,
            samples,
        )?;
        Ok(Some(QuickDevelop {
            image,
            full_width: u32::try_from(full_width)
                .map_err(|_| DevelopError::Dimension { value: full_width })?,
            full_height: u32::try_from(full_height)
                .map_err(|_| DevelopError::Dimension { value: full_height })?,
        }))
    }

    /// White balance, highlight recovery, and the ACEScg matrix, on
    /// demosaiced camera RGB.
    fn finish_linear(
        &self,
        width: u32,
        height: u32,
        mut samples: Vec<f32>,
    ) -> Result<LinearImage, DevelopError> {
        let white_balance = normalized_white_balance(self.as_shot_wb())?;
        recover_highlights(&mut samples, white_balance);
        let transform = acescg_transform(self.camera_matrix(), [1.0, 1.0, 1.0, 0.0])?;
        apply_transform(&mut samples, transform);
        LinearImage::new(width, height, samples)
    }

    /// The X-Trans frame a develop returns, in full-sensor coordinates: the
    /// camera's default crop when it sits inside the active area, else the
    /// active area.
    fn xtrans_frame(&self, full: Rect) -> Rect {
        let decoded = self.decoded();
        let active = decoded.active_area.unwrap_or(full);
        match decoded.crop_area {
            Some(crop)
                if crop.p.x >= active.p.x
                    && crop.p.y >= active.p.y
                    && crop.p.x + crop.d.w <= active.p.x + active.d.w
                    && crop.p.y + crop.d.h <= active.p.y + active.d.h =>
            {
                crop
            }
            _ => active,
        }
    }
}

/// A fast, smaller develop of a frame, and the size of its full develop.
pub struct QuickDevelop {
    /// Scene-linear ACEScg at a third of the full size.
    pub image: LinearImage,
    /// Width [`RawImage::develop_linear`] will return.
    pub full_width: u32,
    /// Height [`RawImage::develop_linear`] will return.
    pub full_height: u32,
}

impl RawImage {
    /// Rescale, then demosaic X-Trans with 3-pass Markesteijn.
    ///
    /// `rawler`'s own develop path uses bilinear for X-Trans, which zippers
    /// and fringes on high-contrast edges. Its Markesteijn demosaic is
    /// directional and works in CIELab, so edges stay clean.
    fn demosaic_xtrans(&self) -> Result<Intermediate, DevelopError> {
        let scaled = RawDevelop::new_with(&[ProcessingStep::Rescale])
            .develop_intermediate(self.decoded())
            .map_err(|source| DevelopError::Demosaic { source })?;
        let Intermediate::Monochrome(mosaic) = scaled else {
            return Err(DevelopError::UnexpectedChannels { channels: 0 });
        };
        let RawPhotometricInterpretation::Cfa(config) = &self.decoded().photometric else {
            return Err(DevelopError::UnexpectedChannels { channels: 0 });
        };
        // Demosaic the active area only: the sensor strip outside it is
        // optical black and would bleed into the edge pixels.
        let decoded = self.decoded();
        let full = mosaic.rect();
        let active = decoded.active_area.unwrap_or(full);
        let rgb = XTransMarkesteijnDemosaic::new_pass_3().demosaic(
            &mosaic,
            &config.cfa,
            &config.colors,
            active,
        );
        // Then the camera's default crop, which sits in full-sensor
        // coordinates, so the result matches the camera's own JPEG framing.
        let frame = self.xtrans_frame(full);
        if frame == active {
            return Ok(Intermediate::ThreeColor(rgb));
        }
        let local = Rect::new(
            Point::new(frame.p.x - active.p.x, frame.p.y - active.p.y),
            frame.d,
        );
        Ok(Intermediate::ThreeColor(rgb.crop(local)))
    }
}

fn round_div(numerator: u64, denominator: u64) -> u32 {
    let value = (numerator + denominator / 2) / denominator;
    u32::try_from(value.max(1)).unwrap_or(u32::MAX)
}

fn area_box(src: &[f32], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<f32> {
    let src_w = src_w as usize;
    let src_h = src_h as usize;
    let dst_w = dst_w as usize;
    let dst_h = dst_h as usize;
    let mut out = vec![0.0f32; dst_w * dst_h * 3];
    let src_wf = src_w as f64;
    let src_hf = src_h as f64;
    let dst_wf = dst_w as f64;
    let dst_hf = dst_h as f64;
    for y in 0..dst_h {
        let y0 = y as f64 * src_hf / dst_hf;
        let y1 = (y + 1) as f64 * src_hf / dst_hf;
        let y_start = y0.floor() as usize;
        let y_end = (y1.ceil() as usize).min(src_h);
        for x in 0..dst_w {
            let x0 = x as f64 * src_wf / dst_wf;
            let x1 = (x + 1) as f64 * src_wf / dst_wf;
            let x_start = x0.floor() as usize;
            let x_end = (x1.ceil() as usize).min(src_w);
            let mut acc = [0.0f64; 3];
            let mut weight = 0.0f64;
            for sy in y_start..y_end {
                let row_weight = coverage(sy as f64, sy as f64 + 1.0, y0, y1);
                if row_weight == 0.0 {
                    continue;
                }
                for sx in x_start..x_end {
                    let covered = row_weight * coverage(sx as f64, sx as f64 + 1.0, x0, x1);
                    if covered == 0.0 {
                        continue;
                    }
                    let index = (sy * src_w + sx) * 3;
                    acc[0] += f64::from(src[index]) * covered;
                    acc[1] += f64::from(src[index + 1]) * covered;
                    acc[2] += f64::from(src[index + 2]) * covered;
                    weight += covered;
                }
            }
            let index = (y * dst_w + x) * 3;
            if weight > 0.0 {
                out[index] = (acc[0] / weight) as f32;
                out[index + 1] = (acc[1] / weight) as f32;
                out[index + 2] = (acc[2] / weight) as f32;
            }
        }
    }
    out
}

fn coverage(start: f64, end: f64, window_start: f64, window_end: f64) -> f64 {
    (end.min(window_end) - start.max(window_start)).max(0.0)
}

fn take_rgb(pixels: rawler::pixarray::RgbF32) -> Result<(u32, u32, Vec<f32>), DevelopError> {
    let width = u32::try_from(pixels.width).map_err(|_| DevelopError::Dimension {
        value: pixels.width,
    })?;
    let height = u32::try_from(pixels.height).map_err(|_| DevelopError::Dimension {
        value: pixels.height,
    })?;
    let samples = pixels.into_flatten();
    Ok((width, height, samples))
}

/// Bake as-shot white balance into the camera → ACEScg matrix.
fn acescg_transform(
    xyz_to_camera: [[f32; 3]; 4],
    as_shot_wb: [f32; 4],
) -> Result<[[f32; 3]; 3], DevelopError> {
    let xyz_to_camera = [
        row(xyz_to_camera[0]),
        row(xyz_to_camera[1]),
        row(xyz_to_camera[2]),
    ];
    let white_balance = normalized_white_balance(as_shot_wb)?;
    let to_aces =
        matrices::camera_to_acescg(xyz_to_camera).ok_or(DevelopError::SingularCameraMatrix)?;
    let mut baked = [[0.0f32; 3]; 3];
    for row_index in 0..3 {
        for col in 0..3 {
            baked[row_index][col] = (to_aces[row_index][col] * white_balance[col]) as f32;
        }
    }
    Ok(baked)
}

fn row(values: [f32; 3]) -> [f64; 3] {
    [
        f64::from(values[0]),
        f64::from(values[1]),
        f64::from(values[2]),
    ]
}

/// Scale as-shot RGB multipliers so green is 1.
///
/// Green = 1 keeps a fully exposed green photosite at 1.0 after black/white
/// normalization. The fourth (RGBE) coefficient is unused: demosaic returns
/// three channels.
fn normalized_white_balance(as_shot_wb: [f32; 4]) -> Result<[f64; 3], DevelopError> {
    let rgb = [as_shot_wb[0], as_shot_wb[1], as_shot_wb[2]];
    if rgb.iter().any(|coeff| !coeff.is_finite() || *coeff <= 0.0) {
        return Err(DevelopError::MissingWhiteBalance);
    }
    let green = f64::from(rgb[1]);
    Ok([f64::from(rgb[0]) / green, 1.0, f64::from(rgb[2]) / green])
}

/// Clip photosites to the white level, apply white balance, then clip to the
/// first channel that would have clipped on a neutral.
///
/// `white_balance` is green-normalized. A fully exposed neutral is `(1, 1, 1)`
/// on the sensor. Multiplying by a balance whose red or blue coefficient is
/// above 1, then by the color matrix, leaves green short and the highlight
/// encodes magenta. Cutting every balanced channel at `min(white_balance)`
/// pulls that neutral back to camera white. A colorful pixel that only clips
/// one channel stays colorful: only the channels past the cap move.
fn recover_highlights(samples: &mut [f32], white_balance: [f64; 3]) {
    let wb = white_balance.map(|channel| channel as f32);
    let cap = wb[0].min(wb[1]).min(wb[2]);
    for pixel in samples.as_chunks_mut::<3>().0 {
        for channel in 0..3 {
            let sensor = pixel[channel];
            let sensor = if sensor.is_finite() {
                sensor.clamp(0.0, 1.0)
            } else {
                0.0
            };
            pixel[channel] = (sensor * wb[channel]).min(cap);
        }
    }
}

fn apply_transform(samples: &mut [f32], matrix: [[f32; 3]; 3]) {
    for pixel in samples.as_chunks_mut::<3>().0 {
        let red = pixel[0];
        let green = pixel[1];
        let blue = pixel[2];
        pixel[0] = matrix[0][0] * red + matrix[0][1] * green + matrix[0][2] * blue;
        pixel[1] = matrix[1][0] * red + matrix[1][1] * green + matrix[1][2] * blue;
        pixel[2] = matrix[2][0] * red + matrix[2][1] * green + matrix[2][2] * blue;
    }
}

/// Encode scene-linear ACEScg as display-encoded sRGB.
///
/// Each pixel goes through the ACEScg → linear sRGB matrix, is clipped to
/// `0..=1`, then through the IEC 61966-2-1 transfer function. Clipping is the
/// gamut map: a channel outside sRGB is cut. A trailing incomplete pixel is
/// omitted.
pub fn encode_srgb(linear_acescg: &[f32]) -> Vec<f32> {
    let matrix = crate::matrices::acescg_to_linear_srgb();
    let mut encoded = Vec::with_capacity(linear_acescg.len());
    for pixel in linear_acescg.as_chunks::<3>().0 {
        let linear = pixel.map(f64::from);
        encoded.extend(
            crate::matrices::apply(matrix, linear).map(|channel| srgb_oetf(channel as f32)),
        );
    }
    encoded
}

/// IEC 61966-2-1 encoding function. Input is clipped to `0..=1`.
fn srgb_oetf(linear: f32) -> f32 {
    let linear = if linear.is_finite() {
        linear.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_a_short_buffer() {
        let err = LinearImage::new(2, 2, vec![0.0; 3]).unwrap_err();
        assert!(err.to_string().contains("3 samples"));
    }

    #[test]
    fn exr_roundtrip_preserves_samples() {
        let image = LinearImage::new(2, 1, vec![0.25, -0.5, 1.5, 0.0, 1.0, 2.0]).unwrap();
        let path = std::env::temp_dir().join("rawstock-linear-roundtrip.exr");
        image.write_exr(&path).unwrap();
        let read_back = LinearImage::read_exr(&path).unwrap();
        assert_eq!((read_back.width(), read_back.height()), (2, 1));
        for (got, expected) in read_back
            .pixels()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(image.pixels().as_chunks::<3>().0)
        {
            for (channel, want) in got.iter().zip(expected) {
                assert!((channel - want).abs() < 1.0e-6, "{got:?} != {expected:?}");
            }
        }

        let decoded = exr::prelude::read_first_rgba_layer_from_file(
            &path,
            |resolution, _channels| {
                assert_eq!((resolution.x(), resolution.y()), (2, 1));
                vec![[0.0f32; 4]; resolution.x() * resolution.y()]
            },
            |buffer: &mut Vec<[f32; 4]>, position, pixel: (f32, f32, f32, f32)| {
                buffer[position.y() * 2 + position.x()] = [pixel.0, pixel.1, pixel.2, pixel.3];
            },
        )
        .unwrap();

        let pixels = &decoded.layer_data.channel_data.pixels;
        assert_eq!(pixels.len(), 2);
        for (got, expected) in pixels.iter().zip(image.pixels().as_chunks::<3>().0) {
            for (channel, want) in got.iter().take(3).zip(expected) {
                assert!((channel - want).abs() < 1.0e-6, "{got:?} != {expected:?}");
            }
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn camera_white_maps_to_acescg_white() {
        // sRGB's XYZ→RGB matrix stands in for a D65 camera matrix. The camera
        // value is that matrix applied to D65, which must come back as ACEScg white.
        let xyz_to_camera = srgb_xyz_to_rgb();
        let matrix = acescg_transform(xyz_to_camera, [1.0, 1.0, 1.0, 0.0]).unwrap();
        let white = apply_pixel(matrix, camera_rgb_of_d65(xyz_to_camera));
        assert_white(white);
    }

    #[test]
    fn white_balance_scales_relative_to_green() {
        let xyz_to_camera = srgb_xyz_to_rgb();
        let neutral = camera_rgb_of_d65(xyz_to_camera);
        // Multipliers (2, 1, 0.5), green already 1, so the preimage is divided
        // by those same coefficients.
        let matrix = acescg_transform(xyz_to_camera, [2.0, 1.0, 0.5, 0.0]).unwrap();
        let white = apply_pixel(matrix, [neutral[0] / 2.0, neutral[1], neutral[2] / 0.5]);
        assert_white(white);
    }

    #[test]
    fn unnormalized_camera_matrix_still_maps_white_to_white() {
        // Fujifilm X-S10 XYZ→camera matrix from the CC0 fixture. Its rows do
        // not already sum to a D65 response of (1, 1, 1).
        let xyz_to_camera = [
            [1.3426, -0.6334, -0.1177],
            [-0.4244, 1.2136, 0.2371],
            [-0.058, 0.1303, 0.598],
            [0.0, 0.0, 0.0],
        ];
        let matrix = acescg_transform(xyz_to_camera, [1.0, 1.0, 1.0, 0.0]).unwrap();
        assert_white(apply_pixel(matrix, [1.0, 1.0, 1.0]));
    }

    #[test]
    fn clipped_highlight_stays_neutral_under_fuji_balance() {
        // X-S10 matrix and an as-shot balance in the same shape as the CC0
        // fixture (green already 1, blue the largest coefficient).
        let xyz_to_camera = [
            [1.3426, -0.6334, -0.1177],
            [-0.4244, 1.2136, 0.2371],
            [-0.058, 0.1303, 0.598],
            [0.0, 0.0, 0.0],
        ];
        let balance = normalized_white_balance([1.6953642, 1.0, 2.3509934, f32::NAN]).unwrap();
        let matrix = acescg_transform(xyz_to_camera, [1.0, 1.0, 1.0, 0.0]).unwrap();

        let mut white = vec![1.4, 1.2, 1.8];
        recover_highlights(&mut white, balance);
        apply_transform(&mut white, matrix);
        assert_white([white[0], white[1], white[2]]);
        let encoded = encode_srgb(&white);
        for channel in &encoded {
            assert!((channel - 1.0).abs() < 1.0e-4, "{encoded:?}");
        }

        // A red that only just clips stays red. Baking the balance into the
        // matrix and clipping after it paints this pixel magenta.
        let mut red = vec![1.0, 0.25, 0.20];
        recover_highlights(&mut red, balance);
        apply_transform(&mut red, matrix);
        assert!(
            red[0] > red[1] + 0.15 && red[0] > red[2] + 0.05,
            "clipped red went magenta: {red:?}"
        );

        // A gray below the cap is only white-balanced, not crushed.
        let level = 0.4_f32;
        let mut gray = vec![level / balance[0] as f32, level, level / balance[2] as f32];
        recover_highlights(&mut gray, balance);
        for channel in &gray {
            assert!((channel - level).abs() < 1.0e-5, "{gray:?}");
        }
    }

    #[test]
    fn missing_white_balance_is_an_error() {
        let xyz_to_camera = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]];
        let err = acescg_transform(xyz_to_camera, [1.0, f32::NAN, 1.0, 0.0]).unwrap_err();
        assert!(err.to_string().contains("white balance"));
    }

    fn apply_pixel(matrix: [[f32; 3]; 3], pixel: [f32; 3]) -> [f32; 3] {
        let mut samples = pixel.to_vec();
        apply_transform(&mut samples, matrix);
        [samples[0], samples[1], samples[2]]
    }

    fn srgb_xyz_to_rgb() -> [[f32; 3]; 4] {
        [
            [3.2404542, -1.5371385, -0.4985314],
            [-0.969_266, 1.8760108, 0.0415560],
            [0.0556434, -0.2040259, 1.0572252],
            [0.0, 0.0, 0.0],
        ]
    }

    fn camera_rgb_of_d65(xyz_to_camera: [[f32; 3]; 4]) -> [f32; 3] {
        let matrix = [
            row(xyz_to_camera[0]),
            row(xyz_to_camera[1]),
            row(xyz_to_camera[2]),
        ];
        let camera = crate::matrices::apply(matrix, crate::matrices::d65_xyz());
        [camera[0] as f32, camera[1] as f32, camera[2] as f32]
    }

    #[test]
    fn acescg_white_encodes_to_srgb_white() {
        let encoded = encode_srgb(&[1.0, 1.0, 1.0]);
        for channel in &encoded {
            assert!((channel - 1.0).abs() < 1.0e-5, "{encoded:?}");
        }
    }

    #[test]
    fn negative_acescg_clips_to_black() {
        let encoded = encode_srgb(&[-1.0, -1.0, -1.0]);
        assert_eq!(encoded, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn proxy_long_edge_rounds_the_fixture_frames() {
        assert_eq!(LinearImage::proxy_dimensions(6384, 4182, 720), (720, 472));
        assert_eq!(LinearImage::proxy_dimensions(6384, 4182, 1440), (1440, 943));
        assert_eq!(LinearImage::proxy_dimensions(4032, 3024, 720), (720, 540));
        assert_eq!(LinearImage::proxy_dimensions(8, 4, 100), (8, 4));
    }

    #[test]
    fn area_downsample_averages_coverage() {
        let image = LinearImage::new(
            2,
            2,
            vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 8.0],
        )
        .unwrap();
        let proxy = image.area_downsample(1).unwrap();
        assert_eq!((proxy.width(), proxy.height()), (1, 1));
        let pixel = proxy.pixels();
        assert!((pixel[0] - 0.5).abs() < 1.0e-5, "{pixel:?}");
        assert!((pixel[1] - 1.0).abs() < 1.0e-5, "{pixel:?}");
        assert!((pixel[2] - 2.0).abs() < 1.0e-5, "{pixel:?}");
    }

    fn assert_white(rgb: [f32; 3]) {
        for channel in rgb {
            assert!((channel - 1.0).abs() < 3.0e-4, "{rgb:?}");
        }
    }
}
