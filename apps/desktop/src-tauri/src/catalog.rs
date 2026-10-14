//! Bundled `stocks/*.stock.toml`, plus the check that Save as does not
//! overwrite one of them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::api::StockSummary;
use rawstock_core::stock::Stock;

pub struct StockCatalog;

impl StockCatalog {
    /// Directory of the stocks that ship with the checkout.
    ///
    /// `RAWSTOCK_STOCKS` wins. Otherwise this is the repo `stocks/` next to
    /// the crate, which is where `tauri dev` from a checkout looks. A walk
    /// up from the current directory covers a binary that was started
    /// somewhere else in the same tree.
    pub fn directory() -> PathBuf {
        if let Some(path) = std::env::var_os("RAWSTOCK_STOCKS") {
            return PathBuf::from(path);
        }
        let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stocks");
        if compiled.join("neutral.stock.toml").is_file() {
            return compiled;
        }
        let mut dir = std::env::current_dir().unwrap_or_default();
        for _ in 0..6 {
            let candidate = dir.join("stocks");
            if candidate.join("neutral.stock.toml").is_file() {
                return candidate;
            }
            if !dir.pop() {
                break;
            }
        }
        compiled
    }

    pub fn list() -> Result<Vec<StockSummary>> {
        let directory = Self::directory();
        let mut paths = std::fs::read_dir(&directory)
            .with_context(|| format!("read stock directory {}", directory.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".stock.toml"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut stocks = Vec::with_capacity(paths.len());
        for path in paths {
            let stock =
                Stock::load(&path).with_context(|| format!("load stock {}", path.display()))?;
            stocks.push(StockSummary {
                path: path.display().to_string(),
                name: stock.name().to_string(),
                model: stock.model().to_string(),
                revision: stock.revision(),
            });
        }
        stocks.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(stocks)
    }

    /// A missing file is resolved against the bundled directory, so
    /// `RAWSTOCK_STOCK=portra400.stock.toml` works from any cwd.
    pub fn resolve(path: PathBuf) -> PathBuf {
        if path.is_file() {
            return path;
        }
        let joined = Self::directory().join(&path);
        if joined.is_file() { joined } else { path }
    }

    pub fn is_bundled(path: &Path) -> bool {
        let Ok(directory) = Self::directory().canonicalize() else {
            return false;
        };
        candidate(path).starts_with(directory)
    }
}

fn candidate(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf())
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_list_includes_neutral_and_portra() {
        let stocks = StockCatalog::list().unwrap();
        let names: Vec<_> = stocks.iter().map(|stock| stock.name.as_str()).collect();
        assert!(names.contains(&"Neutral"), "{names:?}");
        assert!(names.contains(&"Kodak Portra 400"), "{names:?}");
        assert!(stocks.iter().any(|stock| stock.model == "spectral"));
        assert!(stocks.iter().any(|stock| stock.model == "parametric"));
    }

    #[test]
    fn a_new_file_inside_the_stock_directory_is_bundled() {
        let path = StockCatalog::directory().join("not-written.stock.toml");
        assert!(StockCatalog::is_bundled(&path));
        assert!(!StockCatalog::is_bundled(Path::new(
            "/tmp/not-a-bundled-stock.stock.toml"
        )));
    }
}
