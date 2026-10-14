//! A small sRGB ICC profile embedded in exported JPEG and TIFF files.
//!
//! Version 2.1, so the tone curve is a sampled `curv` rather than a v4
//! parametric type. The samples are the IEC 61966-2-1 decoding curve, the
//! inverse of the output transform in [`crate::color`].

use crate::matrices::{self, D50_XYZ};

/// Build the sRGB display profile.
pub(crate) fn srgb_profile() -> Vec<u8> {
    let colorants = matrices::srgb_colorants_d50();
    let trc = srgb_trc();
    assemble([
        (*b"desc", text_description("sRGB")),
        (*b"wtpt", xyz_type(D50_XYZ)),
        (*b"rXYZ", xyz_type(colorants[0])),
        (*b"gXYZ", xyz_type(colorants[1])),
        (*b"bXYZ", xyz_type(colorants[2])),
        (*b"rTRC", trc.clone()),
        (*b"gTRC", trc.clone()),
        (*b"bTRC", trc),
    ])
}

fn assemble<const N: usize>(tags: [([u8; 4], Vec<u8>); N]) -> Vec<u8> {
    let table_bytes = 4 + N * 12;
    let mut data = Vec::new();
    let mut entries = Vec::with_capacity(N);
    let mut offset = 128 + table_bytes;
    for (signature, mut blob) in tags {
        pad4(&mut blob);
        entries.push((signature, offset, blob.len()));
        offset += blob.len();
        data.extend(blob);
    }

    let mut out = Vec::with_capacity(128 + table_bytes + data.len());
    write_header(&mut out, (128 + table_bytes + data.len()) as u32);
    out.extend_from_slice(&(N as u32).to_be_bytes());
    for (signature, tag_offset, len) in entries {
        out.extend_from_slice(&signature);
        out.extend_from_slice(&(tag_offset as u32).to_be_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend(data);
    debug_assert_eq!(
        u32::from_be_bytes(out[0..4].try_into().unwrap()) as usize,
        out.len()
    );
    out
}

fn write_header(out: &mut Vec<u8>, size: u32) {
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&[0; 4]); // preferred CMM
    out.extend_from_slice(&0x0210_0000u32.to_be_bytes()); // profile 2.1.0
    out.extend_from_slice(b"mntr");
    out.extend_from_slice(b"RGB ");
    out.extend_from_slice(b"XYZ ");
    // Fixed date so the bytes do not depend on the clock.
    for part in [2026u16, 1, 1, 0, 0, 0] {
        out.extend_from_slice(&part.to_be_bytes());
    }
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0; 4]); // platform
    out.extend_from_slice(&[0; 4]); // flags
    out.extend_from_slice(&[0; 4]); // manufacturer
    out.extend_from_slice(&[0; 4]); // model
    out.extend_from_slice(&[0; 8]); // attributes
    out.extend_from_slice(&0u32.to_be_bytes()); // perceptual intent
    write_s15fixed16(out, D50_XYZ[0]);
    write_s15fixed16(out, D50_XYZ[1]);
    write_s15fixed16(out, D50_XYZ[2]);
    out.extend_from_slice(&[0; 4]); // creator
    out.extend_from_slice(&[0; 44]);
    debug_assert_eq!(out.len(), 128);
}

fn text_description(text: &str) -> Vec<u8> {
    let mut ascii = text.as_bytes().to_vec();
    ascii.push(0);
    let mut out = Vec::new();
    out.extend_from_slice(b"desc");
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(ascii.len() as u32).to_be_bytes());
    out.extend(ascii);
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode language code
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode length
    out.extend_from_slice(&0u16.to_be_bytes()); // Macintosh script
    out.push(0);
    out.resize(out.len() + 67, 0);
    pad4(&mut out);
    out
}

fn xyz_type(xyz: [f64; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(b"XYZ ");
    out.extend_from_slice(&0u32.to_be_bytes());
    for channel in xyz {
        write_s15fixed16(&mut out, channel);
    }
    out
}

/// 256-entry sRGB tone reproduction curve, device value → linear PCS.
fn srgb_trc() -> Vec<u8> {
    const COUNT: usize = 256;
    let mut out = Vec::with_capacity(12 + COUNT * 2);
    out.extend_from_slice(b"curv");
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(COUNT as u32).to_be_bytes());
    for index in 0..COUNT {
        let encoded = index as f64 / (COUNT - 1) as f64;
        let linear = srgb_to_linear(encoded);
        let sample = (linear.clamp(0.0, 1.0) * 65535.0).round() as u16;
        out.extend_from_slice(&sample.to_be_bytes());
    }
    out
}

/// IEC 61966-2-1 decoding function (encoded sRGB → linear).
fn srgb_to_linear(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn write_s15fixed16(out: &mut Vec<u8>, value: f64) {
    let fixed = (value * 65536.0).round() as i32;
    out.extend_from_slice(&fixed.to_be_bytes());
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_header_names_srgb() {
        let profile = srgb_profile();
        let size = u32::from_be_bytes(profile[0..4].try_into().unwrap()) as usize;
        assert_eq!(size, profile.len());
        assert_eq!(&profile[12..16], b"mntr");
        assert_eq!(&profile[16..20], b"RGB ");
        assert_eq!(&profile[20..24], b"XYZ ");
        assert_eq!(&profile[36..40], b"acsp");
        assert_eq!(&profile[8..12], &0x0210_0000u32.to_be_bytes());
        let tags = u32::from_be_bytes(profile[128..132].try_into().unwrap());
        assert_eq!(tags, 8);
        assert!(profile.windows(4).any(|window| window == *b"sRGB"));
    }

    #[test]
    fn trc_matches_the_srgb_decoding_curve() {
        let trc = srgb_trc();
        let count = u32::from_be_bytes(trc[8..12].try_into().unwrap()) as usize;
        let samples = &trc[12..];
        assert_eq!(samples.len(), count * 2);
        let first = u16::from_be_bytes(samples[0..2].try_into().unwrap());
        let last = u16::from_be_bytes(samples[samples.len() - 2..].try_into().unwrap());
        assert_eq!(first, 0);
        assert_eq!(last, 65535);
        // Sample 128 of 0..=255 is just over encoded 0.5.
        let mid = u16::from_be_bytes(samples[count..count + 2].try_into().unwrap());
        let expect = (srgb_to_linear(128.0 / 255.0) * 65535.0).round() as u16;
        assert!((i32::from(mid) - i32::from(expect)).abs() <= 1);
    }
}
