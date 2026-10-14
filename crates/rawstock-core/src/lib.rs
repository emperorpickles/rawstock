//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → [`color`] → [`stock`] → [`render`] (including the
//! final output transform). Decode reads a RAW file. Color develops it to
//! scene-linear ACEScg. Stocks and rendering are not implemented yet.

#![forbid(unsafe_code)]

pub mod color;
pub mod decode;
mod matrices;
pub mod render;
pub mod stock;
