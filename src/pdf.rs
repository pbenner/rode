//! Bundled PDF/PPTX/DOCX interpretation on the desktop, with a retained document
//! worker and bounded decoded-page cache. SSH hosts transfer original bytes only.
use crate::images;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod docx;
mod positioned;
mod pptx;
mod renderer;
mod text;
pub use positioned::{PageText, TextCell};

/// Separate, killable text interpreter so indexing never holds the page-render lock.
pub struct TextReader(renderer::Renderer);
impl TextReader {
    /// Extract one bounded text layer from the immutable original snapshot.
    pub fn page(&mut self, page: usize, cancel: &AtomicBool) -> Result<PageText> {
        self.0.page_text(page, cancel)
    }
}

/// Desktop interpreter selected from the submitted path, independently of host software.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// PDF pages interpreted by Hayro.
    Pdf,
    /// PowerPoint slides interpreted by BetterOffice.
    Pptx,
    /// Word pages laid out by rdocx and painted by Hayro.
    Docx,
}
impl Format {
    /// Recognize a supported paged preview without reading the file.
    pub fn for_path(path: &str) -> Option<Self> {
        let extension = Path::new(path).extension()?;
        if extension.eq_ignore_ascii_case("pdf") {
            Some(Self::Pdf)
        } else if extension.eq_ignore_ascii_case("pptx") {
            Some(Self::Pptx)
        } else if extension.eq_ignore_ascii_case("docx") {
            Some(Self::Docx)
        } else {
            None
        }
    }
    /// Short format name shared by toolbars and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Pptx => "PPTX",
            Self::Docx => "DOCX",
        }
    }
    /// Formats exposing positioned text for selection and document search.
    pub fn supports_text(self) -> bool {
        matches!(self, Self::Pdf | Self::Pptx)
    }
}

/// Maximum accepted number of pages in a document.
pub const MAX_PAGES: usize = 10_000;
/// Default raster quality, in pixels on the longer side.
pub const RENDER_SIDE: usize = 2048;
const CACHE_PAGES: usize = 4;
const CACHE_BYTES: usize = 48 * 1024 * 1024;

/// Recognize a PDF extension without accessing its bytes.
pub fn is_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

/// Extract bounded Unicode text with the bundled desktop interpreter.
pub fn extract_text(bytes: Vec<u8>, cancel: &AtomicBool) -> Result<String> {
    let (mut renderer, _) = renderer::Renderer::open(&bytes, Format::Pdf, cancel)?;
    renderer.text(cancel)
}

