//! Stocks: data-driven film looks. A stock is a definition file holding a
//! base 3D LUT (.cube) plus parameters for tone curve, grain, and halation.
//! See `stocks/README.md` for the proposed file format.

/// A loaded stock definition. Parsing is not implemented yet.
#[derive(Debug, Clone)]
pub struct Stock {
    pub name: String,
}
