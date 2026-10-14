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
//! 7. export: orientation, an sRGB ICC profile, 8-bit JPEG or 16-bit TIFF
//!
//! A GPU path is not part of this pipeline. The grain hash is integer-only so
//! a later WGSL port can match it bit for bit.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::tiff::TiffEncoder;
use image::{ExtendedColorType, ImageEncoder};
use thiserror::Error;

use crate::color::{self, LinearImage};
use crate::decode::Orientation;
use crate::icc;
use crate::stock::{Curve, FilmModel, Grain, Halation, ModelError};

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
}

impl Pipeline {
    /// Pipeline for `parameters`.
    pub fn new(parameters: Parameters) -> Self {
        Self { parameters }
    }

    /// Run exposure, halation, the film model, grain, print/scan, and the
    /// sRGB output transform.
    ///
    /// Halation and grain run only when the parameters carry them. The result
    /// stays in sensor orientation. [`SrgbImage::write`] applies the camera
    /// orientation while exporting.
    pub fn develop(&self, image: &LinearImage) -> SrgbImage {
        let mut exposure = image.pixels().to_vec();
        apply_exposure(&mut exposure, self.parameters.exposure_ev);
        if let Some(halation) = &self.parameters.halation {
            halation.apply(&mut exposure, image.width(), image.height());
        }
        let mut density = self.parameters.film.density(&exposure);
        if let Some(grain) = &self.parameters.grain {
            grain.apply(&mut density, image.width(), image.height());
        }
        let scanned = self.parameters.film.printing_density(&density);
        let linear = print_scan(
            self.parameters.print_exposure,
            &self.parameters.print_curve,
            scanned.as_deref().unwrap_or(&density),
        );
        SrgbImage {
            width: image.width(),
            height: image.height(),
            pixels: color::encode_srgb(&linear),
        }
    }
}

impl std::fmt::Debug for SrgbImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrgbImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &self.pixels.len())
            .finish()
    }
}

impl SrgbImage {
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

    /// Whether `path` is a JPEG or TIFF this exporter can write.
    pub fn is_supported_output(path: &Path) -> bool {
        OutputFormat::from_path(path).is_some()
    }

    /// Write JPEG (8-bit) or TIFF (16-bit), oriented, with an sRGB ICC profile.
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
                let mut encoder = JpegEncoder::new_with_quality(&mut writer, 90);
                encoder.set_icc_profile(profile).map_err(ExportError::Icc)?;
                encoder
                    .write_image(&samples, width, height, ExtendedColorType::Rgb8)
                    .map_err(|source| ExportError::Encode {
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
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unsupported_extension_is_an_error() {
        let picture = SrgbImage {
            width: 1,
            height: 1,
            pixels: vec![0.0, 0.0, 0.0],
        };
        let err = picture
            .write("/tmp/rawstock-out.png", Orientation::Normal)
            .unwrap_err();
        assert!(err.to_string().contains(".jpg"));
    }
}
