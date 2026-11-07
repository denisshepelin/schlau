use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage};

use crate::{EncodedImage, ExtractError, Extracted, Limits};

const MAX_DIMENSION: u32 = 16_384;
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
const MIN_SIDE: u32 = 32;
const JPEG_QUALITY: u8 = 85;

fn decoder_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

fn malformed(err: impl std::fmt::Display) -> ExtractError {
    ExtractError::Malformed(err.to_string())
}

pub(crate) fn extract(bytes: &[u8], limits: &Limits) -> Result<Extracted, ExtractError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(malformed)?;
    reader.limits(decoder_limits());
    let mut decoder = reader.into_decoder().map_err(malformed)?;
    let (width, height) = decoder.dimensions();
    if width < MIN_SIDE || height < MIN_SIDE {
        return Ok(Extracted::default());
    }
    let orientation = decoder.orientation().ok();
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(malformed)?;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }
    Ok(Extracted {
        text: None,
        images: vec![encode_jpeg(&flatten(&downscale(decoded, limits.max_side)))?],
    })
}

/// Target size with the longest side at most `max_side`, preserving aspect ratio; never upscales.
pub(crate) fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_side || longest == 0 {
        return (width, height);
    }
    let scale = f64::from(max_side) / f64::from(longest);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max_side);
    (side(width), side(height))
}

pub(crate) fn downscale(img: DynamicImage, max_side: u32) -> DynamicImage {
    let (width, height) = fit(img.width(), img.height(), max_side);
    if (width, height) == (img.width(), img.height()) {
        return img;
    }
    let prescaled = if img.width().max(img.height()) > max_side.saturating_mul(4) {
        img.thumbnail(width.saturating_mul(2), height.saturating_mul(2))
    } else {
        img
    };
    prescaled.resize_exact(width, height, FilterType::CatmullRom)
}

fn blend_on_white(channel: u8, alpha: u8) -> u8 {
    let (c, a) = (u16::from(channel), u16::from(alpha));
    ((c * a + 255 * (255 - a) + 127) / 255) as u8
}

pub(crate) fn flatten_rgba(rgba: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        image::Rgb([r, g, b].map(|c| blend_on_white(c, a)))
    })
}

/// Converts to 8-bit RGB, compositing any alpha channel onto white.
pub(crate) fn flatten(img: &DynamicImage) -> RgbImage {
    if img.color().has_alpha() {
        flatten_rgba(&img.to_rgba8())
    } else {
        img.to_rgb8()
    }
}

/// A picture from the web made drawable: PNG and JPEG as they came, other formats (GIF, WebP, BMP,
/// TIFF) decoded, downscaled to `max_side` and encoded as PNG, transparency kept. Any size is fine:
/// a 20-pixel icon is still a picture here.
pub fn web_image(bytes: &[u8], max_side: u32) -> Result<EncodedImage, ExtractError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(malformed)?;
    reader.limits(decoder_limits());
    let format = reader.format().ok_or_else(|| ExtractError::Unsupported("unknown image format".into()))?;
    let passthrough = match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        _ => None,
    };
    let mut decoder = reader.into_decoder().map_err(malformed)?;
    let (width, height) = decoder.dimensions();
    if let Some(mime) = passthrough {
        return Ok(EncodedImage { bytes: bytes.to_vec(), mime, width, height });
    }
    let orientation = decoder.orientation().ok();
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(malformed)?;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }
    let rgba = downscale(decoded, max_side).to_rgba8();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)
        .map_err(malformed)?;
    Ok(EncodedImage { bytes: png, mime: "image/png", width: rgba.width(), height: rgba.height() })
}

pub(crate) fn encode_jpeg(rgb: &RgbImage) -> Result<EncodedImage, ExtractError> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .encode_image(rgb)
        .map_err(malformed)?;
    Ok(EncodedImage {
        bytes,
        mime: "image/jpeg",
        width: rgb.width(),
        height: rgb.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_never_upscales_and_keeps_aspect() {
        assert_eq!(fit(800, 600, 1024), (800, 600));
        assert_eq!(fit(2048, 1024, 1024), (1024, 512));
        assert_eq!(fit(100, 100_000, 1024), (1, 1024));
    }

    #[test]
    fn web_images_pass_png_through_and_turn_gif_into_png() {
        let tiny = RgbaImage::from_pixel(12, 8, image::Rgba([10, 20, 30, 128]));
        let encode = |format| {
            let mut out = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(tiny.clone()).write_to(&mut out, format).unwrap();
            out.into_inner()
        };
        let png = encode(image::ImageFormat::Png);
        let kept = web_image(&png, 1024).unwrap();
        assert_eq!((kept.mime, kept.width, kept.height, kept.bytes == png), ("image/png", 12, 8, true));
        let gif = web_image(&encode(image::ImageFormat::Gif), 1024).unwrap();
        assert_eq!((gif.mime, gif.width, gif.height), ("image/png", 12, 8));
        assert!(web_image(b"<html>not a picture</html>", 1024).is_err());
    }

    #[test]
    fn blends_onto_white() {
        assert_eq!(blend_on_white(0, 0), 255);
        assert_eq!(blend_on_white(0, 255), 0);
        assert_eq!(blend_on_white(10, 255), 10);
    }
}
