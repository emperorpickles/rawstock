//! File-kind checks for the dialogs. The worker rejects anything else.

use std::path::{Path, PathBuf};

pub struct DevelopPaths;

impl DevelopPaths {
    pub fn is_raw(path: &Path) -> bool {
        has_extension(path, &["raf", "dng"])
    }

    pub fn is_stock(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".stock.toml"))
    }

    pub fn is_jpeg(path: &Path) -> bool {
        has_extension(path, &["jpg", "jpeg"])
    }

    pub fn ensure_jpeg(path: PathBuf) -> PathBuf {
        if Self::is_jpeg(&path) {
            return path;
        }
        with_suffix(path, ".jpg")
    }

    pub fn ensure_stock(path: PathBuf) -> PathBuf {
        if Self::is_stock(&path) {
            return path;
        }
        with_suffix(path, ".stock.toml")
    }
}

fn has_extension(path: &Path, expected: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            expected
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn with_suffix(mut path: PathBuf, suffix: &str) -> PathBuf {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".to_string());
    path.set_file_name(format!("{file}{suffix}"));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_and_jpeg_extensions_ignore_case() {
        assert!(DevelopPaths::is_raw(Path::new("IMG.RAF")));
        assert!(DevelopPaths::is_raw(Path::new("phone.dng")));
        assert!(!DevelopPaths::is_raw(Path::new("notes.txt")));
        assert!(DevelopPaths::is_jpeg(Path::new("out.JPEG")));
        assert_eq!(
            DevelopPaths::ensure_jpeg(PathBuf::from("out"))
                .file_name()
                .unwrap(),
            "out.jpg"
        );
        assert!(DevelopPaths::is_stock(Path::new("portra400.stock.toml")));
        assert_eq!(
            DevelopPaths::ensure_stock(PathBuf::from("look"))
                .file_name()
                .unwrap(),
            "look.stock.toml"
        );
    }
}
