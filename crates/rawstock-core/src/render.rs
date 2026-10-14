//! Rendering: applies a [`Stock`](crate::stock::Stock) to linear image data
//! and runs the output transform. A CPU reference path defines correct
//! output; a wgpu GPU path must match it within tolerance.

/// Placeholder for the CPU reference renderer.
#[derive(Debug, Default)]
pub struct CpuRenderer;
