//! Stocks: data-driven film looks. A stock is a definition file holding a
//! base 3D LUT (.cube) plus parameters for tone curve, grain, and halation.
//! See `stocks/README.md` for the proposed file format.

/// Placeholder for a loaded stock definition.
#[derive(Debug, Clone, Default)]
pub struct Stock {
    pub name: String,
}
