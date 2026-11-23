//! Bounded local PNG decoding for installed extension artwork. No URL loaders.
use anyhow::{Result, ensure};
use eframe::egui;
use std::{io::Read, path::Path};

const MAX_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn load(path: &Path) -> Result<egui::ColorImage> {
    ensure!(
        path.is_absolute(),
        "Extension icons require an absolute path"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_BYTES as u64,
        "Invalid extension icon file"
    );
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "Extension icon exceeds 2 MiB");
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    // Filter premultiplied pixels, as the renderer does. Averaging straight
    // RGB against transparent black creates dark edges and turns white theme
    // masks gray. Use egui's conversion to preserve its color/alpha semantics.
    let mut image = reader.decode()?.to_rgba8();
    for pixel in image.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        pixel.0 = egui::Color32::from_rgba_unmultiplied(r, g, b, a).to_array();
    }
    let image = if image.width() > 256 || image.height() > 256 {
        image::DynamicImage::ImageRgba8(image)
            .thumbnail(256, 256)
            .to_rgba8()
    } else {
        image
    };
    // Preserve non-square artwork inside the rail's square image slot.
    let edge = image.width().max(image.height());
    let mut square = image::RgbaImage::new(edge, edge);
    // Copy rather than alpha-blend: the source is already premultiplied.
    image::imageops::replace(
        &mut square,
        &image,
        i64::from((edge - image.width()) / 2),
        i64::from((edge - image.height()) / 2),
    );
    Ok(egui::ColorImage::from_rgba_premultiplied(
        [edge as usize, edge as usize],
        square.as_raw(),
    ))
}

/// White PNG artwork is an alpha mask: it uses the same dynamic tint as native
/// icons. Premultiplied white channels equal alpha, including antialiased edges.
/// Colored artwork keeps its original branding colors.
pub(super) fn is_theme_mask(image: &egui::ColorImage) -> bool {
    image.pixels.iter().any(|p| p.a() > 0)
        && image
            .pixels
            .iter()
            .all(|p| p.r() == p.a() && p.g() == p.a() && p.b() == p.a())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn white_alpha_masks_are_themed_but_colored_or_empty_images_are_not() {
        let mask = egui::ColorImage::from_rgba_unmultiplied(
            [3, 1],
            &[255, 255, 255, 255, 255, 255, 255, 127, 0, 0, 0, 0],
        );
        assert!(is_theme_mask(&mask));
        let color = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[10, 100, 200, 255]);
        assert!(!is_theme_mask(&color));
        let gray = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[128, 128, 128, 255]);
        assert!(!is_theme_mask(&gray));
        let empty = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[0, 0, 0, 0]);
        assert!(!is_theme_mask(&empty));
    }

    #[test]
    fn downsampling_keeps_masks_themeable_and_colored_edges_free_of_dark_fringes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        for color in [[255, 255, 255], [255, 0, 0]] {
            // The odd dimensions and diagonal produce partially covered pixels
            // when reduced. Transparent black is common in exported PNG artwork.
            image::RgbaImage::from_fn(513, 257, |x, y| {
                if x < y {
                    image::Rgba([color[0], color[1], color[2], 255])
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            })
            .save(&path)
            .unwrap();
            let icon = load(&path).unwrap();
            assert_eq!(icon.size, [256, 256]);
            assert!(icon.pixels.iter().any(|p| p.a() > 0 && p.a() < 255));
            assert_eq!(is_theme_mask(&icon), color == [255, 255, 255]);
            for pixel in &icon.pixels {
                assert_eq!(pixel.r(), pixel.a(), "Dark fringe: {pixel:?}");
                assert_eq!(pixel.g(), if color[1] == 255 { pixel.a() } else { 0 });
                assert_eq!(pixel.b(), if color[2] == 255 { pixel.a() } else { 0 });
            }
            // Centering non-square artwork must copy the premultiplied pixels
            // unchanged and leave the padding transparent.
            assert!(icon.pixels[..256].iter().all(|p| p.a() == 0));
            assert!(icon.pixels[255 * 256..].iter().all(|p| p.a() == 0));
        }
    }

    #[test]
    fn png_artwork_preserves_transparency_and_rejects_bad_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 100, 200, 0]))
            .save(&path)
            .unwrap();
        let icon = load(&path).unwrap();
        assert_eq!(icon.size, [32, 32]);
        assert_eq!(icon.pixels[0].a(), 0);
        image::RgbaImage::from_pixel(32, 16, image::Rgba([10, 100, 200, 255]))
            .save(&path)
            .unwrap();
        let icon = load(&path).unwrap();
        assert_eq!(icon.size, [32, 32]);
        assert_eq!(icon.pixels[0].a(), 0);
        assert_eq!(icon.pixels[16 * 32 + 16].a(), 255);
        std::fs::write(&path, b"not a PNG").unwrap();
        assert!(load(&path).is_err());
        assert!(load(dir.path()).is_err());
        assert!(load(Path::new("relative.png")).is_err());
        image::RgbaImage::new(2049, 1).save(&path).unwrap();
        assert!(load(&path).is_err());
        std::fs::write(&path, vec![0; MAX_BYTES + 1]).unwrap();
        assert!(load(&path).is_err());
    }
}
