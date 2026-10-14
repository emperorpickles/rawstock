//! CPU film pipeline and export.
//!
//! Stages, in order:
//!
//! 1. exposure, in scene-linear ACEScg
//! 2. [`Halation`](crate::stock::Halation), when the stock sets it: a threshold,
//!    a wide blur, and a red-weighted add-back, still in scene-linear exposure
//! 3. [`FilmModel`](crate::stock::FilmModel): exposure to per-layer density
//! 4. [`Grain`](crate::stock::Grain), when the stock sets it: per-layer dye-cloud
//!    noise in density
//! 5. print/scan: density back to display-referred linear RGB. A spectral
//!    stock with dye curves first turns those curves into the density this
//!    step sees; every other stock passes the characteristic density through.
//! 6. output transform: ACEScg to encoded sRGB, clipping out-of-gamut channels
//! 7. export: baked orientation (JPEG tag 1), an sRGB ICC profile, 8-bit JPEG or 16-bit TIFF
//!
//! A GPU path is not part of this pipeline. The grain hash is integer-only so
//! a later WGSL port can match it bit for bit.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::tiff::TiffEncoder;
use image::{ExtendedColorType, ImageEncoder};
use thiserror::Error;

use crate::color::{self, LinearImage};
use crate::decode::Orientation;
use crate::icc;
use crate::stock::{Curve, FilmModel, Grain, Halation, ModelError};

/// One stage of [`Pipeline::develop`], in pipeline order.
///
/// A parameter edit names the first stage that has to run again. Decode is
/// not a stage: the session already holds the ACEScg image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Scale scene-linear ACEScg by `exposure_ev`.
    Exposure,
    /// Highlight halation, when the stock has it.
    Halation,
    /// Characteristic density from the film model.
    Density,
    /// Density grain, when the stock has it.
    Grain,
    /// Dye scan. A no-op when the stock has no dye curves.
    DyeScan,
    /// Print exposure and the print curve.
    Print,
    /// ACEScg to encoded sRGB.
    Encode,
}

impl Stage {
    /// Short name used in progress events.
    pub fn name(self) -> &'static str {
        match self {
            Self::Exposure => "Exposure",
            Self::Halation => "Halation",
            Self::Density => "Density",
            Self::Grain => "Grain",
            Self::DyeScan => "Dye scan",
            Self::Print => "Print",
            Self::Encode => "Encode",
        }
    }
}

/// Cancel and progress hook for a develop.
///
/// The desktop shell backs [`Self::cancelled`] with an `AtomicBool`. A wasm
/// host can back it with a `Cell<bool>`. The CLI uses [`NopControl`].
pub trait DevelopControl {
    /// `true` when this develop should stop. A cancelled develop returns
    /// [`Cancelled`] and no image.
    fn cancelled(&self) -> bool;

    /// `done_rows` of `rows` have finished for `stage`. `done_rows` is 0 at
    /// the start of the stage.
    fn on_progress(&mut self, stage: Stage, done_rows: u32, rows: u32);
}

/// Control that never cancels and ignores progress.
#[derive(Debug, Default)]
pub struct NopControl;

impl DevelopControl for NopControl {
    fn cancelled(&self) -> bool {
        false
    }

    fn on_progress(&mut self, _stage: Stage, _done_rows: u32, _rows: u32) {}
}

/// A develop stopped because [`DevelopControl::cancelled`] was set.
///
/// The partial buffer is dropped. Callers keep the previous image.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("development was cancelled")]
pub struct Cancelled;

/// Whether the dye scan was the lookup table or the exact integral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// Dye-scan lookup table. Fit view, and a 100% crop while a control is moving.
    Preview,
    /// Exact dye scan. Settled 100%, full export, and stocks with no dye curves.
    Exact,
}

/// Parameters for one development.
///
/// A plain struct. [`Stock::load`](crate::stock::Stock::load) fills one from a
/// stock file. Tests build their own values rather than a library default.
#[derive(Debug, Clone)]
pub struct Parameters {
    exposure_ev: f32,
    halation: Option<Halation>,
    film: FilmModel,
    grain: Option<Grain>,
    print_exposure: f32,
    print_curve: Curve,
}

