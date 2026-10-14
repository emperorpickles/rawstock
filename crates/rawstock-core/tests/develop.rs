//! Scene-linear development checks against the CC0 fixtures in `decode`.
//!
//! Each test checks the developed dimensions and that a gray surface in the
//! scene stays neutral in ACEScg. Downloads follow the same skip rule as the
//! decode tests.

use rawstock_core::color::LinearImage;
use rawstock_core::decode::RawImage;
use rawstock_core::render::{Parameters, Pipeline};
use rawstock_core::stock::{Curve, FilmModel, Parametric};

#[path = "common/mod.rs"]
mod common;

use common::{Fixture, IPHONE_12_PRO, X_S10};

/// A sensor-space window that should be neutral after as-shot white balance.
struct GrayPatch {
    /// Left edge, in developed pixels.
    x: u32,
    /// Top edge, in developed pixels.
    y: u32,
    /// Window width and height.
    size: u32,
    /// Maximum |channel − mean| / mean.
    tolerance: f32,
}

#[test]
fn develops_fuji_xtrans_to_acescg() {
    // Concrete floor in the foreground of the X-S10 frame.
    develop_fixture(
        &X_S10,
        GrayPatch {
            x: 2128,
            y: 3654,
            size: 96,
            tolerance: 0.08,
        },
    );
}

#[test]
fn develops_iphone_proraw_to_acescg() {
    // Asphalt in the foreground of the iPhone 12 Pro frame.
    develop_fixture(
        &IPHONE_12_PRO,
        GrayPatch {
            x: 336,
            y: 2646,
            size: 64,
            tolerance: 0.04,
        },
    );
}

fn develop_fixture(fixture: &Fixture, patch: GrayPatch) {
    let Some(path) = fixture.prepare() else {
        return;
    };
    let raw = RawImage::open(&path).unwrap_or_else(|err| {
        panic!("decode {} failed: {err}", path.display());
    });
    let linear = raw.develop_linear().unwrap_or_else(|err| {
        panic!("develop {} failed: {err}", path.display());
    });
    assert_eq!(
        (linear.width(), linear.height()),
        fixture.dimensions,
        "developed dimensions differ from the decoded frame"
    );
    assert_eq!(raw.orientation(), fixture.orientation);
    let mean = patch_mean(&linear, patch.x, patch.y, patch.size);
    assert_neutral(fixture.filename, mean, patch.tolerance);

    let developed = Pipeline::new(neutral_parameters()).develop(&linear);
    let dir = std::env::temp_dir().join("rawstock-export");
    std::fs::create_dir_all(&dir).unwrap();
    let jpeg = dir.join(format!("{}.jpg", fixture.filename));
    let tiff = dir.join(format!("{}.tiff", fixture.filename));
    developed
        .write(&jpeg, raw.orientation())
        .unwrap_or_else(|err| panic!("jpeg export failed: {err}"));
    developed
        .write(&tiff, raw.orientation())
        .unwrap_or_else(|err| panic!("tiff export failed: {err}"));
    assert_eq!(
        jpeg_dimensions(&jpeg),
        fixture.export_dimensions,
        "jpeg dimensions"
    );
    assert_eq!(
        tiff_dimensions(&tiff),
        fixture.export_dimensions,
        "tiff dimensions"
    );
    assert_eq!(tiff_bits_per_sample(&tiff), [16, 16, 16]);
    let _ = std::fs::remove_file(jpeg);
    let _ = std::fs::remove_file(tiff);
}

/// Same curve on every layer and no coupling, so the export test does not
/// depend on a stock file. This is not a library default.
fn neutral_parameters() -> Parameters {
    let curve = Curve::new([(-4.0, 0.1), (-1.0, 0.6), (0.0, 1.2), (1.5, 2.1)]).unwrap();
    let print = Curve::new([(-1.5, 0.05), (0.0, 0.5), (1.2, 1.4), (2.5, 2.3)]).unwrap();
    let film = FilmModel::Parametric(
        Parametric::new([curve.clone(), curve.clone(), curve], None).unwrap(),
    );
    Parameters::new(0.0, film, 0.8, print).unwrap()
}

