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
    // Region-growing flood fill: each queued pixel carries the color of the
    // background pixel it spread from. Comparing neighbor-to-neighbor (instead
    // of every pixel against a fixed border seed) lets the fill traverse soft
    // studio gradients — fixed seeds die once the border ring is erased and
    // the fill stalls after one pixel row (the bug this replaced).
    let mut queue: Vec<(u32, u32, [u8; 3])> = Vec::new();

    // Seed from the four corners only: a border seed that carries the
    // subject's own color (subject reaching the frame edge) would erase the
    // subject pixel itself on visit, since reference==pixel always matches.
    // Corner colors are the reliable background samples of studio shots.
    for (x, y) in [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ] {
        let p = result.get_pixel(x, y);
        if p[3] >= 128 {
            queue.push((x, y, [p[0], p[1], p[2]]));
        }
    }

    let mut head = 0usize;
    while head < queue.len() {
        let (x, y, reference) = queue[head];
        head += 1;
        let idx = (y * width + x) as usize;
        if visited[idx] {
            continue;
        }
        visited[idx] = true;

        let pixel = *result.get_pixel(x, y);
        // Already-transparent pixels are background; spread through them but
        // never erase an opaque object pixel just because it is dark — that
        // would hole-punch dark subjects on transparent-background imports.
        let is_background = if pixel[3] < 128 {
            true
        } else {
            let dr = pixel[0] as i32 - reference[0] as i32;
            let dg = pixel[1] as i32 - reference[1] as i32;
            let db = pixel[2] as i32 - reference[2] as i32;
            dr * dr + dg * dg + db * db <= BACKGROUND_TOLERANCE * BACKGROUND_TOLERANCE * 3
        };

        if is_background {
            let current = [pixel[0], pixel[1], pixel[2]];
            result.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            if x > 0 {
                queue.push((x - 1, y, current));
            }
            if x + 1 < width {
                queue.push((x + 1, y, current));
            }
            if y > 0 {
                queue.push((x, y - 1, current));
            }
            if y + 1 < height {
                queue.push((x, y + 1, current));
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

/// Compose the cropped subject centered on a square white canvas of `size`,
/// scaled so the subject occupies `1 - 2*margin` of the canvas side.
fn composite_on_white_with_margin(
    img: &RgbaImage,
    bounds: (u32, u32, u32, u32),
    size: u32,
    margin: f32,
) -> RgbaImage {
    let (min_x, min_y, max_x, max_y) = bounds;
    let subject_w = max_x - min_x + 1;
    let subject_h = max_y - min_y + 1;

    let mut canvas = RgbaImage::from_pixel(size, size, Rgba([255, 255, 255, 255]));
    let scale = (size as f32 * (1.0 - 2.0 * margin)) / subject_w.max(subject_h) as f32;
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

/// Repair a generation whose subject is fully visible but too large in frame
/// (no edge contact, yet no working margin): crop the isolated subject and
/// recompose it centered on a white canvas at a 3D-safe size. Returns the
/// repaired PNG bytes, or None when the image cannot be parsed or has no
/// isolatable subject (callers then fall back to rejection/retry).
pub fn reframe_with_margin(bytes: &[u8]) -> Option<Vec<u8>> {
    let parsed = image::load_from_memory(bytes).ok()?;
    let rgba = parsed.to_rgba8();
    let (width, height) = rgba.dimensions();
    // Safety gate 1: the border ring must be a uniform backdrop. Sample the
    // outer 4% of each edge; if colors vary too much, this is a scene image
    // and any fill would butcher it — refuse.
    let band = (width.min(height) / 25).max(2) as i64;
    let mut samples: Vec<[i32; 3]> = Vec::new();
    for x in 0..width {
        for y in 0..height {
            let near_edge = (x as i64) < band
                || (y as i64) < band
                || (x as i64) >= width as i64 - band
                || (y as i64) >= height as i64 - band;
            if near_edge {
                let p = rgba.get_pixel(x, y);
                if p[3] >= 128 {
                    samples.push([p[0] as i32, p[1] as i32, p[2] as i32]);
                }
            }
        }
    }
    if samples.is_empty() {
        return None;
    }
    let mean: [f32; 3] = {
        let n = samples.len() as f32;
        let mut m = [0.0f32; 3];
        for s in &samples {
            for ch in 0..3 {
                m[ch] += s[ch] as f32;
            }
        }
        [m[0] / n, m[1] / n, m[2] / n]
    };
    let variance: f32 = samples
        .iter()
        .map(|s| {
            let dr = s[0] as f32 - mean[0];
            let dg = s[1] as f32 - mean[1];
            let db = s[2] as f32 - mean[2];
            dr * dr + dg * dg + db * db
        })
        .sum::<f32>()
        / samples.len() as f32;
    // A clean studio backdrop varies by only a few RGB points per channel.
    // Scenes, gradients with objects, and busy borders far exceed this.
    if variance > 90.0 {
        return None;
    }
    let cut = remove_background(&rgba);
    let bounds = subject_bounds(&cut)?;
    // Safety gate 2: if the fill erased most of the frame, the subject was
    // too similar to the background (light bodywork on white) — the result
    // would be a hollow outline, not a product shot. Refuse to repair.
    let (w, h) = cut.dimensions();
    let kept = bounds.2 as f32 * bounds.3 as f32;
    if kept < 0.30 * (w as f32) * (h as f32) {
        return None;
    }
    // REFRAIN_MARGIN leaves ~32% of the frame empty so the recomposed subject
    // passes the 3D-safety gate with a real working margin (not a technical
    // pass right at the 82% boundary).
    const REFRAIN_MARGIN: f32 = 0.16;
    let normalized = composite_on_white_with_margin(&cut, bounds, 1024, REFRAIN_MARGIN);
    let mut out = std::io::Cursor::new(Vec::new());
    normalized
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// True when the isolated subject's bounding box reaches any image edge.
/// A clean studio product shot always leaves margins on every side, so an
/// edge-touching subject means the concept is not 3D-safe: the object is
/// cropped, or the background is a scene the fill could not clear.
/// Hunyuan3D cannot reconstruct geometry it cannot see, so such a concept is
/// rejected and regenerated with a fresh seed instead of being promoted into
/// the mesh pipeline. Returns false for undecodable images (never blocks on
/// detector failure — the normalization path still handles those).
pub fn subject_touches_border(bytes: &[u8]) -> bool {
    let Ok(parsed) = image::load_from_memory(bytes) else {
        return false;
    };
    let rgba = parsed.to_rgba8();
    let cut = remove_background(&rgba);
    let Some((min_x, min_y, max_x, max_y)) = subject_bounds(&cut) else {
        return false;
    };
    let (width, height) = cut.dimensions();
    let edge = 3u32;
    // Two rejection criteria for a 3D-safe concept:
    // 1. Edge contact: the subject reaching any edge means it is cropped, or
    //    the "background" is a scene the fill could not clear (trees, roads).
    // 2. No headroom: even without touching an edge, a subject spanning more
    //    than 82% of the width or 88% of the height leaves no working margin —
    //    Hunyuan3D produces distorted geometry at frame edges. Real product
    //    shots keep the subject around two-thirds of the frame.
    let touches = min_x <= edge
        || min_y <= edge
        || max_x + edge >= width
        || max_y + edge >= height;
    let span_x = (max_x - min_x + 1) as f32 / width as f32;
    let span_y = (max_y - min_y + 1) as f32 / height as f32;
    touches || span_x > 0.82 || span_y > 0.88
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
    let (_width, _height) = rgba.dimensions();
    let cut = remove_background(&rgba);
    let Some(bounds) = subject_bounds(&cut) else {
        return bytes.to_vec();
    };

    // Sanity gate: the fill must leave a real, solid subject. Concept images
    // whose subject is bright (white/silver bodywork on a bright studio
    // background) get almost entirely erased by the flood fill — sending the
    // ghost outline to Hunyuan3D then crashes rembg with an empty mask
    // ("min(): Expected reduction dim 0"). In that case the original bytes
    // (which Hunyuan's own rembg handles correctly) flow through unchanged.
    let (cut_w, cut_h) = cut.dimensions();
    let kept = (bounds.2 - bounds.0 + 1) as f32 * (bounds.3 - bounds.1 + 1) as f32;
    let kept_fraction = kept / (cut_w as f32 * cut_h as f32);
    if kept_fraction < 0.12 {
        return bytes.to_vec();
    }
    // Also require the subject to actually be there in ink: an outline-only
    // result (subject erased from within) has very few opaque pixels inside
    // its bounding box. Count opaque pixels, not just the bbox area.
    let opaque_pixels = cut
        .enumerate_pixels()
        .filter(|(_, _, p)| p[3] > 16)
        .count() as f32;
    if opaque_pixels < 0.10 * (cut_w as f32 * cut_h as f32) {
        return bytes.to_vec();
    }

    let normalized = composite_on_white_with_margin(&cut, bounds, 1024, MARGIN_FRACTION);
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

    fn png_bytes(img: RgbaImage) -> Vec<u8> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        png.into_inner()
    }

    #[test]
    fn cropped_subject_touches_border() {
        // Subject runs off the right AND bottom edges — the classic crop that
        // makes 3D reconstruction impossible.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 64..256 {
            for y in 64..256 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        assert!(subject_touches_border(&png_bytes(img)));
    }

    #[test]
    fn single_edge_touch_is_rejected() {
        // Subject touching only the bottom edge is still not 3D-safe: the
        // object is cut, or the "background" is a scene the fill could not
        // clear. Both produce unusable meshes.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 40..216 {
            for y in 64..256 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        assert!(subject_touches_border(&png_bytes(img)));
    }

    #[test]
    fn framed_subject_does_not_touch_border() {
        // Subject fully inside the frame with margins on every side (50% span).
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 64..192 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        assert!(!subject_touches_border(&png_bytes(img)));
    }

    #[test]
    fn reframe_repairs_tight_span() {
        // 90%-width subject: rejected by the gate, repairable by reframe.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 12..244 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        let png = png_bytes(img);
        assert!(subject_touches_border(&png));
        let repaired = reframe_with_margin(&png).expect("repair expected");
        assert!(!subject_touches_border(&repaired), "repaired must pass gate");
    }

    #[test]
    fn reframe_refuses_light_subject_on_white() {
        // Very light gray subject on white: the fill would erase the body and
        // leave an outline — reframe must refuse instead of butchering it.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 12..244 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([250, 250, 252, 255]));
            }
        }
        for x in 60..200 {
            for y in 80..176 {
                img.put_pixel(x, y, Rgba([240, 240, 244, 255]));
            }
        }
        assert_eq!(reframe_with_margin(&png_bytes(img)), None);
    }

    #[test]
    fn reframe_refuses_busy_border_scene() {
        // A colorful scene border is not a studio backdrop: refuse repair.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 0..256 {
            for y in 0..20 {
                img.put_pixel(x, y, Rgba([(x % 200) as u8, 100, (y * 3 % 200) as u8, 255]));
            }
        }
        for x in 48..208 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        assert_eq!(reframe_with_margin(&png_bytes(img)), None);
    }

    #[test]
    fn tight_span_without_edge_contact_is_rejected() {
        // 90% width span with a technical margin: no headroom for the mesh
        // pipeline, so it is rejected even though nothing was cropped.
        let mut img = RgbaImage::from_pixel(256, 256, Rgba([255, 255, 255, 255]));
        for x in 12..244 {
            for y in 64..192 {
                img.put_pixel(x, y, Rgba([200, 30, 30, 255]));
            }
        }
        assert!(subject_touches_border(&png_bytes(img)));
    }
}
