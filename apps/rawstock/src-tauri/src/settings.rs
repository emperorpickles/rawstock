//! Per-user app settings in `settings.json`, next to `recent.json` in the app
//! config directory. Preferences only: photos, sidecars, and exports are
//! never read or written here.
//!
//! Missing keys fall back to defaults. Unknown keys, including whole
//! namespaces a later version adds (`export.*`), are kept on save, so
//! another issue can add its own keys without a migration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::contract::{AppError, ErrorCode};

pub const VERSION: u32 = 1;
pub const RECENT_MIN: usize = 5;
pub const RECENT_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Surround {
    #[default]
    Dark,
    Mid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReduceMotion {
    #[default]
    System,
    On,
    Off,
}

/// Warm is the brass palette. Neutral is the grey Darkroom palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    Warm,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Appearance {
    pub surround: Surround,
    pub reduce_motion: ReduceMotion,
    pub theme: Theme,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct NewImage {
    /// `None` starts new images on the first stock `list_stocks` returns.
    pub default_stock_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecentPrefs {
    pub limit: usize,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for RecentPrefs {
    fn default() -> Self {
        Self {
            limit: crate::recent::LIMIT,
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Onboarding {
    pub completed: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub version: u32,
    pub appearance: Appearance,
    pub new_image: NewImage,
    pub recent: RecentPrefs,
    pub onboarding: Onboarding,
    /// Namespaces this version does not know. Kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: VERSION,
            appearance: Appearance::default(),
            new_image: NewImage::default(),
            recent: RecentPrefs::default(),
            onboarding: Onboarding::default(),
            extra: Map::new(),
        }
    }
}

impl AppSettings {
    /// Rejects values the UI should never send. Unknown keys pass.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.version != VERSION {
            return Err(invalid(format!(
                "Settings version {} is not supported.",
                self.version
            )));
        }
        if !(RECENT_MIN..=RECENT_MAX).contains(&self.recent.limit) {
            return Err(invalid(format!(
                "Recent files must be between {RECENT_MIN} and {RECENT_MAX}."
            )));
        }
        if let Some(id) = &self.new_image.default_stock_id
            && id.trim().is_empty()
        {
            return Err(invalid("The default stock id is empty.".to_string()));
        }
        Ok(())
    }
}

/// Payload of `settings:changed`, so every open window can update.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsChanged {
    pub settings: AppSettings,
}

/// What [`SettingsStore::load`] found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadNote {
    Loaded,
    Missing,
    /// The file could not be parsed or validated. It was renamed to
    /// `settings.json.bak` and the store started on defaults.
    Corrupt {
        reason: String,
        backup: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    file: PathBuf,
    settings: AppSettings,
}

impl SettingsStore {
    pub fn load(file: PathBuf) -> (Self, LoadNote) {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(_) => {
                let store = Self {
                    file,
                    settings: AppSettings::default(),
                };
                return (store, LoadNote::Missing);
            }
        };
        let parsed = serde_json::from_str::<AppSettings>(&text)
            .map_err(|error| error.to_string())
            .and_then(|settings| {
                settings
                    .validate()
                    .map(|()| settings)
                    .map_err(|error| error.message)
            });
        match parsed {
            Ok(settings) => (Self { file, settings }, LoadNote::Loaded),
            Err(reason) => {
                let backup = file.with_extension("json.bak");
                let _ = std::fs::rename(&file, &backup);
                let store = Self {
                    file,
                    settings: AppSettings::default(),
                };
                (store, LoadNote::Corrupt { reason, backup })
            }
        }
    }

    pub fn get(&self) -> &AppSettings {
        &self.settings
    }

    /// Validate, then save atomically. The old value stays on any error.
    pub fn set(&mut self, settings: AppSettings) -> Result<(), AppError> {
        settings.validate()?;
        write_atomic(&self.file, &settings).map_err(|error| io_error(&self.file, error))?;
        self.settings = settings;
        Ok(())
    }

    /// Restore every default. Only `settings.json` is written.
    pub fn reset(&mut self) -> Result<(), AppError> {
        self.set(AppSettings::default())
    }
}

fn write_atomic(file: &Path, settings: &AppSettings) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

fn invalid(message: String) -> AppError {
    AppError {
        code: ErrorCode::Internal,
        message,
        path: None,
    }
}

