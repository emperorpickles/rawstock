//! Decode checks against CC0 samples from <https://raw.pixls.us>.
//!
//! The bytes are fetched into the temp directory and checked against the
//! SHA-256 published in `filelist.sha256`. A missing download skips the
//! test, unless `RAWSTOCK_REQUIRE_FIXTURES` is set.

use rawstock_core::decode::RawImage;

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
