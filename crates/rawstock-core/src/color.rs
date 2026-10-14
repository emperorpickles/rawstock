//! Color management: demosaiced camera RGB → a linear, wide-gamut working
//! space, and the final conversion to an output space (e.g. sRGB, Display P3).
//!
//! All creative processing happens in linear light; encoding to the output
//! space happens last.

/// Placeholder for the pipeline's working color space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkingSpace {
    /// Linear, wide-gamut working space (exact primaries TBD).
    #[default]
    LinearWideGamut,
}
