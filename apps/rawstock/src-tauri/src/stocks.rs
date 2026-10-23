//! Bundled stock list. Reads each `.stock.toml` through `rawstock-core`
//! and does not render. A spectral stock's own model says black and white
//! (monochrome) or slide (reversal). Otherwise the category is inferred
//! from the file name until the stock format grows a category field.

use std::path::{Path, PathBuf};

use crate::contract::{AppError, StockCategory, StockSummary, category_for};
use rawstock_core::stock::{Emulsion, FilmModel, Process, Stock};

pub fn list() -> Result<Vec<StockSummary>, AppError> {
    let directory = directory();
    let mut paths = std::fs::read_dir(&directory)
        .map_err(|_| {
            AppError::internal("The stock folder could not be read. Reinstall rawstock, or set RAWSTOCK_STOCKS to a folder of stock files.")
        })?
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
        let Some(id) = stock_id(&path) else {
            continue;
        };
        let stock = Stock::load(&path).map_err(|_| AppError::stock_missing(&id))?;
        let name = stock.name().to_string();
        let category = match stock.parameters().film() {
            FilmModel::Spectral(film) if film.emulsion() == Emulsion::Monochrome => {
                StockCategory::Bw
            }
            FilmModel::Spectral(film) if film.process() == Process::Reversal => {
                StockCategory::Slide
            }
            _ => category_for(&id, &name),
        };
        stocks.push(StockSummary {
            category,
            model: stock.model().to_string(),
            revision: stock.revision(),
            id,
            name,
        });
    }
    stocks.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(stocks)
}

fn stock_id(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(".stock.toml")
        .map(str::to_string)
}

/// `RAWSTOCK_STOCKS` wins. Otherwise the repo `stocks/` directory next to
/// this crate, then a walk up from the current directory.
pub fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("RAWSTOCK_STOCKS") {
        return PathBuf::from(path);
    }
    let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stocks");
    if compiled.join("neutral.stock.toml").is_file() {
        return compiled;
    }
    let mut dir = std::env::current_dir().unwrap_or_default();
    for _ in 0..8 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_stocks_load_with_a_revision() {
        let stocks = list().expect("stocks");
        let portra = stocks.iter().find(|stock| stock.id == "portra400").unwrap();
        assert_eq!(portra.revision, 5);
        assert_eq!(portra.category, StockCategory::ColorNegative);
        assert_eq!(portra.model, "spectral");
        let velvia = stocks.iter().find(|stock| stock.id == "velvia").unwrap();
        assert_eq!(velvia.category, StockCategory::FujiSim);
        assert!(stocks.iter().any(|stock| stock.id == "neutral"));
        let category = |id: &str| stocks.iter().find(|stock| stock.id == id).unwrap().category;
        assert_eq!(category("ilford-hp5-plus"), StockCategory::Bw);
        assert_eq!(category("velvia50-spectral"), StockCategory::Slide);
    }
}
