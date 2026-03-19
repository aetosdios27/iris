use image::ImageDecoder;
use std::path::Path;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicRange {
    Sdr,
    Hdr,
}
#[derive(Debug, Clone)]
pub struct ColorInfo {
    /// Embedded ICC profile bytes if present.
    pub icc_profile: Option<Vec<u8>>,
    /// Whether the decoded image should be treated as SDR or HDR.
    pub dynamic_range: DynamicRange,
}
impl Default for ColorInfo {
    fn default() -> Self {
        Self {
            icc_profile: None,
            dynamic_range: DynamicRange::Sdr,
        }
    }
}
/// Try to extract an embedded ICC profile from a standard image file.
///
/// For now we use the `image` crate metadata path where available.
/// If extraction fails, return None and fall back to sRGB assumptions.
pub fn extract_icc_profile(path: &Path) -> Option<Vec<u8>> {
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    // Re-open because `into_decoder()` consumes the reader.
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    match format {
        image::ImageFormat::Png
        | image::ImageFormat::Jpeg
        | image::ImageFormat::WebP
        | image::ImageFormat::Avif
        | image::ImageFormat::Tiff => decoder.icc_profile().ok()?,
        _ => None,
    }
}
/// Convert RGBA8 pixels from an embedded ICC profile to sRGB using LittleCMS.
///
/// If no ICC is present or conversion fails, returns the original pixels unchanged.
pub fn rgba8_to_srgb_with_icc(rgba: &[u8], width: u32, height: u32, icc: Option<&[u8]>) -> Vec<u8> {
    let Some(icc_bytes) = icc else {
        return rgba.to_vec();
    };
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return rgba.to_vec();
    }
    use lcms2::{Intent, PixelFormat, Profile, Transform};
    let src_profile = match Profile::new_icc(icc_bytes) {
        Ok(p) => p,
        Err(_) => return rgba.to_vec(),
    };
    let dst_profile = Profile::new_srgb();
    let transform = match Transform::new(
        &src_profile,
        PixelFormat::RGBA_8,
        &dst_profile,
        PixelFormat::RGBA_8,
        Intent::Perceptual,
    ) {
        Ok(t) => t,
        Err(_) => return rgba.to_vec(),
    };
    let mut out = vec![0u8; rgba.len()];
    transform.transform_pixels(rgba, &mut out);
    out
}
