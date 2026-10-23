//! Open-path classification. This reads a short header and a sidecar.
//! It does not decode.
//!
//! File > Open, Open Folder, drag-and-drop, and `open:requested` all arrive
//! here as paths. A folder is one level of `.raf` and `.dng`. Unsupported
//! cameras and corrupt files stay in the filmstrip with the user-facing
//! message; a wrong extension or a missing file is refused.
//!
//! A `RAWSTOCK-FIXTURE` text file with a `.raf` or `.dng` extension is the
//! stand-in for tests. It is not a photograph and it is never bundled.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::contract::{
    AppError, CameraInfo, ExifInfo, ImageInfo, OpenOutcome, RejectedPath, SidecarInfo,
    SupportLevel, canonical_path, file_label, image_id_for,
};
use crate::sidecar;

const FIXTURE: &[u8] = b"RAWSTOCK-FIXTURE";
const RAF_MAGIC: &[u8] = b"FUJIFILMCCD-RAW ";
/// Below this size a magic-matched file is still truncated.
const MIN_RAW_BYTES: u64 = 4_096;
/// EXIF lives in the header or the embedded preview, not at the end of a RAF.
const PREFIX_BYTES: usize = 1024 * 1024;

/// Log line paired with a refusal, for the app log rather than the UI.
#[derive(Debug)]
pub struct OpenLog {
    pub detail: String,
}

