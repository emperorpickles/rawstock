//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → scene-linear [`color`] → [`render`]. Rendering runs
//! exposure, a [`stock`] film model, print/scan, the sRGB output transform,
//! and export. The film model is parametric curves. Grain, halation, and
//! stock files are not part of this path.

#![forbid(unsafe_code)]

pub mod color;
pub mod decode;
mod icc;
mod matrices;
pub mod render;
pub mod stock;
