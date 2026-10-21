//! Bundled `stocks/*.stock.toml`, plus the check that Save as does not
//! overwrite one of them.

use std::collections::HashSet;
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
            return canonical(PathBuf::from(path));
        }
        let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stocks");
        if compiled.join("neutral.stock.toml").is_file() {
            return canonical(compiled);
        }
        let mut dir = std::env::current_dir().unwrap_or_default();
        for _ in 0..6 {
            let candidate = dir.join("stocks");
            if candidate.join("neutral.stock.toml").is_file() {
                return canonical(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
        canonical(compiled)
    }

    /// Identity of a stock file. `..` segments and a symlink to the same
    /// file collapse to one id, so a scan and an opened path cannot both
    /// register it.
    pub fn id(path: &Path) -> String {
        canonical(path.to_path_buf()).display().to_string()
    }

    /// Drop a later registration of an id already in the list. The first
    /// one stays, which is the bundled scan when the UI adds an open file.
    pub fn dedupe(stocks: Vec<StockSummary>) -> Vec<StockSummary> {
        let mut seen = HashSet::with_capacity(stocks.len());
        stocks
            .into_iter()
            .filter(|stock| seen.insert(stock.id.clone()))
            .collect()
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
            let id = Self::id(&path);
            let stock =
                Stock::load(&path).with_context(|| format!("load stock {}", path.display()))?;
            stocks.push(StockSummary {
                path: id.clone(),
                id,
                name: stock.name().to_string(),
                model: stock.model().to_string(),
                revision: stock.revision(),
            });
        }
        stocks.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Self::dedupe(stocks))
    }

    /// A missing file is resolved against the bundled directory, so
    /// `RAWSTOCK_STOCK=portra400.stock.toml` works from any cwd.
    pub fn resolve(path: PathBuf) -> PathBuf {
        let found = if path.is_file() {
            path
        } else {
            let joined = Self::directory().join(&path);
            if joined.is_file() { joined } else { path }
        };
        canonical(found)
    }

    pub fn is_bundled(path: &Path) -> bool {
        let Ok(directory) = Self::directory().canonicalize() else {
            return false;
        };
        candidate(path).starts_with(directory)
    }
}

fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
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
    fn bundled_list_includes_velvia() {
        let stocks = StockCatalog::list().unwrap();
        let velvia = stocks
            .iter()
            .find(|stock| stock.name == "Velvia")
            .expect("Velvia is a bundled stock");
        assert_eq!(velvia.model, "parametric");
    }

    #[test]
    fn one_file_keeps_one_id_when_a_second_path_spelling_is_registered() {
        let listed = StockCatalog::list().unwrap();
        let mut seen = std::collections::HashSet::new();
        for stock in &listed {
            assert!(
                seen.insert(stock.id.clone()),
                "duplicate stock id {}",
                stock.id
            );
            assert_eq!(stock.path, stock.id);
            assert!(!stock.id.contains(".."), "{}", stock.id);
        }
        let velvia: Vec<_> = listed
            .iter()
            .filter(|stock| stock.name == "Velvia")
            .collect();
        assert_eq!(velvia.len(), 1, "Velvia registered more than once");

        // The directory scan used to keep `src-tauri/../../../stocks/...`.
        // Opening RAWSTOCK_STOCK with the real path then registered it again.
        let via_parents =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stocks/velvia.stock.toml");
        assert!(
            via_parents
                .components()
                .any(|component| component.as_os_str() == "..")
        );
        let opened = StockCatalog::resolve(via_parents.clone());
        assert_eq!(StockCatalog::id(&via_parents), StockCatalog::id(&opened));
        assert_eq!(velvia[0].id, StockCatalog::id(&opened));

        let second = StockSummary {
            id: velvia[0].id.clone(),
            path: via_parents.display().to_string(),
            name: velvia[0].name.clone(),
            model: velvia[0].model.clone(),
            revision: velvia[0].revision,
        };
        let merged = StockCatalog::dedupe(vec![(*velvia[0]).clone(), second]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, velvia[0].id);
        assert_eq!(merged[0].path, velvia[0].path);
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
