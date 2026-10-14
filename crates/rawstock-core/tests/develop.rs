//! Scene-linear development checks against the CC0 fixtures in `decode`.
//!
//! Each test checks the developed dimensions and that a gray surface in the
//! scene stays neutral in ACEScg. Downloads follow the same skip rule as the
//! decode tests.

use rawstock_core::color::LinearImage;
use rawstock_core::decode::RawImage;

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
    let mean = patch_mean(&linear, patch.x, patch.y, patch.size);
    assert_neutral(fixture.filename, mean, patch.tolerance);
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