/// Expand files and folders into accepted images and refusals.
///
/// Images are in capture order: EXIF date, then file name. Dated photos
/// come before photos with no date.
pub fn classify_inputs(paths: &[String]) -> (OpenOutcome, Vec<OpenLog>) {
    let mut images = Vec::new();
    let mut rejected = Vec::new();
    let mut logs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let revisions = sidecar::revision_map();
    for path in paths {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                expand_dir(
                    Path::new(path),
                    &revisions,
                    &mut images,
                    &mut rejected,
                    &mut logs,
                    &mut seen,
                );
            }
            Ok(_) => classify_one(
                Path::new(path),
                &revisions,
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
    images.sort_by(capture_order);
    (OpenOutcome { images, rejected }, logs)
}

fn capture_order(left: &ImageInfo, right: &ImageInfo) -> Ordering {
    match (&left.exif.captured_at, &right.exif.captured_at) {
        (Some(left_date), Some(right_date)) => left_date
            .cmp(right_date)
            .then_with(|| name_order(left, right)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => name_order(left, right),
    }
}

fn name_order(left: &ImageInfo, right: &ImageInfo) -> Ordering {
    left.file_name
        .to_lowercase()
        .cmp(&right.file_name.to_lowercase())
        .then_with(|| left.path.cmp(&right.path))
}

fn expand_dir(
    dir: &Path,
    revisions: &HashMap<String, u32>,
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
        classify_one(&path, revisions, images, rejected, logs, seen);
    }
}

fn classify_one(
    path: &Path,
    revisions: &HashMap<String, u32>,
    images: &mut Vec<ImageInfo>,
    rejected: &mut Vec<RejectedPath>,
    logs: &mut Vec<OpenLog>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    match classify_file(path, revisions) {
        Ok((info, detail)) => {
            if let Some(detail) = detail {
                logs.push(OpenLog { detail });
            }
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

fn classify_file(
    path: &Path,
    revisions: &HashMap<String, u32>,
) -> Result<(ImageInfo, Option<String>), (AppError, String)> {
    if !is_raw_extension(path) {
        return Err((
            AppError::unsupported_format(path),
            "extension is not raf or dng".to_string(),
        ));
    }
    let meta = std::fs::metadata(path).map_err(|error| (AppError::io(path), error.to_string()))?;
    let prefix =
        read_prefix(path, PREFIX_BYTES).map_err(|error| (AppError::io(path), error.to_string()))?;
    if prefix.is_empty() || meta.len() == 0 {
        return Ok(decorate(
            with_error(
                path,
                "Unknown",
                "Unknown",
                SupportLevel::Unsupported,
                AppError::corrupt_file(path),
            ),
            &prefix,
            revisions,
        ));
    }
    if prefix.starts_with(FIXTURE) {
        return Ok(decorate(
            classify_fixture(path, &prefix),
            &prefix,
            revisions,
        ));
    }
    if meta.len() < MIN_RAW_BYTES {
        return Ok(decorate(
            with_error(
                path,
                "Unknown",
                "Unknown",
                SupportLevel::Unsupported,
                AppError::corrupt_file(path),
            ),
            &prefix,
            revisions,
        ));
    }
    if prefix.starts_with(RAF_MAGIC) {
        return Ok(decorate(
            image(path, "Fujifilm", "Unknown", SupportLevel::Decodes, 0, 0),
            &prefix,
            revisions,
        ));
    }
    if prefix.len() >= 4 && (prefix.starts_with(b"II*\0") || prefix.starts_with(b"MM\0*")) {
        return Ok(decorate(
            image(path, "Unknown", "Unknown", SupportLevel::Decodes, 0, 0),
            &prefix,
            revisions,
        ));
    }
    Ok(decorate(
        with_error(
            path,
            "Unknown",
            "Unknown",
            SupportLevel::Unsupported,
            AppError::corrupt_file(path),
        ),
        &prefix,
        revisions,
    ))
}

fn decorate(
    mut info: ImageInfo,
    prefix: &[u8],
    revisions: &HashMap<String, u32>,
) -> (ImageInfo, Option<String>) {
    if info.exif.captured_at.is_none() {
        info.exif.captured_at = capture_stamp(prefix);
    }
    if info.error.is_some() {
        return (info, None);
    }
    let (sidecar, detail) = sidecar::load(Path::new(&info.path), revisions);
    info.sidecar = sidecar;
    (info, detail)
}

fn classify_fixture(path: &Path, bytes: &[u8]) -> ImageInfo {
    let text = String::from_utf8_lossy(bytes);
    let mut support = "decodes".to_string();
    let mut make = "Test".to_string();
    let mut model = "Body".to_string();
    let mut width = 0u32;
    let mut height = 0u32;
    let mut captured = None;
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
            "captured" => captured = Some(value.to_string()),
            _ => {}
        }
    }
    let mut info = match support.as_str() {
        "corrupt" => with_error(
            path,
            &make,
            &model,
            SupportLevel::Unsupported,
            AppError::corrupt_file(path),
        ),
        "unsupported" => with_error(
            path,
            &make,
            &model,
            SupportLevel::Unsupported,
            AppError::unsupported_camera(path, &make, &model),
        ),
        "calibrated" => image(path, &make, &model, SupportLevel::Calibrated, width, height),
        "decodes" => image(path, &make, &model, SupportLevel::Decodes, width, height),
        _ => with_error(
            path,
            &make,
            &model,
            SupportLevel::Unsupported,
            AppError::corrupt_file(path),
        ),
    };
    info.exif.captured_at = captured;
    info
}

fn with_error(
    path: &Path,
    make: &str,
    model: &str,
    support: SupportLevel,
    error: AppError,
) -> ImageInfo {
    let mut info = image(path, make, model, support, 0, 0);
    info.error = Some(error);
    info
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
        error: None,
    }
}

fn read_prefix(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.by_ref().take(max as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

fn is_raw_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("raf") || ext.eq_ignore_ascii_case("dng"))
}

/// First EXIF capture time in `bytes`, as `YYYY-MM-DDTHH:MM:SS`.
///
/// A TIFF/DNG header is parsed directly. A RAF (and anything else) is
/// scanned for the embedded JPEG `Exif` header. DateTimeOriginal wins over
/// DateTime. This does not decode the mosaic.
pub fn capture_stamp(bytes: &[u8]) -> Option<String> {
    stamp_from_tiff(bytes).or_else(|| {
        let marker = b"Exif\0\0";
        let start = find_bytes(bytes, marker)?;
        stamp_from_tiff(bytes.get(start + marker.len()..)?)
    })
}

struct ExifScan<'a> {
    data: &'a [u8],
    little: bool,
    seen: Vec<usize>,
    original: Option<String>,
    fallback: Option<String>,
}

fn stamp_from_tiff(data: &[u8]) -> Option<String> {
    if data.len() < 8 {
        return None;
    }
    let little = match data.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if read_u16(data, 2, little)? != 42 {
        return None;
    }
    let root = read_u32(data, 4, little)? as usize;
    let mut scan = ExifScan {
        data,
        little,
        seen: Vec::new(),
        original: None,
        fallback: None,
    };
    scan_ifd(&mut scan, root, 0);
    scan.original.or(scan.fallback)
}

