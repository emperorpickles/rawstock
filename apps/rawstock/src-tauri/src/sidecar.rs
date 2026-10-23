//! `<file>.rawstock.toml` beside a RAW photo.
//!
//! The RAW is only ever opened read-only, by the classifier. This module
//! writes the sidecar, or renames a broken one to `.bak`. It never creates,
//! renames, or touches the RAW.

use std::collections::HashMap;
use std::io::{Error as IoError, Write};
use std::path::{Path, PathBuf};

use crate::contract::{
    AppError, EditSettings, SaveResult, SidecarDocument, SidecarInfo, SidecarStatus, file_label,
    parse_sidecar, sidecar_path, sidecar_status, utc_now,
};
use crate::stocks;

/// A refusal or a disk error. `error` is safe to show. `detail` is for the log.
#[derive(Debug)]
pub struct Failure {
    pub error: AppError,
    pub detail: String,
}

/// Installed stock revisions, keyed by stock id. Empty when the catalog
/// cannot be read; a sidecar still opens, and the UI can say the stock is
/// missing.
pub fn revision_map() -> HashMap<String, u32> {
    stocks::list()
        .map(|stocks| {
            stocks
                .into_iter()
                .map(|stock| (stock.id, stock.revision))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the sidecar next to `raw`. A missing file is [`SidecarStatus::None`].
/// A file that does not parse is [`SidecarStatus::Invalid`] and is not
/// modified. The second value is a log line, when there is one.
pub fn load(raw: &Path, revisions: &HashMap<String, u32>) -> (SidecarInfo, Option<String>) {
    let dest = sidecar_path(raw);
    if !dest.is_file() {
        return (SidecarInfo::none(), None);
    }
    let text = match std::fs::read_to_string(&dest) {
        Ok(text) => text,
        Err(error) => {
            return (
                invalid_info(),
                Some(format!("sidecarInvalid {} {error}", dest.display())),
            );
        }
    };
    let document = match parse_sidecar(&text) {
        Ok(document) => document,
        Err(error) => {
            return (
                invalid_info(),
                Some(format!(
                    "sidecarInvalid {} {}",
                    dest.display(),
                    error.message
                )),
            );
        }
    };
    let installed = revisions.get(&document.settings.stock.id).copied();
    let detail = if document.settings.stock.id.is_empty() {
        Some(format!("sidecarInvalid {} empty stock id", dest.display()))
    } else if installed.is_none() {
        Some(format!(
            "stockMissing {} {}",
            dest.display(),
            document.settings.stock.id
        ))
    } else {
        None
    };
    (
        SidecarInfo {
            status: sidecar_status(&document.settings, installed),
            saved_revision: Some(document.settings.stock.revision),
            installed_revision: installed,
            settings: Some(document.settings),
        },
        detail,
    )
}

/// Status after a successful write of `settings`.
pub fn describe(settings: &EditSettings, revisions: &HashMap<String, u32>) -> SidecarInfo {
    let installed = revisions.get(&settings.stock.id).copied();
    SidecarInfo {
        status: sidecar_status(settings, installed),
        saved_revision: Some(settings.stock.revision),
        installed_revision: installed,
        settings: Some(settings.clone()),
    }
}

/// Atomic write of `settings` beside `raw`. An existing sidecar that does
/// not parse is left byte-for-byte alone.
pub fn write(raw: &Path, settings: &EditSettings) -> Result<SaveResult, Failure> {
    if let Err(error) = settings.validate() {
        return Err(Failure {
            detail: format!("sidecar refused {}: {}", raw.display(), error.message),
            error,
        });
    }
    if settings.stock.id.is_empty() {
        let error = AppError::internal(
            "Choose a stock before saving this edit. The photo was not changed.",
        );
        return Err(Failure {
            detail: format!("sidecar refused {}: empty stock id", raw.display()),
            error,
        });
    }
    let dest = sidecar_path(raw);
    if !is_sidecar(&dest) || same_file(&dest, raw) {
        let error = AppError::internal(
            "The edit could not be saved beside the photo. The photo was not changed.",
        );
        return Err(Failure {
            detail: format!("refusing to write {}", dest.display()),
            error,
        });
    }
    if dest.exists() {
        match std::fs::read_to_string(&dest) {
            Ok(text) if parse_sidecar(&text).is_err() => {
                let error = AppError::sidecar_invalid(format!(
                    "{} has an edit file that could not be read, so rawstock did not replace it. Reset edits moves that file aside. The photo was not changed.",
                    file_label(raw)
                ));
                return Err(Failure {
                    detail: format!("refusing to overwrite unreadable {}", dest.display()),
                    error,
                });
            }
            Ok(_) => {}
            Err(error) => {
                return Err(io_failure(&dest, error));
            }
        }
    }
    let saved_at = utc_now();
    let document = SidecarDocument {
        rawstock_version: env!("CARGO_PKG_VERSION").to_string(),
        saved_at: saved_at.clone(),
        settings: settings.clone(),
    };
    let text = toml::to_string_pretty(&document).map_err(|error| {
        let app = AppError::internal("The edit could not be saved. The photo was not changed.");
        Failure {
            detail: format!("sidecar encode {}: {error}", dest.display()),
            error: app,
        }
    })?;
    atomic_write(&dest, text.as_bytes()).map_err(|error| io_failure(&dest, error))?;
    Ok(SaveResult {
        path: dest.display().to_string(),
        saved_at,
    })
}

/// Rename the sidecar to `.bak` (or `.bak-2`, …). The RAW stays put.
/// A missing sidecar is a no-op.
pub fn reset(raw: &Path) -> Result<PathBuf, Failure> {
    let dest = sidecar_path(raw);
    if !dest.is_file() {
        return Ok(dest);
    }
    let bak = backup_path(&dest)?;
    std::fs::rename(&dest, &bak).map_err(|error| Failure {
        error: AppError::io(&dest),
        detail: format!(
            "reset sidecar {} -> {}: {error}",
            dest.display(),
            bak.display()
        ),
    })?;
    Ok(bak)
}

fn invalid_info() -> SidecarInfo {
    SidecarInfo {
        status: SidecarStatus::Invalid,
        settings: None,
        saved_revision: None,
        installed_revision: None,
    }
}

fn is_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".rawstock.toml"))
}

