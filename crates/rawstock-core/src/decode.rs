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

    /// Map an axis-aligned display rectangle back to sensor pixels.
    ///
    /// `x`, `y`, `width`, and `height` are in the oriented image, the same
    /// space [`Self::apply_rgb`] writes. The returned `(x, y, width, height)`
    /// is the sensor rectangle that displays there. A rect that misses the
    /// image comes back empty.
    pub fn display_to_sensor(
        self,
        sensor_width: u32,
        sensor_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> (u32, u32, u32, u32) {
        if sensor_width == 0 || sensor_height == 0 || width == 0 || height == 0 {
            return (0, 0, 0, 0);
        }
        let (display_width, display_height) = self.dimensions(sensor_width, sensor_height);
        if x >= display_width || y >= display_height {
            return (0, 0, 0, 0);
        }
        let width = width.min(display_width - x);
        let height = height.min(display_height - y);
        let (x0, y0) = self.display_pixel_to_sensor(sensor_width, sensor_height, x, y);
        let (x1, y1) = self.display_pixel_to_sensor(
            sensor_width,
            sensor_height,
            x + width - 1,
            y + height - 1,
        );
        let left = x0.min(x1);
        let top = y0.min(y1);
        let right = x0.max(x1);
        let bottom = y0.max(y1);
        (left, top, right - left + 1, bottom - top + 1)
    }

    fn display_pixel_to_sensor(
        self,
        width: u32,
        height: u32,
        dest_x: u32,
        dest_y: u32,
    ) -> (u32, u32) {
        let (transpose, flip_horizontal, flip_vertical) = self.flips();
        let (flipped_x, flipped_y) = if transpose {
            (dest_y, dest_x)
        } else {
            (dest_x, dest_y)
        };
        let x = if flip_horizontal {
            width - 1 - flipped_x
        } else {
            flipped_x
        };
        let y = if flip_vertical {
            height - 1 - flipped_y
        } else {
            flipped_y
        };
        (x, y)
    }

    /// Bake this orientation into interleaved RGB samples.
    ///
    /// Flips happen before the transpose, matching EXIF display orientation.
    /// `T` is one channel sample, and samples are interleaved RGB, so a pixel
    /// is three consecutive values. The preview shell uses this on 8-bit RGB.
    pub fn apply_rgb<T: Copy>(self, width: u32, height: u32, pixels: &[T]) -> (u32, u32, Vec<T>) {
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

    /// EXIF Orientation tag, 1 through 8. [`Self::Unknown`] is reported as 1.
    pub fn exif_value(self) -> u16 {
        match self {
            Self::Normal | Self::Unknown => 1,
            Self::HorizontalFlip => 2,
            Self::Rotate180 => 3,
            Self::VerticalFlip => 4,
            Self::Transpose => 5,
            Self::Rotate90 => 6,
            Self::Transverse => 7,
            Self::Rotate270 => 8,
        }
    }

    /// EXIF Orientation tag. A value outside 1..=8 is [`Self::Unknown`].
    pub fn from_exif(value: u16) -> Self {
        match value {
            1 => Self::Normal,
            2 => Self::HorizontalFlip,
            3 => Self::Rotate180,
            4 => Self::VerticalFlip,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => Self::Unknown,
        }
    }

    /// Pixels stay where they are. A missing tag is included.
    pub fn is_identity(self) -> bool {
        matches!(self, Self::Normal | Self::Unknown)
    }

    /// `turns` clockwise quarter-turns. Zero is [`Self::Normal`].
    pub fn quarters(turns: u8) -> Self {
        match turns % 4 {
            0 => Self::Normal,
            1 => Self::Rotate90,
            2 => Self::Rotate180,
            _ => Self::Rotate270,
        }
    }

    /// This orientation, then `next`, in the image `next` sees.
    ///
    /// Manual rotation uses this: the camera tag runs first, and a quarter
    /// turn is applied to that upright image.
    pub fn then(self, next: Self) -> Self {
        if next.is_identity() {
            return self.canonical();
        }
        if self.is_identity() {
            return next.canonical();
        }
        let source = Self::probe();
        let (width, height, once) = self.apply_rgb(Self::PROBE_WIDTH, Self::PROBE_HEIGHT, &source);
        let (_, _, twice) = next.apply_rgb(width, height, &once);
        Self::matching(&source, &twice)
    }

    /// Orientation that puts pixels back where this one moved them.
    pub fn inverse(self) -> Self {
        match self {
            Self::Rotate90 => Self::Rotate270,
            Self::Rotate270 => Self::Rotate90,
            other => other.canonical(),
        }
    }

    fn canonical(self) -> Self {
        if self == Self::Unknown {
            Self::Normal
        } else {
            self
        }
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

    const PROBE_WIDTH: u32 = 3;
    const PROBE_HEIGHT: u32 = 2;

    /// 3×2 image whose red channel is the pixel index. Wide enough that a
    /// transpose is not the same byte string as the source.
    fn probe() -> Vec<u8> {
        let mut pixels = Vec::with_capacity(18);
        for id in 0..6u8 {
            pixels.extend([id, 0, 0]);
        }
        pixels
    }

    fn matching(source: &[u8], displayed: &[u8]) -> Self {
        for candidate in Self::catalog() {
            let (_, _, out) = candidate.apply_rgb(Self::PROBE_WIDTH, Self::PROBE_HEIGHT, source);
            if out == displayed {
                return candidate;
            }
        }
        Self::Normal
    }

    fn catalog() -> [Self; 8] {
        [
            Self::Normal,
            Self::HorizontalFlip,
            Self::Rotate180,
            Self::VerticalFlip,
            Self::Transpose,
            Self::Rotate90,
            Self::Transverse,
            Self::Rotate270,
        ]
    }
}

/// Camera orientation plus the clockwise quarter-turns added in the tuner.
///
/// The camera tag is applied first, so a portrait file opens upright. Each
/// manual turn is another 90° on that image. [`Self::delta_to`] is the
/// geometric step between two of these: the demosaic buffer is not decoded
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameOrientation {
    camera: Orientation,
    turns: u8,
}

impl FrameOrientation {
    /// No manual turn.
    pub fn new(camera: Orientation) -> Self {
        Self { camera, turns: 0 }
    }

    /// Orientation tag stored in the file, including [`Orientation::Unknown`].
    pub fn camera(self) -> Orientation {
        self.camera
    }

    /// Clockwise quarter-turns added on top of the camera tag, in `0..4`.
    pub fn turns(self) -> u8 {
        self.turns
    }

    /// One EXIF orientation: camera tag, then the manual turns.
    pub fn composed(self) -> Orientation {
        self.camera
            .canonical()
            .then(Orientation::quarters(self.turns))
    }

    /// One more clockwise quarter-turn.
    pub fn rotate_clockwise(self) -> Self {
        Self {
            turns: (self.turns + 1) % 4,
            ..self
        }
    }

    /// One more counter-clockwise quarter-turn.
    pub fn rotate_counterclockwise(self) -> Self {
        Self {
            turns: (self.turns + 3) % 4,
            ..self
        }
    }

    /// Transform that takes pixels already oriented as `self` into `next`.
    pub fn delta_to(self, next: Self) -> Orientation {
        self.composed().inverse().then(next.composed())
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

    /// EXIF Orientation tag, as `rawler` read it from the file.
    ///
    /// RAF and ProRAW DNG both carry the tag. Sensor samples are not rotated;
    /// [`Orientation::apply_rgb`] and [`crate::session::PreviewSession`] apply it.
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
    fn display_rect_maps_back_to_the_sensor_pixel() {
        let ids = [10u8, 20, 30, 40];
        for orientation in [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Rotate270,
            Orientation::Transverse,
            Orientation::Unknown,
        ] {
            let (_width, _height, displayed) = orientation.apply_rgb(2, 2, &square());
            let (display_w, display_h) = orientation.dimensions(2, 2);
            for y in 0..display_h {
                for x in 0..display_w {
                    let shown = displayed[((y * display_w + x) * 3) as usize];
                    let (sx, sy, w, h) = orientation.display_to_sensor(2, 2, x, y, 1, 1);
                    assert_eq!((w, h), (1, 1), "{orientation} {x},{y}");
                    let id = ids[(sy * 2 + sx) as usize];
                    assert_eq!(shown, id, "{orientation} display {x},{y}");
                }
            }
        }
    }

    #[test]
    fn rotate90_swaps_a_wide_frame() {
        let pixels = [1u8, 0, 0, 0, 1, 0];
        let (width, height, out) = Orientation::Rotate90.apply_rgb(2, 1, &pixels);
        assert_eq!((width, height), (1, 2));
        assert_eq!(out, [1, 0, 0, 0, 1, 0]);
    }

    #[test]
    fn exif_tags_round_trip_through_all_eight_values() {
        for value in 1..=8 {
            let orientation = Orientation::from_exif(value);
            assert_eq!(orientation.exif_value(), value, "tag {value}");
            assert_eq!(
                Orientation::from_exif(orientation.exif_value()),
                orientation
            );
        }
        assert_eq!(Orientation::from_exif(0), Orientation::Unknown);
        assert_eq!(Orientation::from_exif(9), Orientation::Unknown);
        assert_eq!(Orientation::Unknown.exif_value(), 1);
    }

    #[test]
    fn quarter_turns_compose_after_every_camera_orientation() {
        let source = Orientation::probe();
        let cameras = [
            Orientation::Normal,
            Orientation::HorizontalFlip,
            Orientation::Rotate180,
            Orientation::VerticalFlip,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
            Orientation::Unknown,
        ];
        for camera in cameras {
            let (width, height, upright) =
                camera.apply_rgb(Orientation::PROBE_WIDTH, Orientation::PROBE_HEIGHT, &source);
            let mut view = FrameOrientation::new(camera);
            for turns in 0..4u8 {
                assert_eq!(view.turns(), turns, "{camera} after {turns}");
                let (turned_w, turned_h, turned) =
                    Orientation::quarters(turns).apply_rgb(width, height, &upright);
                let (got_w, got_h, got) = view.composed().apply_rgb(
                    Orientation::PROBE_WIDTH,
                    Orientation::PROBE_HEIGHT,
                    &source,
                );
                assert_eq!(
                    (got_w, got_h),
                    (turned_w, turned_h),
                    "{camera} turns {turns}"
                );
                assert_eq!(got, turned, "{camera} turns {turns}");
                view = view.rotate_clockwise();
            }
            assert_eq!(view.turns(), 0);
            assert_eq!(view.composed(), camera.canonical());
        }
    }

    #[test]
    fn every_orientation_inverts_and_a_manual_turn_is_a_delta() {
        let source = Orientation::probe();
        for orientation in Orientation::catalog() {
            let (width, height, once) =
                orientation.apply_rgb(Orientation::PROBE_WIDTH, Orientation::PROBE_HEIGHT, &source);
            let (back_w, back_h, back) = orientation.inverse().apply_rgb(width, height, &once);
            assert_eq!(
                (back_w, back_h),
                (Orientation::PROBE_WIDTH, Orientation::PROBE_HEIGHT)
            );
            assert_eq!(back, source, "{orientation}");
            assert_eq!(
                orientation.then(orientation.inverse()),
                Orientation::Normal,
                "{orientation}"
            );
        }

        let camera = Orientation::Rotate90;
        let start = FrameOrientation::new(camera);
        let next = start.rotate_clockwise();
        let (width, height, current) = start.composed().apply_rgb(
            Orientation::PROBE_WIDTH,
            Orientation::PROBE_HEIGHT,
            &source,
        );
        let (_, _, stepped) = start.delta_to(next).apply_rgb(width, height, &current);
        let (_, _, direct) =
            next.composed()
                .apply_rgb(Orientation::PROBE_WIDTH, Orientation::PROBE_HEIGHT, &source);
        assert_eq!(stepped, direct);
        assert_eq!(next.composed(), Orientation::Rotate180);
        assert_eq!(
            start.rotate_counterclockwise().composed(),
            Orientation::Normal
        );
    }
}
