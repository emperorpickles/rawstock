//! Crop, straighten, and quarter turns, applied to a developed frame.
//!
//! The geometry comes after the film pipeline, so halation and grain keep
//! the camera frame's neighbours and size. The frame is in camera
//! orientation on the way in.
//!
//! 1. Quarter turns rotate the frame exactly, clockwise.
//! 2. Straighten rotates the turned frame about its centre, clockwise for a
//!    positive angle, and scales it up just enough that the rotated image
//!    still covers the whole turned frame. The frame keeps its size, so no
//!    corner is ever empty.
//! 3. The crop is a rectangle in normalised coordinates of that frame,
//!    rounded outward to whole pixels.
//!
//! The 1.0 app's viewer draws the same mapping on its canvas
//! (`apps/rawstock/src/lib/geometry.ts`), so the view, the thumbnails and
//! the export frame the photo identically.

use rayon::prelude::*;

use crate::decode::Orientation;
use crate::workers::Workers;

/// Straighten angles below this, in degrees, are treated as level.
const LEVEL: f64 = 1.0e-6;

/// One edit's framing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    quarter_turns: u8,
    straighten_deg: f64,
    crop: Option<[f64; 4]>,
}

impl Framing {
    /// `crop` is `[x, y, w, h]` in normalised coordinates of the turned,
    /// straightened frame. Turns wrap at four.
    pub fn new(quarter_turns: u8, straighten_deg: f64, crop: Option<[f64; 4]>) -> Self {
        Self {
            quarter_turns: quarter_turns % 4,
            straighten_deg: if straighten_deg.is_finite() {
                straighten_deg
            } else {
                0.0
            },
            crop,
        }
    }

    /// Nothing to do: no turn, level, and no crop.
    pub fn is_identity(&self) -> bool {
        self.quarter_turns == 0 && self.straighten_deg.abs() < LEVEL && self.crop.is_none()
    }

    /// The smallest scale at which `width`×`height` rotated by `degrees`
    /// still covers a `width`×`height` frame. 1 when level.
    pub fn cover_scale(degrees: f64, width: f64, height: f64) -> f64 {
        if degrees.abs() < LEVEL || width <= 0.0 || height <= 0.0 {
            return 1.0;
        }
        let radians = degrees.abs().to_radians();
        let (sin, cos) = radians.sin_cos();
        (cos + sin * height / width).max(cos + sin * width / height)
    }

    /// Size of the turned frame for a `width`×`height` input.
    pub fn turned_size(&self, width: u32, height: u32) -> (u32, u32) {
        Orientation::quarters(self.quarter_turns).dimensions(width, height)
    }

