//! Raw preview bytes for `tauri::ipc::Response`.
//!
//! Layout, little-endian:
//!
//! ```text
//! 0..4    magic `RSP1` (0x52535031)
//! 4..8    version, currently 1
//! 8..16   ticket
//! 16..20  width
//! 20..24  height
//! 24      fidelity: 0 preview, 1 exact
//! 25..32  reserved
//! 32..    RGB8, the same rounding as the JPEG writer
//! ```
//!
//! The samples are already oriented for display. Export does not use this
//! buffer; it writes the sensor-oriented full frame with the camera
//! orientation.

use rawstock_core::decode::Orientation;
use rawstock_core::render::{Fidelity, SrgbImage};

pub struct FrameBytes;

impl FrameBytes {
    pub const MAGIC: u32 = 0x5253_5031;
    const VERSION: u32 = 1;
    const HEADER: usize = 32;

    pub fn from_image(ticket: u64, orientation: Orientation, image: &SrgbImage) -> Vec<u8> {
        let rgb = image.to_rgb8();
        let (width, height, rgb) = orientation.apply_rgb(image.width(), image.height(), &rgb);
        Self::pack(ticket, width, height, image.fidelity(), &rgb)
    }

    pub fn pack(ticket: u64, width: u32, height: u32, fidelity: Fidelity, rgb: &[u8]) -> Vec<u8> {
        debug_assert_eq!(rgb.len(), width as usize * height as usize * 3);
        let mut out = Vec::with_capacity(Self::HEADER + rgb.len());
        out.extend(Self::MAGIC.to_le_bytes());
        out.extend(Self::VERSION.to_le_bytes());
        out.extend(ticket.to_le_bytes());
        out.extend(width.to_le_bytes());
        out.extend(height.to_le_bytes());
        out.push(match fidelity {
            Fidelity::Preview => 0,
            Fidelity::Exact => 1,
        });
        out.resize(Self::HEADER, 0);
        out.extend_from_slice(rgb);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_marks_preview_bytes() {
        let rgb = [1_u8, 2, 3, 4, 5, 6];
        let bytes = FrameBytes::pack(9, 2, 1, Fidelity::Preview, &rgb);
        assert_eq!(&bytes[0..4], &FrameBytes::MAGIC.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 9);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert_eq!(bytes[24], 0);
        assert_eq!(bytes.len(), 32 + rgb.len());
        assert_eq!(&bytes[32..], &rgb);
    }

    #[test]
    fn exact_fidelity_is_one() {
        let bytes = FrameBytes::pack(1, 0, 0, Fidelity::Exact, &[]);
        assert_eq!(bytes[24], 1);
        assert_eq!(bytes.len(), 32);
    }
}