fn same_file(left: &Path, right: &Path) -> bool {
    crate::contract::canonical_path(left) == crate::contract::canonical_path(right)
}

fn io_failure(path: &Path, error: IoError) -> Failure {
    Failure {
        error: AppError::io(path),
        detail: format!("sidecar {} {error}", path.display()),
    }
}

fn backup_path(dest: &Path) -> Result<PathBuf, Failure> {
    let first = push_suffix(dest, ".bak");
    if !first.exists() {
        return Ok(first);
    }
    for index in 2..1000 {
        let path = push_suffix(dest, &format!(".bak-{index}"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(Failure {
        error: AppError::io(dest),
        detail: format!("no free .bak name for {}", dest.display()),
    })
}

fn push_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn atomic_write(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = push_suffix(dest, ".tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(error) = rename_over(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    if !to.exists() {
        return std::fs::rename(from, to);
    }
    let aside = push_suffix(to, ".replacing");
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(to, &aside)?;
    if let Err(error) = std::fs::rename(from, to) {
        let _ = std::fs::rename(&aside, to);
        return Err(error);
    }
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

#[cfg(not(windows))]
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{WhiteBalance, settings_hash};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;

    static TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = TEMP.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rawstock-sidecar-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fingerprint(path: &Path) -> (SystemTime, String) {
        let meta = std::fs::metadata(path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        (meta.modified().unwrap(), sha256(&bytes))
    }

    fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn write_raw(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn sample_settings(revision: u32) -> EditSettings {
        EditSettings {
            stock: crate::contract::StockRef {
                id: "velvia".to_string(),
                revision,
            },
            exposure_ev: 0.25,
            white_balance: WhiteBalance::Custom {
                temperature_k: 5200.0,
                tint: 1.5,
            },
            ..EditSettings::default()
        }
    }

    #[test]
    fn a_session_of_edits_does_not_change_the_raw_and_reopens_the_same_hash() {
        let dir = TempDir::new();
        let raw_bytes = b"RAWSTOCK-FIXTURE\nsupport: calibrated\nmake: Fujifilm\nmodel: X-T2\n";
        let raw = write_raw(dir.0.as_path(), "frame.RAF", raw_bytes);
        let before = fingerprint(&raw);
        let revisions = revision_map();
        let installed = *revisions.get("velvia").expect("velvia stock");
        let settings = sample_settings(installed);

        let saved = write(&raw, &settings).unwrap();
        assert!(saved.path.ends_with("frame.RAF.rawstock.toml"));
        assert!(!Path::new(&format!("{}.tmp", saved.path)).exists());
        assert_eq!(fingerprint(&raw), before);
        assert_eq!(std::fs::read(&raw).unwrap(), raw_bytes);

        let (loaded, detail) = load(&raw, &revisions);
        assert!(detail.is_none(), "{detail:?}");
        assert_eq!(loaded.status, SidecarStatus::Loaded);
        let loaded_settings = loaded.settings.expect("settings");
        assert_eq!(settings_hash(&loaded_settings), settings_hash(&settings));
        assert_eq!(
            loaded_settings.white_balance,
            WhiteBalance::Custom {
                temperature_k: 5200.0,
                tint: 1.5,
            }
        );

        let again = sample_settings(installed);
        let mut edited = again;
        edited.exposure_ev = -1.0;
        write(&raw, &edited).unwrap();
        let (reopened, _) = load(&raw, &revisions);
        assert_eq!(
            settings_hash(&reopened.settings.unwrap()),
            settings_hash(&edited)
        );
        assert_eq!(fingerprint(&raw), before);
        assert_eq!(std::fs::read(&raw).unwrap(), raw_bytes);

        let (outcome, _) = crate::files::classify_inputs(&[raw.display().to_string()]);
        assert_eq!(outcome.images.len(), 1);
        assert!(outcome.rejected.is_empty());
        let reopened = outcome.images[0].sidecar.settings.clone().unwrap();
        assert_eq!(settings_hash(&reopened), settings_hash(&edited));
        assert_eq!(fingerprint(&raw), before);
    }

    #[test]
    fn an_older_sidecar_loads_defaults_and_reports_a_revision_mismatch() {
        let dir = TempDir::new();
        let raw = write_raw(
            dir.0.as_path(),
            "old.dng",
            b"RAWSTOCK-FIXTURE\nsupport: decodes\n",
        );
        let revisions = revision_map();
        let installed = *revisions.get("velvia").expect("velvia");
        let saved_revision = installed + 1;
        let sidecar = sidecar_path(&raw);
        std::fs::write(
            &sidecar,
            format!(
                r#"
rawstockVersion = "0.0.1"
savedAt = "2026-01-01T00:00:00Z"
exposureEv = 0.25

[stock]
id = "velvia"
revision = {saved_revision}

[whiteBalance]
mode = "custom"
temperatureK = 5200.0
tint = 1.5
"#
            ),
        )
        .unwrap();
        let before = std::fs::read(&sidecar).unwrap();
        let (loaded, _) = load(&raw, &revisions);
        assert_eq!(loaded.status, SidecarStatus::StockVersionMismatch);
        assert_eq!(loaded.saved_revision, Some(saved_revision));
        assert_eq!(loaded.installed_revision, Some(installed));
        let settings = loaded.settings.expect("settings");
        assert_eq!(settings.grain.amount, 1.0);
        assert_eq!(settings.grain.size, 1.0);
        assert!(settings.halation.threshold.is_none());
        assert!(!settings.halation.enabled);
        assert!(settings.lens.ca_correction);
        let hash = settings_hash(&settings);
        write(&raw, &settings).unwrap();
        let (again, _) = load(&raw, &revisions);
        assert_eq!(settings_hash(&again.settings.unwrap()), hash);
        assert_eq!(
            fingerprint(&raw).1,
            sha256(b"RAWSTOCK-FIXTURE\nsupport: decodes\n")
        );
        assert_ne!(std::fs::read(&sidecar).unwrap(), before);
    }

    #[test]
    fn a_malformed_sidecar_is_never_overwritten_until_reset() {
        let dir = TempDir::new();
        let raw_bytes = b"RAWSTOCK-FIXTURE\nsupport: decodes\nmake: Fujifilm\nmodel: X-T2\n";
        let raw = write_raw(dir.0.as_path(), "bad.RAF", raw_bytes);
        let sidecar = sidecar_path(&raw);
        let garbage = b"this is not a sidecar {{{";
        std::fs::write(&sidecar, garbage).unwrap();
        let before_raw = fingerprint(&raw);
        let before_side = std::fs::read(&sidecar).unwrap();

        let revisions = revision_map();
        let (loaded, detail) = load(&raw, &revisions);
        assert_eq!(loaded.status, SidecarStatus::Invalid);
        assert!(loaded.settings.is_none());
        assert!(detail.unwrap().contains("sidecarInvalid"));

        let installed = *revisions.get("portra400").unwrap_or(&3);
        let refused = write(&raw, &sample_settings(installed)).unwrap_err();
        assert_eq!(
            refused.error.code,
            crate::contract::ErrorCode::SidecarInvalid
        );
        assert!(refused.error.message.contains("did not replace"));
        assert_eq!(std::fs::read(&sidecar).unwrap(), before_side);
        assert_eq!(fingerprint(&raw), before_raw);

        let bak = reset(&raw).unwrap();
        assert!(bak.ends_with("bad.RAF.rawstock.toml.bak"));
        assert!(!sidecar.exists());
        assert_eq!(std::fs::read(&bak).unwrap(), garbage);
        assert_eq!(fingerprint(&raw), before_raw);
        assert_eq!(std::fs::read(&raw).unwrap(), raw_bytes);

        let saved = write(&raw, &sample_settings(installed)).unwrap();
        assert!(Path::new(&saved.path).is_file());
        assert_eq!(fingerprint(&raw), before_raw);

        std::fs::write(&sidecar, garbage).unwrap();
        let bak_again = reset(&raw).unwrap();
        assert!(
            bak_again
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".bak-2")
        );
        assert_eq!(fingerprint(&raw), before_raw);
    }
}
