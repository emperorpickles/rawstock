use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rawstock_core::decode::RawImage;
use rawstock_core::render::{Parameters, Pipeline, SrgbImage};
use rawstock_core::stock::{Curve, FilmModel, Parametric};

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
        Command::Develop { input, output } => develop(&input, &output),
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

fn develop(input: &Path, output: &Path) -> Result<()> {
    if !SrgbImage::is_supported_output(output) {
        bail!("output must be a .jpg, .jpeg, .tif, or .tiff file");
    }
    let raw = RawImage::open(input)?;
    let linear = raw.develop_linear()?;
    let developed = Pipeline::new(builtin_parameters()).develop(&linear);
    developed.write(output, raw.orientation())?;
    Ok(())
}

/// Parametric look used until stock files exist.
///
/// The same D-logH curve on each layer, no interlayer coupling, and a print
/// curve that inverts the negative. Midtones stay near a straight print;
/// the shoulder rolls highlights off instead of clipping them.
fn builtin_parameters() -> Parameters {
    const NEGATIVE: [(f32, f32); 6] = [
        (-3.5, 0.15),
        (-2.0, 0.30),
        (-0.7, 0.70),
        (0.0, 1.15),
        (0.6, 1.70),
        (1.5, 2.05),
    ];
    const PRINT: [(f32, f32); 5] = [
        (-1.5, 0.04),
        (-0.5, 0.20),
        (0.2, 0.70),
        (1.0, 1.50),
        (2.0, 2.20),
    ];
    let negative = Curve::new(NEGATIVE).expect("builtin negative curve is monotone");
    let print = Curve::new(PRINT).expect("builtin print curve is monotone");
    let film = FilmModel::Parametric(
        Parametric::new([negative.clone(), negative.clone(), negative], None)
            .expect("builtin film model is valid"),
    );
    Parameters::new(0.0, film, 0.9, print).expect("builtin exposure is finite")
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
    use rawstock_core::stock::FilmModel;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn develop_parses() {
        let cli = Cli::try_parse_from(["rawstock", "develop", "IMG.RAF", "--out", "out.jpg"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn develop_rejects_an_unsupported_output() {
        let cli =
            Cli::try_parse_from(["rawstock", "develop", "IMG.RAF", "--out", "out.png"]).unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains(".jpg"));
        assert!(!err.to_string().contains("not implemented"));
    }

    #[test]
    fn develop_reports_a_missing_file() {
        let cli = Cli::try_parse_from([
            "rawstock",
            "develop",
            "/tmp/rawstock-does-not-exist.RAF",
            "--out",
            "out.jpg",
        ])
        .unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains("failed to decode"));
    }

    #[test]
    fn builtin_parameters_construct() {
        let parameters = builtin_parameters();
        assert!(matches!(
            parameters.film(),
            FilmModel::Parametric(model) if model.coupling().is_none()
        ));
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
