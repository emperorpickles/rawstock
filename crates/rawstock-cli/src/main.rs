use std::path::PathBuf;

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
    }
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
}