/// The shared film pipeline.
#[derive(Debug, Clone)]
pub struct Pipeline {
    parameters: Parameters,
}

/// Display-encoded sRGB image, interleaved RGB in `0..=1`, sensor orientation.
#[derive(Clone)]
pub struct SrgbImage {
    width: u32,
    height: u32,
    pixels: Vec<f32>,
    fidelity: Fidelity,
}

/// Failure while writing a developed image.
#[derive(Debug, Error)]
pub enum ExportError {
    /// The path is not `.jpg`, `.jpeg`, `.tif`, or `.tiff`.
    #[error("output {path} must be a .jpg, .jpeg, .tif, or .tiff file")]
    UnsupportedFormat {
        /// The rejected path.
        path: String,
    },

    /// The output file could not be created.
    #[error("failed to create {path}")]
    Create {
        /// The output path.
        path: String,
        /// The filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// The encoded bytes could not be written.
    #[error("failed to write {path}")]
    Write {
        /// The output path.
        path: String,
        /// The filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// The encoder refused the ICC profile.
    #[error("failed to embed an sRGB ICC profile")]
    Icc(#[source] image::error::UnsupportedError),

    /// JPEG or TIFF encoding failed.
    #[error("failed to encode {path}")]
    Encode {
        /// The output path.
        path: String,
        /// The encoder error.
        #[source]
        source: image::ImageError,
    },
}

enum OutputFormat {
    Jpeg,
    Tiff16,
}

impl Parameters {
    /// `exposure_ev` is in stops, applied before the film model.
    ///
    /// `print_exposure` is the log-exposure offset of the print stage:
    /// print log-exposure is `print_exposure - density`.
    pub fn new(
        exposure_ev: f32,
        film: FilmModel,
        print_exposure: f32,
        print_curve: Curve,
    ) -> Result<Self, ModelError> {
        if !exposure_ev.is_finite() || !print_exposure.is_finite() {
            return Err(ModelError::NonFiniteExposure);
        }
        Ok(Self {
            exposure_ev,
            halation: None,
            film,
            grain: None,
            print_exposure,
            print_curve,
        })
    }

    /// Attach highlight halation. It runs after exposure and before the film model.
    pub fn with_halation(mut self, halation: Halation) -> Self {
        self.halation = Some(halation);
        self
    }

    /// Attach density grain. It runs after the film model and before print.
    pub fn with_grain(mut self, grain: Grain) -> Self {
        self.grain = Some(grain);
        self
    }

    /// Exposure adjustment in stops.
    pub fn exposure_ev(&self) -> f32 {
        self.exposure_ev
    }

    /// Highlight halation, when the stock sets it.
    pub fn halation(&self) -> Option<&Halation> {
        self.halation.as_ref()
    }

    /// The film model that turns exposure into density.
    pub fn film(&self) -> &FilmModel {
        &self.film
    }

    /// Density grain, when the stock sets it.
    pub fn grain(&self) -> Option<&Grain> {
        self.grain.as_ref()
    }

    /// Log-exposure offset of the print stage.
    pub fn print_exposure(&self) -> f32 {
        self.print_exposure
    }

    /// Print characteristic curve.
    pub fn print_curve(&self) -> &Curve {
        &self.print_curve
    }

    /// Set exposure in stops. Dirties [`Stage::Exposure`].
    pub fn set_exposure_ev(&mut self, exposure_ev: f32) -> Result<Stage, ModelError> {
        if !exposure_ev.is_finite() {
            return Err(ModelError::NonFiniteExposure);
        }
        self.exposure_ev = exposure_ev;
        Ok(Stage::Exposure)
    }

    /// Set the print log-exposure offset. Dirties [`Stage::Print`].
    pub fn set_print_exposure(&mut self, print_exposure: f32) -> Result<Stage, ModelError> {
        if !print_exposure.is_finite() {
            return Err(ModelError::NonFiniteExposure);
        }
        self.print_exposure = print_exposure;
        Ok(Stage::Print)
    }

    /// Replace highlight halation. `None` clears the stage. Dirties [`Stage::Halation`].
    pub fn set_halation(&mut self, halation: Option<Halation>) -> Result<Stage, ModelError> {
        self.halation = halation;
        Ok(Stage::Halation)
    }

    /// Replace density grain. `None` clears the stage. Dirties [`Stage::Grain`].
    pub fn set_grain(&mut self, grain: Option<Grain>) -> Result<Stage, ModelError> {
        self.grain = grain;
        Ok(Stage::Grain)
    }

    /// Replace the print curve. Dirties [`Stage::Print`].
    pub fn set_print_curve(&mut self, print_curve: Curve) -> Result<Stage, ModelError> {
        self.print_curve = print_curve;
        Ok(Stage::Print)
    }

    /// Replace the film model.
    ///
    /// Curves, coupling, or sensitivities dirty [`Stage::Density`]. A change
    /// that only touches dye curves or `reference_log_exposure` dirties
    /// [`Stage::DyeScan`]. An identical model reports [`Stage::Encode`] and
    /// still stores the value.
    pub fn set_film(&mut self, film: FilmModel) -> Result<Stage, ModelError> {
        let stage = self.film.edit_stage(&film);
        self.film = film;
        Ok(stage)
    }
}

impl Pipeline {
    /// Pipeline for `parameters`.
    pub fn new(parameters: Parameters) -> Self {
        Self { parameters }
    }

    /// Run exposure, halation, the film model, grain, print/scan, and the
    /// sRGB output transform.
    ///
    /// Halation and grain run only when the parameters carry them, in whatever
    /// orientation `image` already has. [`SrgbImage::write`] can still rotate
    /// for a caller that held the sensor frame, and the JPEG tag is always 1.
    /// This is the exact path: the dye scan is never the preview lookup table.
    pub fn develop(&self, image: &LinearImage) -> SrgbImage {
        self.develop_controlled(image, &mut NopControl)
            .expect("nop control does not cancel")
    }

    /// [`Self::develop`] with cancel and progress.
    ///
    /// The same stage functions the preview session calls. `Err(Cancelled)`
    /// means no image.
    pub fn develop_controlled(
        &self,
        image: &LinearImage,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        let exposure = exposure_buffer(image.pixels(), self.parameters.exposure_ev, control)?;
        let exposed = halation_buffer(
            exposure,
            self.parameters.halation.as_ref(),
            image.width(),
            image.height(),
            None,
            control,
        )?;
        let density = density_buffer(&exposed, &self.parameters.film, image.width(), control)?;
        let grained = grain_buffer(
            density,
            self.parameters.grain.as_ref(),
            crate::stock::GrainRegion {
                width: image.width(),
                height: image.height(),
                origin_x: 0,
                origin_y: 0,
                frame_width: image.width(),
            },
            control,
        )?;
        let (scanned, fidelity) =
            dye_buffer(&grained, &self.parameters.film, image.width(), control)?;
        let linear = print_buffer(
            &scanned,
            self.parameters.print_exposure,
            &self.parameters.print_curve,
            control,
        )?;
        encode_buffer(linear, image.width(), image.height(), fidelity, control)
    }
}

impl std::fmt::Debug for SrgbImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrgbImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &self.pixels.len())
            .field("fidelity", &self.fidelity)
            .finish()
    }
}

