use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rawstock_core::decode::RawImage;

/// Rawstock: develop RAW files with film stocks.
#[derive(Parser)]
#[command(name = "rawstock", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Develop a RAW file with a stock.
    Develop {
        /// Input RAW file (e.g. IMG.RAF).
        input: PathBuf,
        /// Name of the stock to apply.
        #[arg(long)]
        stock: String,
        /// Output image path.
        #[arg(short, long)]
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
        Command::Develop { .. } => bail!("develop is not implemented yet"),
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

fn is_exr(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn develop_parses() {
        let cli = Cli::try_parse_from([
            "rawstock", "develop", "IMG.RAF", "--stock", "demo", "-o", "out.jpg",
        ]);
        assert!(cli.is_ok());
    }

    #[test]
    fn develop_is_not_implemented() {
        let cli = Cli::try_parse_from([
            "rawstock", "develop", "IMG.RAF", "--stock", "demo", "-o", "out.jpg",
        ])
        .unwrap();
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains("not implemented"));
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
