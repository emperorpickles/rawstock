//! Scene-linear color: demosaiced camera RGB → ACEScg, and the sRGB output transform.
//!
//! [`RawImage::develop_linear`](crate::decode::RawImage::develop_linear) asks
//! [`rawler`] to rescale black/white levels and demosaic (PPG for Bayer,
//! Markesteijn for X-Trans, pass-through for linear DNG). White balance and the
//! camera → ACEScg matrix live here. ACEScg negatives are kept.
//! [`encode_srgb`] is the output transform: ACEScg to encoded sRGB, applied
//! after the film pipeline.

use std::path::Path;
use std::sync::Arc;

use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};
use rawler::imgop::sensor::Demosaic;
use rawler::imgop::sensor::xtrans::markesteijn::XTransMarkesteijnDemosaic;
use rawler::imgop::{Point, Rect};
use rawler::rawimage::RawPhotometricInterpretation;
use rayon::prelude::*;
use thiserror::Error;

use crate::decode::{CfaKind, RawImage};
use crate::defringe::Defringe;
use crate::lens::{LensWarp, RadialGain};
use crate::matrices;
use crate::white_balance::{WhiteBalance, WhiteBalanceSolver};
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

    /// A custom white balance is out of range, or this file's color matrix
    /// cannot map one to camera multipliers.
    #[error("custom white balance is not available for this file")]
    CustomWhiteBalance,

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
    /// demosaic steps (Bayer via PPG, X-Trans via Markesteijn, linear DNG
    /// unchanged). Mosaic files then get chromatic aberration correction,
    /// unless [`Self::set_chromatic_aberration_correction`] turned it off:
    /// red and blue are resampled onto green from the camera's CA table when
    /// the file has one, and from an image estimate otherwise
    /// ([`crate::lateral_ca::LateralCa`]), as the camera does for its own
    /// JPEGs, and after the
    /// ACEScg conversion the blue/purple glow beside bright edges is
    /// desaturated ([`Defringe`]). Rescale does not clip above the white
    /// level. Highlight recovery white-balances with green at 1 and rebuilds
    /// a channel that sat on that level from the channels that still hold
    /// signal, so exposure and the film model keep the highlight, and a
    /// clipped neutral stays neutral instead of turning magenta. The D65 camera
    /// matrix is row-scaled so D65 maps to camera white, inverted, then a
    /// Bradford adaptation takes D65 to the ACES white and AP1 takes that XYZ
    /// into ACEScg. White balance is already in the samples, so the matrix
    /// sees an identity balance.
    ///
    /// Markesteijn and the CA correction run on the capped [`Workers`] pool.
    pub fn develop_linear(&self) -> Result<LinearImage, DevelopError> {
        Workers::install(|| self.develop_linear_here())
    }

    /// [`Self::develop_linear`] on whatever pool the caller is on.
    fn develop_linear_here(&self) -> Result<LinearImage, DevelopError> {
        self.develop_camera_here()?.convert(WhiteBalance::AsShot)
    }

    /// Demosaic, correct chromatic aberration and lens distortion, but stop
    /// in camera RGB, before white balance. [`CameraImage::develop`]
    /// finishes it with any white balance, as often as needed, without
    /// demosaicing again.
    /// `develop_camera()?.develop(WhiteBalance::AsShot)` is
    /// [`Self::develop_linear`].
    pub fn develop_camera(&self) -> Result<CameraImage, DevelopError> {
        Workers::install(|| self.develop_camera_here())
    }

    /// [`Self::develop_camera`] on a pool of its own, `threads` wide.
    pub fn develop_camera_on(&self, threads: usize) -> Result<CameraImage, DevelopError> {
        Workers::install_separate(threads, || self.develop_camera_here())
    }

    fn develop_camera_here(&self) -> Result<CameraImage, DevelopError> {
        let developed = if self.cfa() == CfaKind::XTrans {
            self.demosaic_xtrans()?
        } else {
            RawDevelop::new_with(&[ProcessingStep::Rescale, ProcessingStep::Demosaic])
                .develop_intermediate(self.decoded())
                .map_err(|source| DevelopError::Demosaic { source })?
        };

        let (width, height, mut samples) = match developed {
            Intermediate::ThreeColor(pixels) => take_rgb(pixels)?,
            Intermediate::FourColor(_) => {
                return Err(DevelopError::UnexpectedChannels { channels: 4 });
            }
            Intermediate::Monochrome(_) => {
                return Err(DevelopError::UnexpectedChannels { channels: 1 });
            }
        };

        // ProRAW arrives already corrected by the phone.
        let correct = self.corrects_chromatic_aberration() && self.cfa() != CfaKind::LinearDng;
        if correct {
            let (columns, rows) = (width as usize, height as usize);
            self.lens_profile()
                .correct_lateral_ca(columns, rows, &mut samples);
        }
        let (samples, vignetting) = self.correct_lens(width as usize, height as usize, samples);
        CameraImage::from_raw(self, width, height, samples, correct, vignetting)
    }

    /// Undistort `samples` as [`Self::lens_corrections`] asks, and the
    /// vignetting gain [`CameraImage`] applies after highlight recovery.
    fn correct_lens(
        &self,
        width: usize,
        height: usize,
        samples: Vec<f32>,
    ) -> (Vec<f32>, Option<Arc<RadialGain>>) {
        let corrections = self.lens_corrections();
        let profile = self.lens_profile();
        let half_width = (width as f64 - 1.0) / 2.0;
        let half_height = (height as f64 - 1.0) / 2.0;
        let warp = profile
            .distortion()
            .filter(|_| corrections.distortion)
            .and_then(|table| LensWarp::new(table, half_width, half_height));
        let vignetting = profile
            .vignetting()
            .filter(|_| corrections.vignetting)
            .map(|table| Arc::new(RadialGain::new(table, warp.as_ref())));
        let samples = match &warp {
            Some(warp) => warp.apply(width, height, &samples),
            None => samples,
        };
        (samples, vignetting)
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
    /// Chromatic aberration is corrected as in [`Self::develop_linear`].
    /// The camera table is a fraction of radius, so it agrees at this size;
    /// the image estimate is measured on this frame. `None` for a sensor
    /// that is not X-Trans.
    pub fn develop_quick_linear(&self) -> Result<Option<QuickDevelop>, DevelopError> {
        let Some(quick) = self.develop_quick_camera()? else {
            return Ok(None);
        };
        Ok(Some(QuickDevelop {
            image: quick.image.develop(WhiteBalance::AsShot)?,
            full_width: quick.full_width,
            full_height: quick.full_height,
        }))
    }

    /// [`Self::develop_quick_linear`], stopped in camera RGB before white
    /// balance, like [`Self::develop_camera`].
    pub fn develop_quick_camera(&self) -> Result<Option<QuickDevelop<CameraImage>>, DevelopError> {
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
        let correct = self.corrects_chromatic_aberration();
        if correct {
            let profile = self.lens_profile();
            Workers::install(|| profile.correct_lateral_ca(width, height, &mut samples));
        }
        let (samples, vignetting) = Workers::install(|| self.correct_lens(width, height, samples));
        let image = CameraImage::from_raw(
            self,
            u32::try_from(width).map_err(|_| DevelopError::Dimension { value: width })?,
            u32::try_from(height).map_err(|_| DevelopError::Dimension { value: height })?,
            samples,
            correct,
            vignetting,
        )?;
        Ok(Some(QuickDevelop {
            image,
            full_width: u32::try_from(full_width)
                .map_err(|_| DevelopError::Dimension { value: full_width })?,
            full_height: u32::try_from(full_height)
                .map_err(|_| DevelopError::Dimension { value: full_height })?,
        }))
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
pub struct QuickDevelop<Image = LinearImage> {
    /// Scene-linear ACEScg ([`LinearImage`]) or camera RGB ([`CameraImage`])
    /// at a third of the full size.
    pub image: Image,
    /// Width [`RawImage::develop_linear`] will return.
    pub full_width: u32,
    /// Height [`RawImage::develop_linear`] will return.
    pub full_height: u32,
}

/// Demosaiced camera RGB before white balance: black and white levels
/// normalized, chromatic aberration registered, lens distortion undone,
/// nothing else. Vignetting is applied by [`Self::develop`].
///
/// [`Self::develop`] finishes it to ACEScg for a [`WhiteBalance`]. Keeping
/// this buffer is what lets a white balance change skip the decode and the
/// demosaic.
#[derive(Clone)]
pub struct CameraImage {
    width: u32,
    height: u32,
    pixels: Vec<f32>,
    camera_matrix: [[f32; 3]; 4],
    as_shot_wb: [f32; 4],
    /// Desaturate blue/purple fringes after the ACEScg conversion. Only a
    /// full-size, CA-corrected develop sets it: the defringe radius is in
    /// sensor pixels.
    defringe: bool,
    /// Lens vignetting gain by normalized radius, applied after highlight
    /// recovery. It is resolution independent, so a downsample keeps it.
    vignetting: Option<Arc<RadialGain>>,
}

impl std::fmt::Debug for CameraImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &self.pixels.len())
            .field("defringe", &self.defringe)
            .field("vignetting", &self.vignetting.is_some())
            .finish()
    }
}

