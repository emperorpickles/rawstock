//! Raw frame bytes for `tauri::ipc::Response`.
//!
//! Layout, little-endian: `u32` width, `u32` height, then tightly packed
//! RGBA8. The samples are display-referred sRGB and already oriented.
//! JSON never carries pixels.

use crate::contract::AppError;

pub const HEADER: usize = 8;

/// Width, height, and RGBA8 with the 8-byte header in front.
pub fn pack(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let mut out = Vec::with_capacity(HEADER + rgba.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(rgba);
    out
}

pub fn dimensions(packed: &[u8]) -> Option<(u32, u32)> {
    if packed.len() < HEADER {
        return None;
    }
    let width = u32::from_le_bytes(packed[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(packed[4..8].try_into().ok()?);
    let pixels = (width as usize).checked_mul(height as usize)?;
    let bytes = pixels.checked_mul(4)?;
    if packed.len() != HEADER + bytes {
        return None;
    }
    Some((width, height))
}

/// Neutral checker the stub jobs hand to the webview. Not a photograph.
pub fn placeholder(width: u32, height: u32, salt: u8) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    let dark = 96u8.saturating_add(salt / 4);
    for y in 0..height {
        for x in 0..width {
            let on = (x / 4 + y / 4).is_multiple_of(2);
            let value = if on { dark } else { 164 };
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }
    pack(width, height, &rgba)
}

/// Long edge used when a stub needs a bitmap. Real develops replace this.
pub fn stub_size(long_edge: u32) -> (u32, u32) {
    let long = long_edge.clamp(2, 48);
    let short = u32::try_from((u64::from(long) * 2) / 3).unwrap_or(2).max(2);
    (long, short)
}

/// Crop `packed` in oriented pixel coordinates and return a new frame.
pub fn crop(packed: &[u8], x: u32, y: u32, width: u32, height: u32) -> Result<Vec<u8>, AppError> {
    let Some((full_width, full_height)) = dimensions(packed) else {
        return Err(AppError::cache_miss());
    };
    if width == 0 || height == 0 || x >= full_width || y >= full_height {
        return Err(AppError::internal("That area is outside the photo."));
    }
    let width = width.min(full_width - x);
    let height = height.min(full_height - y);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in y..y + height {
        let start = HEADER + (row as usize * full_width as usize + x as usize) * 4;
        let end = start + width as usize * 4;
        rgba.extend_from_slice(&packed[start..end]);
    }
    Ok(pack(width, height, &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_eight_bytes_then_rgba() {
        let frame = placeholder(2, 1, 0);
        assert_eq!(u32::from_le_bytes(frame[0..4].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(frame[4..8].try_into().unwrap()), 1);
        assert_eq!(frame.len(), 8 + 8);
        assert_eq!(&frame[8..12], &[96, 96, 96, 255]);
    }

    #[test]
    fn crop_returns_the_requested_pixels() {
        let frame = placeholder(4, 4, 0);
        let cropped = crop(&frame, 1, 1, 2, 2).unwrap();
        assert_eq!(dimensions(&cropped), Some((2, 2)));
        assert_eq!(cropped.len(), 8 + 16);
    }
}
