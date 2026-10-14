use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rawstock_core::decode::RawImage;
use rawstock_core::render::{Pipeline, SrgbImage};
use rawstock_core::stock::Stock;

/// Rawstock: develop RAW files with film stocks.
#[derive(Parser)]
#[command(name = "rawstock", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Develop a RAW file through the film pipeline.
    Develop {
        /// Input RAW file (e.g. IMG.RAF).
        input: PathBuf,
        /// Stock file to load into the film pipeline.
        #[arg(long = "stock")]
        stock: PathBuf,
        /// Output image path (`.jpg`, `.jpeg`, `.tif`, or `.tiff`).
        #[arg(long = "out", short = 'o')]
        output: PathBuf,
    },
    /// Print decode metadata for a RAW file.
    Inspect {
        /// Input RAW file.
        file: PathBuf,
    },
    /// Write a scene-linear ACEScg OpenEXR from a RAW file.
    DumpLinear {
        /// Input RAW file.
        input: PathBuf,
        /// Output `.exr` path.
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    run(Cli::parse())
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Develop {
            input,
            stock,
            output,
        } => develop(&input, &stock, &output),
        Command::Inspect { file } => {
            let image = RawImage::open(&file)?;
            println!("{image}");
            Ok(())
        }
        Command::DumpLinear { input, output } => {
            if !is_exr(&output) {
                bail!("output must be an .exr file");
            }
            let image = RawImage::open(&input)?;
            image.develop_linear()?.write_exr(&output)?;
            Ok(())
        }
    }
}

fn develop(input: &Path, stock: &Path, output: &Path) -> Result<()> {
    if !SrgbImage::is_supported_output(output) {
        bail!("output must be a .jpg, .jpeg, .tif, or .tiff file");
    }
    let parameters = Stock::load(stock)?.into_parameters();
    let raw = RawImage::open(input)?;
    let linear = raw.develop_linear()?;
    let developed = Pipeline::new(parameters).develop(&linear);
    developed.write(output, raw.orientation())?;
    Ok(())
}

fn is_exr(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn neutral_stock() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stocks/neutral.stock.toml")
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn develop_parses() {
        let cli = Cli::try_parse_from([
            "rawstock",
            "develop",
            "IMG.RAF",
            "--stock",
            "neutral.stock.toml",
            "--out",
            "out.jpg",
        ]);
        assert!(cli.is_ok());
    }

    #[test]
    fn develop_requires_a_stock() {
        let cli = Cli::try_parse_from(["rawstock", "develop", "IMG.RAF", "--out", "out.jpg"]);
        assert!(cli.is_err());
    }

    #[test]
    fn develop_rejects_an_unsupported_output() {
        let cli = Cli::try_parse_from([
            "rawstock",
            "develop",
            "IMG.RAF",
            "--stock",
            "neutral.stock.toml",
            "--out",
            "out.png",
        ])
        .unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains(".jpg"));
        assert!(!err.to_string().contains("not implemented"));
    }

    #[test]
    fn develop_reports_a_missing_file() {
        let stock = neutral_stock();
        let cli = Cli::try_parse_from([
            "rawstock",
            "develop",
            "/tmp/rawstock-does-not-exist.RAF",
            "--stock",
            stock.to_str().unwrap(),
            "--out",
            "out.jpg",
        ])
        .unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains("failed to decode"));
    }

    #[test]
    fn inspect_parses() {
        let cli = Cli::try_parse_from(["rawstock", "inspect", "IMG.RAF"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn dump_linear_parses() {
        let cli = Cli::try_parse_from(["rawstock", "dump-linear", "IMG.RAF", "out.exr"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn dump_linear_rejects_a_non_exr_output() {
        let cli = Cli::try_parse_from(["rawstock", "dump-linear", "IMG.RAF", "out.jpg"]).unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains(".exr"));
    }
}
