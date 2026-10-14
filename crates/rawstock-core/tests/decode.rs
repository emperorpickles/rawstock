//! Decode checks against CC0 samples from <https://raw.pixls.us>.
//!
//! The bytes are fetched into the temp directory and checked against the
//! SHA-256 published in `filelist.sha256`. A missing download skips the
//! test, unless `RAWSTOCK_REQUIRE_FIXTURES` is set.

use std::path::{Path, PathBuf};
use std::process::Command;

use rawstock_core::decode::{CfaKind, RawImage};
use sha2::{Digest, Sha256};

struct Fixture {
    filename: &'static str,
    url: &'static str,
    sha256: &'static str,
    cfa: CfaKind,
    dimensions: (u32, u32),
    black: f32,
    black_len: usize,
    white: f32,
    white_len: usize,
}

impl Fixture {
    fn prepare(&self) -> Option<PathBuf> {
        let path = std::env::temp_dir()
            .join("rawstock-fixtures")
            .join(self.filename);
        if self.checksum_matches(&path) {
            return Some(path);
        }
        if let Err(err) = self.fetch(&path) {
            return self.missing(&err);
        }
        if self.checksum_matches(&path) {
            Some(path)
        } else {
            panic!("{} checksum does not match {}", path.display(), self.sha256);
        }
    }

    fn fetch(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(path)
            .arg(self.url)
            .status()
            .map_err(|err| std::io::Error::other(format!("curl is unavailable: {err}")))?;
        if status.success() {
            Ok(())
        } else {
            let _ = std::fs::remove_file(path);
            Err(std::io::Error::other(format!("curl exited {status}")))
        }
    }

    fn checksum_matches(&self, path: &Path) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        Self::hex(&Sha256::digest(bytes)) == self.sha256
    }

    fn hex(digest: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        out
    }

    fn missing(&self, err: &std::io::Error) -> Option<PathBuf> {
        let message = format!("fixture {} is unavailable: {err}", self.filename);
        if std::env::var_os("RAWSTOCK_REQUIRE_FIXTURES").is_some() {
            panic!("{message}");
        }
        eprintln!("skipping {message}");
        None
    }

    fn assert_open(&self, path: &Path) {
        let image = RawImage::open(path).unwrap_or_else(|err| {
            panic!("decode {} failed: {err}", path.display());
        });
        assert_eq!(image.cfa(), self.cfa);
        assert_eq!(image.dimensions(), self.dimensions);
        assert_eq!(image.black_levels().len(), self.black_len);
        assert_eq!(image.white_levels().len(), self.white_len);
        assert!(
            image
                .black_levels()
                .iter()
                .all(|level| *level == self.black)
        );
        assert!(
            image
                .white_levels()
                .iter()
                .all(|level| *level == self.white)
        );
        assert!(image.as_shot_wb().iter().any(|coeff| *coeff > 0.0));
        assert!(
            image
                .camera_matrix()
                .iter()
                .flatten()
                .any(|coeff| *coeff != 0.0)
        );
    }
}

const X_S10: Fixture = Fixture {
    filename: "Fujifilm-X-S10-compressed.RAF",
    url: "https://raw.pixls.us/data/Fujifilm/X-S10/Fujifilm-X-S10-compressed.RAF",
    sha256: "9419d1408ebf850395e5dd563beb96309466546252caf541466034c9cb7e9724",
    cfa: CfaKind::XTrans,
    dimensions: (6384, 4182),
    black: 1022.0,
    black_len: 36,
    white: 16382.0,
    white_len: 1,
};

const IPHONE_12_PRO: Fixture = Fixture {
    filename: "IMG_1361.DNG",
    url: "https://raw.pixls.us/data/Apple/iPhone%2012%20Pro/IMG_1361.DNG",
    sha256: "e91e77a4533ed7cce551d83330676ea5c47dd5e55fb38adda7819366afdbdfc2",
    cfa: CfaKind::LinearDng,
    dimensions: (4032, 3024),
    black: 0.0,
    black_len: 3,
    white: 65535.0,
    white_len: 3,
};

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
