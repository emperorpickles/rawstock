//! RAW decoding: turns a camera file (RAF, DNG, CR3, NEF, ARW, ...) into
//! linear sensor data plus the metadata needed to interpret it (white
//! balance, black/white levels, camera color matrices).
//!
//! Planned backend: the pure-Rust `rawler` crate, with LibRaw as a desktop
//! fallback if X-Trans/RAF support is insufficient.

/// Placeholder for a decoded RAW image.
#[derive(Debug, Default)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
}
