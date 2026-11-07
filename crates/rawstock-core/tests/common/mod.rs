//! Shared CC0 fixture download for integration tests.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use rawstock_core::decode::{CfaKind, Orientation, RawImage};
use sha2::{Digest, Sha256};

/// One shared download at a time in this test process. Other test binaries
/// still race, which is why the bytes land in a partial file first.
fn prepare_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn partial_path(path: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("fixture");
    path.with_file_name(format!(".{name}.{}.{n}.partial", std::process::id()))
}

enum FetchError {
    /// curl could not be run, or it exited without a file.
    Unavailable(io::Error),
    /// The downloaded bytes are not the published fixture.
    Checksum,
}

#[allow(dead_code)]
pub struct Fixture {
    pub filename: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub cfa: CfaKind,
    pub dimensions: (u32, u32),
    /// EXIF orientation stored in the file.
    pub orientation: Orientation,
    /// Width and height after that orientation is applied at export.
    pub export_dimensions: (u32, u32),
    pub black: f32,
    pub black_len: usize,
    pub white: f32,
    pub white_len: usize,
}

#[allow(dead_code)]
impl Fixture {
    /// The fixture on disk, downloading it when the checksum is not already there.
    ///
    /// Several tests call this at once, in this process and in others. The
    /// download is a private partial file in the same directory, and only a
    /// file whose checksum matches is renamed onto the shared path. A bad
    /// checksum is fetched again before this panics.
    pub fn prepare(&self) -> Option<PathBuf> {
        let _guard = prepare_lock();
        let path = std::env::temp_dir()
            .join("rawstock-fixtures")
            .join(self.filename);
        // The first fetch, then two retries. CI runs these tests in parallel,
        // and a torn download must not be the copy the next test opens.
        let mut unavailable: Option<io::Error> = None;
        for _attempt in 0..3 {
            if self.checksum_matches(&path) {
                return Some(path);
            }
            match self.fetch(&path) {
                Ok(()) if self.checksum_matches(&path) => return Some(path),
                Ok(()) => {}
                Err(FetchError::Checksum) => unavailable = None,
                Err(FetchError::Unavailable(err)) => unavailable = Some(err),
            }
        }
        if let Some(err) = unavailable {
            return self.missing(&err);
        }
        panic!("{} checksum does not match {}", path.display(), self.sha256);
    }

    /// Download to a partial file, and rename it onto `path` only when the
    /// checksum matches. `path` is never written in pieces.
    fn fetch(&self, path: &Path) -> Result<(), FetchError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(FetchError::Unavailable)?;
        }
        let partial = partial_path(path);
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(&partial)
            .arg(self.url)
            .status()
            .map_err(|err| {
                FetchError::Unavailable(io::Error::other(format!("curl is unavailable: {err}")))
            })?;
        if !status.success() {
            let _ = std::fs::remove_file(&partial);
            return Err(FetchError::Unavailable(io::Error::other(format!(
                "curl exited {status}"
            ))));
        }
        if !self.checksum_matches(&partial) {
            let _ = std::fs::remove_file(&partial);
            return Err(FetchError::Checksum);
        }
        std::fs::rename(&partial, path).map_err(|err| {
            let _ = std::fs::remove_file(&partial);
            FetchError::Unavailable(err)
        })
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

    pub fn assert_open(&self, path: &Path) {
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

pub const X_S10: Fixture = Fixture {
    filename: "Fujifilm-X-S10-compressed.RAF",
    url: "https://raw.pixls.us/data/Fujifilm/X-S10/Fujifilm-X-S10-compressed.RAF",
    sha256: "9419d1408ebf850395e5dd563beb96309466546252caf541466034c9cb7e9724",
    cfa: CfaKind::XTrans,
    dimensions: (6384, 4182),
    orientation: Orientation::Normal,
    export_dimensions: (6384, 4182),
    black: 1022.0,
    black_len: 36,
    white: 16382.0,
    white_len: 1,
};

pub const IPHONE_12_PRO: Fixture = Fixture {
    filename: "IMG_1361.DNG",
    url: "https://raw.pixls.us/data/Apple/iPhone%2012%20Pro/IMG_1361.DNG",
    sha256: "e91e77a4533ed7cce551d83330676ea5c47dd5e55fb38adda7819366afdbdfc2",
    cfa: CfaKind::LinearDng,
    dimensions: (4032, 3024),
    orientation: Orientation::Rotate90,
    export_dimensions: (3024, 4032),
    black: 0.0,
    black_len: 3,
    white: 65535.0,
    white_len: 3,
};
