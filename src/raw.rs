use crate::color::{ColorInfo, DynamicRange};
use std::path::Path;

const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "nef", "nrw", "arw", "srf", "raf", "orf", "rw2", "pef", "dng", "srw", "x3f",
    "erf", "kdc", "dcr", "mrw", "3fr", "iiq",
];

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_lowercase();
            RAW_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

pub struct RawImage {
    pub data: Vec<u16>,
    pub width: u32,
    pub height: u32,
    pub color: ColorInfo,
}

pub fn decode_raw(path: &Path) -> Option<RawImage> {
    let mut pipeline = imagepipe::Pipeline::new_from_file(path).ok()?;
    let decoded = pipeline.output_16bit(None).ok()?;
    let width = decoded.width as u32;
    let height = decoded.height as u32;

    let pixel_count = (width as usize) * (height as usize);
    let mut rgba = Vec::with_capacity(pixel_count * 4);

    for pixel in decoded.data.chunks_exact(3) {
        rgba.push(pixel[0]);
        rgba.push(pixel[1]);
        rgba.push(pixel[2]);
        rgba.push(0xFFFF);
    }

    Some(RawImage {
        data: rgba,
        width,
        height,
        color: ColorInfo {
            icc_profile: None,
            dynamic_range: DynamicRange::Sdr,
        },
    })
}

pub fn linear_16_to_srgb_8(data: &[u16], width: u32, height: u32) -> Vec<u8> {
    let pixel_count = (width as usize) * (height as usize);
    let mut out = Vec::with_capacity(pixel_count * 4);

    for pixel in data.chunks_exact(4) {
        for i in 0..3 {
            let linear = pixel[i] as f32 / 65535.0;
            let srgb = if linear <= 0.0031308 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            }
            .clamp(0.0, 1.0);
            out.push((srgb * 255.0) as u8);
        }
        out.push(255u8);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_extension_detection_is_case_insensitive() {
        assert!(is_raw(Path::new("photo.CR2")));
        assert!(is_raw(Path::new("photo.NEF")));
        assert!(is_raw(Path::new("photo.ArW")));
        assert!(is_raw(Path::new("photo.DnG")));
    }

    #[test]
    fn raw_extension_detection_catches_expected_formats() {
        let yes = [
            "a.cr2", "a.cr3", "a.nef", "a.nrw", "a.arw", "a.srf", "a.raf", "a.orf", "a.rw2",
            "a.pef", "a.dng", "a.srw", "a.x3f", "a.erf", "a.kdc", "a.dcr", "a.mrw", "a.3fr",
            "a.iiq",
        ];

        for p in yes {
            assert!(is_raw(Path::new(p)), "{p} should be recognized as RAW");
        }
    }

    #[test]
    fn raw_extension_detection_rejects_standard_formats() {
        let no = [
            "a.jpg", "a.jpeg", "a.png", "a.gif", "a.webp", "a.avif", "a.tiff", "a.bmp", "a.txt",
            "a",
        ];

        for p in no {
            assert!(!is_raw(Path::new(p)), "{p} should not be recognized as RAW");
        }
    }

    #[test]
    fn linear_16_to_srgb_8_black_white_midpoint() {
        let data: Vec<u16> = vec![
            0, 0, 0, 65535, 65535, 65535, 65535, 65535, 32768, 32768, 32768, 65535,
        ];

        let out = linear_16_to_srgb_8(&data, 3, 1);
        assert_eq!(out.len(), 12);

        // black
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 0);
        assert_eq!(out[3], 255);

        // white — sRGB transfer function rounding may produce 254 or 255
        assert!(out[4] >= 254);
        assert!(out[5] >= 254);
        assert!(out[6] >= 254);
        assert_eq!(out[7], 255);

        // middle gray should be around 186-190 in sRGB
        assert!((out[8] as i32 - 188).abs() <= 3);
        assert!((out[9] as i32 - 188).abs() <= 3);
        assert!((out[10] as i32 - 188).abs() <= 3);
        assert_eq!(out[11], 255);
    }

    #[test]
    fn linear_16_to_srgb_8_preserves_expected_length() {
        let px = vec![65535u16; 4 * 10];
        let out = linear_16_to_srgb_8(&px, 10, 1);
        assert_eq!(out.len(), 10 * 4);
    }
}
