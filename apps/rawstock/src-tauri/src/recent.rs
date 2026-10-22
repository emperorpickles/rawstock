//! Recent-files list. A convenience cache in the app config directory,
//! never the source of truth. At most ten paths, newest first.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::contract::canonical_path;

pub const LIMIT: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RecentFileDocument {
    paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RecentFiles {
    file: PathBuf,
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    pub fn load(file: PathBuf) -> Self {
        let paths = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<RecentFileDocument>(&text).ok())
            .map(|document| document.paths.into_iter().map(PathBuf::from).collect())
            .unwrap_or_default();
        Self { file, paths }
    }

    /// Move `path` to the front and drop the oldest past [`LIMIT`].
    pub fn remember(&mut self, path: &Path) {
        let canonical = canonical_path(path);
        self.paths.retain(|existing| existing != &canonical);
        self.paths.insert(0, canonical);
        self.paths.truncate(LIMIT);
    }

    /// Drop paths that are no longer files. Returns how many were removed.
    pub fn prune_missing(&mut self) -> usize {
        let before = self.paths.len();
        self.paths.retain(|path| path.is_file());
        before - self.paths.len()
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn list(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|path| path.display().to_string())
            .collect()
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let document = RecentFileDocument { paths: self.list() };
        let text = serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".to_string());
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rawstock-recent-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("recent.json")
    }

    #[test]
    fn recent_files_cap_at_ten_and_prune_missing() {
        let store = temp_file("cap");
        let dir = store.parent().unwrap().to_path_buf();
        let mut recent = RecentFiles::load(store);
        for index in 0..12 {
            let path = dir.join(format!("frame-{index}.raf"));
            std::fs::write(&path, b"x").unwrap();
            recent.remember(&path);
        }
        assert_eq!(recent.list().len(), 10);
        assert!(recent.list()[0].ends_with("frame-11.raf"));
        let gone = dir.join("frame-11.raf");
        std::fs::remove_file(&gone).unwrap();
        assert_eq!(recent.prune_missing(), 1);
        assert_eq!(recent.list().len(), 9);
        recent.save().unwrap();
        let loaded = RecentFiles::load(recent.file.clone());
        assert_eq!(loaded.list().len(), 9);
        let _ = std::fs::remove_dir_all(dir);
    }
}
