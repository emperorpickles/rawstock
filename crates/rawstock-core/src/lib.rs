//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → scene-linear [`color`] → [`render`]. Rendering runs
//! exposure, optional halation, a [`stock`] film model, optional grain,
//! print/scan, the sRGB output transform, and export. [`stock::Stock::load`]
//! reads a versioned stock file into [`render::Parameters`].
//! [`session::PreviewSession`] caches those stages for a live preview. It does
//! not open files. Parallel stages run on the capped [`workers::Workers`]
//! pool. With the `gpu` feature, `gpu` runs the
//! same stages on wgpu.

#![forbid(unsafe_code)]

mod cie1931;
pub mod color;
pub mod decode;
#[cfg(feature = "gpu")]
pub mod gpu;
mod grain;
mod icc;
mod lateral_ca;
mod matrices;
pub mod render;
pub mod session;
mod spectrum;
pub mod stock;
pub mod workers;