/// Immutable source snapshot and a private, persistent desktop renderer.
/// The page cache shares its leases with displayed textures and evicts old
/// entries before exceeding either its page count or retained-pixel allowance.
pub struct Source {
    bytes: Arc<[u8]>,
    /// Interpreter and document kind retained across reload, caching and fullscreen.
    pub format: Format,
    /// Validated page count.
    pub pages: usize,
    /// Rotated/cropped page dimensions in PDF points, used for virtual layout.
    pub sizes: Vec<[f32; 2]>,
    /// Bounded, plain-language rendering approximations reported by the interpreter.
    pub warnings: Vec<String>,
    renderer: Mutex<Option<renderer::Renderer>>,
    cache: Mutex<VecDeque<(usize, usize, images::Decoded)>>,
    skipped_images: Mutex<std::collections::BTreeMap<usize, usize>>,
    _lease: images::Lease,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PagedSource")
            .field("format", &self.format)
            .field("pages", &self.pages)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
/// A rendered page with its retained original document.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Original source reused by all page requests.
    pub source: Arc<Source>,
    /// One-based page number.
    pub page: usize,
    /// Raster pixels and their preview-memory reservation.
    pub pixels: images::Decoded,
}
/// Decode base64 source bytes and render a page on a background worker.
pub fn load(
    encoded: &str,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
) -> Result<Loaded> {
    load_cancellable(
        encoded,
        page,
        budget,
        texture_limit,
        &AtomicBool::new(false),
    )
}
/// Load a source and initial page with explicit cancellation.
pub fn load_cancellable(
    encoded: &str,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
    cancel: &AtomicBool,
) -> Result<Loaded> {
    load_format_cancellable(encoded, Format::Pdf, page, budget, texture_limit, cancel)
}
/// Load a PDF, PowerPoint or Word snapshot through the same bounded desktop worker.
pub fn load_format_cancellable(
    encoded: &str,
    format: Format,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
    cancel: &AtomicBool,
) -> Result<Loaded> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Document rendering cancelled."
    );
    load_bytes_cancellable(
        STANDARD
            .decode(encoded)
            .context("Invalid document data from the file helper")?,
        page,
        budget,
        texture_limit,
        cancel,
        format,
    )
}
/// Load an owned snapshot and initial page without any external PDF tools.
pub fn load_bytes(
    bytes: Vec<u8>,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
) -> Result<Loaded> {
    load_bytes_cancellable(
        bytes,
        page,
        budget,
        texture_limit,
        &AtomicBool::new(false),
        Format::Pdf,
    )
}
fn load_bytes_cancellable(
    bytes: Vec<u8>,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
    cancel: &AtomicBool,
    format: Format,
) -> Result<Loaded> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Document rendering cancelled."
    );
    ensure!(
        format != Format::Pdf
            || bytes[..bytes.len().min(1024)]
                .windows(5)
                .any(|s| s == b"%PDF-"),
        "Cannot open PDF: missing PDF header."
    );
    let lease = budget.reserve_source(bytes.len())?;
    let (renderer, metadata) = renderer::Renderer::open(&bytes, format, cancel)?;
    let source = Arc::new(Source {
        bytes: bytes.into(),
        format,
        pages: metadata.sizes.len(),
        sizes: metadata.sizes,
        warnings: metadata.warnings,
        renderer: Mutex::new(Some(renderer)),
        cache: Mutex::new(VecDeque::new()),
        skipped_images: Default::default(),
        _lease: lease,
    });
    let cache: Arc<dyn images::PreviewCache> = source.clone();
    budget.register_cache(&cache);
    let page = page.clamp(1, source.pages);
    let pixels = source.render(page, budget, texture_limit, cancel)?;
    Ok(Loaded {
        source,
        page,
        pixels,
    })
}
impl images::PreviewCache for Source {
    fn release_one(&self) -> bool {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .is_some()
    }
}
impl Source {
    /// Release the retained interpreter while keeping the immutable snapshot
    /// and cached pages usable after a refused project close. Call on a worker.
    pub fn close_renderer(&self) {
        let renderer = self
            .renderer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(renderer);
    }
    /// Open an independent private text reader on a background thread. PPTX uses
    /// desktop fonts like its raster worker; no project-host tools are consulted.
    pub fn text_reader(&self, cancel: &AtomicBool) -> Result<TextReader> {
        ensure!(
            self.format.supports_text(),
            "Text selection is available for PDF and PowerPoint files."
        );
        Ok(TextReader(
            renderer::Renderer::open(&self.bytes, self.format, cancel)?.0,
        ))
    }
    /// Number of missing or unsupported images skipped on a rendered page.
    /// Counts survive pixel-cache eviction and are bounded by the document's page count.
    pub fn skipped_images(&self, page: usize) -> usize {
        self.skipped_images
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&page)
            .copied()
            .unwrap_or(0)
    }
    /// Return a cached page immediately, without starting work or taking the
    /// renderer lock. Clones share the exact pixels and memory reservation.
    pub fn cached(&self, page: usize, texture_limit: usize) -> Option<images::Decoded> {
        let side = RENDER_SIDE.min(texture_limit.clamp(1, 4096));
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let index = cache
            .iter()
            .position(|(p, s, _)| *p == page && *s == side)?;
        let entry = cache.remove(index)?;
        let decoded = entry.2.clone();
        cache.push_back(entry);
        Some(decoded)
    }
    /// Render a page with persistent parsing/font caches; cancellation or a
    /// failed worker discards that worker and the next request can restart it.
    pub fn render(
        &self,
        page: usize,
        budget: &Arc<images::Budget>,
        texture_limit: usize,
        cancel: &AtomicBool,
    ) -> Result<images::Decoded> {
        ensure!(
            (1..=self.pages).contains(&page),
            "Page is outside this document."
        );
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Document rendering cancelled."
        );
        if let Some(pixels) = self.cached(page, texture_limit) {
            return Ok(pixels);
        }
        let mut renderer = self.renderer.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Document rendering cancelled."
        );
        if let Some(pixels) = self.cached(page, texture_limit) {
            return Ok(pixels);
        }
        let side = RENDER_SIDE.min(texture_limit.clamp(1, 4096));
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            while cache.len() >= CACHE_PAGES
                || cache
                    .iter()
                    .map(|e| e.2.pixels.pixels.len() * 4)
                    .sum::<usize>()
                    + side * side * 4
                    > CACHE_BYTES
            {
                if cache.pop_front().is_none() {
                    break;
                }
            }
        }
        if renderer.is_none() {
            *renderer = Some(renderer::Renderer::open(&self.bytes, self.format, cancel)?.0);
        }
        let result =
            renderer
                .as_mut()
                .unwrap()
                .render(page, side, budget, self.bytes.len(), cancel);
        match result {
            Ok((pixels, skipped)) => {
                self.skipped_images
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(page, skipped);
                ensure!(
                    !cancel.load(Ordering::Acquire),
                    "Document rendering cancelled."
                );
                self.cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back((page, side, pixels.clone()));
                Ok(pixels)
            }
            Err(error) => {
                *renderer = None;
                Err(error)
            }
        }
    }
}

