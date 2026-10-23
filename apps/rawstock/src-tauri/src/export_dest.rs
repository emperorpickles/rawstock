//! Where a JPEG export is allowed to go.
//!
//! The RAW and its sidecar are never the destination. An existing file needs
//! `overwrite`, which the dialog sets only after the user confirms. Cancelling
//! deletes the partial file beside the destination and does not rename or
//! remove anything else.
//!
//! The same rules and sentences are in `apps/rawstock/src/lib/export.ts`.

use std::path::{Path, PathBuf};

use crate::contract::{file_label, sidecar_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestKind {
    Missing,
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportAction {
    Export,
    Confirm,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub path: String,
    pub action: ExportAction,
    pub message: String,
}

pub fn kind_of(path: &Path) -> DestKind {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => DestKind::Directory,
        Ok(_) => DestKind::File,
        Err(_) => DestKind::Missing,
    }
}

/// `<stem>.jpg` from the RAW's file name. The dialog builds the path the
/// save sheet opens on; this is the same rule, checked without the UI.
#[cfg(test)]
pub fn jpeg_name(raw: &Path) -> String {
    let base = file_label(raw);
    let stem = match base.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => base.as_str(),
    };
    let stem = if stem.is_empty() { "photo" } else { stem };
    format!("{stem}.jpg")
}

/// Last folder when it is set, otherwise the RAW's folder.
#[cfg(test)]
pub fn default_path(raw: &Path, last_folder: Option<&str>) -> PathBuf {
    let name = jpeg_name(raw);
    let folder = last_folder
        .map(str::trim)
        .filter(|folder| !folder.is_empty());
    match folder {
        Some(folder) => PathBuf::from(folder).join(name),
        None => raw.parent().unwrap_or_else(|| Path::new("")).join(name),
    }
}

/// `kind` describes the path after a missing JPEG extension is added.
pub fn review(raw: &Path, destination: &str, kind: DestKind) -> Review {
    let trimmed = destination.trim();
    if trimmed.is_empty() || file_name(trimmed).is_empty() {
        return Review {
            path: trimmed.to_string(),
            action: ExportAction::Refuse,
            message: "Choose a destination for the export.".to_string(),
        };
    }
    let sidecar = sidecar_path(raw);
    if matches_protected(raw, trimmed) {
        return refuse_raw(trimmed, raw);
    }
    if matches_protected(&sidecar, trimmed) {
        return refuse_sidecar(trimmed, &sidecar);
    }
    let path = ensure_jpeg(trimmed);
    if matches_protected(raw, &path) {
        return refuse_raw(&path, raw);
    }
    if matches_protected(&sidecar, &path) {
        return refuse_sidecar(&path, &sidecar);
    }
    if kind == DestKind::Directory {
        return Review {
            path,
            action: ExportAction::Refuse,
            message: "Choose a file. That destination is a folder.".to_string(),
        };
    }
    if kind == DestKind::File {
        let name = file_name(&path).to_string();
        return Review {
            path,
            action: ExportAction::Confirm,
            message: format!("{name} already exists. Replace it? The RAW photo is not changed."),
        };
    }
    Review {
        path,
        action: ExportAction::Export,
        message: String::new(),
    }
}

/// Stat `destination`, then apply [`review`].
pub fn review_path(raw: &Path, destination: &str) -> Review {
    let first = review(raw, destination, DestKind::Missing);
    if first.action == ExportAction::Refuse {
        return first;
    }
    let kind = kind_of(Path::new(&first.path));
    if kind == DestKind::Missing {
        return first;
    }
    review(raw, destination, kind)
}

pub fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

/// Delete the partial beside `destination`. The destination, the RAW, and
/// the sidecar are left as they are.
pub fn remove_partial(raw: Option<&Path>, destination: &Path) {
    let partial = partial_path(destination);
    if same_path(destination, &partial) {
        return;
    }
    if let Some(raw) = raw {
        let sidecar = sidecar_path(raw);
        if same_path(raw, &partial) || same_path(&sidecar, &partial) {
            return;
        }
    }
    let _ = std::fs::remove_file(partial);
}

fn refuse_raw(path: &str, raw: &Path) -> Review {
    Review {
        path: path.to_string(),
        action: ExportAction::Refuse,
        message: format!(
            "Choose a different file. Export cannot replace {}, the RAW photo.",
            file_label(raw)
        ),
    }
}

fn refuse_sidecar(path: &str, sidecar: &Path) -> Review {
    Review {
        path: path.to_string(),
        action: ExportAction::Refuse,
        message: format!(
            "Choose a different file. Export cannot replace {}, the edit saved beside the photo.",
            file_label(sidecar)
        ),
    }
}

