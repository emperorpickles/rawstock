//! Open-path classification. This reads a short header and does not decode.
//!
//! A `RAWSTOCK-FIXTURE` text file with a `.raf` or `.dng` extension is the
//! stand-in for unsupported-camera and corrupt-file tests. It is not a
//! photograph and it is never bundled.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::contract::{
    AppError, CameraInfo, ExifInfo, ImageInfo, OpenOutcome, RejectedPath, SidecarInfo,
    SupportLevel, canonical_path, file_label, image_id_for,
};

const FIXTURE: &[u8] = b"RAWSTOCK-FIXTURE";
const RAF_MAGIC: &[u8] = b"FUJIFILMCCD-RAW ";
/// Below this size a magic-matched file is still truncated.
const MIN_RAW_BYTES: u64 = 4_096;

/// Log line paired with a refusal, for the app log rather than the UI.
#[derive(Debug)]
pub struct OpenLog {
    pub detail: String,
}

/// Expand files and folders into accepted images and refusals.
pub fn classify_inputs(paths: &[String]) -> (OpenOutcome, Vec<OpenLog>) {
    let mut images = Vec::new();
    let mut rejected = Vec::new();
    let mut logs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in paths {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                expand_dir(
                    Path::new(path),
                    &mut images,
                    &mut rejected,
                    &mut logs,
                    &mut seen,
                );
            }
            Ok(_) => classify_one(
                Path::new(path),
                &mut images,
                &mut rejected,
                &mut logs,
                &mut seen,
            ),
            Err(error) => {
                push_reject(
                    &mut rejected,
                    &mut logs,
                    Path::new(path),
                    AppError::io(Path::new(path)),
                    error.to_string(),
                );
            }
        }
    }
    images.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    (OpenOutcome { images, rejected }, logs)
}

fn expand_dir(
    dir: &Path,
    images: &mut Vec<ImageInfo>,
    rejected: &mut Vec<RejectedPath>,
    logs: &mut Vec<OpenLog>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            push_reject(rejected, logs, dir, AppError::io(dir), error.to_string());
            return;
        }
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_raw_extension(path))
        .collect();
    files.sort();
    for path in files {
        classify_one(&path, images, rejected, logs, seen);
    }
}

fn classify_one(
    path: &Path,
    images: &mut Vec<ImageInfo>,
    rejected: &mut Vec<RejectedPath>,
    logs: &mut Vec<OpenLog>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    match classify_file(path) {
        Ok(info) => {
            if seen.insert(info.image_id.clone()) {
                images.push(info);
            }
        }
        Err((error, detail)) => push_reject(rejected, logs, path, error, detail),
    }
}

fn push_reject(
    rejected: &mut Vec<RejectedPath>,
    logs: &mut Vec<OpenLog>,
    path: &Path,
    error: AppError,
    detail: String,
) {
    logs.push(OpenLog {
        detail: format!(
            "{} {} {detail}",
            error_code_name(error.code),
            path.display()
        ),
    });
    rejected.push(RejectedPath {
        path: path.display().to_string(),
        error,
    });
}

fn error_code_name(code: crate::contract::ErrorCode) -> &'static str {
    match code {
        crate::contract::ErrorCode::UnsupportedCamera => "unsupportedCamera",
        crate::contract::ErrorCode::CorruptFile => "corruptFile",
        crate::contract::ErrorCode::UnsupportedFormat => "unsupportedFormat",
        crate::contract::ErrorCode::Io => "io",
        crate::contract::ErrorCode::SidecarInvalid => "sidecarInvalid",
        crate::contract::ErrorCode::StockMissing => "stockMissing",
        crate::contract::ErrorCode::CacheMiss => "cacheMiss",
        crate::contract::ErrorCode::Internal => "internal",
    }
}

fn classify_file(path: &Path) -> Result<ImageInfo, (AppError, String)> {
    if !is_raw_extension(path) {
        return Err((
            AppError::unsupported_format(path),
            "extension is not raf or dng".to_string(),
        ));
    }
    let meta = std::fs::metadata(path).map_err(|error| (AppError::io(path), error.to_string()))?;
    let mut file = File::open(path).map_err(|error| (AppError::io(path), error.to_string()))?;
    let mut header = [0u8; 2_048];
    let read = file
        .read(&mut header)
        .map_err(|error| (AppError::io(path), error.to_string()))?;
    if read == 0 || meta.len() == 0 {
        return Err((
            AppError::corrupt_file(path),
            format!("empty file, {} bytes", meta.len()),
        ));
    }
    if header[..read].starts_with(FIXTURE) {
        return classify_fixture(path, &header[..read]);
    }
    if meta.len() < MIN_RAW_BYTES {
        return Err((
            AppError::corrupt_file(path),
            format!("truncated, {} bytes", meta.len()),
        ));
    }
    if header[..read].starts_with(RAF_MAGIC) {
        return Ok(image(
            path,
            "Fujifilm",
            "Unknown",
            SupportLevel::Decodes,
            0,
            0,
        ));
    }
    if read >= 4 && (header.starts_with(b"II*\0") || header.starts_with(b"MM\0*")) {
        return Ok(image(
            path,
            "Unknown",
            "Unknown",
            SupportLevel::Decodes,
            0,
            0,
        ));
    }
    Err((
        AppError::corrupt_file(path),
        "header is not a RAF, a TIFF/DNG, or a fixture".to_string(),
    ))
}