/// Internal renderer entry point. The editor calls this before GUI initialization.
pub fn coordinator(path: &Path) -> Result<()> {
    coordinator_for(path, Format::Pdf)
}
/// Internal paged-document renderer entry point; never starts a GUI or host service.
pub fn coordinator_for(path: &Path, format: Format) -> Result<()> {
    renderer::serve(path, format)
}

fn limit_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // Bound the decoder's address space in addition to pixels and pipe output.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 1024 * 1024 * 1024,
                rlim_max: 1024 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_renderer_retains_cached_pages_and_can_restart_from_the_snapshot() {
        let budget = Arc::new(images::Budget::default());
        let loaded = load_bytes(
            include_bytes!("../tests/fixtures/pages.pdf").to_vec(),
            1,
            &budget,
            128,
        )
        .unwrap();
        assert!(loaded.source.renderer.lock().unwrap().is_some());
        loaded.source.close_renderer();
        assert!(loaded.source.renderer.lock().unwrap().is_none());
        assert!(Arc::ptr_eq(
            &loaded.pixels.pixels,
            &loaded.source.cached(1, 128).unwrap().pixels
        ));
        loaded
            .source
            .render(2, &budget, 128, &AtomicBool::new(false))
            .unwrap();
        assert!(loaded.source.renderer.lock().unwrap().is_some());
    }
    #[test]
    fn memory_pressure_evicts_page_caches_without_invalidating_displayed_pixels() {
        let budget = Arc::new(images::Budget::default());
        let loaded = load_bytes(
            include_bytes!("../tests/fixtures/pages.pdf").to_vec(),
            1,
            &budget,
            128,
        )
        .unwrap();
        let second = loaded
            .source
            .render(2, &budget, 128, &AtomicBool::new(false))
            .unwrap();
        drop(second);
        let shown_bytes = loaded.pixels.pixels.pixels.len() * 4;
        let remaining = budget.reserve(images::MAX_RETAINED - shown_bytes).unwrap();
        assert!(loaded.source.cached(1, 128).is_none());
        assert!(loaded.source.cached(2, 128).is_none());
        assert_eq!(loaded.pixels.pixels.size, [103, 128]);
        assert!(
            budget.reserve(1).is_err(),
            "An active view's reservation cannot be evicted"
        );
        drop((loaded, remaining));
        assert_eq!(
            budget.used(),
            0,
            "Weak cache registration must not retain a source"
        );
    }
}