fn ensure_jpeg(path: &str) -> String {
    let base = file_name(path);
    let ext = base.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
        return path.to_string();
    }
    format!("{path}.jpg")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn matches_protected(protected: &Path, destination: &str) -> bool {
    same_path(protected, Path::new(destination.trim()))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if identity(&left.to_string_lossy()) == identity(&right.to_string_lossy()) {
        return true;
    }
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => {
            identity(&left.to_string_lossy()) == identity(&right.to_string_lossy())
        }
        _ => false,
    }
}

fn identity(path: &str) -> String {
    let text = path.trim().replace('\\', "/");
    let absolute = text.starts_with('/');
    let mut parts = Vec::new();
    for part in text.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
            continue;
        }
        parts.push(part.to_lowercase());
    }
    let mut out = String::new();
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rawstock-export-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_name_is_the_stem_as_jpeg() {
        let raw = Path::new("/photos/DSCF0001.RAF");
        assert_eq!(jpeg_name(raw), "DSCF0001.jpg");
        assert_eq!(
            default_path(raw, None),
            PathBuf::from("/photos/DSCF0001.jpg")
        );
        assert_eq!(
            default_path(raw, Some("/exports")),
            PathBuf::from("/exports/DSCF0001.jpg")
        );
        assert_eq!(
            default_path(Path::new("/photos/My.Photo.RAF"), None),
            PathBuf::from("/photos/My.Photo.jpg")
        );
    }

    #[test]
    fn raw_and_sidecar_are_refused_and_an_existing_jpeg_needs_confirmation() {
        let raw = Path::new("/photos/DSCF0001.RAF");
        let jpeg = "/photos/DSCF0001.jpg";
        let ready = review(raw, jpeg, DestKind::Missing);
        assert_eq!(ready.action, ExportAction::Export);
        assert_eq!(ready.path, jpeg);

        let replace = review(raw, jpeg, DestKind::File);
        assert_eq!(replace.action, ExportAction::Confirm);
        assert_eq!(
            replace.message,
            "DSCF0001.jpg already exists. Replace it? The RAW photo is not changed."
        );

        let onto_raw = review(raw, "/photos/DSCF0001.RAF", DestKind::File);
        assert_eq!(onto_raw.action, ExportAction::Refuse);
        assert!(onto_raw.message.contains("DSCF0001.RAF"));
        assert!(onto_raw.message.contains("RAW photo"));

        let folded = review(raw, "\\photos\\dscf0001.raf", DestKind::File);
        assert_eq!(folded.action, ExportAction::Refuse);

        let dotted = review(raw, "/photos/roll/../DSCF0001.RAF", DestKind::Missing);
        assert_eq!(dotted.action, ExportAction::Refuse);

        let sidecar = review(raw, "/photos/DSCF0001.RAF.rawstock.toml", DestKind::Missing);
        assert_eq!(sidecar.action, ExportAction::Refuse);
        assert!(sidecar.message.contains("edit saved beside the photo"));

        let elsewhere = review(raw, "/other/DSCF0001.RAF", DestKind::Missing);
        assert_eq!(elsewhere.action, ExportAction::Export);
        assert_eq!(elsewhere.path, "/other/DSCF0001.RAF.jpg");

        let added = review(raw, "/photos/DSCF0001", DestKind::Missing);
        assert_eq!(added.path, "/photos/DSCF0001.jpg");

        let kept = review(raw, "/photos/DSCF0001.JPEG", DestKind::File);
        assert_eq!(kept.action, ExportAction::Confirm);
        assert_eq!(kept.path, "/photos/DSCF0001.JPEG");

        assert_eq!(
            review(raw, "  ", DestKind::Missing).action,
            ExportAction::Refuse
        );
        assert_eq!(
            review(raw, "/photos/out", DestKind::Directory).action,
            ExportAction::Refuse
        );
    }

    #[test]
    fn review_path_stats_the_jpeg_and_cancel_removes_only_the_partial() {
        let dir = temp_dir("dest");
        let raw = dir.join("frame.RAF");
        let sidecar = sidecar_path(&raw);
        let jpeg = dir.join("frame.jpg");
        std::fs::write(&raw, b"raw-bytes").unwrap();
        std::fs::write(&sidecar, b"side").unwrap();
        std::fs::write(&jpeg, b"old-jpeg").unwrap();

        let refused = review_path(&raw, &raw.display().to_string());
        assert_eq!(refused.action, ExportAction::Refuse);
        let sidecar_refused = review_path(&raw, &sidecar.display().to_string());
        assert_eq!(sidecar_refused.action, ExportAction::Refuse);
        let replace = review_path(&raw, &jpeg.display().to_string());
        assert_eq!(replace.action, ExportAction::Confirm);

        let partial = partial_path(&jpeg);
        std::fs::write(&partial, b"partial").unwrap();
        remove_partial(Some(&raw), &jpeg);
        assert!(!partial.exists());
        assert_eq!(std::fs::read(&jpeg).unwrap(), b"old-jpeg");
        assert_eq!(std::fs::read(&raw).unwrap(), b"raw-bytes");
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"side");
        let _ = std::fs::remove_dir_all(dir);
    }
}
