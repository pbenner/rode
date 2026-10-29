//! Codec decoding, orientation, shared-memory limits and native/Python binary transport under
//! actual sandbox grants.

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, RgbImage};
use rode::{
    files::{self, Request},
    images::{self, Budget},
};
use std::{
    fs,
    io::{Cursor, Write},
    process::{Command, Stdio},
    sync::Arc,
};

fn fixture(format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_fn(4, 3, |x, y| {
        image::Rgb([x as u8 * 40, y as u8 * 60, 128])
    }));
    let image = if format == ImageFormat::Ico {
        DynamicImage::ImageRgba8(image.into_rgba8())
    } else {
        image
    };
    let mut result = Cursor::new(Vec::new());
    image.write_to(&mut result, format).unwrap();
    result.into_inner()
}
fn peer(request: &Request) -> anyhow::Result<files::Response> {
    let mut child = Command::new("python3")
        .args(["-c", files::REMOTE_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request)?)?;
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn common_formats_decode_without_using_the_filename_as_the_format_authority() {
    for (format, name) in [
        (ImageFormat::Png, "wrong.jpg"),
        (ImageFormat::Jpeg, "photo.JPG"),
        (ImageFormat::WebP, "picture.webp"),
        (ImageFormat::Gif, "animation.gif"),
        (ImageFormat::Bmp, "bitmap.bmp"),
        (ImageFormat::Ico, "icon.ico"),
        (ImageFormat::Tiff, "page.tiff"),
        (ImageFormat::Pnm, "image.pnm"),
        (ImageFormat::Tga, "texture.tga"),
    ] {
        let data = fixture(format);
        let budget = Arc::new(Budget::default());
        let preview = images::decode_bytes(&data, name, &budget, 4096).unwrap();
        assert_eq!(preview.original, [4, 3], "{name}");
        assert_eq!(preview.pixels.size, [4, 3]);
        assert_eq!(budget.used(), 4 * 3 * 4);
        assert_eq!(preview.encoded_bytes, data.len());
        let second = preview.clone();
        drop(preview);
        assert!(budget.used() > 0);
        drop(second);
        assert_eq!(budget.used(), 0);
    }
    assert!(images::format_for("ASSET.PNG").is_some());
    assert!(images::format_for("source.svg").is_none());
}

#[test]
fn binary_transport_matches_local_and_python_and_enforces_limits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("quoted ' 🦀.png");
    let bytes = fixture(ImageFormat::Png);
    fs::write(&path, &bytes).unwrap();
    let request = Request::ReadImage {
        path: path.to_string_lossy().into(),
    };
    for response in [files::local(&request).unwrap(), peer(&request).unwrap()] {
        assert_eq!(STANDARD.decode(response.image.unwrap()).unwrap(), bytes);
        assert_eq!(response.revision, files::digest(&bytes));
        assert!(response.preview.is_none());
    }
    assert!(
        files::local(&Request::Read {
            path: path.to_string_lossy().into()
        })
        .is_err()
    );
    fs::File::create(&path)
        .unwrap()
        .set_len(images::MAX_ENCODED as u64 + 1)
        .unwrap();
    assert!(
        files::local(&request)
            .unwrap_err()
            .to_string()
            .contains("32 MiB")
    );
    assert!(peer(&request).unwrap_err().to_string().contains("32 MiB"));
    let svg = root.path().join("source.svg");
    fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    assert!(
        files::local(&Request::Read {
            path: svg.to_string_lossy().into()
        })
        .unwrap()
        .text
        .starts_with("<svg")
    );
}

#[test]
fn transparency_orientation_and_gpu_preview_scaling_are_preserved() {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 128, 96]));
    let mut bytes = Cursor::new(vec![]);
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    let budget = Arc::new(Budget::default());
    let preview = images::decode_bytes(bytes.get_ref(), "alpha.png", &budget, 1).unwrap();
    assert_eq!(preview.original, [2, 2]);
    assert_eq!(preview.pixels.size, [1, 1]);
    assert_eq!(preview.pixels.pixels[0].a(), 96);
    assert_eq!(budget.used(), 4);
    drop(preview);
    assert_eq!(budget.used(), 0);

    let mut jpeg = fixture(ImageFormat::Jpeg);
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut marker = vec![0xff, 0xe1];
    marker.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    marker.extend_from_slice(exif);
    jpeg.splice(2..2, marker);
    let preview = images::decode_bytes(&jpeg, "rotated.jpg", &budget, 4096).unwrap();
    assert_eq!(preview.original, [3, 4]);
    assert_eq!(preview.pixels.size, [3, 4]);
}

fn dimensions(mut png: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let mut crc = 0xffff_ffffu32;
    for byte in &png[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    png[29..33].copy_from_slice(&(!crc).to_be_bytes());
    png
}

#[test]
fn corrupt_images_and_decompression_bombs_do_not_consume_the_preview_budget() {
    let budget = Arc::new(Budget::default());
    for bytes in [
        vec![],
        b"not an image".to_vec(),
        dimensions(fixture(ImageFormat::Png), 8192, 8192),
        dimensions(fixture(ImageFormat::Png), images::MAX_DIMENSION + 1, 1),
        fixture(ImageFormat::Png)[..45].to_vec(),
    ] {
        assert!(images::decode_bytes(&bytes, "bad.png", &budget, 4096).is_err());
        assert_eq!(budget.used(), 0);
    }
    assert!(images::decode("invalid base64!", "bad.png", &budget, 4096).is_err());
}

#[test]
fn active_sandbox_image_reads_cannot_escape_through_paths_or_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let private = root.path().join("private.png");
    fs::write(&private, fixture(ImageFormat::Png)).unwrap();
    fs::copy(&private, project.join("inside.png")).unwrap();
    std::os::unix::fs::symlink(&private, project.join("escape.png")).unwrap();
    let plan = rode::sandbox::prepare(
        project.to_str().unwrap(),
        &rode::sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let helper = std::path::Path::new(env!("CARGO_BIN_EXE_rode"));
    let read = |path: &std::path::Path| {
        plan.local_request_with_helper(
            &Request::ReadImage {
                path: path.to_string_lossy().into(),
            },
            helper,
        )
    };
    assert!(read(&project.join("inside.png")).unwrap().image.is_some());
    if plan.active {
        assert!(read(&private).is_err());
        assert!(read(&project.join("escape.png")).is_err());
    } else {
        eprintln!("Sandbox unavailable: {}", plan.reason);
    }
}
