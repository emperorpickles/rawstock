//! Rawstock core: the shared library behind every front end.
//!
//! Pipeline: [`decode`] → [`color`] → [`stock`] → [`render`] (including the
//! final output transform). All modules are placeholders for now.

pub mod color;
pub mod decode;
pub mod render;
pub mod stock;

#[cfg(test)]
mod tests {
    #[test]
    fn placeholders_construct() {
        let _ = crate::decode::RawImage::default();
        let _ = crate::stock::Stock::default();
        let _ = crate::render::CpuRenderer;
        let _ = crate::color::WorkingSpace::default();
    }
}
