//! RAW decoding: a camera file becomes the samples and metadata needed to
//! develop it.
//!
//! The backend is [`rawler`]. [`RawImage::develop_linear`] demosaics those
//! samples and converts them to scene-linear ACEScg.

use std::fmt;
use std::path::Path;

use rawler::imgop::sensor::SensorType;
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawImageData;
use rawler::rawimage::RawPhotometricInterpretation;
use thiserror::Error;

/// How the sensor samples are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfaKind {
    /// Fujifilm 6×6 color filter array.
    XTrans,
    /// 2×2 color filter array.
    Bayer,
    /// Already-demosaiced linear DNG, such as iPhone ProRAW.
    LinearDng,
}

impl fmt::Display for CfaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::XTrans => "X-Trans",
            Self::Bayer => "Bayer",
            Self::LinearDng => "linear DNG",
        })
    }
}

/// How the camera asked for the sensor image to be displayed.
///
/// Values follow the EXIF Orientation tag. [`Self::apply_rgb`] bakes that
/// transform into pixels at export, so the file itself is stored upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Tag 1. No transform.
    Normal,
    /// Tag 2.
    HorizontalFlip,
    /// Tag 3.
    Rotate180,
    /// Tag 4.
    VerticalFlip,
    /// Tag 5. Mirror across the diagonal.
    Transpose,
    /// Tag 6. 90 degrees clockwise.
    Rotate90,
    /// Tag 7.
    Transverse,
    /// Tag 8. 90 degrees counter-clockwise.
    Rotate270,
    /// Missing or unrecognized tag. Treated as [`Self::Normal`].
    Unknown,
}

impl Orientation {
    /// Width and height after this orientation is applied.
    pub fn dimensions(self, width: u32, height: u32) -> (u32, u32) {
        if self.transposes() {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Bake this orientation into interleaved RGB samples.
    ///
    /// Flips happen before the transpose, matching EXIF display orientation.
    pub(crate) fn apply_rgb<T: Copy>(
        self,
        width: u32,
        height: u32,
        pixels: &[T],
    ) -> (u32, u32, Vec<T>) {
        let (out_width, out_height) = self.dimensions(width, height);
        if pixels.is_empty() {
            return (out_width, out_height, Vec::new());
        }
        let (transpose, flip_horizontal, flip_vertical) = self.flips();
        if !transpose && !flip_horizontal && !flip_vertical {
            return (width, height, pixels.to_vec());
        }
        debug_assert_eq!(pixels.len(), width as usize * height as usize * 3);
        let mut out = vec![pixels[0]; pixels.len()];
        for y in 0..height {
            for x in 0..width {
                let flipped_x = if flip_horizontal { width - 1 - x } else { x };
                let flipped_y = if flip_vertical { height - 1 - y } else { y };
                let (dest_x, dest_y) = if transpose {
                    (flipped_y, flipped_x)
                } else {
                    (flipped_x, flipped_y)
                };
                let source = (y as usize * width as usize + x as usize) * 3;
                let dest = (dest_y as usize * out_width as usize + dest_x as usize) * 3;
                out[dest..dest + 3].copy_from_slice(&pixels[source..source + 3]);
            }
        }
        (out_width, out_height, out)
    }

    fn transposes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }

    /// `(transpose, flip horizontal, flip vertical)`.
    fn flips(self) -> (bool, bool, bool) {
        match self {
            Self::Normal | Self::Unknown => (false, false, false),
            Self::VerticalFlip => (false, false, true),
            Self::HorizontalFlip => (false, true, false),
            Self::Rotate180 => (false, true, true),
            Self::Transpose => (true, false, false),
            Self::Rotate90 => (true, false, true),
            Self::Rotate270 => (true, true, false),
            Self::Transverse => (true, true, true),
        }
    }
}

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::HorizontalFlip => "flip horizontal",
            Self::Rotate180 => "rotate 180",
            Self::VerticalFlip => "flip vertical",
            Self::Transpose => "transpose",
            Self::Rotate90 => "rotate 90 CW",
            Self::Transverse => "transverse",
            Self::Rotate270 => "rotate 90 CCW",
            Self::Unknown => "unknown",
        })
    }
}

/// Failure while opening a RAW file or reading the metadata we keep.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// `rawler` could not decode the file.
    #[error("failed to decode {path}")]
    Backend {
        path: String,
        #[source]
        source: rawler::RawlerError,
    },

    /// The decoded buffer length did not match width × height × components.
    #[error("{path} decoded to {actual} samples, expected {expected}")]
    UnexpectedSamples {
        path: String,
        actual: usize,
        expected: usize,
    },

    /// The file is neither a Bayer/X-Trans mosaic nor a linear DNG.
    #[error("{path} is not X-Trans, Bayer, or a linear DNG")]
    UnsupportedLayout { path: String },

    /// Black or white levels were missing.
    #[error("{path} is missing black or white levels")]
    MissingLevels { path: String },

    /// Neither `xyz_to_cam` nor a DNG color matrix was usable.
    #[error("{path} is missing a camera color matrix")]
    MissingCameraMatrix { path: String },

    /// Width or height did not fit in `u32`.
    #[error("{path} dimensions do not fit in u32")]
    Dimensions { path: String },
}

