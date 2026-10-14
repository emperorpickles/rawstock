use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Develop { .. } => println!("not implemented yet"),
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
}
