//! RAW decoding: a camera file becomes the metadata needed to interpret it.
//!
//! The backend is [`rawler`]. Pixel samples are checked, then dropped: color,
//! stocks, and export are out of scope here.

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

/// Failure while opening a RAW file or reading the metadata we keep.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// `rawler` could not decode the file.
    #[error("failed to decode {path}")]
    Backend {
        /// Path that failed to decode.
        path: String,
        /// The decoder error.
        #[source]
        source: rawler::RawlerError,
    },

    /// The decoded buffer length did not match width × height × components.
    #[error("{path} decoded to {actual} samples, expected {expected}")]
    UnexpectedSamples {
        /// Path that decoded to the wrong number of samples.
        path: String,
        /// Samples present in the decoded buffer.
        actual: usize,
        /// Samples required by the reported dimensions.
        expected: usize,
    },

    /// The file is neither a Bayer/X-Trans mosaic nor a linear DNG.
    #[error("{path} is not X-Trans, Bayer, or a linear DNG")]
    UnsupportedLayout {
        /// Path with an unsupported photometric layout.
        path: String,
    },

    /// Black or white levels were missing.
    #[error("{path} is missing black or white levels")]
    MissingLevels {
        /// Path with no black or white level.
        path: String,
    },

    /// Neither `xyz_to_cam` nor a DNG color matrix was usable.
    #[error("{path} is missing a camera color matrix")]
    MissingCameraMatrix {
        /// Path with no camera color matrix.
        path: String,
    },

    /// Width or height did not fit in `u32`.
    #[error("{path} dimensions do not fit in u32")]
    Dimensions {
        /// Path whose dimensions did not fit.
        path: String,
    },
}

/// Decoded RAW metadata.
///
/// Fields stay private so callers go through the accessors. Pixel samples are
/// not retained.
#[derive(Debug)]
pub struct RawImage {
    width: u32,
    height: u32,
    cfa: CfaKind,
    black_levels: Vec<f32>,
    white_levels: Vec<f32>,
    /// As-shot white balance in RGBE order, as encoded by the file.
    as_shot_wb: [f32; 4],
    /// XYZ → camera RGBE matrix.
    camera_matrix: [[f32; 3]; 4],
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
        Self::from_decoded(&path_string, &decoded)
    }

    /// Width and height of the decoded frame, in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Color-filter layout, or linear DNG when the file is already demosaiced.
    pub fn cfa(&self) -> CfaKind {
        self.cfa
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

    fn from_decoded(path: &str, decoded: &rawler::rawimage::RawImage) -> Result<Self, DecodeError> {
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
            Self::camera_matrix_from(decoded).ok_or_else(|| DecodeError::MissingCameraMatrix {
                path: path.to_owned(),
            })?;

        Ok(Self {
            width,
            height,
            cfa: Self::cfa_kind(decoded).ok_or_else(|| DecodeError::UnsupportedLayout {
                path: path.to_owned(),
            })?,
            black_levels,
            white_levels,
            as_shot_wb: decoded.wb_coeffs,
            camera_matrix,
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
        if let Some((_, flat)) = decoded.color_matrix_find_first(preferred)
            && let Some(matrix) = Self::matrix_from_flat(&flat)
        {
            return Some(matrix);
        }

        let mut entries: Vec<_> = decoded.color_matrix.iter().collect();
        entries.sort_by_key(|(illuminant, _)| *illuminant);
        entries
            .into_iter()
            .find_map(|(_, flat)| Self::matrix_from_flat(flat))
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

impl fmt::Display for RawImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (width, height) = self.dimensions();
        writeln!(f, "dimensions: {width}x{height}")?;
        writeln!(f, "cfa: {}", self.cfa())?;
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