/// Decoded RAW file.
///
/// Fields stay private so callers go through the accessors. Sensor samples
/// are retained for [`Self::develop_linear`].
pub struct RawImage {
    width: u32,
    height: u32,
    cfa: CfaKind,
    orientation: Orientation,
    black_levels: Vec<f32>,
    white_levels: Vec<f32>,
    /// As-shot white balance in RGBE order, as encoded by the file.
    as_shot_wb: [f32; 4],
    /// XYZ → camera RGBE matrix.
    camera_matrix: [[f32; 3]; 4],
    /// Sensor samples and CFA layout. Not part of [`Debug`] output.
    decoded: rawler::rawimage::RawImage,
}

impl RawImage {
    /// Decode `path` and keep the metadata needed to develop it later.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DecodeError> {
        let path = path.as_ref();
        let path_string = path.display().to_string();
        let decoded = rawler::decode_file(path).map_err(|source| DecodeError::Backend {
            path: path_string.clone(),
            source,
        })?;
        Self::from_decoded(&path_string, decoded)
    }

    /// Width and height of the decoded frame, in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Color-filter layout, or linear DNG when the file is already demosaiced.
    pub fn cfa(&self) -> CfaKind {
        self.cfa
    }

    /// Display orientation stored in the file.
    ///
    /// Sensor samples are not rotated. Export applies this to the pixels.
    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Black levels in the order `rawler` reports them.
    pub fn black_levels(&self) -> &[f32] {
        &self.black_levels
    }

    /// White levels in the order `rawler` reports them.
    pub fn white_levels(&self) -> &[f32] {
        &self.white_levels
    }

    /// As-shot white balance coefficients in RGBE order.
    pub fn as_shot_wb(&self) -> [f32; 4] {
        self.as_shot_wb
    }

    /// Camera color matrix, XYZ → camera RGBE, row-major.
    pub fn camera_matrix(&self) -> [[f32; 3]; 4] {
        self.camera_matrix
    }

    /// Sensor samples backing this image.
    pub(crate) fn decoded(&self) -> &rawler::rawimage::RawImage {
        &self.decoded
    }

    fn from_decoded(path: &str, decoded: rawler::rawimage::RawImage) -> Result<Self, DecodeError> {
        let actual = match &decoded.data {
            RawImageData::Integer(samples) => samples.len(),
            RawImageData::Float(samples) => samples.len(),
        };
        let expected = decoded
            .width
            .checked_mul(decoded.height)
            .and_then(|count| count.checked_mul(decoded.cpp))
            .unwrap_or(0);
        if actual == 0 || actual != expected {
            return Err(DecodeError::UnexpectedSamples {
                path: path.to_owned(),
                actual,
                expected,
            });
        }

        let width = u32::try_from(decoded.width).map_err(|_| DecodeError::Dimensions {
            path: path.to_owned(),
        })?;
        let height = u32::try_from(decoded.height).map_err(|_| DecodeError::Dimensions {
            path: path.to_owned(),
        })?;

        let black_levels = decoded.blacklevel.as_vec();
        let white_levels = decoded.whitelevel.as_vec();
        if black_levels.is_empty() || white_levels.is_empty() {
            return Err(DecodeError::MissingLevels {
                path: path.to_owned(),
            });
        }

        let camera_matrix =
            Self::camera_matrix_from(&decoded).ok_or_else(|| DecodeError::MissingCameraMatrix {
                path: path.to_owned(),
            })?;

        Ok(Self {
            width,
            height,
            cfa: Self::cfa_kind(&decoded).ok_or_else(|| DecodeError::UnsupportedLayout {
                path: path.to_owned(),
            })?,
            orientation: orientation_from(decoded.orientation),
            black_levels,
            white_levels,
            as_shot_wb: decoded.wb_coeffs,
            camera_matrix,
            decoded,
        })
    }

    fn cfa_kind(decoded: &rawler::rawimage::RawImage) -> Option<CfaKind> {
        match &decoded.photometric {
            RawPhotometricInterpretation::LinearRaw => Some(CfaKind::LinearDng),
            RawPhotometricInterpretation::Cfa(config) => match config.sensor {
                SensorType::Xtrans => Some(CfaKind::XTrans),
                SensorType::Bayer => Some(CfaKind::Bayer),
            },
            RawPhotometricInterpretation::BlackIsZero => None,
        }
    }

    /// `xyz_to_cam` when `rawler` filled it, otherwise the preferred DNG matrix.
    fn camera_matrix_from(decoded: &rawler::rawimage::RawImage) -> Option<[[f32; 3]; 4]> {
        if Self::matrix_has_value(&decoded.xyz_to_cam) {
            return Some(decoded.xyz_to_cam);
        }

        let preferred = [
            Illuminant::D65,
            Illuminant::D50,
            Illuminant::D55,
            Illuminant::A,
        ];
        let (_, flat) = decoded.color_matrix_find_first(preferred)?;
        Self::matrix_from_flat(&flat)
    }

    fn matrix_from_flat(flat: &[f32]) -> Option<[[f32; 3]; 4]> {
        if flat.len() != 9 && flat.len() != 12 {
            return None;
        }
        let mut matrix = [[0.0; 3]; 4];
        for (index, value) in flat.iter().copied().enumerate() {
            matrix[index / 3][index % 3] = value;
        }
        Self::matrix_has_value(&matrix).then_some(matrix)
    }

    fn matrix_has_value(matrix: &[[f32; 3]; 4]) -> bool {
        matrix.iter().flatten().any(|value| *value != 0.0)
    }

    fn format_levels(levels: &[f32]) -> String {
        levels
            .iter()
            .map(|level| level.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl fmt::Debug for RawImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("cfa", &self.cfa)
            .field("orientation", &self.orientation)
            .field("black_levels", &self.black_levels)
            .field("white_levels", &self.white_levels)
            .field("as_shot_wb", &self.as_shot_wb)
            .field("camera_matrix", &self.camera_matrix)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for RawImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (width, height) = self.dimensions();
        writeln!(f, "dimensions: {width}x{height}")?;
        writeln!(f, "cfa: {}", self.cfa())?;
        writeln!(f, "orientation: {}", self.orientation())?;
        writeln!(f, "black: {}", Self::format_levels(self.black_levels()))?;
        writeln!(f, "white: {}", Self::format_levels(self.white_levels()))?;
        let [red, green, blue, extra] = self.as_shot_wb();
        writeln!(f, "as_shot_wb (RGBE): {red} {green} {blue} {extra}")?;
        writeln!(f, "camera_matrix:")?;
        for (index, row) in self.camera_matrix().into_iter().enumerate() {
            let line = format!("  {} {} {}", row[0], row[1], row[2]);
            if index + 1 == 4 {
                write!(f, "{line}")?;
            } else {
                writeln!(f, "{line}")?;
            }
        }
        Ok(())
    }
}

