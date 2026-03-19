use gtk4::prelude::*;
use image::GenericImageView;
use std::path::{Path, PathBuf};

const THUMB_SIZE: u32 = 128;

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("thumbnails")
        .join("normal")
}

fn key_for(path: &Path) -> String {
    let uri = format!(
        "file://{}",
        path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
    );
    format!("{:x}", md5::compute(uri.as_bytes()))
}

fn thumb_path(path: &Path) -> PathBuf {
    cache_dir().join(format!("{}.png", key_for(path)))
}

pub fn load(path: &Path) -> Option<gtk4::gdk::Texture> {
    let src_meta = std::fs::metadata(path).ok()?;
    let src_mtime = src_meta.modified().ok()?;
    let thumb = thumb_path(path);
    let thumb_meta = std::fs::metadata(&thumb).ok()?;
    let thumb_mtime = thumb_meta.modified().ok()?;
    if thumb_mtime < src_mtime {
        return None;
    }
    gtk4::gdk::Texture::from_file(&gtk4::gio::File::for_path(thumb)).ok()
}

pub fn generate(path: &Path) -> Option<gtk4::gdk::Texture> {
    let texture = if crate::raw::is_raw(path) {
        let raw_img = crate::raw::decode_raw(path)?;
        let rgba8 = crate::raw::linear_16_to_srgb_8(&raw_img.data, raw_img.width, raw_img.height);
        let img = image::RgbaImage::from_raw(raw_img.width, raw_img.height, rgba8)?;
        let thumb = image::imageops::resize(
            &img,
            THUMB_SIZE,
            THUMB_SIZE,
            image::imageops::FilterType::Triangle,
        );
        save_thumbnail_png(path, &thumb)?;
        let bytes = gtk4::glib::Bytes::from_owned(thumb.into_raw());
        gtk4::gdk::MemoryTexture::new(
            THUMB_SIZE as i32,
            THUMB_SIZE as i32,
            gtk4::gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            (THUMB_SIZE * 4) as usize,
        )
        .upcast::<gtk4::gdk::Texture>()
    } else {
        let img = image::open(path).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        let icc = crate::color::extract_icc_profile(path);
        let srgb_pixels = crate::color::rgba8_to_srgb_with_icc(img.as_raw(), w, h, icc.as_deref());
        let corrected = image::RgbaImage::from_raw(w, h, srgb_pixels)?;
        let thumb = image::imageops::resize(
            &corrected,
            THUMB_SIZE,
            THUMB_SIZE,
            image::imageops::FilterType::Triangle,
        );
        save_thumbnail_png(path, &thumb)?;
        let bytes = gtk4::glib::Bytes::from_owned(thumb.into_raw());
        gtk4::gdk::MemoryTexture::new(
            THUMB_SIZE as i32,
            THUMB_SIZE as i32,
            gtk4::gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            (THUMB_SIZE * 4) as usize,
        )
        .upcast::<gtk4::gdk::Texture>()
    };
    Some(texture)
}

fn save_thumbnail_png(path: &Path, img: &image::RgbaImage) -> Option<()> {
    let thumb_path = thumb_path(path);
    if let Some(parent) = thumb_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    img.save_with_format(thumb_path, image::ImageFormat::Png)
        .ok()?;
    Some(())
}