    /// The crop in whole pixels of a `width`×`height` turned frame, as
    /// `(left, top, columns, rows)`, rounded outward and at least 1×1.
    pub fn crop_pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let Some([x, y, w, h]) = self.crop else {
            return (0, 0, width, height);
        };
        let left = ((x * f64::from(width)).floor().max(0.0) as u32).min(width.saturating_sub(1));
        let top = ((y * f64::from(height)).floor().max(0.0) as u32).min(height.saturating_sub(1));
        let right = ((x + w) * f64::from(width)).ceil().min(f64::from(width)) as u32;
        let bottom = ((y + h) * f64::from(height)).ceil().min(f64::from(height)) as u32;
        (
            left,
            top,
            right.max(left + 1) - left,
            bottom.max(top + 1) - top,
        )
    }

    /// Size of the framed output for a `width`×`height` input.
    pub fn output_size(&self, width: u32, height: u32) -> (u32, u32) {
        let (width, height) = self.turned_size(width, height);
        let (_, _, columns, rows) = self.crop_pixels(width, height);
        (columns, rows)
    }

    /// Where the centre of output pixel `(column, row)` reads in the turned
    /// frame, in continuous pixel coordinates (pixel `i` spans `i..i+1`).
    pub fn source_point(&self, width: u32, height: u32, column: u32, row: u32) -> (f64, f64) {
        let (left, top, _, _) = self.crop_pixels(width, height);
        let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
        let dx = f64::from(left + column) + 0.5 - cx;
        let dy = f64::from(top + row) + 0.5 - cy;
        if self.straighten_deg.abs() < LEVEL {
            return (cx + dx, cy + dy);
        }
        let scale = Self::cover_scale(self.straighten_deg, f64::from(width), f64::from(height));
        let (sin, cos) = self.straighten_deg.to_radians().sin_cos();
        // The frame shows the image rotated clockwise (y down) and scaled,
        // so a frame point reads the image through the inverse.
        (
            cx + (dx * cos + dy * sin) / scale,
            cy + (-dx * sin + dy * cos) / scale,
        )
    }

    /// Frame interleaved RGB floats of a `width`×`height` image.
    pub fn apply_f32(&self, width: u32, height: u32, pixels: &[f32]) -> (u32, u32, Vec<f32>) {
        let turns = Orientation::quarters(self.quarter_turns);
        let (width, height, turned) = turns.apply_rgb(width, height, pixels);
        let (left, top, columns, rows) = self.crop_pixels(width, height);
        if self.straighten_deg.abs() < LEVEL {
            if (left, top, columns, rows) == (0, 0, width, height) {
                return (width, height, turned);
            }
            let mut out = Vec::with_capacity(columns as usize * rows as usize * 3);
            for row in top..top + rows {
                let start = (row as usize * width as usize + left as usize) * 3;
                out.extend_from_slice(&turned[start..start + columns as usize * 3]);
            }
            return (columns, rows, out);
        }
        let source = Bicubic {
            width: width as usize,
            height: height as usize,
            pixels: &turned,
        };
        let mut out = vec![0.0f32; columns as usize * rows as usize * 3];
        Workers::install(|| {
            out.par_chunks_mut(columns as usize * 3)
                .enumerate()
                .for_each(|(row, line)| {
                    for (column, pixel) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                        let (x, y) = self.source_point(width, height, column as u32, row as u32);
                        pixel.copy_from_slice(&source.sample(x - 0.5, y - 0.5));
                    }
                });
        });
        (columns, rows, out)
    }

    /// Frame interleaved RGB8 of a `width`×`height` image.
    pub fn apply_rgb8(&self, width: u32, height: u32, pixels: &[u8]) -> (u32, u32, Vec<u8>) {
        if self.straighten_deg.abs() < LEVEL {
            // Turns and a crop only move whole pixels.
            let turns = Orientation::quarters(self.quarter_turns);
            let (width, height, turned) = turns.apply_rgb(width, height, pixels);
            let (left, top, columns, rows) = self.crop_pixels(width, height);
            let mut out = Vec::with_capacity(columns as usize * rows as usize * 3);
            for row in top..top + rows {
                let start = (row as usize * width as usize + left as usize) * 3;
                out.extend_from_slice(&turned[start..start + columns as usize * 3]);
            }
            return (columns, rows, out);
        }
        let floats: Vec<f32> = pixels.iter().map(|&value| f32::from(value)).collect();
        let (width, height, framed) = self.apply_f32(width, height, &floats);
        let bytes = framed
            .iter()
            .map(|&value| value.round().clamp(0.0, 255.0) as u8)
            .collect();
        (width, height, bytes)
    }
}

/// Catmull-Rom sampling of interleaved RGB, clamped at the edges. Sharper
/// than bilinear, so straightened grain keeps its texture.
struct Bicubic<'a> {
    width: usize,
    height: usize,
    pixels: &'a [f32],
}

