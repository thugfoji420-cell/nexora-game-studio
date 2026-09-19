//! Source-image normalization for image-to-3D generation.
//!
//! Hunyuan3D reconstructs whatever it is given: a scene image (streets, grass,
//! dramatic lighting) becomes garbage geometry around the object. This module
//! isolates the subject and normalizes the image into the clean single-object
//! reference the model expects:
//!
//! 1. Remove the background via border-color flood fill (the concept prompts
//!    request a plain white studio background, so the border color is a
//!    reliable background seed; tolerance absorbs studio gradients).
//! 2. Crop to the subject bounding box with a small margin.
//! 3. Composite onto a square white canvas (Hunyuan prefers square input).
//! 4. Re-encode as PNG.
//!
//! If anything fails (undecodable image, empty subject), the ORIGINAL bytes
//! are returned unchanged — normalization must never break generation.

use image::{GenericImageView, Rgba, RgbaImage};

/// How far a pixel may drift from the border color and still count as
/// background. Generous enough for soft studio gradients and compression
/// noise, tight enough to stop at the object edge.
const BACKGROUND_TOLERANCE: i32 = 42;

/// Fraction of the subject bounding box added as margin on each side.
const MARGIN_FRACTION: f32 = 0.06;

/// Flood-fill background removal from the image border. Pixels connected to
/// any border pixel and within tolerance of that border's seed color become
/// transparent. Scanline BFS keeps this fast for 512-2048px inputs.
fn remove_background(img: &RgbaImage) -> RgbaImage {
    let (width, height) = img.dimensions();
    let mut result = img.clone();
    let mut visited = vec![false; (width * height) as usize];
    let mut queue: Vec<(u32, u32)> = Vec::new();

    // Seed from all four borders.
    for x in 0..width {
        queue.push((x, 0));
        queue.push((x, height - 1));
    }
    for y in 0..height {
        queue.push((0, y));
        queue.push((width - 1, y));
    }

    let mut head = 0usize;
    while head < queue.len() {
        let (x, y) = queue[head];
        head += 1;
        let idx = (y * width + x) as usize;
        if visited[idx] {
            continue;
        }
        visited[idx] = true;

        let pixel = result.get_pixel(x, y);
        // Already-transparent pixels are background; spread through them but
        // never erase an opaque object pixel just because it is dark — that
        // would hole-punch dark subjects on transparent-background imports.
        let is_background = if pixel[3] < 128 {
            true
        } else {
            // Against each opaque border seed color (sampled live from the
            // result so the fill also crosses soft gradients between similar
            // background tones).
            [
                result.get_pixel(0, 0),
                result.get_pixel(width - 1, 0),
                result.get_pixel(0, height - 1),
                result.get_pixel(width - 1, height - 1),
            ]
            .iter()
            .filter(|seed| seed[3] >= 128)
            .any(|seed| {
                let dr = pixel[0] as i32 - seed[0] as i32;
                let dg = pixel[1] as i32 - seed[1] as i32;
                let db = pixel[2] as i32 - seed[2] as i32;
                dr * dr + dg * dg + db * db <= BACKGROUND_TOLERANCE * BACKGROUND_TOLERANCE * 3
            })
        };

        if is_background {
            result.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            if x > 0 {
                queue.push((x - 1, y));
            }
            if x + 1 < width {
                queue.push((x + 1, y));
            }
            if y > 0 {
                queue.push((x, y - 1));
            }
            if y + 1 < height {
                queue.push((x, y + 1));
            }
        }
    }
    result
}

/// Bounding box of all remaining (non-transparent) pixels.
fn subject_bounds(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = img.dimensions();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x: i64 = -1;
    let mut max_y: i64 = -1;
    for (x, y, pixel) in img.enumerate_pixels() {
        if pixel[3] > 16 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x as i64);
            max_y = max_y.max(y as i64);
        }
    }
    if max_x < 0 {
        return None;
    }
    Some((min_x, min_y, max_x as u32, max_y as u32))
}

/// Compose the cropped subject centered on a square white canvas of `size`.
fn composite_on_white(img: &RgbaImage, bounds: (u32, u32, u32, u32), size: u32) -> RgbaImage {
    let (min_x, min_y, max_x, max_y) = bounds;
    let subject_w = max_x - min_x + 1;
    let subject_h = max_y - min_y + 1;

    let mut canvas = RgbaImage::from_pixel(size, size, Rgba([255, 255, 255, 255]));
    let scale = (size as f32 * (1.0 - 2.0 * MARGIN_FRACTION)) / subject_w.max(subject_h) as f32;
    let draw_w = (subject_w as f32 * scale).round().max(1.0) as u32;
    let draw_h = (subject_h as f32 * scale).round().max(1.0) as u32;
    let off_x = size.saturating_sub(draw_w) / 2;
    let off_y = size.saturating_sub(draw_h) / 2;

    let scaled = image::imageops::resize(
        &img.view(min_x, min_y, subject_w, subject_h).to_image(),
        draw_w,
        draw_h,
        image::imageops::FilterType::Triangle,
    );
    for (x, y, pixel) in scaled.enumerate_pixels() {
        if pixel[3] > 0 {
            canvas.put_pixel(off_x + x, off_y + y, *pixel);
        }
    }
    canvas
}

/// Normalize raw PNG/JPEG bytes into a clean single-object reference image.
/// Returns the original bytes untouched when the image cannot be parsed or
/// no subject can be isolated — generation proceeds unblocked either way.
pub fn normalize_source_image(bytes: &[u8]) -> Vec<u8> {
    let parsed = match image::load_from_memory(bytes) {
        Ok(img) => img,
        Err(_) => return bytes.to_vec(),
    };

    let rgba = parsed.to_rgba8();
    let cut = remove_background(&rgba);
    let Some(bounds) = subject_bounds(&cut) else {
        return bytes.to_vec();
    };

    let normalized = composite_on_white(&cut, bounds, 1024);
    let mut out = std::io::Cursor::new(Vec::new());
    if normalized
        .write_to(&mut out, image::ImageFormat::Png)
        .is_err()
    {
        return bytes.to_vec();
    }
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white_bg_image() -> Vec<u8> {
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 64..192 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        png.into_inner()
    }

    #[test]
    fn normalizes_white_background_scene_to_clean_subject() {
        let png = white_bg_image();
        let normalized = normalize_source_image(&png);
        let img = image::load_from_memory(&normalized).unwrap();
        // The red square should survive, centered; corners must be white.
        assert_eq!(img.get_pixel(4, 4)[0], 255);
        let center = img.get_pixel(512, 512);
        assert!(center[0] > 120, "subject red channel expected, got {center:?}");
    }

    #[test]
    fn passthrough_when_undecodable() {
        let garbage = b"not an image at all".to_vec();
        assert_eq!(normalize_source_image(&garbage), garbage);
    }
}
