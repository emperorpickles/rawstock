//! `rawstock develop --stock` on a CC0 fixture.
//!
//! Downloads follow the same skip rule as the core decode tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

struct Fixture {
    filename: &'static str,
    url: &'static str,
    sha256: &'static str,
    export_dimensions: (u32, u32),
}

const IPHONE_12_PRO: Fixture = Fixture {
    filename: "IMG_1361.DNG",
    url: "https://raw.pixls.us/data/Apple/iPhone%2012%20Pro/IMG_1361.DNG",
    sha256: "e91e77a4533ed7cce551d83330676ea5c47dd5e55fb38adda7819366afdbdfc2",
    export_dimensions: (3024, 4032),
};

#[test]
fn develop_with_stock_writes_the_fixture() {
    let Some(input) = IPHONE_12_PRO.prepare() else {
        return;
    };
    let stock = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/neutral.stock.toml");
    let output = std::env::temp_dir().join(format!("rawstock-neutral-{}.jpg", std::process::id()));
    let bin = env!("CARGO_BIN_EXE_rawstock");
    let run = Command::new(bin)
        .arg("develop")
        .arg(&input)
        .arg("--stock")
        .arg(&stock)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("rawstock failed to start");
    assert!(
        run.status.success(),
        "develop --stock failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        jpeg_dimensions(&output),
        IPHONE_12_PRO.export_dimensions,
        "developed jpeg dimensions"
    );
    let _ = std::fs::remove_file(output);
}

#[test]
fn develop_portra_400_writes_the_fixture() {
    let Some(input) = IPHONE_12_PRO.prepare() else {
        return;
    };
    let stock = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/portra400.stock.toml");
    let output = std::env::temp_dir().join(format!("rawstock-portra-{}.jpg", std::process::id()));
    let bin = env!("CARGO_BIN_EXE_rawstock");
    let run = Command::new(bin)
        .arg("develop")
        .arg(&input)
        .arg("--stock")
        .arg(&stock)
        .arg("--out")
        .arg(&output)
        .output()
        .expect("rawstock failed to start");
    assert!(
        run.status.success(),
        "develop --stock portra400 failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        jpeg_dimensions(&output),
        IPHONE_12_PRO.export_dimensions,
        "developed jpeg dimensions"
    );
    let _ = std::fs::remove_file(output);
}

impl Fixture {
    fn prepare(&self) -> Option<PathBuf> {
        let path = std::env::temp_dir()
            .join("rawstock-fixtures")
            .join(self.filename);
        if checksum_matches(&path, self.sha256) {
            return Some(path);
        }
        if let Err(err) = fetch(&path, self.url) {
            return missing(self.filename, &err);
        }
        if checksum_matches(&path, self.sha256) {
            Some(path)
        } else {
            panic!("{} checksum does not match {}", path.display(), self.sha256);
        }
    }
}

fn fetch(path: &Path, url: &str) -> std::io::Result<()> {
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
        .arg(url)
        .status()
        .map_err(|err| std::io::Error::other(format!("curl is unavailable: {err}")))?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(path);
        Err(std::io::Error::other(format!("curl exited {status}")))
    }
}

fn checksum_matches(path: &Path, expected: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    hex(&Sha256::digest(bytes)) == expected
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

fn missing(filename: &str, err: &std::io::Error) -> Option<PathBuf> {
    let message = format!("fixture {filename} is unavailable: {err}");
    if std::env::var_os("RAWSTOCK_REQUIRE_FIXTURES").is_some() {
        panic!("{message}");
    }
    eprintln!("skipping {message}");
    None
}

fn jpeg_dimensions(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    let mut index = 0;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if matches!(marker, 0xc0..=0xc2) {
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]);
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]);
            return (u32::from(width), u32::from(height));
        }
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
