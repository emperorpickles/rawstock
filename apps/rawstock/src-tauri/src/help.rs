//! Help menu content: build details for About and Report an issue, and the
//! local log file. Nothing here leaves the machine.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::contract::format_unix;

/// What About shows and "Copy details" copies. No paths except the log's
/// own location, which only About displays.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub commit: String,
    pub build_date: String,
    pub os: String,
    pub arch: String,
    pub log_path: String,
}

impl AppInfo {
    pub fn current(log_dir: &Path) -> Self {
        let epoch = env!("RAWSTOCK_BUILD_EPOCH").parse::<u64>().unwrap_or(0);
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: env!("RAWSTOCK_GIT_COMMIT").to_string(),
            build_date: format_unix(epoch)[..10].to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            log_path: Log::file(log_dir).display().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Error => "ERROR",
        }
    }
}

/// `rawstock.log` in the app log directory. Each line is
/// `<RFC3339 time> <LEVEL> <detail>`. Past [`Log::CAP`] the file rotates to
/// `rawstock.log.1`, replacing the previous one.
pub struct Log;

impl Log {
    pub const CAP: u64 = 5 * 1024 * 1024;

    pub fn file(dir: &Path) -> PathBuf {
        dir.join("rawstock.log")
    }

    pub fn write(dir: &Path, level: Level, detail: &str) {
        Self::write_capped(dir, level, detail, Self::CAP);
    }

    fn write_capped(dir: &Path, level: Level, detail: &str, cap: u64) {
        use std::io::Write;
        let path = Self::file(dir);
        let _ = std::fs::create_dir_all(dir);
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() >= cap) {
            let _ = std::fs::rename(&path, dir.join("rawstock.log.1"));
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        else {
            return;
        };
        let detail = detail.replace(['\r', '\n'], " ");
        let _ = writeln!(
            file,
            "{} {} {detail}",
            crate::contract::utc_now(),
            level.label()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rawstock-log-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lines_carry_a_time_and_level() {
        let dir = temp_dir("lines");
        Log::write(&dir, Level::Error, "settings\nreset");
        let text = std::fs::read_to_string(Log::file(&dir)).unwrap();
        let line = text.lines().next().unwrap();
        let mut parts = line.splitn(3, ' ');
        let time = parts.next().unwrap();
        assert_eq!(time.len(), 20);
        assert!(time.ends_with('Z'));
        assert_eq!(parts.next(), Some("ERROR"));
        assert_eq!(parts.next(), Some("settings reset"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_log_over_the_cap_rotates_to_dot_one() {
        let dir = temp_dir("rotate");
        let cap = 256;
        for index in 0..10 {
            Log::write_capped(
                &dir,
                Level::Info,
                &format!("line {index} {}", "x".repeat(40)),
                cap,
            );
        }
        let current = std::fs::metadata(Log::file(&dir)).unwrap().len();
        let rotated = dir.join("rawstock.log.1");
        assert!(rotated.exists());
        assert!(current < cap + 100);
        let text = std::fs::read_to_string(Log::file(&dir)).unwrap();
        assert!(text.contains("line 9"));
        assert!(!text.contains("line 0 "));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn about_reports_build_details() {
        let dir = temp_dir("about");
        let info = AppInfo::current(&dir);
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.commit.is_empty());
        assert_eq!(info.build_date.len(), 10);
        assert_eq!(info.os, std::env::consts::OS);
        assert!(info.log_path.ends_with("rawstock.log"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
