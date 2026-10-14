//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → scene-linear [`color`] → [`render`]. Rendering runs
//! exposure, optional halation, a [`stock`] film model, optional grain,
//! print/scan, the sRGB output transform, and export. [`stock::Stock::load`]
//! reads a versioned stock file into [`render::Parameters`].

#![forbid(unsafe_code)]

mod cie1931;
pub mod color;
pub mod decode;
mod icc;
mod matrices;
pub mod render;
mod spectrum;
pub mod stock;