fn jpeg_dimensions(path: &std::path::Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    let mut index = 0;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        // SOF0, SOF1, SOF2: baseline, extended, and progressive.
        if matches!(marker, 0xc0..=0xc2) {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]);
            return (u32::from(width), u32::from(height));
        }
        // Markers without a length field: SOI, EOI, and RSTn.
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            index += 2;
            continue;
        }
        if marker == 0x00 || marker == 0xff {
            index += 1;
            continue;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        index += 2 + length;
    }
    panic!("jpeg has no start-of-frame marker");
}

fn tiff_dimensions(path: &std::path::Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    let (width, height, _) = tiff_tags(&bytes);
    (width, height)
}

fn tiff_bits_per_sample(path: &std::path::Path) -> [u16; 3] {
    let bytes = std::fs::read(path).unwrap();
    let (_, _, bits) = tiff_tags(&bytes);
    bits
}

/// ImageWidth, ImageLength, and BitsPerSample from a little-endian TIFF.
fn tiff_tags(bytes: &[u8]) -> (u32, u32, [u16; 3]) {
    assert_eq!(&bytes[0..4], b"II*\0");
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize;
    let mut width = None;
    let mut height = None;
    let mut bits = None;
    for entry in 0..count {
        let offset = ifd + 2 + entry * 12;
        let tag = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let kind = u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        let value = u32::from_le_bytes(bytes[offset + 8..offset + 12].try_into().unwrap());
        match tag {
            256 => width = Some(tiff_long(kind, value)),
            257 => height = Some(tiff_long(kind, value)),
            258 => bits = Some(tiff_bits(bytes, kind, len, value)),
            _ => {}
        }
    }
    (
        width.expect("ImageWidth"),
        height.expect("ImageLength"),
        bits.expect("BitsPerSample"),
    )
}

fn tiff_long(kind: u16, value: u32) -> u32 {
    match kind {
        3 => value & 0xffff, // SHORT
        4 => value,          // LONG
        other => panic!("unexpected tiff type {other}"),
    }
}

fn tiff_bits(bytes: &[u8], kind: u16, len: u32, value: u32) -> [u16; 3] {
    assert_eq!(kind, 3, "BitsPerSample should be SHORT");
    assert_eq!(len, 3);
    // Three shorts do not fit in the value field, so `value` is an offset.
    let offset = value as usize;
    [
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()),
        u16::from_le_bytes(bytes[offset + 2..offset + 4].try_into().unwrap()),
        u16::from_le_bytes(bytes[offset + 4..offset + 6].try_into().unwrap()),
    ]
}

fn patch_mean(image: &LinearImage, x: u32, y: u32, size: u32) -> [f32; 3] {
    assert!(x + size <= image.width());
    assert!(y + size <= image.height());
    let pixels = image.pixels();
    let width = image.width() as usize;
    let mut sum = [0.0f64; 3];
    let mut count = 0.0f64;
    for row in y..y + size {
        for col in x..x + size {
            let index = (row as usize * width + col as usize) * 3;
            sum[0] += f64::from(pixels[index]);
            sum[1] += f64::from(pixels[index + 1]);
            sum[2] += f64::from(pixels[index + 2]);
            count += 1.0;
        }
    }
    [
        (sum[0] / count) as f32,
        (sum[1] / count) as f32,
        (sum[2] / count) as f32,
    ]
}

fn assert_neutral(name: &str, rgb: [f32; 3], tolerance: f32) {
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    assert!(
        mean > 0.02,
        "{name} gray patch is too dark to judge neutrality: {rgb:?}"
    );
    for channel in rgb {
        let delta = (channel - mean).abs() / mean;
        assert!(
            delta < tolerance,
            "{name} gray patch {rgb:?} deviates by {delta} (mean {mean}, tolerance {tolerance})"
        );
    }
}