fn classify_fixture(path: &Path, bytes: &[u8]) -> Result<ImageInfo, (AppError, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut support = "decodes".to_string();
    let mut make = "Test".to_string();
    let mut model = "Body".to_string();
    let mut width = 0u32;
    let mut height = 0u32;
    for line in text.lines().skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "support" => support = value.to_string(),
            "make" => make = value.to_string(),
            "model" => model = value.to_string(),
            "width" => width = value.parse().unwrap_or(0),
            "height" => height = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    match support.as_str() {
        "corrupt" => Err((
            AppError::corrupt_file(path),
            "fixture requested corruptFile".to_string(),
        )),
        "unsupported" => Err((
            AppError::unsupported_camera(path, &make, &model),
            format!("fixture camera {make} {model}"),
        )),
        "calibrated" => Ok(image(
            path,
            &make,
            &model,
            SupportLevel::Calibrated,
            width,
            height,
        )),
        "decodes" => Ok(image(
            path,
            &make,
            &model,
            SupportLevel::Decodes,
            width,
            height,
        )),
        other => Err((
            AppError::corrupt_file(path),
            format!("fixture support {other}"),
        )),
    }
}

fn image(
    path: &Path,
    make: &str,
    model: &str,
    support: SupportLevel,
    width: u32,
    height: u32,
) -> ImageInfo {
    let canonical = canonical_path(path);
    let path_string = canonical.display().to_string();
    ImageInfo {
        image_id: image_id_for(&path_string),
        file_name: file_label(&canonical),
        path: path_string,
        camera: CameraInfo {
            make: make.to_string(),
            model: model.to_string(),
        },
        support,
        exif: ExifInfo::default(),
        width,
        height,
        sidecar: SidecarInfo::none(),
    }
}

fn is_raw_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("raf") || ext.eq_ignore_ascii_case("dng"))
}

/// Paths from process arguments, skipping the executable and flags.
pub fn paths_from_args(args: &[String]) -> Vec<String> {
    args.iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-') && !arg.is_empty())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ErrorCode;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = TEMP.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rawstock-open-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn fixture_and_magic_files_classify_without_decoding() {
        let dir = TempDir::new();
        write(
            dir.path(),
            "canon.raf",
            b"RAWSTOCK-FIXTURE\nsupport: unsupported\nmake: Canon\nmodel: EOS 5D Mark IV\n",
        );
        write(dir.path(), "empty.dng", b"");
        write(dir.path(), "note.txt", b"hello");
        let mut raf = b"FUJIFILMCCD-RAW ".to_vec();
        raf.resize(4_096, 0);
        write(dir.path(), "body.RAF", &raf);
        let (outcome, _) = classify_inputs(&[
            dir.path().join("canon.raf").display().to_string(),
            dir.path().join("empty.dng").display().to_string(),
            dir.path().join("note.txt").display().to_string(),
            dir.path().join("body.RAF").display().to_string(),
            dir.path().join("missing.raf").display().to_string(),
        ]);
        assert_eq!(outcome.images.len(), 1);
        assert_eq!(outcome.images[0].camera.make, "Fujifilm");
        assert_eq!(outcome.images[0].support, SupportLevel::Decodes);
        assert_eq!(
            outcome.images[0].sidecar.status,
            crate::contract::SidecarStatus::None
        );
        let codes: Vec<_> = outcome
            .rejected
            .iter()
            .map(|item| item.error.code)
            .collect();
        assert!(codes.contains(&ErrorCode::UnsupportedCamera));
        assert!(codes.contains(&ErrorCode::CorruptFile));
        assert!(codes.contains(&ErrorCode::UnsupportedFormat));
        assert!(codes.contains(&ErrorCode::Io));
        let camera = outcome
            .rejected
            .iter()
            .find(|item| item.error.code == ErrorCode::UnsupportedCamera)
            .unwrap();
        assert!(camera.error.message.contains("Canon"));
        assert!(camera.error.message.contains("EOS 5D Mark IV"));
        assert!(camera.error.is_user_facing());
    }

    #[test]
    fn folders_expand_one_level_to_raw_extensions() {
        let dir = TempDir::new();
        write(
            dir.path(),
            "ok.dng",
            b"RAWSTOCK-FIXTURE\nsupport: calibrated\nmake: Apple\nmodel: iPhone\nwidth: 8\nheight: 6\n",
        );
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        write(
            dir.path().join("nested").as_path(),
            "hidden.raf",
            b"RAWSTOCK-FIXTURE\n",
        );
        let (outcome, _) = classify_inputs(&[dir.path().display().to_string()]);
        assert_eq!(outcome.images.len(), 1);
        assert_eq!(outcome.images[0].file_name, "ok.dng");
        assert_eq!(outcome.images[0].width, 8);
        assert!(outcome.rejected.is_empty());
    }
}