impl SrgbImage {
    pub(crate) fn from_encoded(
        width: u32,
        height: u32,
        pixels: Vec<f32>,
        fidelity: Fidelity,
    ) -> Self {
        Self {
            width,
            height,
            pixels,
            fidelity,
        }
    }

    /// Width in pixels, before orientation is applied.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels, before orientation is applied.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Encoded sRGB samples in `0..=1`, interleaved RGB.
    pub fn pixels(&self) -> &[f32] {
        &self.pixels
    }

    /// `preview` for a dye-scan lookup, `exact` for the integral the export uses.
    pub fn fidelity(&self) -> Fidelity {
        self.fidelity
    }

    /// 8-bit RGB with the same rounding as the JPEG writer.
    pub fn to_rgb8(&self) -> Vec<u8> {
        quantize_u8(&self.pixels)
    }

    /// Whether `path` is a JPEG or TIFF this exporter can write.
    pub fn is_supported_output(path: &Path) -> bool {
        OutputFormat::from_path(path).is_some()
    }

    /// Write JPEG (8-bit) or TIFF (16-bit), with an sRGB ICC profile.
    ///
    /// `orientation` is baked into the samples. The JPEG EXIF orientation tag
    /// is 1, because a second rotation would turn an upright file sideways.
    pub fn write(
        &self,
        path: impl AsRef<Path>,
        orientation: Orientation,
    ) -> Result<(), ExportError> {
        let path = path.as_ref();
        let format =
            OutputFormat::from_path(path).ok_or_else(|| ExportError::UnsupportedFormat {
                path: path.display().to_string(),
            })?;
        let file = File::create(path).map_err(|source| ExportError::Create {
            path: path.display().to_string(),
            source,
        })?;
        let mut writer = BufWriter::new(file);
        let profile = icc::srgb_profile();
        match format {
            OutputFormat::Jpeg => {
                let samples = quantize_u8(&self.pixels);
                let (width, height, samples) =
                    orientation.apply_rgb(self.width, self.height, &samples);
                let mut buffer = Vec::new();
                let mut encoder = JpegEncoder::new_with_quality(&mut buffer, 90);
                encoder.set_icc_profile(profile).map_err(ExportError::Icc)?;
                encoder
                    .write_image(&samples, width, height, ExtendedColorType::Rgb8)
                    .map_err(|source| ExportError::Encode {
                        path: path.display().to_string(),
                        source,
                    })?;
                // Pixels are already upright. The tag stays 1 so a viewer does
                // not rotate them a second time.
                let stamped = ExifOrientation::stamp(&buffer);
                writer
                    .write_all(&stamped)
                    .map_err(|source| ExportError::Write {
                        path: path.display().to_string(),
                        source,
                    })?;
            }
            OutputFormat::Tiff16 => {
                let samples = quantize_u16(&self.pixels);
                let (width, height, samples) =
                    orientation.apply_rgb(self.width, self.height, &samples);
                let bytes = u16_to_ne_bytes(&samples);
                let mut encoder = TiffEncoder::new(&mut writer);
                encoder.set_icc_profile(profile).map_err(ExportError::Icc)?;
                encoder
                    .write_image(&bytes, width, height, ExtendedColorType::Rgb16)
                    .map_err(|source| ExportError::Encode {
                        path: path.display().to_string(),
                        source,
                    })?;
            }
        }
        Ok(())
    }
}