fn scan_ifd(scan: &mut ExifScan<'_>, offset: usize, depth: u8) {
    if depth > 4 || scan.original.is_some() || scan.seen.contains(&offset) {
        return;
    }
    scan.seen.push(offset);
    let Some(count) = read_u16(scan.data, offset, scan.little) else {
        return;
    };
    let count = count as usize;
    if count > 256 {
        return;
    }
    let mut nested = Vec::new();
    for index in 0..count {
        let Some(entry) = offset
            .checked_add(2)
            .and_then(|base| base.checked_add(index.saturating_mul(12)))
        else {
            return;
        };
        let Some(value_at) = entry.checked_add(8) else {
            return;
        };
        let Some(tag) = read_u16(scan.data, entry, scan.little) else {
            return;
        };
        let Some(kind) = read_u16(scan.data, entry + 2, scan.little) else {
            return;
        };
        let Some(n) = read_u32(scan.data, entry + 4, scan.little) else {
            return;
        };
        match tag {
            0x0132 if kind == 2 && scan.fallback.is_none() => {
                scan.fallback = ascii_datetime(scan.data, value_at, n as usize, scan.little);
            }
            0x9003 if kind == 2 => {
                scan.original = ascii_datetime(scan.data, value_at, n as usize, scan.little);
            }
            0x8769 | 0x014A if kind == 4 && n >= 1 => {
                if let Some(pointer) = read_u32(scan.data, value_at, scan.little) {
                    nested.push(pointer as usize);
                }
            }
            _ => {}
        }
    }
    for pointer in nested {
        scan_ifd(scan, pointer, depth + 1);
        if scan.original.is_some() {
            return;
        }
    }
}

fn ascii_datetime(data: &[u8], value_field: usize, count: usize, little: bool) -> Option<String> {
    if !(19..=32).contains(&count) {
        return None;
    }
    let bytes = if count <= 4 {
        data.get(value_field..value_field + count)?
    } else {
        let pointer = read_u32(data, value_field, little)? as usize;
        data.get(pointer..pointer.checked_add(count)?)?
    };
    normalize_exif_datetime(bytes)
}

fn normalize_exif_datetime(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes.get(..19)?).ok()?;
    let raw = text.as_bytes();
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let seps = [(4, b':'), (7, b':'), (10, b' '), (13, b':'), (16, b':')];
    if !digits.iter().all(|index| raw[*index].is_ascii_digit())
        || !seps
            .iter()
            .all(|(index, expected)| raw[*index] == *expected)
    {
        return None;
    }
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: u32 = text[11..13].parse().ok()?;
    let minute: u32 = text[14..16].parse().ok()?;
    let second: u32 = text[17..19].parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}",
        &text[0..4],
        &text[5..7],
        &text[8..10],
        &text[11..19]
    ))
}

