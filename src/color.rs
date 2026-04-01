use image::ImageDecoder;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicRange {
    Sdr,
    Hdr,
}

#[derive(Debug, Clone)]
pub struct ColorInfo {
    pub icc_profile: Option<Vec<u8>>,
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

pub fn extract_icc_profile(path: &Path) -> Option<Vec<u8>> {
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_color_info_is_sdr_without_icc() {
        let c = ColorInfo::default();
        assert_eq!(c.dynamic_range, DynamicRange::Sdr);
        assert!(c.icc_profile.is_none());
    }

    #[test]
    fn rgba8_to_srgb_with_icc_returns_original_if_profile_missing() {
        let rgba = vec![10u8, 20, 30, 255, 40, 50, 60, 255];
        let out = rgba8_to_srgb_with_icc(&rgba, 2, 1, None);
        assert_eq!(out, rgba);
    }

    #[test]
    fn rgba8_to_srgb_with_icc_returns_original_if_length_mismatch() {
        let rgba = vec![10u8, 20, 30, 255];
        let fake_icc = &[1u8, 2, 3, 4];
        let out = rgba8_to_srgb_with_icc(&rgba, 2, 1, Some(fake_icc));
        assert_eq!(out, rgba);
    }

    #[test]
    fn rgba8_to_srgb_with_icc_returns_original_if_icc_invalid() {
        let rgba = vec![10u8, 20, 30, 255, 40, 50, 60, 255];
        let fake_icc = &[1u8, 2, 3, 4];
        let out = rgba8_to_srgb_with_icc(&rgba, 2, 1, Some(fake_icc));
        assert_eq!(out, rgba);
    }
}