fn apply_exposure(samples: &mut [f32], ev: f32) {
    let scale = 2.0f32.powf(ev);
    for sample in samples {
        *sample *= scale;
    }
}

pub(crate) fn exposure_buffer(
    pixels: &[f32],
    exposure_ev: f32,
    control: &mut dyn DevelopControl,
) -> Result<Vec<f32>, Cancelled> {
    begin(Stage::Exposure, 1, control)?;
    let mut exposure = pixels.to_vec();
    apply_exposure(&mut exposure, exposure_ev);
    finish(Stage::Exposure, 1, control)?;
    Ok(exposure)
}

pub(crate) fn halation_buffer(
    mut exposure: Vec<f32>,
    halation: Option<&Halation>,
    width: u32,
    height: u32,
    sigma: Option<f32>,
    control: &mut dyn DevelopControl,
) -> Result<Vec<f32>, Cancelled> {
    let Some(halation) = halation else {
        return Ok(exposure);
    };
    let sigma = sigma.unwrap_or_else(|| halation.sigma_px(width, height));
    halation.apply_controlled(&mut exposure, width, height, sigma, control)?;
    Ok(exposure)
}

pub(crate) fn density_buffer(
    exposure: &[f32],
    film: &FilmModel,
    width: u32,
    control: &mut dyn DevelopControl,
) -> Result<Vec<f32>, Cancelled> {
    film.density_controlled(exposure, width, control)
}