fn read_u16(data: &[u8], offset: usize, little: bool) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let bytes: [u8; 2] = data.get(offset..end)?.try_into().ok()?;
    Some(if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(data: &[u8], offset: usize, little: bool) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let bytes: [u8; 4] = data.get(offset..end)?.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

    fn exif_datetime(stamp: &str) -> Vec<u8> {
        let mut data = vec![
            b'I', b'I', 42, 0, // little-endian TIFF
            8, 0, 0, 0, // IFD at 8
            1, 0, // one entry
            0x03, 0x90, // DateTimeOriginal
            2, 0, // ASCII
            20, 0, 0, 0, // count, including NUL
            26, 0, 0, 0, // string offset
            0, 0, 0, 0, // next IFD
        ];
        data.extend(stamp.as_bytes());
        data.push(0);
        data
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
        assert_eq!(outcome.images.len(), 3);
        let body = outcome
            .images
            .iter()
            .find(|image| image.file_name == "body.RAF")
            .unwrap();
        assert_eq!(body.camera.make, "Fujifilm");
        assert_eq!(body.support, SupportLevel::Decodes);
        assert!(body.error.is_none());
        assert_eq!(body.sidecar.status, crate::contract::SidecarStatus::None);
        let canon = outcome
            .images
            .iter()
            .find(|image| image.file_name == "canon.raf")
            .unwrap();
        let canon_error = canon.error.as_ref().unwrap();
        assert_eq!(canon_error.code, ErrorCode::UnsupportedCamera);
        assert!(canon_error.message.contains("Canon"));
        assert!(canon_error.message.contains("EOS 5D Mark IV"));
        assert!(canon_error.is_user_facing());
        let empty = outcome
            .images
            .iter()
            .find(|image| image.file_name == "empty.dng")
            .unwrap();
        assert_eq!(empty.error.as_ref().unwrap().code, ErrorCode::CorruptFile);
        let codes: Vec<_> = outcome
            .rejected
            .iter()
            .map(|item| item.error.code)
            .collect();
        assert!(codes.contains(&ErrorCode::UnsupportedFormat));
        assert!(codes.contains(&ErrorCode::Io));
        assert!(!codes.contains(&ErrorCode::UnsupportedCamera));
        assert!(!codes.contains(&ErrorCode::CorruptFile));
    }

    #[test]
    fn folders_expand_one_level_and_keep_bad_files_in_capture_order() {
        let dir = TempDir::new();
        write(
            dir.path(),
            "later.dng",
            b"RAWSTOCK-FIXTURE\nsupport: calibrated\nmake: Apple\nmodel: iPhone\nwidth: 8\nheight: 6\ncaptured: 2021-06-01T12:00:00\n",
        );
        write(
            dir.path(),
            "early.RAF",
            b"RAWSTOCK-FIXTURE\nsupport: decodes\nmake: Fujifilm\nmodel: X-T2\ncaptured: 2020-01-02T03:04:05\n",
        );
        write(
            dir.path(),
            "broken.dng",
            b"RAWSTOCK-FIXTURE\nsupport: corrupt\ncaptured: 2019-01-01T00:00:00\n",
        );
        write(
            dir.path(),
            "canon.raf",
            b"RAWSTOCK-FIXTURE\nsupport: unsupported\nmake: Canon\nmodel: EOS R5\n",
        );
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        write(
            dir.path().join("nested").as_path(),
            "hidden.raf",
            b"RAWSTOCK-FIXTURE\nsupport: decodes\n",
        );
        write(dir.path(), "notes.txt", b"skip me");
        let (outcome, _) = classify_inputs(&[dir.path().display().to_string()]);
        let names: Vec<_> = outcome
            .images
            .iter()
            .map(|image| image.file_name.as_str())
            .collect();
        assert_eq!(names, ["broken.dng", "early.RAF", "later.dng", "canon.raf"]);
        assert!(outcome.rejected.is_empty());
        assert_eq!(
            outcome.images[0].error.as_ref().unwrap().code,
            ErrorCode::CorruptFile
        );
        assert!(
            outcome.images[0]
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("broken.dng")
        );
        assert_eq!(
            outcome.images[3].error.as_ref().unwrap().code,
            ErrorCode::UnsupportedCamera
        );
        assert!(
            outcome.images[3]
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("EOS R5")
        );
        assert_eq!(outcome.images[2].width, 8);
        assert_eq!(outcome.images[1].support, SupportLevel::Decodes);
        assert!(outcome.images[1].error.is_none());
    }

    #[test]
    fn exif_datetime_original_is_capture_order_for_a_real_header() {
        let dir = TempDir::new();
        let mut early = b"FUJIFILMCCD-RAW ".to_vec();
        early.extend(b"preview");
        early.extend(b"Exif\0\0");
        early.extend(exif_datetime("2018:03:04 05:06:07"));
        early.resize(4_096, 0);
        write(dir.path(), "early.RAF", &early);
        let mut late = exif_datetime("2019:08:09 10:11:12");
        late.resize(4_096, 0);
        write(dir.path(), "late.dng", &late);
        write(dir.path(), "nodate.dng", &{
            let mut bytes = b"II*\0not-exif".to_vec();
            bytes.resize(4_096, 0);
            bytes
        });
        let (outcome, _) = classify_inputs(&[dir.path().display().to_string()]);
        let names: Vec<_> = outcome
            .images
            .iter()
            .map(|image| image.file_name.as_str())
            .collect();
        assert_eq!(names, ["early.RAF", "late.dng", "nodate.dng"]);
        assert_eq!(
            outcome.images[0].exif.captured_at.as_deref(),
            Some("2018-03-04T05:06:07")
        );
        assert_eq!(
            outcome.images[1].exif.captured_at.as_deref(),
            Some("2019-08-09T10:11:12")
        );
        assert!(outcome.images[2].exif.captured_at.is_none());
    }

    #[test]
    fn capture_stamp_reads_datetime_original_from_tiff_and_exif() {
        let tiff = exif_datetime("2020:01:02 03:04:05");
        assert_eq!(capture_stamp(&tiff).as_deref(), Some("2020-01-02T03:04:05"));
        let mut wrapped = b"junk".to_vec();
        wrapped.extend(b"Exif\0\0");
        wrapped.extend(&tiff);
        assert_eq!(
            capture_stamp(&wrapped).as_deref(),
            Some("2020-01-02T03:04:05")
        );
        assert!(capture_stamp(b"no date here").is_none());
        assert!(capture_stamp(&exif_datetime("2020:13:02 03:04:05")).is_none());
    }
}
