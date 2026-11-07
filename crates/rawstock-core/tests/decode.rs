//! Decode checks against CC0 samples from <https://raw.pixls.us>.
//!
//! The bytes are fetched into the temp directory and checked against the
//! SHA-256 published in `filelist.sha256`. A missing download skips the
//! test, unless `RAWSTOCK_REQUIRE_FIXTURES` is set.

use rawstock_core::decode::{EmbeddedPreview, RawImage};
use rawstock_core::lens::{LensCorrections, LensProfile};
use rawstock_core::white_balance::WhiteBalance;

#[path = "common/mod.rs"]
mod common;

use common::{IPHONE_12_PRO, X_S10};

#[test]
fn missing_file_is_an_error() {
    let err = RawImage::open("/tmp/rawstock-does-not-exist.RAF").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("failed to decode"));
}

#[test]
fn decodes_fuji_xtrans_raf() {
    let Some(path) = X_S10.prepare() else {
        return;
    };
    X_S10.assert_open(&path);
}

#[test]
fn decodes_iphone_proraw_dng() {
    let Some(path) = IPHONE_12_PRO.prepare() else {
        return;
    };
    IPHONE_12_PRO.assert_open(&path);
}

#[test]
fn embedded_preview_of_a_missing_file_is_an_error() {
    assert!(EmbeddedPreview::read("/tmp/rawstock-does-not-exist.RAF", 128).is_err());
}

#[test]
fn reads_the_embedded_preview_of_a_raf() {
    let Some(path) = X_S10.prepare() else {
        return;
    };
    let preview = EmbeddedPreview::read(&path, 160)
        .unwrap()
        .expect("Fuji RAFs carry a JPEG preview");
    assert_eq!(preview.width.max(preview.height), 160);
    assert_eq!(
        preview.rgb.len(),
        (preview.width * preview.height * 3) as usize
    );
    assert!(preview.rgb.iter().any(|&value| value > 16));
}

#[test]
fn proraw_records_no_lens_tables() {
    let Some(path) = IPHONE_12_PRO.prepare() else {
        return;
    };
    let image = RawImage::open(&path).unwrap();
    assert_eq!(image.lens_profile(), &LensProfile::NONE);
}

#[test]
fn lens_corrections_follow_the_rafs_own_tables_and_switch_off() {
    let Some(path) = X_S10.prepare() else {
        return;
    };
    let mut raw = RawImage::open(&path).unwrap();
    let quick = |raw: &RawImage| {
        let quick = raw.develop_quick_camera().unwrap().expect("X-Trans");
        quick.image.develop(WhiteBalance::AsShot).unwrap()
    };
    assert_eq!(raw.lens_corrections(), LensCorrections::ALL);
    let on = quick(&raw);
    raw.set_lens_corrections(LensCorrections::OFF);
    let off = quick(&raw);
    assert_eq!((on.width(), on.height()), (off.width(), off.height()));
    let profile = raw.lens_profile();
    let has_table = profile.distortion().is_some() || profile.vignetting().is_some();
    assert_eq!(on.pixels() != off.pixels(), has_table, "{profile:?}");
}