pub(crate) fn grain_buffer(
    mut density: Vec<f32>,
    grain: Option<&Grain>,
    region: crate::stock::GrainRegion,
    control: &mut dyn DevelopControl,
) -> Result<Vec<f32>, Cancelled> {
    let Some(grain) = grain else {
        return Ok(density);
    };
    grain.apply_region(&mut density, region, control)?;
    Ok(density)
}

pub(crate) fn dye_buffer(
    density: &[f32],
    film: &FilmModel,
    width: u32,
    control: &mut dyn DevelopControl,
) -> Result<(Vec<f32>, Fidelity), Cancelled> {
    match film.printing_density_controlled(density, width, control)? {
        Some(scanned) => Ok((scanned, Fidelity::Exact)),
        None => Ok((density.to_vec(), Fidelity::Exact)),
    }
}

pub(crate) fn print_buffer(
    density: &[f32],
    print_exposure: f32,
    curve: &Curve,
    control: &mut dyn DevelopControl,
) -> Result<Vec<f32>, Cancelled> {
    begin(Stage::Print, 1, control)?;
    let linear = print_scan(print_exposure, curve, density);
    finish(Stage::Print, 1, control)?;
    Ok(linear)
}

pub(crate) fn encode_buffer(
    linear: Vec<f32>,
    width: u32,
    height: u32,
    fidelity: Fidelity,
    control: &mut dyn DevelopControl,
) -> Result<SrgbImage, Cancelled> {
    begin(Stage::Encode, height, control)?;
    let pixels = color::encode_srgb(&linear);
    finish(Stage::Encode, height, control)?;
    Ok(SrgbImage::from_encoded(width, height, pixels, fidelity))
}

pub(crate) fn begin(
    stage: Stage,
    rows: u32,
    control: &mut dyn DevelopControl,
) -> Result<(), Cancelled> {
    if control.cancelled() {
        return Err(Cancelled);
    }
    control.on_progress(stage, 0, rows);
    if control.cancelled() {
        return Err(Cancelled);
    }
    Ok(())
}

fn finish(stage: Stage, rows: u32, control: &mut dyn DevelopControl) -> Result<(), Cancelled> {
    control.on_progress(stage, rows, rows);
    if control.cancelled() {
        return Err(Cancelled);
    }
    Ok(())
}

/// Invert the negative and scan it back to display-referred linear RGB.
///
/// Print log-exposure is `print_exposure - density`: a denser negative passes
/// less light. The print curve maps that to print density, and the scan is
/// the print's transmission, `10^(-print density)`. Primaries stay ACEScg
/// until the output transform.
fn print_scan(print_exposure: f32, curve: &Curve, density: &[f32]) -> Vec<f32> {
    density
        .iter()
        .map(|value| {
            let print_density = curve.eval(print_exposure - value);
            10f32.powf(-print_density)
        })
        .collect()
}

fn quantize_u8(samples: &[f32]) -> Vec<u8> {
    samples.iter().copied().map(to_u8).collect()
}

fn quantize_u16(samples: &[f32]) -> Vec<u16> {
    samples.iter().copied().map(to_u16).collect()
}

fn to_u8(sample: f32) -> u8 {
    let sample = finite_unit(sample);
    (sample * 255.0).round() as u8
}

fn to_u16(sample: f32) -> u16 {
    let sample = finite_unit(sample);
    (sample * 65535.0).round() as u16
}

fn finite_unit(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn u16_to_ne_bytes(samples: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend(sample.to_ne_bytes());
    }
    bytes
}

impl OutputFormat {
    fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
            Some(Self::Jpeg)
        } else if extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff") {
            Some(Self::Tiff16)
        } else {
            None
        }
    }
}

/// EXIF Orientation tag written as 1.
///
/// The samples are already rotated, so the tag must not ask for another turn.
struct ExifOrientation;

