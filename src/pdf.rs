//! On-demand PDF rasterization on the desktop. Only bytes cross the host file
//! channel; no renderer is installed or run on SSH hosts.
use crate::images;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Maximum accepted page count in one PDF.
pub const MAX_PAGES: usize = 10_000;
/// Maximum requested raster side in pixels before GPU-limit adjustment.
pub const RENDER_SIDE: usize = 2048;

/// Recognize a `.pdf` path without accessing its bytes.
pub fn is_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

/// Extract complete PDF text on a worker using the desktop's bounded Poppler runner.
/// Scanned/image-only PDFs need OCR or image attachments instead.
pub fn extract_text(bytes: Vec<u8>, cancel: &AtomicBool) -> Result<String> {
    ensure!(bytes.starts_with(b"%PDF-"), "Invalid PDF attachment.");
    let text = run(
        "pdftotext",
        &[
            "-layout".into(),
            "-enc".into(),
            "UTF-8".into(),
            "-".into(),
            "-".into(),
        ],
        bytes.into(),
        rode_agent_core::attachment::CONTENT_LIMIT,
        cancel,
    )?;
    let text = String::from_utf8(text).context("PDF text is not UTF-8")?;
    ensure!(
        !text.trim().is_empty(),
        "This PDF contains no extractable text. Attach page images or an OCR text version."
    );
    Ok(text)
}

/// Immutable PDF byte snapshot with a retained-memory lease; pages are rasterized only on
/// demand.
pub struct Source {
    bytes: Arc<[u8]>,
    /// Validated number of pages in the snapshot.
    pub pages: usize,
    _lease: images::Lease,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfSource")
            .field("pages", &self.pages)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
/// One rendered PDF page and its shared original source snapshot.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Retained source used for subsequent page requests without another host transfer.
    pub source: Arc<Source>,
    /// One-based page number currently represented by the raster.
    pub page: usize,
    /// CPU raster pixels and their shared preview-memory reservation.
    pub pixels: images::Decoded,
}

/// Decode base64 PDF data and rasterize one page using installed desktop Poppler tools. This
/// blocks and belongs on a worker.
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

/// Load/rasterize with explicit cancellation; never run a renderer on the SSH host.
pub fn load_cancellable(
    encoded: &str,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
    cancel: &AtomicBool,
) -> Result<Loaded> {
    ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
    let bytes = STANDARD
        .decode(encoded)
        .context("Invalid PDF data from the file helper")?;
    load_bytes_cancellable(bytes, page, budget, texture_limit, cancel)
}

/// Retain owned PDF source bytes and rasterize one page under decoded-pixel limits.
pub fn load_bytes(
    bytes: Vec<u8>,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
) -> Result<Loaded> {
    load_bytes_cancellable(bytes, page, budget, texture_limit, &AtomicBool::new(false))
}

fn load_bytes_cancellable(
    bytes: Vec<u8>,
    page: usize,
    budget: &Arc<images::Budget>,
    texture_limit: usize,
    cancel: &AtomicBool,
) -> Result<Loaded> {
    ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
    ensure!(
        bytes[..bytes.len().min(1024)]
            .windows(5)
            .any(|bytes| bytes == b"%PDF-"),
        "Cannot open PDF: missing PDF header."
    );
    let lease = budget.reserve_source(bytes.len())?;
    let bytes: Arc<[u8]> = bytes.into();
    let info = run("pdfinfo", &["-".into()], bytes.clone(), 64 * 1024, cancel)?;
    let info = std::str::from_utf8(&info).context("PDF metadata is not UTF-8")?;
    let pages = info
        .lines()
        .find_map(|line| {
            line.strip_prefix("Pages:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .context("PDF renderer did not report a page count")?;
    ensure!(
        pages > 0 && pages <= MAX_PAGES,
        "PDF previews support 1–10,000 pages."
    );
    let source = Arc::new(Source {
        bytes,
        pages,
        _lease: lease,
    });
    let page = page.clamp(1, pages);
    let pixels = source.render(page, budget, texture_limit, cancel)?;
    Ok(Loaded {
        source,
        page,
        pixels,
    })
}
impl Source {
    /// Rasterize a one-based page from this retained source with cancellation and a
    /// texture-side limit. Other pages are not retained automatically.
    pub fn render(
        &self,
        page: usize,
        budget: &Arc<images::Budget>,
        texture_limit: usize,
        cancel: &AtomicBool,
    ) -> Result<images::Decoded> {
        ensure!(
            (1..=self.pages).contains(&page),
            "Page is outside this PDF."
        );
        ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
        let png = run(
            "pdftoppm",
            &[
                "-f".into(),
                page.to_string(),
                "-l".into(),
                page.to_string(),
                "-singlefile".into(),
                "-scale-to".into(),
                RENDER_SIDE.min(texture_limit.max(1)).to_string(),
                "-png".into(),
                "-".into(),
            ],
            self.bytes.clone(),
            32 * 1024 * 1024, // Rendered-page PNG output bound, independent of source file size.
            cancel,
        )?;
        ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
        let mut pixels = images::decode_bytes(&png, "page.png", budget, texture_limit)?;
        pixels.format = "PDF".into();
        pixels.encoded_bytes = self.bytes.len();
        Ok(pixels)
    }
}

/// Bounded binary stdout, concurrent pipes, cancellation, and process-group cleanup.
/// Rendering receives a snapshot on stdin, never the source host's path.
fn run(
    program: &str,
    args: &[String],
    bytes: Arc<[u8]>,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    use std::os::unix::process::CommandExt;
    ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
    let mut command = Command::new(program);
    command.args(args).env("LC_ALL", "C");
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
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = rode_host_core::command::capture_bounded(
        command,
        std::io::Cursor::new(bytes),
        Duration::from_secs(20),
        cancel,
        limit,
    )
    .map_err(|error| {
        if cancel.load(Ordering::Acquire) {
            anyhow::anyhow!("PDF rendering cancelled.")
        } else if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
            error.kind() == std::io::ErrorKind::NotFound
        }) {
            error.context(format!("PDF tools need Poppler's pdfinfo, pdftoppm and pdftotext on this desktop (for example: install poppler-utils). Cannot run {program}"))
        } else {
            error
        }
    })?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.to_ascii_lowercase().contains("password") {
            bail!("Password-protected PDFs are not supported by this preview.");
        }
        bail!(
            "Cannot render PDF: {}",
            error.chars().take(2000).collect::<String>().trim()
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn cancelled_and_excessive_renderer_processes_are_stopped() {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            flag.store(true, Ordering::Release);
        });
        let before = Instant::now();
        let result = run(
            "/bin/sh",
            &["-c".into(), "sleep 30 & wait".into()],
            Arc::from([]),
            1024,
            &cancel,
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(before.elapsed() < Duration::from_secs(3));
        assert!(
            run(
                "/bin/sh",
                &["-c".into(), "head -c 2048 /dev/zero".into()],
                Arc::from([]),
                16,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn renderer_exit_closes_descendant_pipes_before_collecting_output() {
        let before = Instant::now();
        let output = run(
            "/bin/sh",
            &["-c".into(), "/bin/sleep 3 & printf done".into()],
            Arc::from([]),
            1024,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output, b"done");
        assert!(before.elapsed() < Duration::from_secs(2));
    }
}