impl Bicubic<'_> {
    /// RGB at `(x, y)`, where integer coordinates are pixel centres.
    fn sample(&self, x: f64, y: f64) -> [f32; 3] {
        let (x0, y0) = (x.floor(), y.floor());
        let wx = Self::weights((x - x0) as f32);
        let wy = Self::weights((y - y0) as f32);
        let mut rgb = [0.0f32; 3];
        for (j, weight_y) in wy.iter().enumerate() {
            let row = Self::clamp(y0 as isize + j as isize - 1, self.height);
            for (i, weight_x) in wx.iter().enumerate() {
                let column = Self::clamp(x0 as isize + i as isize - 1, self.width);
                let start = (row * self.width + column) * 3;
                let weight = weight_x * weight_y;
                for (channel, value) in rgb.iter_mut().enumerate() {
                    *value += weight * self.pixels[start + channel];
                }
            }
        }
        rgb
    }

    fn weights(t: f32) -> [f32; 4] {
        let t2 = t * t;
        let t3 = t2 * t;
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    }

    fn clamp(index: isize, size: usize) -> usize {
        index.clamp(0, size as isize - 1) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chart: each pixel's value is its index, in every channel.
    fn chart(width: u32, height: u32) -> Vec<f32> {
        (0..width * height).flat_map(|i| [i as f32; 3]).collect()
    }

    #[test]
    fn identity_and_turns_move_whole_pixels() {
        let pixels = chart(4, 2);
        let (w, h, out) = Framing::new(0, 0.0, None).apply_f32(4, 2, &pixels);
        assert_eq!((w, h), (4, 2));
        assert_eq!(out, pixels);
        let (w, h, out) = Framing::new(1, 0.0, None).apply_f32(4, 2, &pixels);
        assert_eq!((w, h), (2, 4));
        // Clockwise: the bottom-left pixel lands top-left.
        assert_eq!(out[0], 4.0);
    }

    #[test]
    fn crop_rounds_outward_in_the_turned_frame() {
        let pixels: Vec<u8> = (0..4 * 2).flat_map(|i| [i as u8; 3]).collect();
        let framing = Framing::new(0, 0.0, Some([0.5, 0.5, 0.5, 0.5]));
        let (w, h, out) = framing.apply_rgb8(4, 2, &pixels);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, [6, 6, 6, 7, 7, 7]);
        let turned = Framing::new(1, 0.0, Some([0.0, 0.0, 0.5, 0.5]));
        assert_eq!(turned.output_size(8, 6), (3, 4));
    }

    #[test]
    fn cover_scale_fills_the_frame() {
        assert_eq!(Framing::cover_scale(0.0, 6000.0, 4000.0), 1.0);
        // Shared with geometry.test.mjs in the app.
        let scale = Framing::cover_scale(10.0, 6000.0, 4000.0);
        assert!((scale - 1.245_280).abs() < 1.0e-5, "{scale}");
        assert_eq!(
            Framing::cover_scale(-10.0, 6000.0, 4000.0),
            Framing::cover_scale(10.0, 6000.0, 4000.0)
        );
        // Every frame corner reads inside the image.
        let framing = Framing::new(0, 7.5, None);
        for (column, row) in [(0, 0), (599, 0), (0, 399), (599, 399)] {
            let (x, y) = framing.source_point(600, 400, column, row);
            assert!((0.0..=600.0).contains(&x) && (0.0..=400.0).contains(&y));
        }
    }

    #[test]
    fn straighten_turns_a_tilted_line_level() {
        // A dark line through the centre, tilted 5 degrees clockwise...
        let (width, height) = (400u32, 300u32);
        let tilt = 5.0f64.to_radians();
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                let dx = f64::from(x) + 0.5 - 200.0;
                let dy = f64::from(y) + 0.5 - 150.0;
                let distance = (dy * tilt.cos() - dx * tilt.sin()).abs();
                let value = if distance < 2.0 { 0.0 } else { 200.0 };
                pixels.extend_from_slice(&[value; 3]);
            }
        }
        // ...is level after straightening 5 degrees counter-clockwise.
        let (w, h, out) = Framing::new(0, -5.0, None).apply_f32(width, height, &pixels);
        assert_eq!((w, h), (width, height));
        for column in [40usize, 200, 360] {
            let darkest = (0..h as usize)
                .min_by(|&a, &b| {
                    let pa = out[(a * w as usize + column) * 3];
                    let pb = out[(b * w as usize + column) * 3];
                    pa.total_cmp(&pb)
                })
                .unwrap();
            assert!(
                (148..=151).contains(&darkest),
                "column {column} row {darkest}"
            );
        }
    }

    #[test]
    fn rgb8_matches_the_float_path() {
        let pixels: Vec<u8> = (0..32 * 24).flat_map(|i| [(i % 251) as u8; 3]).collect();
        let framing = Framing::new(3, 2.5, Some([0.1, 0.2, 0.5, 0.6]));
        let (w, h, bytes) = framing.apply_rgb8(32, 24, &pixels);
        let floats: Vec<f32> = pixels.iter().map(|&v| f32::from(v)).collect();
        let (fw, fh, framed) = framing.apply_f32(32, 24, &floats);
        assert_eq!((w, h), (fw, fh));
        assert_eq!((w, h), framing.output_size(32, 24));
        for (byte, float) in bytes.iter().zip(&framed) {
            assert_eq!(*byte, float.round().clamp(0.0, 255.0) as u8);
        }
    }
}