impl ExifOrientation {
    fn app1() -> &'static [u8] {
        &[
            0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0x00, 0x00, b'I', b'I', 0x2a, 0x00,
            0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]
    }

    fn stamp(jpeg: &[u8]) -> Vec<u8> {
        if jpeg.len() < 2 || jpeg[0] != 0xff || jpeg[1] != 0xd8 {
            return jpeg.to_vec();
        }
        let segment = Self::app1();
        let mut out = Vec::with_capacity(jpeg.len() + segment.len());
        out.extend_from_slice(&jpeg[..2]);
        out.extend_from_slice(segment);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[cfg(test)]
    fn read(jpeg: &[u8]) -> Option<u16> {
        let mut index = 0;
        while index + 4 < jpeg.len() {
            if jpeg[index] != 0xff {
                index += 1;
                continue;
            }
            let marker = jpeg[index + 1];
            if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
                index += 2;
                continue;
            }
            let length = u16::from_be_bytes([jpeg[index + 2], jpeg[index + 3]]) as usize;
            if length < 2 || index + 2 + length > jpeg.len() {
                return None;
            }
            if marker == 0xe1
                && let Some(value) = Self::orientation_in_exif(&jpeg[index + 4..index + 2 + length])
            {
                return Some(value);
            }
            index += 2 + length;
        }
        None
    }

    #[cfg(test)]
    fn orientation_in_exif(segment: &[u8]) -> Option<u16> {
        if segment.len() < 14 || &segment[..6] != b"Exif\0\0" {
            return None;
        }
        let tiff = &segment[6..];
        let little = match &tiff[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let u16_at = |offset: usize| -> Option<u16> {
            let bytes = tiff.get(offset..offset + 2)?;
            Some(if little {
                u16::from_le_bytes([bytes[0], bytes[1]])
            } else {
                u16::from_be_bytes([bytes[0], bytes[1]])
            })
        };
        let u32_at = |offset: usize| -> Option<u32> {
            let bytes = tiff.get(offset..offset + 4)?;
            Some(if little {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            } else {
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            })
        };
        if u16_at(2)? != 42 {
            return None;
        }
        let mut entry = u32_at(4)? as usize;
        let count = u16_at(entry)? as usize;
        entry += 2;
        for _ in 0..count {
            let tag = u16_at(entry)?;
            let kind = u16_at(entry + 2)?;
            if tag == 0x0112 && kind == 3 {
                return u16_at(entry + 8);
            }
            entry += 12;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stock::{Curve, FilmModel, Grain, Granularity, Halation, Parametric};
    use image::ImageDecoder;

    /// Neutral parameters used only by tests. The same curve on every layer
    /// and no coupling, so a gray stays gray. Not a stock, and not a default
    /// the library exposes.
    fn neutral() -> Parameters {
        let negative = Curve::new([(-4.0, 0.10), (-1.0, 0.55), (0.0, 1.20), (1.5, 2.10)]).unwrap();
        let print = Curve::new([(-1.5, 0.05), (0.0, 0.50), (1.0, 1.30), (2.5, 2.40)]).unwrap();
        let film = FilmModel::Parametric(
            Parametric::new([negative.clone(), negative.clone(), negative], None).unwrap(),
        );
        Parameters::new(0.5, film, 0.8, print).unwrap()
    }

    #[test]
    fn neutral_gray_stays_neutral_through_the_pipeline() {
        let levels = [0.0, -0.25, 1.0e-4, 0.02, 0.18, 0.5, 1.0, 2.5, 8.0];
        let mut pixels = Vec::new();
        for gray in levels {
            pixels.extend([gray, gray, gray]);
        }
        let image = LinearImage::new(levels.len() as u32, 1, pixels).unwrap();
        let developed = Pipeline::new(neutral()).develop(&image);
        for pixel in developed.pixels().as_chunks::<3>().0 {
            let spread =
                pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2]);
            assert!(spread < 1.0e-5, "neutral pixel drifted: {pixel:?}");
        }

        // 0.02 is the fourth sample, 2.5 is the eighth. A positive print is
        // brighter for the brighter exposure.
        let dark = developed.pixels()[3 * 3];
        let bright = developed.pixels()[7 * 3];
        assert!(
            bright > dark + 0.05,
            "print did not stay positive: dark {dark}, bright {bright}"
        );
    }

    #[test]
    fn halation_sees_exposure_and_grain_repeats() {
        let pixels = vec![0.6_f32; 8 * 8 * 3];
        let image = LinearImage::new(8, 8, pixels).unwrap();
        let halation = Halation::new(1.0, 0.0, [0.8, 0.1, 0.0]).unwrap();
        let grain = flat_grain(4.0);

        // 0.6 stays under the threshold, so halation is a no-op.
        let plain = Pipeline::new(neutral()).develop(&image);
        let with_halation =
            Pipeline::new(neutral().with_halation(halation.clone())).develop(&image);
        assert_eq!(plain.pixels(), with_halation.pixels());

        // +1 stop lifts 0.6 to 1.2, which is above the threshold.
        let exposed = Pipeline::new(neutral_at(1.0)).develop(&image);
        let exposed_halation =
            Pipeline::new(neutral_at(1.0).with_halation(halation)).develop(&image);
        assert_ne!(exposed.pixels(), exposed_halation.pixels());

        let once = Pipeline::new(neutral().with_grain(grain.clone())).develop(&image);
        let twice = Pipeline::new(neutral().with_grain(grain)).develop(&image);
        assert_eq!(once.pixels(), twice.pixels());
        assert_ne!(once.pixels(), plain.pixels());
    }

    fn neutral_at(exposure_ev: f32) -> Parameters {
        let mut parameters = neutral();
        parameters.exposure_ev = exposure_ev;
        parameters
    }

    fn flat_grain(rms: f32) -> Grain {
        let layer = Granularity::new([(0.0, rms), (3.0, rms)]).unwrap();
        Grain::new(Grain::FRAME_MM, 1, [layer.clone(), layer.clone(), layer]).unwrap()
    }

    #[test]
    fn tiff_is_16_bit_and_oriented_with_an_srgb_profile() {
        let picture = SrgbImage {
            width: 2,
            height: 1,
            pixels: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            fidelity: Fidelity::Exact,
        };
        let path = std::env::temp_dir().join("rawstock-orient.tiff");
        picture.write(&path, Orientation::Rotate90).unwrap();

        // `ImageReader` tightens the TIFF allocation limit to the pixel buffer,
        // which then refuses the ICC tag. Open the decoder directly.
        let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
        let mut decoder = image::codecs::tiff::TiffDecoder::new(file).unwrap();
        assert_eq!(decoder.dimensions(), (1, 2));
        assert_eq!(decoder.color_type(), image::ColorType::Rgb16);
        let profile = decoder.icc_profile().unwrap().expect("tiff icc");
        assert_eq!(&profile[36..40], b"acsp");
        let decoded = image::DynamicImage::from_decoder(decoder).unwrap();
        let rgb = decoded.into_rgb16();
        let pixels: Vec<_> = rgb.pixels().copied().collect();
        assert_eq!(pixels.len(), 2);
        assert_eq!(pixels[0].0, [65535, 0, 0]);
        assert_eq!(pixels[1].0, [0, 65535, 0]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn jpeg_carries_the_srgb_profile_and_orientation() {
        let picture = SrgbImage {
            width: 2,
            height: 1,
            pixels: vec![0.2, 0.2, 0.2, 0.8, 0.8, 0.8],
            fidelity: Fidelity::Exact,
        };
        let path = std::env::temp_dir().join("rawstock-orient.jpg");
        picture.write(&path, Orientation::Rotate90).unwrap();

        let reader = image::ImageReader::open(&path)
            .unwrap()
            .with_guessed_format()
            .unwrap();
        let mut decoder = reader.into_decoder().unwrap();
        assert_eq!(decoder.dimensions(), (1, 2));
        assert_eq!(decoder.color_type(), image::ColorType::Rgb8);
        let profile = decoder.icc_profile().unwrap().expect("jpeg icc");
        assert_eq!(&profile[36..40], b"acsp");
        assert_eq!(&profile[16..20], b"RGB ");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(ExifOrientation::read(&bytes), Some(1));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unsupported_extension_is_an_error() {
        let picture = SrgbImage {
            width: 1,
            height: 1,
            pixels: vec![0.0, 0.0, 0.0],
            fidelity: Fidelity::Exact,
        };
        let err = picture
            .write("/tmp/rawstock-out.png", Orientation::Normal)
            .unwrap_err();
        assert!(err.to_string().contains(".jpg"));
    }
}
