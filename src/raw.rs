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
