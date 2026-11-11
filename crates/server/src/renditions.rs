//! Smaller copies of images for screens: `GET /api/attachments/{id}/content?w=640|960|1280|1920`.
//!
//! A JPEG becomes a JPEG (quality 82), a PNG or WebP a lossless WebP, scaled down to the width
//! (EXIF orientation applied). Copies are made once, on first request, and kept in storage next
//! to the originals (`renditions/<aa>/<sha256>-<width>.<ext>`); when the original is no wider
//! than requested, too large to decode, or the copy wouldn't be smaller, an empty marker is kept
//! instead and the original is served. Copies of a file go when the file is deleted.

use bytes::Bytes;
use tokio::sync::Semaphore;

use crate::app::AppState;

/// The widths copies are made in (the client asks for the smallest one that fills its space).
pub const WIDTHS: [u32; 4] = [640, 960, 1280, 1920];
/// originals beyond this many pixels are served as they are
const MAX_PIXELS: u64 = 60_000_000;

/// decoding large images takes memory and CPU: a few at a time
static PERMITS: Semaphore = Semaphore::const_new(2);

/// The kinds of images copies are made of: (extension, media type) of the copy.
fn output_for(media_type: &str) -> Option<(&'static str, &'static str)> {
    match media_type {
        "image/jpeg" => Some(("jpg", "image/jpeg")),
        "image/png" | "image/webp" => Some(("webp", "image/webp")),
        _ => None,
    }
}

fn key_for(blob: &str, width: u32, ext: &str) -> String {
    format!("renditions/{}/{blob}-{width}.{ext}", &blob[..2])
}

/// The copy of `blob` at `width` (its bytes and media type), or None to serve the original.
pub async fn get(state: &AppState, blob: &str, media_type: &str, width: u32) -> anyhow::Result<Option<(Bytes, &'static str)>> {
    let Some((ext, mime)) = output_for(media_type) else { return Ok(None) };
    if !WIDTHS.contains(&width) || blob.len() < 4 || !blob.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let key = key_for(blob, width, ext);
    if let Ok(b) = state.blobs.get_key(&key).await {
        return Ok((!b.is_empty()).then_some((b, mime)));
    }
    let _permit = PERMITS.acquire().await?;
    // made meanwhile by another request
    if let Ok(b) = state.blobs.get_key(&key).await {
        return Ok((!b.is_empty()).then_some((b, mime)));
    }
    let original = state.blobs.get(blob).await?;
    let made = tokio::task::spawn_blocking(move || scale(&original, width, ext)).await?;
    let copy = match made {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(blob, "no smaller copy: {e:#}");
            None
        }
    };
    let stored = copy.clone().unwrap_or_default();
    state.blobs.put_key(&key, Bytes::from(stored)).await?;
    Ok(copy.map(|c| (Bytes::from(c), mime)))
}

/// Removes the copies of a deleted file.
pub async fn forget(state: &AppState, blob: &str) {
    if blob.len() < 4 {
        return;
    }
    for width in WIDTHS {
        for ext in ["jpg", "webp"] {
            let _ = state.blobs.delete_key(&key_for(blob, width, ext)).await;
        }
    }
}

/// The image scaled down to `width` and encoded, or None when the original should be served.
fn scale(data: &[u8], width: u32, ext: &str) -> anyhow::Result<Option<Vec<u8>>> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let (w, h) = ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?.into_dimensions()?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS || w.max(h) <= width {
        return Ok(None);
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    if img.width() <= width {
        return Ok(None);
    }
    let height = ((u64::from(img.height()) * u64::from(width)) / u64::from(img.width())).max(1) as u32;
    let small = img.resize_exact(width, height, image::imageops::FilterType::CatmullRom);
    let mut out = Vec::new();
    if ext == "jpg" {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        enc.encode_image(&small.to_rgb8())?;
    } else {
        let rgba = small.to_rgba8();
        image::codecs::webp::WebPEncoder::new_lossless(&mut out).encode(&rgba, rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)?;
    }
    Ok((out.len() < data.len()).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn scales_down_wide_images_only() {
        let wide = png(2000, 100);
        let copy = scale(&wide, 640, "webp").unwrap().expect("a smaller copy");
        let img = image::load_from_memory(&copy).unwrap();
        assert_eq!((img.width(), img.height()), (640, 32));
        assert!(scale(&png(300, 300), 640, "webp").unwrap().is_none(), "narrow enough already");
        assert!(scale(b"not an image", 640, "webp").is_err());
        let jpg = {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1500, 1000, image::Rgba([10, 200, 30, 255])))
                .to_rgb8()
                .write_to(&mut out, image::ImageFormat::Jpeg)
                .unwrap();
            out.into_inner()
        };
        let copy = scale(&jpg, 640, "jpg").unwrap().expect("a smaller copy");
        assert_eq!(image::guess_format(&copy).unwrap(), image::ImageFormat::Jpeg);
        assert_eq!(image::load_from_memory(&copy).unwrap().width(), 640);
    }
}