impl CameraImage {
    /// Camera RGB from interleaved samples, with the file's XYZ → camera
    /// matrix and as-shot multipliers (RGBE order), as [`RawImage`] reports
    /// them. No defringe runs on it.
    pub fn new(
        width: u32,
        height: u32,
        pixels: Vec<f32>,
        camera_matrix: [[f32; 3]; 4],
        as_shot_wb: [f32; 4],
    ) -> Result<Self, DevelopError> {
        let expected = width as usize * height as usize * 3;
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
            camera_matrix,
            as_shot_wb,
            defringe: false,
            vignetting: None,
        })
    }

    fn from_raw(
        raw: &RawImage,
        width: u32,
        height: u32,
        pixels: Vec<f32>,
        defringe: bool,
        vignetting: Option<Arc<RadialGain>>,
    ) -> Result<Self, DevelopError> {
        let mut image = Self::new(width, height, pixels, raw.camera_matrix(), raw.as_shot_wb())?;
        image.defringe = defringe;
        image.vignetting = vignetting;
        Ok(image)
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Maps temperature and tint to this camera's multipliers. `None` when
    /// the file cannot take a custom white balance: its color matrix does
    /// not invert, or it recorded no as-shot balance.
    pub fn white_balance_solver(&self) -> Option<WhiteBalanceSolver> {
        WhiteBalanceSolver::new(self.camera_matrix, self.as_shot_wb)
    }

    /// White balance, highlight recovery, the ACEScg matrix, and the
    /// defringe, on the capped [`Workers`] pool. See
    /// [`RawImage::develop_linear`].
    ///
    /// [`WhiteBalance::AsShot`] is what [`RawImage::develop_linear`] returns.
    pub fn develop(&self, balance: WhiteBalance) -> Result<LinearImage, DevelopError> {
        Workers::install(|| self.clone().convert(balance))
    }

    /// [`Self::develop`] on a pool of its own, `threads` wide, for a
    /// conversion in the background. See [`RawImage::develop_linear_on`].
    pub fn develop_on(
        &self,
        balance: WhiteBalance,
        threads: usize,
    ) -> Result<LinearImage, DevelopError> {
        Workers::install_separate(threads, || self.clone().convert(balance))
    }

    /// Area-downsample so the longer side is `long_edge`, as
    /// [`LinearImage::area_downsample`] does. The defringe is dropped at a
    /// reduced size, as it is for a quick develop.
    pub fn area_downsample(&self, long_edge: u32) -> Result<Self, DevelopError> {
        if long_edge == 0 {
            return Err(DevelopError::EmptyProxy);
        }
        let (width, height) = LinearImage::proxy_dimensions(self.width, self.height, long_edge);
        if width == self.width && height == self.height {
            return Ok(self.clone());
        }
        Ok(Self {
            width,
            height,
            pixels: area_box(&self.pixels, self.width, self.height, width, height),
            camera_matrix: self.camera_matrix,
            as_shot_wb: self.as_shot_wb,
            defringe: false,
            vignetting: self.vignetting.clone(),
        })
    }

    /// [`Self::develop`] on whatever pool the caller is on, consuming the
    /// buffer.
    fn convert(mut self, balance: WhiteBalance) -> Result<LinearImage, DevelopError> {
        let white_balance = match balance {
            WhiteBalance::AsShot => normalized_white_balance(self.as_shot_wb)?,
            WhiteBalance::Custom(_) => self
                .white_balance_solver()
                .and_then(|solver| solver.multipliers(balance))
                .ok_or(DevelopError::CustomWhiteBalance)?,
        };
        let transform = acescg_transform(self.camera_matrix, [1.0, 1.0, 1.0, 0.0])?;
        let wb = white_balance.map(|channel| channel as f32);
        const CHUNK: usize = 4096;
        let (width, height) = (self.width as usize, self.height as usize);
        let vignetting = self.vignetting.as_deref();
        self.pixels
            .par_chunks_mut(3 * CHUNK)
            .enumerate()
            .for_each(|(index, chunk)| {
                HighlightRecovery::apply(chunk, wb);
                if let Some(gain) = vignetting {
                    gain.apply(width, height, index * CHUNK, chunk);
                }
                apply_transform(chunk, transform);
            });
        let mut linear = LinearImage::new(self.width, self.height, self.pixels)?;
        if self.defringe {
            Defringe::apply(
                linear.width as usize,
                linear.height as usize,
                &mut linear.pixels,
            );
        }
        Ok(linear)
    }
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

/// White balance and highlight recovery in camera RGB, before the ACEScg matrix.
///
/// `rawler`'s rescale maps the white level to 1 and does not clip above it.
/// A photosite below the rail is a measurement: it is multiplied by the
/// green-normalized balance and kept, even when that product sits above
/// diffuse white, so a later exposure can still pull the highlight back.
///
/// A photosite on the rail (the white level, or the fraction of a code rawler
/// leaves above it) has no further scene data. Where the pixel still has a
/// channel well below white, that channel is the color: a clipped channel
/// that fell short of it is raised to it, and a clipped channel that is
/// already brighter stays, so a blue sky stays blue. Where nothing is well
/// below white, the ratios are the clip. Balancing them paints magenta under
/// a Fuji-style balance, whose red or blue coefficient is above 1, so the
/// pixel is rebuilt as a neutral at the level the surviving photosites
/// measured, or at [`Self::peak`] when every channel is on the rail. `peak`
/// is where the last channel of a neutral hits the rail, which meets the
/// partially recovered pixels around it instead of dropping to diffuse white.
///
/// A neutral recorded entirely above the white level is not on the rail. Its
/// channels already agree, and that exposure is kept.
struct HighlightRecovery;

impl HighlightRecovery {
    /// Sensor value `rawler` calls white. The rail is a narrow band around
    /// it: just under white, and the fraction of a code rescale leaves above.
    const SENSOR_WHITE: f32 = 1.0;
    /// Darkest photosite at or above this, with any channel on the rail, has
    /// no trustworthy chroma. A saturated color still has a channel below it.
    const NEAR_WHITE: f32 = 0.75;

    /// Recover interleaved camera RGB in place. `wb` is green-normalized.
    fn apply(samples: &mut [f32], wb: [f32; 3]) {
        let peak = Self::peak(wb);
        for pixel in samples.as_chunks_mut::<3>().0 {
            let recovered = Self::recover([pixel[0], pixel[1], pixel[2]], wb, peak);
            pixel.copy_from_slice(&recovered);
        }
    }

    /// Scene level at which the last channel of a neutral hits the rail.
    fn peak(wb: [f32; 3]) -> f32 {
        wb[0].max(wb[1]).max(wb[2])
    }

    fn on_rail(value: f32) -> bool {
        let white = Self::SENSOR_WHITE;
        ((white - 0.02)..=(white + 0.02)).contains(&value)
    }

    fn recover(sensor: [f32; 3], wb: [f32; 3], peak: f32) -> [f32; 3] {
        let mut sample = [0.0_f32; 3];
        let mut balanced = [0.0_f32; 3];
        let mut rail = [false; 3];
        let mut any_rail = false;
        for channel in 0..3 {
            let raw = sensor[channel];
            let value = if raw.is_finite() { raw.max(0.0) } else { 0.0 };
            sample[channel] = value;
            balanced[channel] = value * wb[channel];
            rail[channel] = Self::on_rail(value);
            any_rail |= rail[channel];
        }

        let darkest = sample[0].min(sample[1]).min(sample[2]);
        if any_rail && darkest >= Self::NEAR_WHITE {
            let mut level = 0.0_f32;
            let mut trusted = false;
            for channel in 0..3 {
                if !rail[channel] {
                    level = level.max(balanced[channel]);
                    trusted = true;
                }
            }
            return if trusted {
                [level, level, level]
            } else {
                [peak, peak, peak]
            };
        }
        if !any_rail {
            return balanced;
        }

        let mut level = 0.0_f32;
        for channel in 0..3 {
            if !rail[channel] {
                level = level.max(balanced[channel]);
            }
        }
        for channel in 0..3 {
            if rail[channel] && balanced[channel] < level {
                balanced[channel] = level;
            }
        }
        balanced
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

    fn fuji_balance() -> ([f32; 3], [[f32; 3]; 3]) {
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
        (balance.map(|channel| channel as f32), matrix)
    }

    #[test]
    fn clipped_highlight_stays_neutral_under_fuji_balance() {
        let (wb, matrix) = fuji_balance();
        let peak = HighlightRecovery::peak(wb);

        // On the rail, and the fraction of a code rawler leaves above white.
        // Both are a neutral at the last channel's clip, not magenta, and
        // the display encode is still white.
        for sensor in [[1.0_f32, 1.0, 1.0], [1.004, 1.0, 1.008]] {
            let mut white = sensor.to_vec();
            HighlightRecovery::apply(&mut white, wb);
            for channel in &white {
                assert!((channel - peak).abs() < 1.0e-5, "{sensor:?} -> {white:?}");
            }
            apply_transform(&mut white, matrix);
            for channel in &white {
                assert!(
                    (channel - peak).abs() < 2.0e-3,
                    "{sensor:?} left neutrality: {white:?}"
                );
            }
            let encoded = encode_srgb(&white);
            for channel in &encoded {
                assert!((channel - 1.0).abs() < 1.0e-4, "{encoded:?}");
            }
        }

        // A red that only just clips stays red. Baking the balance into the
        // matrix and clipping after it paints this pixel magenta.
        let mut red = vec![1.0, 0.25, 0.20];
        HighlightRecovery::apply(&mut red, wb);
        assert!(
            (red[0] - wb[0]).abs() < 1.0e-5,
            "clipped red was crushed: {red:?}"
        );
        apply_transform(&mut red, matrix);
        assert!(
            red[0] > red[1] + 0.15 && red[0] > red[2] + 0.05,
            "clipped red went magenta: {red:?}"
        );

        // A gray below the rail is only white-balanced, not crushed.
        let level = 0.4_f32;
        let mut gray = vec![level / wb[0], level, level / wb[2]];
        HighlightRecovery::apply(&mut gray, wb);
        for channel in &gray {
            assert!((channel - level).abs() < 1.0e-5, "{gray:?}");
        }
    }

    #[test]
    fn partial_clip_neutral_keeps_the_measured_level() {
        let (wb, matrix) = fuji_balance();
        // One stop over the first clip. Green is on the rail; red and blue
        // still measure the neutral, and green is rebuilt up to them.
        let level = 1.5_f32;
        let mut neutral = vec![level / wb[0], level, level / wb[2]];
        assert!(neutral[1] > 1.0 && neutral[0] < 1.0 && neutral[2] < 1.0);
        HighlightRecovery::apply(&mut neutral, wb);
        for channel in &neutral {
            assert!((channel - level).abs() < 1.0e-5, "{neutral:?}");
        }
        apply_transform(&mut neutral, matrix);
        for channel in &neutral {
            assert!(
                (channel - level).abs() < 2.0e-3,
                "recovered neutral was crushed: {neutral:?}"
            );
        }

        // Hotter: red has joined green on the rail, blue still measures it.
        let level = 2.0_f32;
        let mut hotter = vec![1.0, 1.0, level / wb[2]];
        HighlightRecovery::apply(&mut hotter, wb);
        for channel in &hotter {
            assert!((channel - level).abs() < 1.0e-4, "{hotter:?}");
        }
    }

    #[test]
    fn partial_clip_keeps_a_saturated_color() {
        let (wb, _matrix) = fuji_balance();
        // Blue sky: blue is on the rail, red and green are not. Blue stays
        // at the recorded balance instead of being cut to the smallest
        // multiplier, and the other two are untouched.
        let mut sky = vec![0.30_f32, 0.45, 1.0];
        HighlightRecovery::apply(&mut sky, wb);
        assert!((sky[0] - 0.30 * wb[0]).abs() < 1.0e-5, "{sky:?}");
        assert!((sky[1] - 0.45).abs() < 1.0e-5, "{sky:?}");
        assert!(
            (sky[2] - wb[2]).abs() < 1.0e-5 && sky[2] > sky[0] && sky[2] > sky[1],
            "clipped blue was flattened: {sky:?}"
        );

        // An unclipped red whose balance exceeds diffuse white is kept.
        // The old cap cut that channel to `min(wb)`.
        let mut warm = vec![0.80_f32, 0.20, 0.15];
        HighlightRecovery::apply(&mut warm, wb);
        assert!((warm[0] - 0.80 * wb[0]).abs() < 1.0e-5, "{warm:?}");
        assert!(
            warm[0] > 1.0,
            "headroom above white was discarded: {warm:?}"
        );
    }

    #[test]
    fn neutral_headroom_above_the_rail_is_kept() {
        let (wb, _matrix) = fuji_balance();
        let peak = HighlightRecovery::peak(wb);
        let level = peak + 0.75;
        let mut sensor = vec![level / wb[0], level / wb[1], level / wb[2]];
        assert!(sensor.iter().all(|channel| *channel > 1.0));
        HighlightRecovery::apply(&mut sensor, wb);
        for channel in &sensor {
            assert!((channel - level).abs() < 1.0e-4, "{sensor:?}");
        }
    }

    #[test]
    fn recovered_neutral_ramps_without_a_hole() {
        let (wb, _matrix) = fuji_balance();
        let peak = HighlightRecovery::peak(wb);
        let mut previous = 0.0_f32;
        for step in 0..48 {
            let level = 0.4 + step as f32 * 0.1;
            let mut sensor: Vec<f32> = wb
                .into_iter()
                .map(|channel| (level / channel).min(HighlightRecovery::SENSOR_WHITE))
                .collect();
            HighlightRecovery::apply(&mut sensor, wb);
            let recovered = sensor[0];
            for channel in &sensor {
                assert!((channel - recovered).abs() < 1.0e-4, "{sensor:?}");
            }
            assert!(
                recovered + 1.0e-4 >= previous,
                "highlight ramp fell from {previous} to {recovered} at {level}"
            );
            assert!(recovered <= peak + 1.0e-4, "{recovered} overshot {peak}");
            previous = recovered;
        }
        assert!((previous - peak).abs() < 1.0e-4, "{previous}");
    }

    #[test]
    fn non_finite_sensor_codes_become_black() {
        let (wb, _matrix) = fuji_balance();
        let mut pixel = vec![f32::NAN, 0.25, f32::INFINITY];
        HighlightRecovery::apply(&mut pixel, wb);
        assert_eq!(pixel[0], 0.0);
        assert!((pixel[1] - 0.25).abs() < 1.0e-6, "{pixel:?}");
        assert_eq!(pixel[2], 0.0);
    }

    #[test]
    fn near_blown_specular_does_not_keep_the_rail_gain() {
        // Measured on a clipped X-T30 specular. Red is a hair over white,
        // green just under it, blue still recording. Keeping red times its
        // balance (above 1) leaves green short and the highlight goes magenta.
        let wb = [1.8841059_f32, 1.0, 1.7417219];
        let blue = 0.77526486_f32;
        let mut pixel = vec![1.0081387, 0.9974481, blue];
        HighlightRecovery::apply(&mut pixel, wb);
        let level = blue * wb[2];
        for channel in &pixel {
            assert!(
                (channel - level).abs() < 1.0e-4,
                "near-blown specular went magenta: {pixel:?}"
            );
        }
    }

    #[test]
    fn near_white_codes_do_not_balance_to_magenta() {
        let (wb, matrix) = fuji_balance();
        let peak = HighlightRecovery::peak(wb);
        // Every channel is on the rail, a hair under white. Balancing the raw
        // codes would leave green short. Recovery treats the pixel as blown.
        let mut pixel = vec![0.995_f32, 0.996, 0.997];
        HighlightRecovery::apply(&mut pixel, wb);
        for channel in &pixel {
            assert!((channel - peak).abs() < 1.0e-5, "{pixel:?}");
        }
        apply_transform(&mut pixel, matrix);
        let green_short = pixel[1] + 0.05 < pixel[0] && pixel[1] + 0.05 < pixel[2];
        assert!(!green_short, "near-white highlight went magenta: {pixel:?}");
    }

    #[test]
    fn develop_keeps_a_partial_clip_above_acescg_white() {
        let level = 1.5_f32;
        let as_shot = [1.6953642_f32, 1.0, 2.3509934, f32::NAN];
        let (wb, _matrix) = fuji_balance();
        let image = CameraImage::new(
            1,
            1,
            vec![level / wb[0], level, level / wb[2]],
            [
                [1.3426, -0.6334, -0.1177],
                [-0.4244, 1.2136, 0.2371],
                [-0.058, 0.1303, 0.598],
                [0.0, 0.0, 0.0],
            ],
            as_shot,
        )
        .unwrap();
        let linear = image.develop(WhiteBalance::AsShot).unwrap();
        let pixel = linear.pixels();
        for channel in pixel {
            assert!(
                (channel - level).abs() < 2.0e-3,
                "develop crushed the recovered highlight: {pixel:?}"
            );
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
