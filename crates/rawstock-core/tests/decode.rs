//! Decode checks against CC0 samples from <https://raw.pixls.us>.
//!
//! The bytes are fetched into the temp directory and checked against the
//! SHA-256 published in `filelist.sha256`. A missing download skips the
//! test, unless `RAWSTOCK_REQUIRE_FIXTURES` is set.

use rawstock_core::decode::{EmbeddedPreview, RawImage};

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
