//! Bounded, CPU-only image decoding. The desktop worker owns decoding; remote
//! hosts only return bytes through the normal sandbox-aware file channel.
//!
//! # Design decisions
//!
//! Decode on the desktop so an SSH host needs no image library and both local
//! and remote files use the same supported codecs. Native textures avoid a
//! browser/viewer runtime; this provides bounded static previews rather than a
//! general image-editing or animation subsystem.
//!
//! RAII leases count retained worker/tab snapshots, not only visible textures.
//! A reload can overlap the old preview, transfer bytes and new pixels, so a
//! per-image limit alone would not bound total memory. Serializing transfer and
//! decode also bounds temporary peaks, at the cost of parallel preview loading.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use eframe::egui::ColorImage;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::{
    io::Cursor,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Maximum encoded image/preview input size in bytes.
pub const MAX_ENCODED: usize = 32 * 1024 * 1024;
/// Maximum decoded image allocation in bytes.
pub const MAX_DECODED: usize = 64 * 1024 * 1024;
/// Shared retained-preview allocation budget in bytes.
pub const MAX_RETAINED: usize = 128 * 1024 * 1024;
/// Maximum accepted source width or height in pixels.
pub const MAX_DIMENSION: u32 = 16_384;

/// Thread-safe shared preview accounting and transfer/decode serialization.
#[derive(Debug, Default)]
pub struct Budget {
    used: AtomicUsize,
    /// Serialize byte transfer/decode, including temporary transport buffers.
    pub io: Mutex<()>,
}
impl Budget {
    /// Atomically reserve decoded bytes within the shared retained-preview budget;
    /// return a lease whose final drop releases the exact reservation, or fail before allocation.
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Lease> {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let next=used.checked_add(bytes).filter(|total|*total<=MAX_RETAINED)
                .context("Preview memory limit reached (128 MiB). Close another image or PDF tab and try again.")?;
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(current) => used = current,
            }
        }
        Ok(Lease {
            budget: self.clone(),
            bytes,
        })
    }
    /// Return currently reserved bytes, including shared reservations retained by workers/tabs.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
}
/// RAII preview reservation; dropping the final owning lease returns its bytes to the budget.
#[derive(Debug)]
pub struct Lease {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// CPU-decoded raster ready for one desktop texture upload.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// Shared RGBA pixels, possibly reduced to the texture-side limit.
    pub pixels: Arc<ColorImage>,
    /// Oriented source width and height before GPU-size reduction.
    pub original: [u32; 2],
    /// Decoded codec label for display.
    pub format: String,
    /// Original encoded input length in bytes.
    pub encoded_bytes: usize,
    /// Shared reservation retained as long as the preview pixels are owned.
    pub lease: Arc<Lease>,
}

/// Return a supported raster codec hint from the extension; byte signatures still determine
/// actual decoding.
pub fn format_for(path: &str) -> Option<ImageFormat> {
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" | "jfif" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::WebP),
        "gif" => Some(ImageFormat::Gif),
        "bmp" | "dib" => Some(ImageFormat::Bmp),
        "ico" => Some(ImageFormat::Ico),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "pnm" | "pbm" | "pgm" | "ppm" | "pam" => Some(ImageFormat::Pnm),
        "tga" => Some(ImageFormat::Tga),
        _ => None,
    }
}

/// Decode bounded standard base64 image data on a worker under the shared memory budget.
pub fn decode(
    encoded: &str,
    path: &str,
    budget: &Arc<Budget>,
    texture_limit: usize,
) -> Result<Decoded> {
    ensure!(
        encoded.len() <= MAX_ENCODED.div_ceil(3) * 4,
        "Image file exceeds 32 MiB."
    );
    let bytes = STANDARD
        .decode(encoded)
        .context("Invalid image data from the file helper")?;
    decode_bytes(&bytes, path, budget, texture_limit)
}

/// Decode bounded raster bytes, apply orientation and reduce to `texture_limit` where
/// necessary. No texture is uploaded and no remote loader is invoked.
pub fn decode_bytes(
    bytes: &[u8],
    path: &str,
    budget: &Arc<Budget>,
    texture_limit: usize,
) -> Result<Decoded> {
    ensure!(bytes.len() <= MAX_ENCODED, "Image file exceeds 32 MiB.");
    let mut reader = ImageReader::new(Cursor::new(bytes));
    if let Some(format) = format_for(path) {
        reader.set_format(format);
    }
    reader = reader
        .with_guessed_format()
        .context("Cannot identify image format")?;
    let format = reader.format().context("Unsupported image format")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED as u64);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .context("Cannot read image; it may be corrupt or exceed image limits")?;
    let (width, height) = decoder.dimensions();
    let rgba_bytes = u64::from(width) * u64::from(height) * 4;
    ensure!(
        width > 0
            && height > 0
            && rgba_bytes <= MAX_DECODED as u64
            && decoder.total_bytes() <= MAX_DECODED as u64,
        "Image is too large: {width} × {height}. Previews allow 64 MiB of decoded pixels (16 megapixels at RGBA8)."
    );
    let mut lease = budget.reserve(rgba_bytes.max(decoder.total_bytes()) as usize)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image =
        DynamicImage::from_decoder(decoder).context("Cannot decode image; it may be corrupt")?;
    image.apply_orientation(orientation);
    let original = [image.width(), image.height()];
    let limit = texture_limit.clamp(1, 4096) as u32;
    if image.width() > limit || image.height() > limit {
        image = image.thumbnail(limit, limit);
    }
    let rgba = image.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let retained = size[0] * size[1] * 4;
    lease
        .budget
        .used
        .fetch_sub(lease.bytes - retained, Ordering::AcqRel);
    lease.bytes = retained;
    Ok(Decoded {
        pixels: Arc::new(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())),
        original,
        format: format!("{format:?}").to_uppercase(),
        encoded_bytes: bytes.len(),
        lease: Arc::new(lease),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_reservations_are_bounded_and_released() {
        let budget = Arc::new(Budget::default());
        let barrier = std::sync::Barrier::new(9);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|_| {
                    let budget = &budget;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let lease = budget.reserve(MAX_RETAINED / 4).ok();
                        barrier.wait();
                        barrier.wait();
                        drop(lease);
                    })
                })
                .collect();
            barrier.wait();
            assert_eq!(budget.used(), MAX_RETAINED);
            barrier.wait();
            for job in jobs {
                job.join().unwrap();
            }
        });
        assert_eq!(budget.used(), 0);
    }
}
