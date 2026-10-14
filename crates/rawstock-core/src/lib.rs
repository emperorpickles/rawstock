//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → [`color`] → [`stock`] → [`render`] (including the
//! final output transform). Decode reads RAW metadata. Color, stocks, and
//! rendering are not implemented yet.

#![forbid(unsafe_code)]

pub mod color;
pub mod decode;
pub mod render;
pub mod stock;