fn orientation_from(orientation: rawler::Orientation) -> Orientation {
    match orientation {
        rawler::Orientation::Normal => Orientation::Normal,
        rawler::Orientation::HorizontalFlip => Orientation::HorizontalFlip,
        rawler::Orientation::Rotate180 => Orientation::Rotate180,
        rawler::Orientation::VerticalFlip => Orientation::VerticalFlip,
        rawler::Orientation::Transpose => Orientation::Transpose,
        rawler::Orientation::Rotate90 => Orientation::Rotate90,
        rawler::Orientation::Transverse => Orientation::Transverse,
        rawler::Orientation::Rotate270 => Orientation::Rotate270,
        rawler::Orientation::Unknown => Orientation::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 image, red channel holding the pixel id:
    ///
    /// ```text
    /// 10 20
    /// 30 40
    /// ```
    fn square() -> Vec<u8> {
        let mut pixels = Vec::new();
        for id in [10u8, 20, 30, 40] {
            pixels.extend([id, 0, 0]);
        }
        pixels
    }

    fn reds(pixels: &[u8]) -> Vec<u8> {
        pixels.iter().step_by(3).copied().collect()
    }

    #[test]
    fn orientation_matches_exif_display_transforms() {
        let cases = [
            (Orientation::Normal, 2, 2, vec![10, 20, 30, 40]),
            (Orientation::HorizontalFlip, 2, 2, vec![20, 10, 40, 30]),
            (Orientation::Rotate180, 2, 2, vec![40, 30, 20, 10]),
            (Orientation::VerticalFlip, 2, 2, vec![30, 40, 10, 20]),
            (Orientation::Transpose, 2, 2, vec![10, 30, 20, 40]),
            (Orientation::Rotate90, 2, 2, vec![30, 10, 40, 20]),
            (Orientation::Rotate270, 2, 2, vec![20, 40, 10, 30]),
            (Orientation::Transverse, 2, 2, vec![40, 20, 30, 10]),
            (Orientation::Unknown, 2, 2, vec![10, 20, 30, 40]),
        ];
        for (orientation, width, height, expected) in cases {
            let (got_w, got_h, pixels) = orientation.apply_rgb(2, 2, &square());
            assert_eq!((got_w, got_h), (width, height), "{orientation}");
            assert_eq!(reds(&pixels), expected, "{orientation}");
        }
    }

    #[test]
    fn rotate90_swaps_a_wide_frame() {
        let pixels = [1u8, 0, 0, 0, 1, 0];
        let (width, height, out) = Orientation::Rotate90.apply_rgb(2, 1, &pixels);
        assert_eq!((width, height), (1, 2));
        assert_eq!(out, [1, 0, 0, 0, 1, 0]);
    }
}
