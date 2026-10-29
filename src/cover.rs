//! Album art: decoding a JPEG, PNG or SVG cover and scaling it for display.

use std::io::Cursor;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Longest side of a decoded cover, in pixels.
const SIZE: u32 = 256;

/// A decoded cover image.
#[derive(Clone, Debug)]
pub struct Cover {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Pixels, row by row, four bytes each.
    pub rgba: Vec<u8>,
}

/// Decodes a cover from the bytes of a JPEG, PNG or SVG file.
///
/// Returns `None` for any other format and for anything malformed, which lets
/// the window draw its placeholder instead of failing over a bad tag.
pub fn decode(bytes: &[u8]) -> Option<Cover> {
    let image = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        jpeg(bytes)?
    } else if bytes.starts_with(b"\x89PNG") {
        png(bytes)?
    } else if looks_like_svg(bytes) {
        svg(bytes)?
    } else {
        return None;
    };
    Some(shrink(image))
}

/// An image in straight RGBA, before scaling.
struct Image {
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// Pixels, row by row, four bytes each.
    rgba: Vec<u8>,
}

/// Decodes a JPEG, asking for RGBA so every pixel is four bytes.
fn jpeg(bytes: &[u8]) -> Option<Image> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(Cursor::new(bytes), options);
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    let channels = decoder.output_colorspace()?.num_components();
    let pixels = decoder.decode().ok()?;
    let rgba = match channels {
        1 => pixels
            .iter()
            .flat_map(|gray| [*gray, *gray, *gray, 0xff])
            .collect(),
        4 => pixels,
        _ => return None,
    };
    Some(Image {
        width: u32::from(info.width),
        height: u32::from(info.height),
        rgba,
    })
}

/// Decodes a PNG, expanding palettes and transparency to RGBA.
fn png(bytes: &[u8]) -> Option<Image> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 0xff])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Grayscale => pixels
            .iter()
            .flat_map(|gray| [*gray, *gray, *gray, 0xff])
            .collect(),
        _ => return None,
    };
    Some(Image {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

/// Renders an SVG at the size the window shows it at.
fn svg(bytes: &[u8]) -> Option<Image> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = (SIZE as f32 / size.width().max(size.height())).min(1.0);
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    Some(Image {
        width,
        height,
        rgba,
    })
}

/// Whether the bytes open like an SVG document.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    head.windows(4).any(|window| window == b"<svg")
}

/// Scales the image down so its longer side is at most [`SIZE`].
///
/// The window crops covers to a square, so the aspect ratio is kept and the
/// shrink averages the source pixels each one lands on.
fn shrink(image: Image) -> Cover {
    if image.width <= SIZE && image.height <= SIZE {
        return Cover {
            width: image.width as usize,
            height: image.height as usize,
            rgba: image.rgba,
        };
    }
    let scale = f64::from(SIZE) / f64::from(image.width.max(image.height));
    let width = ((f64::from(image.width) * scale).round() as u32).max(1);
    let height = ((f64::from(image.height) * scale).round() as u32).max(1);

    let columns = weights(image.width, width);
    let mut across = vec![0u8; (width * image.height * 4) as usize];
    for y in 0..image.height {
        for (x, column) in columns.iter().enumerate() {
            let mut sum = [0.0_f32; 4];
            let mut total = 0.0_f32;
            for &(pixel, weight) in column {
                let index = ((y * image.width + pixel) * 4) as usize;
                for (channel, value) in sum.iter_mut().enumerate() {
                    *value += f32::from(image.rgba[index + channel]) * weight;
                }
                total += weight;
            }
            let out = ((y * width + x as u32) * 4) as usize;
            for (channel, value) in sum.iter().enumerate() {
                across[out + channel] = (value / total).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    let rows = weights(image.height, height);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (y, row) in rows.iter().enumerate() {
        for x in 0..width {
            let mut sum = [0.0_f32; 4];
            let mut total = 0.0_f32;
            for &(pixel, weight) in row {
                let index = ((pixel * width + x) * 4) as usize;
                for (channel, value) in sum.iter_mut().enumerate() {
                    *value += f32::from(across[index + channel]) * weight;
                }
                total += weight;
            }
            let out = ((y as u32 * width + x) * 4) as usize;
            for (channel, value) in sum.iter().enumerate() {
                rgba[out + channel] = (value / total).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    Cover {
        width: width as usize,
        height: height as usize,
        rgba,
    }
}

/// The source pixels each destination pixel averages, with their weights.
fn weights(source: u32, target: u32) -> Vec<Vec<(u32, f32)>> {
    let scale = f64::from(source) / f64::from(target);
    (0..target)
        .map(|index| {
            let start = f64::from(index) * scale;
            let end = f64::from(index + 1) * scale;
            let first = start.floor() as u32;
            let last = (end.ceil() as u32).min(source);
            (first..last)
                .map(|pixel| {
                    let left = f64::from(pixel).max(start);
                    let right = (f64::from(pixel + 1)).min(end);
                    (pixel, (right - left).max(0.0) as f32)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_image_is_left_alone() {
        let cover = shrink(Image {
            width: 2,
            height: 1,
            rgba: vec![0; 8],
        });
        assert_eq!((cover.width, cover.height), (2, 1));
    }

    #[test]
    fn a_large_image_keeps_its_aspect_ratio() {
        let cover = shrink(Image {
            width: 1000,
            height: 500,
            rgba: vec![0; 1000 * 500 * 4],
        });
        assert_eq!((cover.width, cover.height), (256, 128));
        assert_eq!(cover.rgba.len(), 256 * 128 * 4);
    }

    #[test]
    fn a_flat_colour_survives_the_shrink() {
        let cover = shrink(Image {
            width: 8,
            height: 2,
            rgba: [10, 20, 30, 255].repeat(16),
        });
        assert_eq!(cover.rgba[..4], [10, 20, 30, 255]);
    }

    #[test]
    fn a_gif_is_no_cover() {
        assert!(decode(b"GIF89a").is_none());
    }
}