fn io_error(file: &Path, error: std::io::Error) -> AppError {
    AppError {
        code: ErrorCode::Io,
        message: format!("Settings could not be saved: {error}."),
        path: Some(file.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rawstock-settings-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_and_missing_keys_fall_back_to_defaults() {
        let dir = temp_dir("defaults");
        let (store, note) = SettingsStore::load(dir.join("settings.json"));
        assert_eq!(note, LoadNote::Missing);
        assert_eq!(store.get(), &AppSettings::default());

        std::fs::write(
            dir.join("settings.json"),
            r#"{ "version": 1, "appearance": { "surround": "mid" } }"#,
        )
        .unwrap();
        let (store, note) = SettingsStore::load(dir.join("settings.json"));
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(store.get().appearance.surround, Surround::Mid);
        assert_eq!(store.get().appearance.reduce_motion, ReduceMotion::System);
        assert_eq!(store.get().appearance.theme, Theme::Warm);
        assert_eq!(store.get().recent.limit, 10);
        assert!(!store.get().onboarding.completed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_is_moved_to_bak_and_defaults_load() {
        let dir = temp_dir("corrupt");
        let file = dir.join("settings.json");
        std::fs::write(&file, "{ not json").unwrap();
        let (store, note) = SettingsStore::load(file.clone());
        let LoadNote::Corrupt { backup, .. } = note else {
            panic!("expected a corrupt note, got {note:?}");
        };
        assert_eq!(backup, dir.join("settings.json.bak"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{ not json");
        assert!(!file.exists());
        assert_eq!(store.get(), &AppSettings::default());

        std::fs::write(&file, r#"{ "version": 1, "recent": { "limit": 99 } }"#).unwrap();
        let (_, note) = SettingsStore::load(file);
        assert!(matches!(note, LoadNote::Corrupt { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = temp_dir("round-trip");
        let file = dir.join("settings.json");
        let (mut store, _) = SettingsStore::load(file.clone());
        let mut next = store.get().clone();
        next.appearance.surround = Surround::Mid;
        next.appearance.reduce_motion = ReduceMotion::On;
        next.appearance.theme = Theme::Neutral;
        next.new_image.default_stock_id = Some("velvia".to_string());
        next.recent.limit = 15;
        next.onboarding.completed = true;
        store.set(next.clone()).unwrap();
        assert!(!dir.join("settings.json.tmp").exists());

        let (loaded, note) = SettingsStore::load(file.clone());
        assert_eq!(note, LoadNote::Loaded);
        assert_eq!(loaded.get(), &next);
        let text = std::fs::read_to_string(file).unwrap();
        assert!(text.contains("\"reduceMotion\": \"on\""));
        assert!(text.contains("\"theme\": \"neutral\""));
        assert!(text.contains("\"defaultStockId\": \"velvia\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_future_keys_are_preserved() {
        let dir = temp_dir("future");
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            r#"{
              "version": 1,
              "export": { "quality": 92, "lastFolder": "/tmp/out" },
              "appearance": { "surround": "dark", "accent": "grey" }
            }"#,
        )
        .unwrap();
        let (mut store, note) = SettingsStore::load(file.clone());
        assert_eq!(note, LoadNote::Loaded);
        let mut next = store.get().clone();
        next.recent.limit = 7;
        store.set(next).unwrap();

        let saved: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(saved["export"]["quality"], 92);
        assert_eq!(saved["export"]["lastFolder"], "/tmp/out");
        assert_eq!(saved["appearance"]["accent"], "grey");
        assert_eq!(saved["recent"]["limit"], 7);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_values_are_rejected_and_nothing_is_written() {
        let dir = temp_dir("invalid");
        let file = dir.join("settings.json");
        let (mut store, _) = SettingsStore::load(file.clone());
        let mut next = store.get().clone();
        next.recent.limit = RECENT_MAX + 1;
        assert!(store.set(next).is_err());
        let mut next = store.get().clone();
        next.new_image.default_stock_id = Some(" ".to_string());
        assert!(store.set(next).is_err());
        assert!(!file.exists());
        assert_eq!(store.get(), &AppSettings::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reset_restores_defaults_and_leaves_photos_and_sidecars_alone() {
        let dir = temp_dir("reset");
        let file = dir.join("settings.json");
        let photo = dir.join("DSCF0001.RAF");
        let sidecar = dir.join("DSCF0001.RAF.rawstock.toml");
        let export = dir.join("DSCF0001.jpg");
        std::fs::write(&photo, b"raw").unwrap();
        std::fs::write(&sidecar, b"schema = 1\n").unwrap();
        std::fs::write(&export, b"jpeg").unwrap();

        let (mut store, _) = SettingsStore::load(file.clone());
        let mut next = store.get().clone();
        next.appearance.surround = Surround::Mid;
        next.onboarding.completed = true;
        store.set(next).unwrap();
        store.reset().unwrap();

        assert_eq!(store.get(), &AppSettings::default());
        let (loaded, _) = SettingsStore::load(file);
        assert_eq!(loaded.get(), &AppSettings::default());
        assert_eq!(std::fs::read(&photo).unwrap(), b"raw");
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"schema = 1\n");
        assert_eq!(std::fs::read(&export).unwrap(), b"jpeg");
        let _ = std::fs::remove_dir_all(dir);
    }
}
